use super::probe::{fetch_release_reference, probe_model};
use super::storage::{capability_is_fresh, default_capability_path, unix_seconds};
use super::types::*;
use super::*;

impl WebSearchCapabilities {
    pub fn clear_default_cache() -> anyhow::Result<bool> {
        let path = default_capability_path();
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        Ok(true)
    }

    pub fn from_default_path(config: &GatewayConfig) -> anyhow::Result<Self> {
        Self::load(default_capability_path(), config)
    }

    pub fn load(path: PathBuf, config: &GatewayConfig) -> anyhow::Result<Self> {
        let upstream = UpstreamIdentity::from_config(config);
        let models = if path.exists() {
            let raw =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            let snapshot: CapabilitySnapshot =
                serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
            if snapshot.version != CAPABILITY_FILE_VERSION {
                tracing::info!(
                    path = %path.display(),
                    cached_version = snapshot.version,
                    current_version = CAPABILITY_FILE_VERSION,
                    "discarding incompatible web search capability cache"
                );
                BTreeMap::new()
            } else if snapshot.upstream == upstream {
                snapshot.models
            } else {
                BTreeMap::new()
            }
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            path: Arc::new(path),
            upstream,
            models: Arc::new(RwLock::new(models)),
        })
    }

    pub fn supports_model(&self, model: &str) -> bool {
        let model = canonical_upstream_model_alias(model);
        let now = unix_seconds().expect("system clock before Unix epoch");
        self.models
            .read()
            .expect("web search capability lock poisoned")
            .get(model)
            .is_some_and(|capability| capability.supported && capability_is_fresh(capability, now))
    }

    pub fn annotate_models(&self, models: &mut [ModelInfo]) {
        let now = unix_seconds().expect("system clock before Unix epoch");
        let capabilities = self
            .models
            .read()
            .expect("web search capability lock poisoned");
        for model in models {
            model.supports_web_search = capabilities
                .get(&model.id)
                .filter(|capability| capability_is_fresh(capability, now))
                .map(|capability| capability.supported);
        }
    }

    pub fn supported_model_ids(&self) -> HashSet<String> {
        let now = unix_seconds().expect("system clock before Unix epoch");
        self.models
            .read()
            .expect("web search capability lock poisoned")
            .values()
            .filter(|capability| capability.supported && capability_is_fresh(capability, now))
            .map(|capability| capability.model.clone())
            .collect()
    }

    pub fn results(&self) -> Vec<ModelWebSearchCapability> {
        self.models
            .read()
            .expect("web search capability lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub async fn probe_models(
        &self,
        models: &mut [ModelInfo],
        config: &GatewayConfig,
        force: bool,
    ) -> anyhow::Result<WebSearchProbeSummary> {
        let now = unix_seconds()?;
        let mut model_ids = models
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>();
        model_ids.sort_by_key(|model| model.to_ascii_lowercase());
        model_ids.dedup();
        let current_models = model_ids.iter().cloned().collect::<HashSet<_>>();
        let candidates = {
            let capabilities = self
                .models
                .read()
                .expect("web search capability lock poisoned");
            model_ids
                .iter()
                .filter(|model| {
                    force
                        || capabilities.get(*model).is_none_or(|capability| {
                            capability.error.is_some()
                                || now.saturating_sub(capability.probed_at)
                                    >= CAPABILITY_TTL.as_secs()
                        })
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let pruned = {
            let mut capabilities = self
                .models
                .write()
                .expect("web search capability lock poisoned");
            let previous_len = capabilities.len();
            capabilities.retain(|model, _| current_models.contains(model));
            capabilities.len() != previous_len
        };
        let attempted = candidates.len();
        if !candidates.is_empty() {
            let client = Client::builder().build()?;
            let release_reference = if candidates
                .iter()
                .any(|model| model.to_ascii_lowercase().starts_with("gpt-"))
            {
                match fetch_release_reference(&client).await {
                    Ok(reference) => Some(reference),
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "flattened GPT web search results cannot be verified"
                        );
                        None
                    }
                }
            } else {
                None
            };
            let config = Arc::new(config.clone());
            let client = Arc::new(client);
            let probe_results = stream::iter(candidates.into_iter().map(|model| {
                let client = Arc::clone(&client);
                let config = Arc::clone(&config);
                let release_reference = release_reference.clone();
                async move {
                    let result =
                        probe_model(&client, &config, &model, release_reference.as_deref()).await;
                    match result {
                        Ok((supported, evidence)) => ModelWebSearchCapability {
                            model,
                            supported,
                            evidence,
                            error: None,
                            probed_at: now,
                        },
                        Err(error) => ModelWebSearchCapability {
                            model,
                            supported: false,
                            evidence: "probe_failed".to_owned(),
                            error: Some(error.to_string()),
                            probed_at: now,
                        },
                    }
                }
            }))
            .buffer_unordered(PROBE_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;

            for capability in &probe_results {
                if let Some(error) = &capability.error {
                    tracing::warn!(
                        model = capability.model,
                        error,
                        "web search capability probe failed"
                    );
                } else {
                    tracing::info!(
                        model = capability.model,
                        supported = capability.supported,
                        evidence = capability.evidence,
                        "web search capability probed"
                    );
                }
            }
            {
                let mut capabilities = self
                    .models
                    .write()
                    .expect("web search capability lock poisoned");
                for capability in probe_results {
                    capabilities.insert(capability.model.clone(), capability);
                }
            }
            self.save()?;
        } else if pruned {
            self.save()?;
        }

        self.annotate_models(models);
        let results = self
            .results()
            .into_iter()
            .filter(|capability| current_models.contains(&capability.model))
            .collect::<Vec<_>>();
        Ok(WebSearchProbeSummary {
            attempted,
            cached: results.len().saturating_sub(attempted),
            supported: results
                .iter()
                .filter(|capability| capability.supported && capability.error.is_none())
                .count(),
            unsupported: results
                .iter()
                .filter(|capability| !capability.supported && capability.error.is_none())
                .count(),
            failed: results
                .iter()
                .filter(|capability| capability.error.is_some())
                .count(),
            results,
        })
    }

    pub(super) fn save(&self) -> anyhow::Result<()> {
        let snapshot = CapabilitySnapshot {
            version: CAPABILITY_FILE_VERSION,
            upstream: self.upstream.clone(),
            models: self
                .models
                .read()
                .expect("web search capability lock poisoned")
                .clone(),
        };
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("web-search-capabilities.json");
        let temporary_path = self
            .path
            .with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .with_context(|| format!("open {}", temporary_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&serde_json::to_vec_pretty(&snapshot)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary_path, self.path.as_ref())
            .with_context(|| format!("replace {}", self.path.display()))
    }
}
