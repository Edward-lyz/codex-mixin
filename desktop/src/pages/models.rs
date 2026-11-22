//! One provider's model list. Selection, order and context edits stay a
//! draft until "Apply changes"; refresh, capability probing and benchmark
//! runs act immediately.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::tr;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::radio::Radio;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    Anchor, AnimationExt as _, AnyElement, App, AppContext as _, ClickEvent, Context, ElementId,
    Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div,
    prelude::FluentBuilder as _, uniform_list,
};
use rust_i18n::t;

use super::{is_busy, run_change, run_change_then};
use crate::model::{
    BenchmarkResult, BenchmarkSnapshot, BenchmarkStatus, ModelRefresh, Provider, ProviderModel,
};
use crate::store::{Change, Store, query};
use crate::theme;

/// Manually added models start with this context window; the user edits it
/// once the provider documents the real value.
const MANUAL_MODEL_CONTEXT: u64 = 128_000;
const BENCHMARK_POLL_INTERVAL: Duration = Duration::from_millis(900);
const BENCHMARK_TIMEOUTS: [u64; 4] = [10, 30, 60, 120];
/// The first entry is the gateway default used before a provider picks one.
const REFRESH_INTERVALS: [u64; 6] = [30, 5 * 60, 15 * 60, 30 * 60, 60 * 60, 6 * 60 * 60];
const DEFAULT_REFRESH_INTERVAL: u64 = REFRESH_INTERVALS[0];

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Added,
    NotAdded,
}

impl Filter {
    fn label(self) -> SharedString {
        match self {
            Filter::All => t!("models.filter.all"),
            Filter::Added => t!("models.filter.added"),
            Filter::NotAdded => t!("models.filter.not_added"),
        }
        .into()
    }
}

struct Row {
    id: String,
    model: Option<ProviderModel>,
}

