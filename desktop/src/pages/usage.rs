//! Usage page: daily totals from `usage --daily`, compared with the period
//! before, plus a per-model breakdown.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::chart::BarChart;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, AppContext as _, Context, ElementId, Entity,
    Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use rust_i18n::t;
use serde::Deserialize;

use super::placeholder;
use crate::store::query;
use crate::theme;
use crate::tr;
use crate::widgets::{Segmented, pill, status_dot};

const RANGES: [u64; 3] = [7, 30, 90];
const SECONDS_PER_DAY: i64 = 86_400;

#[derive(Clone, Debug, Default, Deserialize)]
struct DailyUsage {
    day: i64,
    provider_id: String,
    model_id: String,
    request_count: u64,
    #[serde(default)]
    error_count: u64,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    estimated_cost_usd: Option<f64>,
    #[serde(default)]
    average_ttft_ms: Option<f64>,
    #[serde(default)]
    output_tps: Option<f64>,
}

#[derive(Clone, Default)]
struct Totals {
    requests: u64,
    errors: u64,
    input: u64,
    output: u64,
    cache_read: u64,
    cost: f64,
    /// Request-weighted latency sum, divided by `latency_requests`.
    latency_sum: f64,
    latency_requests: u64,
}

impl Totals {
    fn add(&mut self, row: &DailyUsage) {
        self.requests += row.request_count;
        self.errors += row.error_count;
        self.input += row.input_tokens;
        self.output += row.output_tokens;
        self.cache_read += row.cache_read_tokens;
        self.cost += row.estimated_cost_usd.unwrap_or_default();
        if let Some(latency) = row.average_ttft_ms {
            self.latency_sum += latency * row.request_count as f64;
            self.latency_requests += row.request_count;
        }
    }

    fn latency(&self) -> Option<f64> {
        (self.latency_requests > 0).then(|| self.latency_sum / self.latency_requests as f64)
    }

    fn cache_hit(&self) -> Option<f64> {
        (self.input > 0).then(|| self.cache_read as f64 / self.input as f64 * 100.0)
    }
}

/// Newest turns loaded by the Turn view.
const TURN_LIMIT: usize = 200;
/// How often the Turn view reads new turns while it follows the ledger.
const FOLLOW_INTERVAL: Duration = Duration::from_secs(3);
/// Turns of the selected session listed in the inspector.
const SESSION_TURNS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UsageView {
    Overview,
    Turns,
}

/// One upstream reply from `usage --turns`.
#[derive(Clone, Debug, Deserialize)]
struct Turn {
    at_ms: i64,
    session: String,
    #[serde(default)]
    session_display: Option<String>,
    #[serde(default)]
    client: Option<String>,
    provider_id: String,
    catalog_slug: String,
    #[serde(default)]
    upstream_model_id: String,
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_tokens: Option<u64>,
    #[serde(default)]
    ttft_ms: Option<f64>,
    #[serde(default)]
    output_tps: Option<f64>,
    #[serde(default)]
    estimated_cost_usd: Option<f64>,
}

impl Turn {
    fn key(&self) -> String {
        format!(
            "{}-{}-{}-{}-{}",
            self.at_ms, self.session, self.provider_id, self.upstream_model_id, self.catalog_slug
        )
    }

    fn failed(&self) -> bool {
        self.status == "failed"
    }

    fn title(&self) -> SharedString {
        self.session_display
            .clone()
            .unwrap_or_else(|| self.session.clone())
            .into()
    }

    fn agent(&self) -> SharedString {
        self.client
            .clone()
            .unwrap_or_else(|| NO_VALUE.to_owned())
            .into()
    }

    fn tokens(&self) -> Option<u64> {
        match (self.input_tokens, self.output_tokens) {
            (None, None) => None,
            (input, output) => Some(input.unwrap_or_default() + output.unwrap_or_default()),
        }
    }

