//! Shared, lightweight application state: the latest gateway status,
//! provider snapshot and interface contract, plus the single queue of
//! configuration changes. Pages keep their own drafts and page data.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Task};

use crate::cli::{Cli, CliError, CliResult, Progress, Secret};
use crate::model::{Interface, ProviderList, QuotaEntry, ServiceStatus};

const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(20);

/// The latest successful value plus the failure of the most recent attempt.
/// A failed refresh keeps the previous value visible.
pub struct Snapshot<T> {
    pub value: Option<T>,
    pub error: Option<SharedString>,
    pub loading: bool,
    /// A refresh was requested while one was in flight; the in-flight result
    /// may predate a change, so the query runs again when it lands.
    stale: bool,
}

impl<T> Default for Snapshot<T> {
    fn default() -> Self {
        Self {
            value: None,
            error: None,
            loading: false,
            stale: false,
        }
    }
}

impl<T> Snapshot<T> {
    /// Marks a query as started; `false` means one is already running and
    /// will be repeated when it finishes.
    fn begin(&mut self) -> bool {
        if self.loading {
            self.stale = true;
            return false;
        }
        self.loading = true;
        true
    }

    /// Stores the result and reports whether the query must run again.
    fn finish(&mut self, result: CliResult<T>) -> bool {
        self.loading = false;
        match result {
            Ok(value) => {
                self.value = Some(value);
                self.error = None;
            }
            Err(error) => self.error = Some(error.message.into()),
        }
        std::mem::take(&mut self.stale)
    }
}

/// A configuration change the shell asks the CLI to perform.
pub struct Change {
    pub label: SharedString,
    pub args: Vec<String>,
    pub secrets: Vec<Secret>,
    /// Run `config apply` after a successful change.
    pub apply: bool,
}

impl Change {
    pub fn new(
        label: impl Into<SharedString>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            label: label.into(),
            args: args.into_iter().map(Into::into).collect(),
            secrets: Vec::new(),
            apply: false,
        }
    }

    pub fn applied(mut self) -> Self {
        self.apply = true;
        self
    }
}

pub enum StoreEvent {
    /// A change finished; views show the outcome in their own window.
    ChangeFinished {
        label: SharedString,
        result: Result<String, CliError>,
    },
}

pub struct Store {
    cli: Arc<Cli>,
    pub service: Snapshot<ServiceStatus>,
    pub providers: Snapshot<ProviderList>,
    pub interface: Snapshot<Interface>,
    pub quotas: BTreeMap<String, Snapshot<Vec<QuotaEntry>>>,
    /// Running changes in start order, each with its label and latest
    /// progress line.
    running: Vec<(u64, SharedString, Option<SharedString>)>,
    next_change: u64,
    /// Bumped after every change so pages reload their own data.
    pub revision: u64,
    _poll: Task<()>,
}

impl EventEmitter<StoreEvent> for Store {}

struct GlobalStore(Entity<Store>);

impl Global for GlobalStore {}