pub struct ModelsView {
    provider: Provider,
    search: Entity<InputState>,
    add_input: Entity<InputState>,
    query: String,
    filter: Filter,
    selected: Vec<String>,
    contexts: BTreeMap<String, Option<u64>>,
    manual: Vec<String>,
    manual_order: bool,
    add_open: bool,
    refreshing: bool,
    benchmark_starting: bool,
    benchmark: Option<BenchmarkSnapshot>,
    benchmark_run: Option<String>,
    benchmark_status_error: Option<SharedString>,
    benchmark_ttft_only: bool,
    benchmark_timeout: u64,
    _benchmark_poll: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ModelsView {
    pub fn new(provider: Provider, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("models.search")));
        let add_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("models.add_placeholder")));
        let search_subscription = cx.subscribe(&search, |view, state, event, cx| {
            if matches!(event, InputEvent::Change) {
                view.query = state.read(cx).value().trim().to_lowercase();
                cx.notify();
            }
        });
        let add_subscription =
            cx.subscribe_in(
                &add_input,
                window,
                |view, _, event, window, cx| match event {
                    InputEvent::PressEnter { .. } => view.add_manual(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            );
        let mut view = Self {
            selected: provider.selected_models.clone(),
            contexts: BTreeMap::new(),
            manual_order: provider.model_order.as_deref() == Some("manual"),
            provider,
            search,
            add_input,
            query: String::new(),
            filter: Filter::All,
            manual: Vec::new(),
            add_open: false,
            refreshing: false,
            benchmark_starting: false,
            benchmark: None,
            benchmark_run: None,
            benchmark_status_error: None,
            benchmark_ttft_only: false,
            benchmark_timeout: 60,
            _benchmark_poll: None,
            _subscriptions: vec![search_subscription, add_subscription],
        };
        view.poll_benchmark(cx);
        view
    }

    /// Adopts a new snapshot. A clean view follows the saved state; a dirty
    /// draft keeps the user's edits.
    pub fn sync(&mut self, provider: Provider, _: &mut Window, cx: &mut Context<Self>) {
        let dirty = self.is_dirty();
        self.provider = provider;
        self.manual.retain(|id| {
            !self
                .provider
                .cached_models
                .iter()
                .any(|model| &model.id == id)
        });
        if dirty {
            self.contexts.retain(|id, tokens| {
                self.provider.model_context_overrides.get(id).copied() != *tokens
                    && (tokens.is_none()
                        || self.manual.contains(id)
                        || self
                            .provider
                            .cached_models
                            .iter()
                            .any(|model| &model.id == id))
            });
        } else {
            self.discard(cx);
        }
        cx.notify();
    }

    fn selection_changed(&self) -> bool {
        // Family order lets the CLI sort the selection, so only membership
        // counts; manual order keeps the user's sequence.
        if self.manual_order {
            self.selected != self.provider.selected_models
        } else {
            let mut draft = self.selected.clone();
            let mut saved = self.provider.selected_models.clone();
            draft.sort();
            saved.sort();
            draft != saved
        }
    }

    fn is_dirty(&self) -> bool {
        self.selection_changed()
            || !self.contexts.is_empty()
            || self.manual_order != (self.provider.model_order.as_deref() == Some("manual"))
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        self.selected = self.provider.selected_models.clone();
        self.contexts.clear();
        self.manual_order = self.provider.model_order.as_deref() == Some("manual");
        self.manual.clear();
        cx.notify();
    }

    fn apply(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let id = &self.provider.id;
        let order = if self.manual_order {
            "manual"
        } else {
            "family"
        };
        let mut args = vec!["provider".to_owned(), "select".to_owned(), id.clone()];
        let selection_changed = self.selection_changed();
        if selection_changed
            || self.manual_order != (self.provider.model_order.as_deref() == Some("manual"))
        {
            args.extend(["--model-order".to_owned(), order.to_owned()]);
        }
        if selection_changed {
            args.extend(self.selected.iter().map(|model| format!("--model={model}")));
        }
        for (model, tokens) in &self.contexts {
            if let Some(tokens) = tokens {
                args.push(format!("--model-context={model}={tokens}"));
            } else {
                args.push(format!("--clear-model-context={model}"));
            }
        }
        run_change(Change::new(t!("models.apply_label"), args).applied(), cx);
    }

    fn rows(&self) -> Vec<Row> {
        let cached = self.provider.cached_models.iter().map(|model| Row {
            id: model.id.clone(),
            model: Some(model.clone()),
        });
        let manual = self.manual.iter().map(|id| Row {
            id: id.clone(),
            model: None,
        });
        let mut rows: Vec<Row> = manual.chain(cached).collect();
        // Selected models the catalog no longer lists stay visible so they
        // can be removed.
        for id in &self.selected {
            if !rows.iter().any(|row| &row.id == id) {
                rows.push(Row {
                    id: id.clone(),
                    model: None,
                });
            }
        }
        if self.manual_order {
            rows.sort_by_key(|row| {
                self.selected
                    .iter()
                    .position(|id| id == &row.id)
                    .unwrap_or(usize::MAX)
            });
        }
        rows.retain(|row| self.matches(row));
        rows
    }

    fn matches(&self, row: &Row) -> bool {
        let selected = self.selected.contains(&row.id);
        let passes_filter = match self.filter {
            Filter::All => true,
            Filter::Added => selected,
            Filter::NotAdded => !selected,
        };
        if !passes_filter {
            return false;
        }
        if self.query.is_empty() {
            return true;
        }
        let name = row
            .model
            .as_ref()
            .and_then(|model| model.display_name.as_deref())
            .unwrap_or_default();
        row.id.to_lowercase().contains(&self.query) || name.to_lowercase().contains(&self.query)
    }

    fn set_selected(&mut self, id: &str, selected: bool, cx: &mut Context<Self>) {
        let present = self.selected.iter().any(|model| model == id);
        if selected && !present {
            self.selected.push(id.to_owned());
        } else if !selected && present {
            self.selected.retain(|model| model != id);
            if self.manual.iter().any(|model| model == id) {
                self.contexts.remove(id);
            }
        }
        cx.notify();
    }

    fn select_visible(&mut self, selected: bool, cx: &mut Context<Self>) {
        let unavailable = self.provider.unavailable_selected_models.clone();
        for row in self.rows() {
            if selected && unavailable.contains(&row.id) {
                continue;
            }
            self.set_selected(&row.id, selected, cx);
        }
    }

    fn move_selected(&mut self, id: &str, offset: isize, cx: &mut Context<Self>) {
        let Some(index) = self.selected.iter().position(|model| model == id) else {
            return;
        };
        let Some(target) = index
            .checked_add_signed(offset)
            .filter(|target| *target < self.selected.len())
        else {
            return;
        };
        self.selected.swap(index, target);
        cx.notify();
    }

    fn add_manual(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.add_input.read(cx).value().trim().to_owned();
        if id.is_empty() || self.provider.is_official() {
            return;
        }
        let known = self
            .provider
            .cached_models
            .iter()
            .any(|model| model.id == id);
        if !known && !self.manual.contains(&id) {
            self.manual.insert(0, id.clone());
        }
        self.set_selected(&id, true, cx);
        self.add_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.add_open = false;
        cx.notify();
    }

    fn open_context_editor(
        &self,
        id: &str,
        context: Option<u64>,
        editable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let id = id.to_owned();
        let has_override = self.context_override(&id).is_some();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(context.map(|tokens| tokens.to_string()).unwrap_or_default())
        });
        if editable {
            let input = input.clone();
            window.on_next_frame(move |window, cx| {
                input.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.select_all(window, cx);
                });
            });
        }
        window.open_dialog(cx, move |dialog, _, cx| {
            let valid_tokens = input
                .read(cx)
                .value()
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|tokens| *tokens > 0 && editable);
            let restore_view = view.clone();
            let restore_id = id.clone();
            let apply_view = view.clone();
            let apply_id = id.clone();
            let apply = move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let Some(tokens) = valid_tokens else {
                    return false;
                };
                apply_view
                    .update(cx, |view, cx| {
                        if Some(tokens) != context {
                            view.set_context(&apply_id, Some(tokens), cx);
                        }
                    })
                    .is_ok()
            };
            dialog
                .title(tr!("models.context.edit"))
                .w(gpui_kit::px(440.0))
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().child(id.clone()))
                        .child(Input::new(&input).disabled(!editable))
                        .when(editable, |body| {
                            body.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child(t!("models.context.tokens")),
                            )
                        }),
                )
                .on_ok(apply.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(
                            Button::new("restore-model-context")
                                .secondary()
                                .label(t!("models.context.restore"))
                                .disabled(!has_override)
                                .on_click(move |_, window, cx| {
                                    restore_view
                                        .update(cx, |view, cx| {
                                            view.set_context(&restore_id, None, cx);
                                        })
                                        .ok();
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("cancel-model-context")
                                .ghost()
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("update-model-context")
                                .primary()
                                .label(t!("models.context.done"))
                                .disabled(valid_tokens.is_none())
                                .on_click(move |event, window, cx| {
                                    if apply(event, window, cx) {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }

    fn context_override(&self, id: &str) -> Option<u64> {
        self.contexts
            .get(id)
            .copied()
            .unwrap_or_else(|| self.provider.model_context_overrides.get(id).copied())
    }

    fn set_context(&mut self, id: &str, tokens: Option<u64>, cx: &mut Context<Self>) {
        if self.provider.model_context_overrides.get(id).copied() == tokens {
            self.contexts.remove(id);
        } else {
            self.contexts.insert(id.to_owned(), tokens);
        }
        cx.notify();
    }

    fn refresh_models(&mut self, cx: &mut Context<Self>) {
        self.refreshing = true;
        let args = ["provider", "discover", self.provider.id.as_str()];
        let task = run_change_then(Change::new(t!("models.refresh_label"), args).applied(), cx);
        cx.notify();
        cx.spawn(async move |view, cx| {
            task.await;
            view.update(cx, |view, cx| {
                view.refreshing = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_model_refresh(&self, policy: ModelRefresh, cx: &mut App) {
        let args = vec![
            "provider".to_owned(),
            "update".to_owned(),
            self.provider.id.clone(),
            "--auto-refresh-models".to_owned(),
            policy.enabled.to_string(),
            "--model-refresh-interval-seconds".to_owned(),
            policy.interval_seconds.to_string(),
        ];
        run_change(Change::new(t!("models.refresh_policy_label"), args), cx);
    }

    fn refresh_policy(&self) -> ModelRefresh {
        self.provider.model_refresh.clone().unwrap_or(ModelRefresh {
            enabled: true,
            interval_seconds: DEFAULT_REFRESH_INTERVAL,
        })
    }

    fn start_benchmark(&mut self, cx: &mut Context<Self>) {
        self.benchmark_starting = true;
        let mut args = vec![
            "benchmark".to_owned(),
            "start".to_owned(),
            "--provider".to_owned(),
            self.provider.id.clone(),
            "--timeout-seconds".to_owned(),
            self.benchmark_timeout.to_string(),
        ];
        if self.benchmark_ttft_only {
            args.extend(["--target-output-tokens".to_owned(), "1".to_owned()]);
        }
        let task = Store::global(cx).update(cx, |store, cx| {
            store.run(Change::new(t!("models.benchmark_label"), args), cx)
        });
        cx.notify();
        cx.spawn(async move |view, cx| {
            let started = task.await;
            view.update(cx, |view, cx| {
                view.benchmark_starting = false;
                if let Ok(output) = started {
                    let acknowledged = serde_json::from_str::<BenchmarkStatus>(&output)
                        .ok()
                        .and_then(|status| status.snapshot);
                    view.benchmark_run = acknowledged
                        .as_ref()
                        .map(|snapshot| snapshot.run_id.clone());
                    if let Some(snapshot) = acknowledged {
                        view.benchmark = Some(snapshot);
                    }
                    view.poll_benchmark(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Polls `benchmark status` while a run is in progress.
    fn poll_benchmark(&mut self, cx: &mut Context<Self>) {
        self.benchmark_status_error = None;
        cx.notify();
        self._benchmark_poll = Some(cx.spawn(async move |view, cx| {
            loop {
                let status = cx
                    .update(|cx| {
                        query::<BenchmarkStatus>(vec!["benchmark".into(), "status".into()], cx)
                    })
                    .await;
                let running = view.update(cx, |view, cx| match status {
                    Ok(status) => {
                        if view.benchmark != status.snapshot {
                            view.benchmark = status.snapshot;
                            cx.notify();
                        }
                        view.benchmark_running()
                    }
                    Err(error) => {
                        if view.benchmark_running() {
                            view.benchmark_status_error = Some(error.message.into());
                            cx.notify();
                        }
                        false
                    }
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
                cx.background_executor()
                    .timer(BENCHMARK_POLL_INTERVAL)
                    .await;
            }
        }));
    }

    fn benchmark_result(&self, model: &str) -> Option<&BenchmarkResult> {
        self.benchmark
            .as_ref()?
            .results
            .iter()
            .find(|result| result.provider_id == self.provider.id && result.upstream_model == model)
    }

    fn probe_capabilities(&self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.provider.id.clone();
        let count = self.provider.selected_models.len();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let id = id.clone();
            dialog
                .title(SharedString::from(t!("models.probe_title")))
                .description(SharedString::from(t!("models.probe_detail", count = count)))
                .confirm()
                .ok_text(t!("models.probe_confirm"))
                .cancel_text(t!("common.cancel"))
                .on_ok(move |_, _, cx| {
                    run_change(
                        Change::new(t!("models.probe_label"), ["provider", "probe", id.as_str()])
                            .applied(),
                        cx,
                    );
                    true
                })
        });
    }
}

pub fn format_context(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens.is_multiple_of(1_000) {
        format!("{}K", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

pub fn format_latency(milliseconds: f64) -> String {
    if milliseconds >= 1_000.0 {
        format!("{:.2} s", milliseconds / 1_000.0)
    } else {
        format!("{milliseconds:.0} ms")
    }
}

pub fn format_interval(seconds: u64) -> SharedString {
    if seconds >= 3_600 {
        t!("time.hours", count = seconds / 3_600).into()
    } else if seconds < 60 {
        t!("time.seconds", count = seconds).into()
    } else {
        t!("time.minutes", count = seconds / 60).into()
    }
}

pub fn format_timestamp(milliseconds: i64) -> Option<String> {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(milliseconds)
        .single()
        .map(|time| time.format("%m-%d %H:%M").to_string())
}

/// Fixed column widths shared by the header and every row.
fn column(kind: Column) -> gpui_kit::Div {
    let cell = div().flex_none().flex().items_center();
    match kind {
        Column::Join => cell.w_20().justify_center(),
        Column::Order => cell.w_12().justify_center(),
        Column::Model => div().flex_1().min_w_0().flex().items_center(),
        Column::Context => cell.w_20().justify_end(),
        Column::Capabilities => cell.w_24().justify_center(),
        Column::Latency | Column::Speed => cell.w_24().justify_end(),
    }
}

#[derive(Clone, Copy)]
enum Column {
    Join,
    Order,
    Model,
    Context,
    Capabilities,
    Latency,
    Speed,
}

impl ModelsView {
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = is_busy(cx);
        let view = cx.entity().downgrade();
        let filter = self.filter;
        let filter_button = Button::new("model-filter")
            .secondary()
            .small()
            .label(filter.label())
            .dropdown_caret(true)
            .dropdown_menu({
                let view = view.clone();
                move |menu, _, _| {
                    [Filter::All, Filter::Added, Filter::NotAdded]
                        .into_iter()
                        .fold(menu, |menu, option| {
                            let view = view.clone();
                            menu.item(
                                PopupMenuItem::new(option.label())
                                    .checked(option == filter)
                                    .on_click(move |_, _, cx| {
                                        view.update(cx, |view, cx| {
                                            view.filter = option;
                                            cx.notify();
                                        })
                                        .ok();
                                    }),
                            )
                        })
                }
            });
        let manual_order = self.manual_order;
        let order_button = Button::new("model-order")
            .secondary()
            .small()
            .icon(IconName::ArrowUpDown)
            .tooltip(t!("models.order_tooltip"))
            .dropdown_menu({
                let view = view.clone();
                move |menu, _, _| {
                    let family = view.clone();
                    let manual = view.clone();
                    menu.item(
                        PopupMenuItem::new(t!("models.order.family"))
                            .checked(!manual_order)
                            .on_click(move |_, _, cx| set_order(&family, false, cx)),
                    )
                    .item(
                        PopupMenuItem::new(t!("models.order.manual"))
                            .checked(manual_order)
                            .on_click(move |_, _, cx| set_order(&manual, true, cx)),
                    )
                }
            });
        let saved_count = self.provider.selected_models.len();
        let retry_status = self.benchmark_status_error.is_some();
        h_flex()
            .px_6()
            .gap_2()
            .child(
                div().w_64().child(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).small()),
                ),
            )
            .child(filter_button)
            .child(order_button)
            .child(div().flex_1())
            .child(
                Button::new("add-model")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label(t!("models.add"))
                    .disabled(self.provider.is_official())
                    .toggled(self.add_open)
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.add_open = !view.add_open;
                        if view.add_open {
                            view.add_input
                                .update(cx, |input, cx| input.focus(window, cx));
                        }
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .child(
                        Button::new("refresh-models")
                            .secondary()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(t!("models.refresh"))
                            .loading(self.refreshing)
                            .disabled(busy)
                            .on_click(cx.listener(|view, _, _, cx| view.refresh_models(cx))),
                    )
                    .child(
                        Popover::new("refresh-settings")
                            .anchor(Anchor::TopRight)
                            .trigger(
                                Button::new("refresh-settings-trigger")
                                    .secondary()
                                    .small()
                                    .icon(IconName::ChevronDown)
                                    .tooltip(t!("models.refresh_settings")),
                            )
                            .content({
                                let view = view.clone();
                                move |_, _, cx| render_refresh_settings(&view, cx)
                            }),
                    ),
            )
            .child(
                h_flex()
                    .child(
                        Button::new("start-benchmark")
                            .secondary()
                            .small()
                            .icon(IconName::Gauge)
                            .label(if retry_status {
                                t!("models.benchmark_retry_status")
                            } else {
                                t!("models.benchmark", count = saved_count)
                            })
                            .loading(self.benchmark_starting)
                            .disabled(
                                !retry_status
                                    && (busy || saved_count == 0 || self.benchmark_running()),
                            )
                            .on_click(cx.listener(|view, _, _, cx| {
                                if view.benchmark_status_error.is_some() {
                                    view.poll_benchmark(cx);
                                } else {
                                    view.start_benchmark(cx);
                                }
                            })),
                    )
                    .child(
                        Popover::new("benchmark-settings")
                            .anchor(Anchor::TopRight)
                            .trigger(
                                Button::new("benchmark-settings-trigger")
                                    .secondary()
                                    .small()
                                    .icon(IconName::ChevronDown)
                                    .tooltip(t!("models.benchmark_settings")),
                            )
                            .content(move |_, _, cx| render_benchmark_settings(&view, cx)),
                    ),
            )
    }

    fn benchmark_running(&self) -> bool {
        self.benchmark
            .as_ref()
            .is_some_and(|snapshot| snapshot.status == "running")
    }
}

fn set_order(view: &WeakEntity<ModelsView>, manual: bool, cx: &mut App) {
    view.update(cx, |view, cx| {
        view.manual_order = manual;
        cx.notify();
    })
    .ok();
}

fn popover_row(label: SharedString, control: impl IntoElement) -> impl IntoElement {
    h_flex()
        .gap_6()
        .justify_between()
        .child(div().text_sm().child(label))
        .child(control)
}

fn render_refresh_settings(view: &WeakEntity<ModelsView>, cx: &mut App) -> AnyElement {
    let Some(entity) = view.upgrade() else {
        return div().into_any_element();
    };
    let colors = cx.theme().colors;
    let model_view = entity.read(cx);
    let policy = model_view.refresh_policy();
    let refreshed = model_view
        .provider
        .models_refreshed_at_ms
        .and_then(format_timestamp)
        .map_or_else(
            || t!("models.never_refreshed"),
            |time| t!("models.last_refreshed", time = time),
        );
    let error = model_view.provider.last_model_refresh_error.clone();
    let toggle_view = view.clone();
    let toggle_policy = policy.clone();
    let interval_view = view.clone();
    let interval_policy = policy.clone();
    v_flex()
        .w_72()
        .gap_3()
        .p_1()
        .child(popover_row(
            t!("models.auto_refresh").into(),
            Switch::new("auto-refresh")
                .small()
                .checked(policy.enabled)
                .on_change(move |enabled, _, cx| {
                    let policy = ModelRefresh {
                        enabled: *enabled,
                        ..toggle_policy.clone()
                    };
                    toggle_view
                        .update(cx, |view, cx| view.set_model_refresh(policy, cx))
                        .ok();
                }),
        ))
        .child(popover_row(
            t!("models.refresh_interval").into(),
            Button::new("refresh-interval")
                .secondary()
                .xsmall()
                .label(format_interval(policy.interval_seconds))
                .dropdown_caret(true)
                .disabled(!policy.enabled)
                .dropdown_menu(move |menu, _, _| {
                    REFRESH_INTERVALS.into_iter().fold(menu, |menu, seconds| {
                        let view = interval_view.clone();
                        let policy = ModelRefresh {
                            interval_seconds: seconds,
                            ..interval_policy.clone()
                        };
                        menu.item(
                            PopupMenuItem::new(format_interval(seconds))
                                .checked(seconds == interval_policy.interval_seconds)
                                .on_click(move |_, _, cx| {
                                    let policy = policy.clone();
                                    view.update(cx, |view, cx| view.set_model_refresh(policy, cx))
                                        .ok();
                                }),
                        )
                    })
                }),
        ))
        .child(div().h_px().bg(colors.border))
        .child(
            v_flex()
                .gap_1()
                .text_xs()
                .text_color(colors.muted_foreground)
                .child(refreshed)
                .when_some(error, |text, error| {
                    text.child(div().text_color(colors.danger).child(error))
                }),
        )
        .into_any_element()
}

fn render_benchmark_settings(view: &WeakEntity<ModelsView>, cx: &mut App) -> AnyElement {
    let Some(entity) = view.upgrade() else {
        return div().into_any_element();
    };
    let colors = cx.theme().colors;
    let model_view = entity.read(cx);
    let ttft_only = model_view.benchmark_ttft_only;
    let timeout = model_view.benchmark_timeout;
    let mode = |ttft: bool| {
        let view = view.clone();
        move |_: &bool, _: &mut Window, cx: &mut App| {
            view.update(cx, |view, cx| {
                view.benchmark_ttft_only = ttft;
                cx.notify();
            })
            .ok();
        }
    };
    let timeout_view = view.clone();
    v_flex()
        .w_72()
        .gap_2()
        .p_1()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(colors.muted_foreground)
                .child(t!("models.benchmark_content")),
        )
        .child(
            Radio::new("benchmark-ttft")
                .label(tr!("models.benchmark_ttft"))
                .checked(ttft_only)
                .on_change(mode(true)),
        )
        .child(
            Radio::new("benchmark-full")
                .label(tr!("models.benchmark_full"))
                .checked(!ttft_only)
                .on_change(mode(false)),
        )
        .child(div().my_1().h_px().bg(colors.border))
        .child(popover_row(
            t!("models.benchmark_timeout").into(),
            Button::new("benchmark-timeout")
                .secondary()
                .xsmall()
                .label(t!("time.seconds", count = timeout))
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    BENCHMARK_TIMEOUTS.into_iter().fold(menu, |menu, seconds| {
                        let view = timeout_view.clone();
                        menu.item(
                            PopupMenuItem::new(t!("time.seconds", count = seconds))
                                .checked(seconds == timeout)
                                .on_click(move |_, _, cx| {
                                    view.update(cx, |view, cx| {
                                        view.benchmark_timeout = seconds;
                                        cx.notify();
                                    })
                                    .ok();
                                }),
                        )
                    })
                }),
        ))
        .into_any_element()
}

impl ModelsView {
    fn render_add_form(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let empty = self.add_input.read(cx).value().trim().is_empty();
        h_flex()
            .mx_6()
            .mt_3()
            .px_3()
            .py_2()
            .gap_3()
            .rounded_lg()
            .bg(colors.muted)
            .child(div().text_sm().font_medium().child(t!("models.model_id")))
            .child(div().w_72().child(Input::new(&self.add_input).small()))
            .child(
                Button::new("confirm-add-model")
                    .primary()
                    .small()
                    .label(t!("models.add_confirm"))
                    .disabled(empty)
                    .on_click(cx.listener(|view, _, window, cx| view.add_manual(window, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("close-add-model")
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.add_open = false;
                        cx.notify();
                    })),
            )
            .with_animation(
                "add-model-form",
                gpui_kit::Animation::new(theme::ENTER_DURATION)
                    .with_easing(theme::curve(theme::ENTER_CURVE)),
                |form, progress| form.opacity(progress),
            )
    }

    fn render_header_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let cell = |kind: Column, label: SharedString| column(kind).child(label);
        h_flex()
            .mx_6()
            .h_8()
            .text_xs()
            .font_medium()
            .text_color(colors.table_head_foreground)
            .border_b_1()
            .border_color(colors.border)
            .child(cell(Column::Join, t!("models.column.join").into()))
            .child(cell(Column::Order, t!("models.column.order").into()))
            .child(cell(Column::Model, t!("models.column.model").into()))
            .child(cell(Column::Context, t!("models.column.context").into()))
            .child(cell(
                Column::Capabilities,
                t!("models.column.capabilities").into(),
            ))
            .child(cell(Column::Latency, t!("models.column.ttft").into()))
            .child(cell(Column::Speed, t!("models.column.speed").into()))
    }

    fn render_row(&self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let position = self.selected.iter().position(|id| id == &row.id);
        let selected = position.is_some();
        let unavailable = self.provider.unavailable_selected_models.contains(&row.id);
        let model = row.model.as_ref();
        let name = model
            .and_then(|model| model.display_name.as_deref())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&row.id);
        let manual = self.manual.contains(&row.id);
        let context_override = self.context_override(&row.id);
        let context = context_override
            .or_else(|| {
                model.and_then(|model| {
                    if self.provider.model_context_overrides.contains_key(&row.id) {
                        model.source_context_window
                    } else {
                        model.context_window
                    }
                })
            })
            .or(manual.then_some(MANUAL_MODEL_CONTEXT));
        let context_label = context.map_or_else(|| "\u{2014}".to_owned(), format_context);
        let context_editable = model.is_some() || (manual && selected);
        let context_id = row.id.clone();
        let result = self.benchmark_result(&row.id);
        let muted = |text: String| div().text_color(colors.muted_foreground).child(text);
        let context_cell = column(Column::Context);
        let context_cell = if context_editable || context_override.is_some() {
            context_cell.child(
                Button::new("edit-model-context")
                    .ghost()
                    .xsmall()
                    .label(context_label)
                    .icon(IconName::Pencil)
                    .tooltip(t!("models.context.edit"))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.open_context_editor(
                            &context_id,
                            context,
                            context_editable,
                            window,
                            cx,
                        );
                    })),
            )
        } else {
            context_cell.child(muted(context_label))
        };
        let latency = match result {
            Some(result) if result.status == "completed" => result.ttft_ms.map(format_latency),
            Some(result) if result.status == "running" => Some(t!("models.measuring").into()),
            Some(_) => Some(t!("models.failed").into()),
            None => None,
        };
        let speed = result
            .and_then(|result| result.tps)
            .map(|tps| format!("{tps:.1} tok/s"));
        let id = row.id.clone();
        let toggle_id = row.id.clone();
        let up_id = row.id.clone();
        let down_id = row.id.clone();
        let capability = |supported: Option<bool>, icon: IconName, tooltip: SharedString| {
            supported.filter(|supported| *supported).map(|_| {
                div()
                    .id(ElementId::Name(tooltip.clone()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_5()
                    .rounded_md()
                    .bg(colors.muted)
                    .child(Icon::new(icon).xsmall().text_color(colors.muted_foreground))
            })
        };
        h_flex()
            .id(ElementId::Name(format!("model-{id}").into()))
            .group("model-row")
            .w_full()
            .h_9()
            .text_sm()
            .rounded_md()
            .hover(|style| style.bg(colors.table_hover))
            .child(
                column(Column::Join).child(
                    Checkbox::new(ElementId::Name(format!("join-{id}").into()))
                        .checked(selected)
                        .on_change(cx.listener(move |view, checked: &bool, _, cx| {
                            view.set_selected(&toggle_id, *checked, cx);
                        })),
                ),
            )
            .child(column(Column::Order).when_some(position, |cell, position| {
                cell.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .min_w_5()
                        .h_5()
                        .px_1()
                        .rounded_full()
                        .bg(colors.muted)
                        .text_xs()
                        .font_semibold()
                        .text_color(colors.foreground)
                        .child((position + 1).to_string()),
                )
            }))
            .child(
                column(Column::Model)
                    .gap_2()
                    .child(
                        div()
                            .truncate()
                            .when(selected, |name| name.font_medium())
                            .when(!selected, |name| name.text_color(colors.muted_foreground))
                            .child(name.to_owned()),
                    )
                    .when(model.is_none(), |cell| {
                        cell.child(crate::widgets::pill(
                            t!("models.manual"),
                            colors.muted_foreground,
                        ))
                    })
                    .when(unavailable, |cell| {
                        cell.child(crate::widgets::pill(
                            t!("models.unavailable"),
                            colors.warning,
                        ))
                    })
                    .when(self.manual_order && selected, |cell| {
                        cell.child(
                            h_flex()
                                .invisible()
                                .group_hover("model-row", |style| style.visible())
                                .child(
                                    Button::new(ElementId::Name(format!("up-{id}").into()))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::ChevronUp)
                                        .on_click(cx.listener(move |view, _, _, cx| {
                                            view.move_selected(&up_id, -1, cx)
                                        })),
                                )
                                .child(
                                    Button::new(ElementId::Name(format!("down-{id}").into()))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::ChevronDown)
                                        .on_click(cx.listener(move |view, _, _, cx| {
                                            view.move_selected(&down_id, 1, cx)
                                        })),
                                ),
                        )
                    }),
            )
            .child(context_cell)
            .child(
                column(Column::Capabilities)
                    .gap_1()
                    .children(capability(
                        model.and_then(|model| model.supports_image),
                        IconName::Image,
                        t!("models.capability.image").into(),
                    ))
                    .children(capability(
                        model.and_then(|model| model.supports_function_tools),
                        IconName::Wrench,
                        t!("models.capability.tools").into(),
                    ))
                    .children(capability(
                        model.and_then(|model| model.supports_thinking),
                        IconName::Brain,
                        t!("models.capability.thinking").into(),
                    )),
            )
            .child(column(Column::Latency).when_some(latency, |cell, text| cell.child(muted(text))))
            .child(column(Column::Speed).when_some(speed, |cell, text| cell.child(muted(text))))
            .into_any_element()
    }
}

impl ModelsView {
    fn render_footer(&self, visible: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        h_flex()
            .px_6()
            .h_11()
            .gap_2()
            .border_t_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(t!(
                        "models.summary",
                        visible = visible,
                        selected = self.selected.len()
                    )),
            )
            .child(
                Button::new("select-visible")
                    .ghost()
                    .xsmall()
                    .label(t!("models.select_visible"))
                    .disabled(visible == 0)
                    .on_click(cx.listener(|view, _, _, cx| view.select_visible(true, cx))),
            )
            .child(
                Button::new("deselect-visible")
                    .ghost()
                    .xsmall()
                    .label(t!("models.deselect_visible"))
                    .disabled(visible == 0)
                    .on_click(cx.listener(|view, _, _, cx| view.select_visible(false, cx))),
            )
            .child(div().flex_1())
            .children(self.benchmark_progress(cx))
            .child(
                Button::new("probe-capabilities")
                    .secondary()
                    .xsmall()
                    .icon(IconName::ScanSearch)
                    .label(t!("models.probe"))
                    .disabled(is_busy(cx) || self.provider.selected_models.is_empty())
                    .on_click(
                        cx.listener(|view, _, window, cx| view.probe_capabilities(window, cx)),
                    ),
            )
    }

    fn benchmark_progress(&self, cx: &App) -> Option<impl IntoElement> {
        let snapshot = self.benchmark.as_ref()?;
        let running = snapshot.status == "running";
        let own_run = self.benchmark_run.as_deref() == Some(snapshot.run_id.as_str());
        let colors = cx.theme().colors;
        let done = snapshot.results.len();
        let total = snapshot.total_models;
        let failed = snapshot
            .results
            .iter()
            .filter(|result| result.status != "completed")
            .count();
        let (label, icon, color) = match snapshot.status.as_str() {
            "running" if self.benchmark_status_error.is_some() => (
                t!("models.benchmark_status_unavailable"),
                Some(IconName::CircleAlert),
                colors.warning,
            ),
            "running" => (
                t!("models.benchmark_progress", done = done, total = total),
                None,
                colors.muted_foreground,
            ),
            "completed" if own_run && failed > 0 => (
                t!("models.benchmark_errors", failed = failed, total = total),
                Some(IconName::CircleAlert),
                colors.warning,
            ),
            "completed" if own_run => (
                t!("models.benchmark_completed", count = done),
                Some(IconName::CircleCheck),
                colors.success,
            ),
            "interrupted" if own_run => (
                t!("models.benchmark_interrupted"),
                Some(IconName::CircleAlert),
                colors.warning,
            ),
            "failed" if own_run => (
                t!("models.benchmark_failed"),
                Some(IconName::CircleAlert),
                colors.danger,
            ),
            _ => return None,
        };
        Some(
            h_flex()
                .id("benchmark-status")
                .gap_2()
                .min_w_0()
                .text_xs()
                .text_color(color)
                .when_some(icon, |status, icon| {
                    status.child(Icon::new(icon).xsmall().flex_none())
                })
                .when(
                    running && self.benchmark_status_error.is_none() && total > 0,
                    |status| {
                        status.child(
                            Progress::new((SharedString::from(snapshot.run_id.clone()), 0usize))
                                .xsmall()
                                .w_16()
                                .flex_none()
                                .value(100.0 * done as f32 / total as f32)
                                .accessibility_label(t!("models.measuring")),
                        )
                    },
                )
                .child(div().truncate().child(label))
                .when_some(
                    self.benchmark_status_error
                        .clone()
                        .or_else(|| snapshot.error.clone().map(SharedString::from)),
                    |status, error| {
                        status.tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(error.clone())
                                .build(window, cx)
                        })
                    },
                ),
        )
    }

    fn render_apply_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let removing_all = self.selected.is_empty() && self.selection_changed();
        h_flex()
            .absolute()
            .bottom_16()
            .left_0()
            .right_0()
            .justify_center()
            .child(
                h_flex()
                    .gap_3()
                    .pl_4()
                    .pr_2()
                    .py_2()
                    .rounded_xl()
                    .bg(colors.popover)
                    .border_1()
                    .border_color(colors.border)
                    .shadow_lg()
                    .child(div().text_sm().child(if removing_all {
                        t!("models.keep_one")
                    } else {
                        t!("models.unapplied")
                    }))
                    .child(
                        Button::new("discard-models")
                            .ghost()
                            .small()
                            .label(t!("common.discard"))
                            .on_click(cx.listener(|view, _, _, cx| view.discard(cx))),
                    )
                    .child(
                        Button::new("apply-models")
                            .primary()
                            .small()
                            .label(t!("common.apply"))
                            .loading(is_busy(cx))
                            .disabled(is_busy(cx) || removing_all)
                            .on_click(cx.listener(|view, _, window, cx| view.apply(window, cx))),
                    ),
            )
            .with_animation(
                "apply-bar",
                gpui_kit::Animation::new(theme::MOVE_DURATION)
                    .with_easing(theme::curve(theme::MOVE_CURVE)),
                |bar, progress| {
                    bar.opacity(progress)
                        .mb(gpui_kit::px(-16.0 * (1.0 - progress)))
                },
            )
    }
}

impl Render for ModelsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows();
        let count = rows.len();
        let list = uniform_list(
            "model-rows",
            count,
            cx.processor(|view, range: std::ops::Range<usize>, _, cx| {
                let rows = view.rows();
                range
                    .filter_map(|index| rows.get(index).map(|row| view.render_row(row, cx)))
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0()
        .px_6()
        .pb_2();
        let empty = count == 0;
        v_flex()
            .relative()
            .size_full()
            .child(self.render_toolbar(cx))
            .when(self.add_open, |body| body.child(self.render_add_form(cx)))
            .child(div().h_3())
            .child(self.render_header_row(cx))
            .when(!empty, |body| body.child(list))
            .when(empty, |body| {
                body.child(div().flex_1().child(super::placeholder(
                    IconName::Search,
                    t!("models.no_results"),
                    None,
                    None,
                    cx,
                )))
            })
            .child(self.render_footer(count, cx))
            .when(self.is_dirty(), |body| {
                body.child(self.render_apply_bar(cx))
            })
    }
}