    /// Clock time for today's turns, date and time for older ones.
    fn time(&self) -> SharedString {
        let Some(time) = chrono::DateTime::from_timestamp_millis(self.at_ms) else {
            return NO_VALUE.into();
        };
        let local = time.with_timezone(&chrono::Local);
        let format = if local.date_naive() == chrono::Local::now().date_naive() {
            "%H:%M:%S"
        } else {
            "%m-%d %H:%M"
        };
        local.format(format).to_string().into()
    }

    fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || [
                self.session_display.as_deref().unwrap_or_default(),
                self.client.as_deref().unwrap_or_default(),
                self.catalog_slug.as_str(),
                self.provider_id.as_str(),
            ]
            .iter()
            .any(|field| field.to_lowercase().contains(query))
    }
}

#[derive(Clone)]
struct DayBar {
    label: SharedString,
    tokens: f64,
}

pub struct UsagePage {
    view: UsageView,
    range: usize,
    rows: Option<Vec<DailyUsage>>,
    error: Option<SharedString>,
    loading: bool,
    turns: Option<Vec<Turn>>,
    turns_error: Option<SharedString>,
    turns_loading: bool,
    /// Whether the console shows this page; the Turn view only polls then.
    active: bool,
    /// Whether the Turn view reads new turns as they are recorded.
    follow: bool,
    failed_only: bool,
    selected_turn: Option<String>,
    /// Turns that arrived with the last read, drawn with an entrance.
    fresh: HashSet<String>,
    search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

fn today() -> i64 {
    chrono::Local::now().timestamp().div_euclid(SECONDS_PER_DAY)
}

fn day_label(day: i64) -> SharedString {
    chrono::DateTime::from_timestamp(day * SECONDS_PER_DAY, 0)
        .map(|time| time.format("%m-%d").to_string())
        .unwrap_or_default()
        .into()
}

pub fn compact_number(value: f64) -> String {
    if value >= 1_000_000_000.0 {
        format!("{:.2}B", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.2}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

impl UsagePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("usage.turn.search")));
        let search_subscription = cx.subscribe(&search, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        // Follow the ledger while the Turn view is on screen; the loop ends
        // with the page.
        cx.spawn(async move |page, cx| {
            loop {
                cx.background_executor().timer(FOLLOW_INTERVAL).await;
                let polled = page.update(cx, |page, cx| {
                    if page.active && page.follow && page.view == UsageView::Turns {
                        page.reload_turns(cx);
                    }
                });
                if polled.is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut page = Self {
            view: UsageView::Overview,
            range: 0,
            rows: None,
            error: None,
            loading: false,
            turns: None,
            turns_error: None,
            turns_loading: false,
            active: true,
            follow: true,
            failed_only: false,
            selected_turn: None,
            fresh: HashSet::new(),
            search,
            _subscriptions: vec![search_subscription],
        };
        page.reload(cx);
        page
    }

    /// Called by the console when it shows or leaves this page.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active && self.view == UsageView::Turns {
            self.reload_turns(cx);
        }
    }

    fn reload_turns(&mut self, cx: &mut Context<Self>) {
        if self.turns_loading {
            return;
        }
        self.turns_loading = true;
        let task = query::<Vec<Turn>>(
            vec![
                "usage".into(),
                "--turns".into(),
                "--json".into(),
                "--limit".into(),
                TURN_LIMIT.to_string(),
            ],
            cx,
        );
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                page.turns_loading = false;
                match result {
                    Ok(turns) => {
                        // The first read has nothing to compare with, so
                        // only later reads mark turns as new.
                        page.fresh = match &page.turns {
                            Some(previous) => {
                                let known: HashSet<String> =
                                    previous.iter().map(Turn::key).collect();
                                turns
                                    .iter()
                                    .map(Turn::key)
                                    .filter(|key| !known.contains(key))
                                    .collect()
                            }
                            None => HashSet::new(),
                        };
                        page.turns = Some(turns);
                        page.turns_error = None;
                    }
                    Err(error) => page.turns_error = Some(error.message.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show(&mut self, view: UsageView, cx: &mut Context<Self>) {
        self.view = view;
        if view == UsageView::Turns {
            self.reload_turns(cx);
        }
        cx.notify();
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        // Twice the range, so the cards can compare with the period before.
        let days = RANGES[self.range] * 2;
        let task = query::<Vec<DailyUsage>>(
            vec![
                "usage".into(),
                "--daily".into(),
                "--json".into(),
                "--days".into(),
                days.to_string(),
            ],
            cx,
        );
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                page.loading = false;
                match result {
                    Ok(rows) => {
                        page.rows = Some(rows);
                        page.error = None;
                    }
                    Err(error) => page.error = Some(error.message.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn card(
        title: SharedString,
        value: String,
        current: f64,
        previous: f64,
        lower_is_better: bool,
        cx: &App,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let change = (previous > 0.0).then(|| (current - previous) / previous * 100.0);
        let delta = match change {
            Some(change) => {
                let better = (change < 0.0) == lower_is_better;
                let color: Hsla = if change.abs() < 0.5 {
                    colors.muted_foreground
                } else if better {
                    colors.success
                } else {
                    colors.danger
                };
                let arrow = if change >= 0.0 {
                    "\u{2191}"
                } else {
                    "\u{2193}"
                };
                div().text_xs().text_color(color).child(t!(
                    "usage.change",
                    arrow = arrow,
                    percent = format!("{:.0}", change.abs())
                ))
            }
            None => div()
                .text_xs()
                .text_color(colors.muted_foreground)
                .child(t!("usage.no_previous")),
        };
        v_flex()
            .flex_1()
            .min_w_32()
            .gap_1p5()
            .p_5()
            .rounded_xl()
            .border_1()
            .border_color(colors.border)
            .bg(colors.group_box)
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(title),
            )
            .child(div().text_2xl().font_semibold().child(value))
            .child(delta)
    }
}

const NO_VALUE: &str = "\u{2014}";

impl UsagePage {
    fn render_overview(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let Some(rows) = self.rows.clone() else {
            return match self.error.clone() {
                Some(error) => placeholder(
                    IconName::TriangleAlert,
                    t!("usage.load_failed"),
                    Some(error),
                    None,
                    cx,
                )
                .into_any_element(),
                None => placeholder(IconName::LoaderCircle, t!("usage.loading"), None, None, cx)
                    .into_any_element(),
            };
        };
        let days = RANGES[self.range] as i64;
        let today = today();
        let start = today - days + 1;
        let mut current = Totals::default();
        let mut previous = Totals::default();
        let mut per_day: BTreeMap<i64, f64> = (start..=today).map(|day| (day, 0.0)).collect();
        let mut per_model: BTreeMap<(String, String), (Totals, Option<f64>)> = BTreeMap::new();
        for row in &rows {
            if row.day < start {
                previous.add(row);
                continue;
            }
            current.add(row);
            *per_day.entry(row.day).or_default() += (row.input_tokens + row.output_tokens) as f64;
            let entry = per_model
                .entry((row.model_id.clone(), row.provider_id.clone()))
                .or_default();
            entry.0.add(row);
            if row.output_tps.is_some() {
                entry.1 = row.output_tps;
            }
        }
        let bars: Vec<DayBar> = per_day
            .into_iter()
            .map(|(day, tokens)| DayBar {
                label: day_label(day),
                tokens,
            })
            .collect();
        let mut models: Vec<_> = per_model.into_iter().collect();
        models.sort_by_key(|(_, (totals, _))| std::cmp::Reverse(totals.requests));
        let primary = colors.chart_1;
        let chart = BarChart::new(bars)
            .id("usage-daily")
            .band(|bar: &DayBar| bar.label.clone())
            .value(|bar: &DayBar| bar.tokens)
            .fill_gradient(move |_, _, _| {
                [
                    gpui_kit::linear_color_stop(primary.opacity(0.35), 0.0),
                    gpui_kit::linear_color_stop(primary, 1.0),
                ]
            })
            .corner_radii(px(3.0))
            .value_axis(true)
            .value_tick_format(compact_number)
            .tooltip_value(|_, value| SharedString::from(compact_number(value)))
            .band_tick_count(if days > 30 { 8 } else { days as usize });
        let head = |label: SharedString| {
            div()
                .w_24()
                .text_right()
                .text_xs()
                .font_medium()
                .text_color(colors.table_head_foreground)
                .child(label)
        };
        let rows_view =
            models
                .iter()
                .enumerate()
                .map(|(index, ((model, provider), (totals, tps)))| {
                    let cell = |text: String| div().w_24().text_right().text_sm().child(text);
                    h_flex()
                        .px_5()
                        .h_12()
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(colors.border)
                        })
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().truncate().child(model.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.muted_foreground)
                                        .child(provider.clone()),
                                ),
                        )
                        .child(cell(totals.requests.to_string()))
                        .child(cell(compact_number((totals.input + totals.output) as f64)))
                        .child(cell(
                            totals
                                .latency()
                                .map(super::models::format_latency)
                                .unwrap_or_default(),
                        ))
                        .child(cell(
                            tps.map(|tps| format!("{tps:.1}/s")).unwrap_or_default(),
                        ))
                        .child(cell(format!("${:.2}", totals.cost)))
                });
        let table = v_flex()
            .child(
                h_flex()
                    .px_5()
                    .h_10()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_medium()
                            .text_color(colors.table_head_foreground)
                            .child(t!("usage.column.model")),
                    )
                    .child(head(tr!("usage.column.requests")))
                    .child(head(tr!("usage.column.tokens")))
                    .child(head(tr!("usage.column.ttft")))
                    .child(head(tr!("usage.column.speed")))
                    .child(head(tr!("usage.column.cost"))),
            )
            .children(rows_view);
        let cache_hit = current
            .cache_hit()
            .map_or_else(|| NO_VALUE.to_owned(), |hit| format!("{hit:.1}%"));
        let latency = current
            .latency()
            .map_or_else(|| NO_VALUE.to_owned(), super::models::format_latency);
        let cards = h_flex()
            .gap_3()
            .flex_wrap()
            .child(Self::card(
                tr!("usage.requests"),
                current.requests.to_string(),
                current.requests as f64,
                previous.requests as f64,
                false,
                cx,
            ))
            .child(Self::card(
                tr!("usage.tokens"),
                compact_number((current.input + current.output) as f64),
                (current.input + current.output) as f64,
                (previous.input + previous.output) as f64,
                false,
                cx,
            ))
            .child(Self::card(
                tr!("usage.cost"),
                format!("${:.2}", current.cost),
                current.cost,
                previous.cost,
                true,
                cx,
            ))
            .child(Self::card(
                tr!("usage.cache_hit"),
                cache_hit,
                current.cache_hit().unwrap_or_default(),
                previous.cache_hit().unwrap_or_default(),
                false,
                cx,
            ))
            .child(Self::card(
                tr!("usage.latency"),
                latency,
                current.latency().unwrap_or_default(),
                previous.latency().unwrap_or_default(),
                true,
                cx,
            ));
        div()
            .id("usage-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .mx_auto()
                    .w_full()
                    .max_w(gpui_kit::rems(64.0))
                    .px_6()
                    .pt_5()
                    .pb_6()
                    .gap_5()
                    .child(cards)
                    .child(
                        v_flex()
                            .gap_3()
                            .p_5()
                            .rounded_xl()
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.group_box)
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(colors.muted_foreground)
                                    .child(t!("usage.daily_tokens")),
                            )
                            .child(div().h_56().child(chart)),
                    )
                    .child(
                        v_flex()
                            .rounded_xl()
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.group_box)
                            .overflow_hidden()
                            .when(models.is_empty(), |list| {
                                list.child(
                                    div()
                                        .p_6()
                                        .text_sm()
                                        .text_color(colors.muted_foreground)
                                        .child(t!("usage.empty")),
                                )
                            })
                            .when(!models.is_empty(), |list| list.child(table)),
                    )
                    .when(current.errors > 0, |body| {
                        body.child(
                            div()
                                .text_xs()
                                .text_color(colors.warning)
                                .child(t!("usage.errors", count = current.errors)),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl UsagePage {
    fn render_turns(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let Some(turns) = self.turns.clone() else {
            return match self.turns_error.clone() {
                Some(error) => placeholder(
                    IconName::TriangleAlert,
                    t!("usage.turn.load_failed"),
                    Some(error),
                    None,
                    cx,
                )
                .into_any_element(),
                None => placeholder(
                    IconName::LoaderCircle,
                    t!("usage.turn.loading"),
                    None,
                    None,
                    cx,
                )
                .into_any_element(),
            };
        };
        let query = self.search.read(cx).value().trim().to_lowercase();
        let visible: Vec<&Turn> = turns
            .iter()
            .filter(|turn| !self.failed_only || turn.failed())
            .filter(|turn| turn.matches(&query))
            .collect();
        // Without a choice the newest turn is inspected; a chosen turn stays
        // in the inspector while newer turns arrive above it.
        let inspected = self
            .selected_turn
            .as_ref()
            .and_then(|key| turns.iter().find(|turn| &turn.key() == key))
            .or_else(|| visible.first().copied())
            .cloned();
        let inspected_key = inspected.as_ref().map(Turn::key);
        let follow = self.follow;
        let toolbar = h_flex()
            .gap_2()
            .child(
                div().w_64().child(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).small()),
                ),
            )
            .child(
                Segmented::new("turn-filter")
                    .items([tr!("usage.turn.all"), tr!("usage.turn.failed_only")])
                    .selected(usize::from(self.failed_only))
                    .on_change(cx.listener(|page, index: &usize, _, cx| {
                        page.failed_only = *index == 1;
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .when_some(self.turns_error.clone(), |toolbar, error| {
                toolbar.child(
                    div()
                        .max_w_64()
                        .text_xs()
                        .text_color(colors.warning)
                        .truncate()
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(status_dot(if follow {
                        colors.success
                    } else {
                        colors.muted_foreground
                    }))
                    .child(if follow {
                        t!("usage.turn.following")
                    } else {
                        t!("usage.turn.paused")
                    }),
            )
            .child(
                Button::new("toggle-follow")
                    .ghost()
                    .small()
                    .icon(if follow {
                        IconName::Pause
                    } else {
                        IconName::Play
                    })
                    .label(if follow {
                        t!("usage.turn.pause")
                    } else {
                        t!("usage.turn.resume")
                    })
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.follow = !page.follow;
                        if page.follow {
                            page.reload_turns(cx);
                        }
                        cx.notify();
                    })),
            );
        let head = |label: SharedString| {
            div()
                .text_xs()
                .font_medium()
                .text_color(colors.table_head_foreground)
                .child(label)
        };
        let header = h_flex()
            .px_3()
            .h_8()
            .gap_3()
            .border_b_1()
            .border_color(colors.border)
            .child(head(tr!("usage.turn.column.time")).w_20())
            .child(head(tr!("usage.turn.column.session")).flex_1())
            .child(head(tr!("usage.turn.column.model")).w_40())
            .child(head(tr!("usage.turn.column.status")).w_16())
            .child(head(tr!("usage.turn.column.tokens")).w_16().text_right());
        let rows =
            visible.iter().map(|turn| {
                let key = turn.key();
                let selected = inspected_key.as_deref() == Some(key.as_str());
                let fresh = self.fresh.contains(&key);
                let status_color = if turn.failed() {
                    colors.warning
                } else {
                    colors.success
                };
                let row =
                    h_flex()
                        .id(ElementId::Name(format!("turn-{key}").into()))
                        .px_3()
                        .h_11()
                        .gap_3()
                        .text_sm()
                        .cursor_pointer()
                        .border_b_1()
                        .border_color(colors.border.opacity(0.5))
                        .when(selected, |row| row.bg(colors.list_active))
                        .when(!selected, |row| {
                            row.hover(|style| style.bg(colors.list_hover))
                        })
                        .on_click(cx.listener({
                            let key = key.clone();
                            move |page, _, _, cx| {
                                page.selected_turn = Some(key.clone());
                                cx.notify();
                            }
                        }))
                        .child(
                            div()
                                .w_20()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .child(turn.time()),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().child(turn.title()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.muted_foreground)
                                        .truncate()
                                        .child(turn.agent()),
                                ),
                        )
                        .child(
                            div()
                                .w_40()
                                .text_xs()
                                .truncate()
                                .child(turn.catalog_slug.clone()),
                        )
                        .child(
                            h_flex()
                                .w_16()
                                .gap_1p5()
                                .text_xs()
                                .child(status_dot(status_color))
                                .child(if turn.failed() {
                                    t!("usage.turn.failed")
                                } else {
                                    t!("usage.turn.completed")
                                }),
                        )
                        .child(div().w_16().text_right().text_xs().child(
                            turn.tokens().map_or_else(
                                || NO_VALUE.to_owned(),
                                |tokens| compact_number(tokens as f64),
                            ),
                        ));
                if fresh {
                    row.with_animation(
                        ElementId::Name(format!("turn-enter-{key}").into()),
                        Animation::new(theme::ENTER_DURATION)
                            .with_easing(theme::curve(theme::ENTER_CURVE)),
                        |row, progress| row.opacity(progress).mt(px(-8.0 * (1.0 - progress))),
                    )
                    .into_any_element()
                } else {
                    row.into_any_element()
                }
            });
        let list = v_flex()
            .flex_1()
            .min_w_0()
            .rounded_lg()
            .border_1()
            .border_color(colors.border)
            .overflow_hidden()
            .child(header)
            .child(
                div()
                    .id("turn-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(visible.is_empty(), |list| {
                        list.child(
                            div()
                                .p_6()
                                .text_sm()
                                .text_color(colors.muted_foreground)
                                .child(t!("usage.turn.empty")),
                        )
                    })
                    .children(rows),
            );
        let inspector = self.render_inspector(inspected.as_ref(), &turns, cx);
        v_flex()
            .mx_auto()
            .w_full()
            .max_w(gpui_kit::rems(64.0))
            .h_full()
            .px_6()
            .pt_3()
            .pb_5()
            .gap_3()
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .gap_4()
                    .child(list.h_full())
                    .child(inspector),
            )
            .into_any_element()
    }

    fn render_inspector(
        &self,
        turn: Option<&Turn>,
        turns: &[Turn],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let panel = v_flex()
            .id("turn-inspector")
            .w_72()
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .gap_4()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(colors.border)
            .bg(colors.group_box);
        let Some(turn) = turn else {
            return panel.child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(t!("usage.turn.none_selected")),
            );
        };
        let pair = |label: SharedString, value: String| {
            h_flex()
                .justify_between()
                .gap_3()
                .text_sm()
                .child(div().text_color(colors.muted_foreground).child(label))
                .child(div().truncate().child(value))
        };
        let optional = |value: Option<u64>| {
            value.map_or_else(|| NO_VALUE.to_owned(), |value| compact_number(value as f64))
        };
        let session: Vec<&Turn> = turns
            .iter()
            .filter(|other| other.session == turn.session)
            .take(SESSION_TURNS)
            .collect();
        let session_count = turns
            .iter()
            .filter(|other| other.session == turn.session)
            .count();
        let current = turn.key();
        let timeline = session.into_iter().map(|other| {
            let key = other.key();
            let active = key == current;
            h_flex()
                .id(ElementId::Name(format!("session-turn-{key}").into()))
                .gap_2p5()
                .text_xs()
                .cursor_pointer()
                .on_click(cx.listener({
                    let key = key.clone();
                    move |page, _, _, cx| {
                        page.selected_turn = Some(key.clone());
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .size_2()
                        .flex_none()
                        .rounded_full()
                        .when(active, |dot| dot.bg(colors.primary))
                        .when(!active, |dot| {
                            dot.border_1().border_color(colors.muted_foreground)
                        }),
                )
                .child(
                    div()
                        .w_16()
                        .text_color(colors.muted_foreground)
                        .child(other.time()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(active, |text| text.font_medium())
                        .child(other.catalog_slug.clone()),
                )
                .child(div().child(optional(other.tokens())))
        });
        panel
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(Icon::new(IconName::Activity).small())
                    .child(t!("usage.turn.details")),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(div().text_base().font_semibold().child(turn.title()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(if turn.failed() {
                                pill(t!("usage.turn.failed"), colors.warning)
                            } else {
                                pill(t!("usage.turn.completed"), colors.success)
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(turn.time()),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(pair(tr!("usage.turn.agent"), turn.agent().to_string()))
                    .child(pair(tr!("usage.turn.provider"), turn.provider_id.clone()))
                    .child(pair(tr!("usage.turn.model"), turn.catalog_slug.clone()))
                    .child(pair(
                        tr!("usage.turn.ttft"),
                        turn.ttft_ms
                            .map_or_else(|| NO_VALUE.to_owned(), super::models::format_latency),
                    ))
                    .child(pair(
                        tr!("usage.turn.speed"),
                        turn.output_tps
                            .map_or_else(|| NO_VALUE.to_owned(), |tps| format!("{tps:.1}/s")),
                    ))
                    .child(pair(tr!("usage.turn.input"), optional(turn.input_tokens)))
                    .child(pair(tr!("usage.turn.output"), optional(turn.output_tokens)))
                    .child(pair(
                        tr!("usage.turn.cache"),
                        optional(turn.cache_read_tokens),
                    ))
                    .child(pair(
                        tr!("usage.turn.cost"),
                        turn.estimated_cost_usd
                            .map_or_else(|| NO_VALUE.to_owned(), |cost| format!("${cost:.4}")),
                    )),
            )
            .when_some(turn.error.clone(), |panel, error| {
                panel.child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(colors.warning.opacity(0.1))
                        .text_xs()
                        .text_color(colors.warning)
                        .child(error),
                )
            })
            .child(
                v_flex()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .text_color(colors.muted_foreground)
                            .child(t!("usage.turn.session_requests", count = session_count)),
                    )
                    .children(timeline),
            )
    }
}

impl Render for UsagePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.view;
        let body = match view {
            UsageView::Overview => self.render_overview(cx),
            UsageView::Turns => self.render_turns(cx),
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .mx_auto()
                    .w_full()
                    .max_w(gpui_kit::rems(64.0))
                    .px_6()
                    .pt_2()
                    .gap_4()
                    .child(
                        div()
                            .text_2xl()
                            .font_semibold()
                            .child(t!("console.page.usage")),
                    )
                    .child(
                        Segmented::new("usage-view")
                            .items([tr!("usage.view_overview"), tr!("usage.view_turns")])
                            .selected(usize::from(view == UsageView::Turns))
                            .on_change(cx.listener(|page, index: &usize, _, cx| {
                                let view = if *index == 1 {
                                    UsageView::Turns
                                } else {
                                    UsageView::Overview
                                };
                                page.show(view, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .when(view == UsageView::Overview, |header| {
                        header
                            .child(
                                Segmented::new("usage-range")
                                    .items(
                                        RANGES.iter().map(|days| tr!("usage.range", days = days)),
                                    )
                                    .selected(self.range)
                                    .on_change(cx.listener(|page, index: &usize, _, cx| {
                                        page.range = *index;
                                        page.reload(cx);
                                    })),
                            )
                            .child(
                                Button::new("reload-usage")
                                    .ghost()
                                    .small()
                                    .icon(IconName::RefreshCw)
                                    .loading(self.loading)
                                    .on_click(cx.listener(|page, _, _, cx| page.reload(cx))),
                            )
                    }),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
