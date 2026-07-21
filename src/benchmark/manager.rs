use super::runner::{benchmark_model, fetch_used_quota};
use super::types::*;
use super::*;

impl ModelBenchmarkManager {
    pub fn new(snapshot_path: PathBuf) -> Self {
        Self {
            snapshot_path: Arc::new(snapshot_path),
            running: Arc::new(AtomicBool::new(false)),
            snapshot_cache: Arc::new(RwLock::new(None)),
        }
    }

    pub fn from_default_path() -> Self {
        Self::new(default_benchmark_path())
    }

    pub fn snapshot(&self) -> anyhow::Result<Option<ModelBenchmarkSnapshot>> {
        let cached = self
            .snapshot_cache
            .read()
            .map_err(|_| anyhow::anyhow!("model benchmark snapshot cache is poisoned"))?
            .clone();
        let (mut snapshot, loaded_from_disk) = match cached {
            Some(snapshot) => (snapshot, false),
            None => {
                let Some(snapshot) = load_snapshot(&self.snapshot_path)? else {
                    return Ok(None);
                };
                (snapshot, true)
            }
        };
        if snapshot.status == BenchmarkRunStatus::Running && !self.running.load(Ordering::Acquire) {
            let now = unix_millis()?;
            snapshot.status = BenchmarkRunStatus::Interrupted;
            snapshot.updated_at = now;
            snapshot.finished_at = Some(now);
            snapshot.current_model = None;
            snapshot.error = Some("gateway stopped before the benchmark completed".to_owned());
            if snapshot.estimated_cost.is_none() && snapshot.cost_error.is_none() {
                snapshot.cost_error =
                    Some("benchmark stopped before cost could be calculated".to_owned());
            }
            self.persist_snapshot(&snapshot)?;
        } else if loaded_from_disk {
            *self
                .snapshot_cache
                .write()
                .map_err(|_| anyhow::anyhow!("model benchmark snapshot cache is poisoned"))? =
                Some(snapshot.clone());
        }
        Ok(Some(snapshot))
    }

