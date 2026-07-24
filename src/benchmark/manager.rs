use super::runner::{benchmark_model, fetch_used_quota};
use super::types::*;
use super::*;

const MAX_CONCURRENT_PROVIDER_GROUPS: usize = 4;

struct BenchmarkProviderGroup {
    provider: ProviderRuntime,
    targets: std::collections::VecDeque<BenchmarkTarget>,
    quota_before: Option<f64>,
    cost: ProviderBenchmarkCost,
}

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

    pub(crate) fn start(
        &self,
        targets: Vec<BenchmarkTarget>,
        timeout: Duration,
    ) -> anyhow::Result<ModelBenchmarkSnapshot> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            anyhow::bail!("model benchmark timeout must be between 1 and 300 seconds");
        }
        if targets.is_empty() {
            anyhow::bail!("model benchmark requires at least one available model");
        }
        if self.running.swap(true, Ordering::AcqRel) {
            anyhow::bail!("a model benchmark is already running");
        }
        let unique_providers = targets
            .iter()
            .map(|target| target.provider_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let single_currency = if unique_providers.len() == 1 {
            targets
                .first()
                .and_then(|target| target.provider.quota_currency())
                .map(str::to_owned)
        } else {
            None
        };
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
            total_models: targets.len(),
            current_model: None,
            results: Vec::with_capacity(targets.len()),
            error: None,
            estimated_cost: None,
            cost_currency: single_currency,
            cost_error: None,
            provider_costs: Vec::new(),
        };
        if let Err(error) = self.persist_snapshot(&snapshot) {
            self.running.store(false, Ordering::Release);
            return Err(error);
        }

        let manager = self.clone();
        let task_snapshot = snapshot.clone();
        tokio::spawn(async move {
            let _running_reset = RunningReset(Arc::clone(&manager.running));
            if let Err(error) = manager.run(task_snapshot, targets, timeout).await {
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
        targets: Vec<BenchmarkTarget>,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let client = Client::builder().build()?;
        let target_order = targets
            .iter()
            .enumerate()
            .map(|(index, target)| (target.catalog_slug.clone(), index))
            .collect::<std::collections::HashMap<_, _>>();
        let mut grouped_targets: Vec<(ProviderRuntime, Vec<BenchmarkTarget>)> = Vec::new();
        for target in targets {
            if let Some((_, group_targets)) = grouped_targets
                .iter_mut()
                .find(|(provider, _)| provider.id() == target.provider_id)
            {
                group_targets.push(target);
            } else {
                grouped_targets.push((target.provider.clone(), vec![target]));
            }
        }
        let mut groups = grouped_targets
            .into_iter()
            .map(|(provider, targets)| BenchmarkProviderGroup {
                cost: ProviderBenchmarkCost {
                    provider_id: provider.id().to_owned(),
                    currency: provider.quota_currency().map(str::to_owned),
                    estimated_cost: None,
                    error: None,
                },
                provider,
                targets: targets.into(),
                quota_before: None,
            })
            .collect::<Vec<_>>();

        let quota_providers = groups
            .iter()
            .enumerate()
            .map(|(index, group)| (index, group.provider.clone()))
            .collect::<Vec<_>>();
        let quota_before =
            futures_util::stream::iter(quota_providers.into_iter().map(|(index, provider)| {
                let client = &client;
                async move {
                    (
                        index,
                        fetch_benchmark_quota(client, &provider, "before benchmark").await,
                    )
                }
            }))
            .buffer_unordered(MAX_CONCURRENT_PROVIDER_GROUPS)
            .collect::<Vec<_>>()
            .await;
        for (index, quota) in quota_before {
            match quota {
                Ok(value) => groups[index].quota_before = Some(value),
                Err(error) => groups[index].cost.error = Some(error),
            }
        }

        loop {
            let batch = groups
                .iter_mut()
                .enumerate()
                .filter_map(|(index, group)| {
                    group.targets.pop_front().map(|target| (index, target))
                })
                .collect::<Vec<_>>();
            if batch.is_empty() {
                break;
            }
            snapshot.current_model = batch.first().map(|(_, target)| target.catalog_slug.clone());
            snapshot.updated_at = unix_millis()?;
            self.persist_snapshot(&snapshot)?;

            let mut results =
                futures_util::stream::iter(batch.into_iter().map(|(group_index, target)| {
                    let client = &client;
                    async move { (group_index, benchmark_model(client, &target, timeout).await) }
                }))
                .buffer_unordered(MAX_CONCURRENT_PROVIDER_GROUPS)
                .collect::<Vec<_>>()
                .await;
            results.sort_by_key(|(group_index, _)| *group_index);
            for (_, result) in results {
                snapshot.results.push(result?);
                snapshot.updated_at = unix_millis()?;
                self.persist_snapshot(&snapshot)?;
            }
        }

        let quota_providers = groups
            .iter()
            .enumerate()
            .filter_map(|(index, group)| {
                group
                    .quota_before
                    .map(|quota_before| (index, quota_before, group.provider.clone()))
            })
            .collect::<Vec<_>>();
        let quota_after = futures_util::stream::iter(quota_providers.into_iter().map(
            |(index, quota_before, provider)| {
                let client = &client;
                async move {
                    (
                        index,
                        quota_before,
                        fetch_benchmark_quota(client, &provider, "after benchmark").await,
                    )
                }
            },
        ))
        .buffer_unordered(MAX_CONCURRENT_PROVIDER_GROUPS)
        .collect::<Vec<_>>()
        .await;
        for (index, quota_before, quota_after) in quota_after {
            match quota_after {
                Ok(quota_after) if quota_after >= quota_before => {
                    groups[index].cost.estimated_cost = Some(quota_after - quota_before);
                    groups[index].cost.error = None;
                }
                Ok(_) => {
                    groups[index].cost.error =
                        Some("used quota decreased while benchmark was running".to_owned());
                }
                Err(error) => groups[index].cost.error = Some(error),
            }
        }
        snapshot.provider_costs = groups.into_iter().map(|group| group.cost).collect();
        snapshot.results.sort_by_key(|result| {
            target_order
                .get(&result.model)
                .copied()
                .unwrap_or(usize::MAX)
        });

        if snapshot.provider_costs.len() == 1 {
            snapshot.estimated_cost = snapshot.provider_costs[0].estimated_cost;
            snapshot.cost_error = snapshot.provider_costs[0].error.clone();
        } else {
            snapshot.estimated_cost = None;
            snapshot.cost_currency = None;
            snapshot.cost_error = None;
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

async fn fetch_benchmark_quota(
    client: &Client,
    provider: &ProviderRuntime,
    phase: &str,
) -> Result<f64, String> {
    if provider.quota_url().is_none() {
        return Err("quota endpoint is not configured".to_owned());
    }
    match tokio::time::timeout(Duration::from_secs(10), fetch_used_quota(client, provider)).await {
        Ok(Ok(used)) => Ok(used),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!("quota endpoint timed out {phase}")),
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