impl Store {
    pub fn init(cli: Arc<Cli>, cx: &mut App) {
        let store = cx.new(|cx| {
            let poll = cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(STATUS_POLL_INTERVAL).await;
                    if this
                        .update(cx, |store: &mut Store, cx| store.refresh_status(cx))
                        .is_err()
                    {
                        break;
                    }
                }
            });
            let mut store = Self {
                cli,
                service: Snapshot::default(),
                providers: Snapshot::default(),
                interface: Snapshot::default(),
                quotas: BTreeMap::new(),
                running: Vec::new(),
                next_change: 0,
                revision: 0,
                _poll: poll,
            };
            store.refresh_all(cx);
            store
        });
        cx.set_global(GlobalStore(store));
    }

    pub fn global(cx: &App) -> Entity<Store> {
        cx.global::<GlobalStore>().0.clone()
    }

    pub fn cli(&self) -> Arc<Cli> {
        self.cli.clone()
    }

    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.refresh_status(cx);
        self.refresh_providers(cx);
        if self.interface.value.is_none() {
            self.refresh_interface(cx);
        }
    }

    /// Label and latest progress line of the oldest running change.
    pub fn busy(&self) -> Option<(SharedString, Option<SharedString>)> {
        self.running
            .first()
            .map(|(_, label, detail)| (label.clone(), detail.clone()))
    }

    pub fn refresh_status(&mut self, cx: &mut Context<Self>) {
        if !self.service.begin() {
            return;
        }
        let cli = self.cli.clone();
        cx.spawn(async move |this, cx| {
            let result =
                cli_thread(move || cli.query::<ServiceStatus>(&["service", "status", "--json"]))
                    .await;
            this.update(cx, |store, cx| {
                if store.service.finish(result) {
                    store.refresh_status(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh_providers(&mut self, cx: &mut Context<Self>) {
        if !self.providers.begin() {
            return;
        }
        let cli = self.cli.clone();
        cx.spawn(async move |this, cx| {
            let result =
                cli_thread(move || cli.query::<ProviderList>(&["provider", "list", "--json"]))
                    .await;
            this.update(cx, |store, cx| {
                if store.providers.finish(result) {
                    store.refresh_providers(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_interface(&mut self, cx: &mut Context<Self>) {
        self.interface.loading = true;
        let cli = self.cli.clone();
        cx.spawn(async move |this, cx| {
            let result = cli_thread(move || cli.query::<Interface>(&["interface", "--json"])).await;
            this.update(cx, |store, cx| {
                store.interface.finish(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh_quota(&mut self, provider_id: &str, cx: &mut Context<Self>) {
        let snapshot = self.quotas.entry(provider_id.to_owned()).or_default();
        // Quota reads are remote and slow; an in-flight one is not repeated.
        if snapshot.loading {
            return;
        }
        snapshot.loading = true;
        cx.notify();
        let cli = self.cli.clone();
        let id = provider_id.to_owned();
        cx.spawn(async move |this, cx| {
            let query_id = id.clone();
            let result = cli_thread(move || {
                cli.query::<Vec<QuotaEntry>>(&["quota", "--provider", &query_id, "--json"])
            })
            .await;
            this.update(cx, |store, cx| {
                let _ = store.quotas.entry(id).or_default().finish(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Queues a change. Snapshots reload afterwards whether or not it
    /// succeeded, because a failure may still have committed part of it.
    pub fn run(
        &mut self,
        change: Change,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, CliError>> {
        let cli = self.cli.clone();
        let label = change.label.clone();
        let change_id = self.next_change;
        self.next_change += 1;
        self.running.push((change_id, label.clone(), None));
        cx.notify();
        let (progress_sender, progress_receiver) = smol::channel::unbounded::<String>();
        let progress: Progress = Arc::new(move |line| {
            let _ = progress_sender.try_send(line);
        });
        cx.spawn(async move |this, cx| {
            let progress_task = {
                let this = this.clone();
                cx.spawn(async move |cx| {
                    while let Ok(line) = progress_receiver.recv().await {
                        let updated = this.update(cx, |store, cx| {
                            if let Some((_, _, detail)) =
                                store.running.iter_mut().find(|(id, _, _)| *id == change_id)
                            {
                                *detail = Some(line.into());
                                cx.notify();
                            }
                        });
                        if updated.is_err() {
                            break;
                        }
                    }
                })
            };
            let result = cli_thread(move || {
                cli.change(&change.args, &change.secrets, change.apply, Some(progress))
            })
            .await;
            drop(progress_task);
            this.update(cx, |store, cx| {
                store.running.retain(|(id, _, _)| *id != change_id);
                store.revision += 1;
                store.refresh_status(cx);
                store.refresh_providers(cx);
                cx.emit(StoreEvent::ChangeFinished {
                    label,
                    result: result.clone(),
                });
                cx.notify();
            })
            .ok();
            result
        })
    }
}

/// Runs a read-only CLI query off the main thread.
pub fn query<T: serde::de::DeserializeOwned + Send + 'static>(
    args: Vec<String>,
    cx: &App,
) -> Task<CliResult<T>> {
    let cli = Store::global(cx).read(cx).cli();
    let work = cli_thread(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        cli.query::<T>(&args)
    });
    cx.background_spawn(work)
}

/// Runs CLI work on a plain OS thread. Executor threads carry a background
/// QoS that child processes inherit, which throttles the CLI on macOS.
fn cli_thread<T: Send + 'static>(
    work: impl FnOnce() -> CliResult<T> + Send + 'static,
) -> impl std::future::Future<Output = CliResult<T>> + Send + 'static {
    let (sender, receiver) = smol::channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("codex-mixin-cli".into())
        .spawn(move || {
            let _ = sender.try_send(work());
        });
    async move {
        if let Err(error) = spawned {
            return Err(CliError::new(error.to_string()));
        }
        receiver
            .recv()
            .await
            .unwrap_or_else(|_| Err(CliError::new(rust_i18n::t!("cli.worker_stopped"))))
    }
}