    pub fn start(
        &self,
        mut models: Vec<ModelInfo>,
        config: GatewayConfig,
        timeout: Duration,
    ) -> anyhow::Result<ModelBenchmarkSnapshot> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            anyhow::bail!("model benchmark timeout must be between 1 and 300 seconds");
        }
        if models.is_empty() {
            anyhow::bail!("model benchmark requires at least one available model");
        }
        if self.running.swap(true, Ordering::AcqRel) {
            anyhow::bail!("a model benchmark is already running");
        }
        models.sort_by_key(|model| model.id.to_lowercase());
        let now = unix_millis()?;
        let snapshot = ModelBenchmarkSnapshot {
            version: BENCHMARK_FILE_VERSION,
            run_id: Uuid::new_v4().simple().to_string(),
            status: BenchmarkRunStatus::Running,
            started_at: now,
            updated_at: now,
            finished_at: None,
            timeout_seconds: timeout.as_secs(),
            target_output_tokens: BENCHMARK_TARGET_OUTPUT_TOKENS,
            total_models: models.len(),
            current_model: None,
            results: Vec::with_capacity(models.len()),
            error: None,
            estimated_cost: None,
            cost_currency: match config.provider_preset {
                ProviderPreset::BaiduOneApi => Some("CNY".to_owned()),
                ProviderPreset::OpenRouter => Some("USD".to_owned()),
                ProviderPreset::Custom | ProviderPreset::DeepSeek => None,
            },
            cost_error: None,
        };
        if let Err(error) = self.persist_snapshot(&snapshot) {
            self.running.store(false, Ordering::Release);
            return Err(error);
        }

        let manager = self.clone();
        let task_snapshot = snapshot.clone();
        tokio::spawn(async move {
            let _running_reset = RunningReset(Arc::clone(&manager.running));
            if let Err(error) = manager.run(task_snapshot, models, config, timeout).await {
                tracing::error!(error = %error, "model benchmark stopped unexpectedly");
                if let Err(persist_error) = manager.persist_failed_run(error.to_string()) {
                    tracing::error!(
                        error = %persist_error,
                        "failed to persist model benchmark failure"
                    );
                }
            }
        });
        Ok(snapshot)
    }

    async fn run(
        &self,
        mut snapshot: ModelBenchmarkSnapshot,
        models: Vec<ModelInfo>,
        config: GatewayConfig,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let client = Client::builder().build()?;
        let quota_before = if config.quota_url.is_some() {
            match tokio::time::timeout(Duration::from_secs(10), fetch_used_quota(&client, &config))
                .await
            {
                Ok(Ok(used)) => Some(used),
                Ok(Err(error)) => {
                    snapshot.cost_error = Some(error.to_string());
                    None
                }
                Err(_) => {
                    snapshot.cost_error =
                        Some("quota endpoint timed out before benchmark".to_owned());
                    None
                }
            }
        } else {
            snapshot.cost_error = Some("quota endpoint is not configured".to_owned());
            None
        };
        snapshot.updated_at = unix_millis()?;
        self.persist_snapshot(&snapshot)?;

        for model in models {
            snapshot.current_model = Some(model.id.clone());
            snapshot.updated_at = unix_millis()?;
            self.persist_snapshot(&snapshot)?;

            let result = benchmark_model(&client, &config, &model.id, timeout).await?;
            snapshot.results.push(result);
            snapshot.updated_at = unix_millis()?;
            self.persist_snapshot(&snapshot)?;
        }

        if let Some(quota_before) = quota_before {
            match tokio::time::timeout(Duration::from_secs(10), fetch_used_quota(&client, &config))
                .await
            {
                Ok(Ok(quota_after)) if quota_after >= quota_before => {
                    snapshot.estimated_cost = Some(quota_after - quota_before);
                    snapshot.cost_error = None;
                }
                Ok(Ok(_)) => {
                    snapshot.cost_error =
                        Some("used quota decreased while benchmark was running".to_owned());
                }
                Ok(Err(error)) => snapshot.cost_error = Some(error.to_string()),
                Err(_) => {
                    snapshot.cost_error =
                        Some("quota endpoint timed out after benchmark".to_owned());
                }
            }
        }

        let now = unix_millis()?;
        snapshot.status = BenchmarkRunStatus::Completed;
        snapshot.updated_at = now;
        snapshot.finished_at = Some(now);
        snapshot.current_model = None;
        self.persist_snapshot(&snapshot)
    }

    fn persist_failed_run(&self, message: String) -> anyhow::Result<()> {
        let Some(mut snapshot) = self.snapshot()? else {
            anyhow::bail!("model benchmark snapshot disappeared while the run was active");
        };
        let now = unix_millis()?;
        snapshot.status = BenchmarkRunStatus::Failed;
        snapshot.updated_at = now;
        snapshot.finished_at = Some(now);
        snapshot.current_model = None;
        snapshot.error = Some(message);
        if snapshot.estimated_cost.is_none() && snapshot.cost_error.is_none() {
            snapshot.cost_error =
                Some("benchmark failed before cost could be calculated".to_owned());
        }
        self.persist_snapshot(&snapshot)
    }

    fn persist_snapshot(&self, snapshot: &ModelBenchmarkSnapshot) -> anyhow::Result<()> {
        save_snapshot(&self.snapshot_path, snapshot)?;
        *self
            .snapshot_cache
            .write()
            .map_err(|_| anyhow::anyhow!("model benchmark snapshot cache is poisoned"))? =
            Some(snapshot.clone());
        Ok(())
    }
}

pub fn default_benchmark_path() -> PathBuf {
    stored_config_path().with_file_name("model-benchmarks.json")
}

pub fn load_snapshot(path: &Path) -> anyhow::Result<Option<ModelBenchmarkSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let snapshot: ModelBenchmarkSnapshot =
        serde_json::from_slice(&contents).with_context(|| format!("parse {}", path.display()))?;
    if snapshot.version != BENCHMARK_FILE_VERSION {
        anyhow::bail!(
            "unsupported model benchmark file version {} in {}",
            snapshot.version,
            path.display()
        );
    }
    Ok(Some(snapshot))
}

pub(super) fn save_snapshot(path: &Path, snapshot: &ModelBenchmarkSnapshot) -> anyhow::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!("benchmark result path has no parent: {}", path.display())
    })?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid benchmark result filename: {}", path.display()))?;
    let temporary_path =
        path.with_file_name(format!("{file_name}.tmp.{}", Uuid::new_v4().simple()));
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer_pretty(&mut file, snapshot)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result.with_context(|| format!("write {}", path.display()))
}

pub(super) fn unix_millis() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64)
}
