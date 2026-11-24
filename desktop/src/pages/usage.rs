//! Usage page: daily totals from `usage --daily`, compared with the period
//! before, plus a per-model breakdown.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use chrono::{Datelike as _, Timelike as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::chart::{AreaChart, BarChart};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
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
use crate::model::UsageSeriesBucket;
use crate::quota_card::{
    MiniActivityBar, ProviderMiniSeries, ProviderSpendSummary, QuotaCardDensity, render_quota_card,
};
use crate::store::{Store, query};
use crate::theme;
use crate::tr;
use crate::widgets::{Segmented, page_header, pill, status_dot};

const RANGES: [u64; 4] = [1, 7, 30, 90];
const SECONDS_PER_DAY: i64 = 86_400;
const TRAFFIC_POLL_INTERVAL: Duration = Duration::from_secs(15);

fn range_series_window(days: u64) -> &'static str {
    match days {
        1 => "24h",
        7 => "7d",
        30 => "30d",
        _ => "90d",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UsageChartMode {
    Traffic,
    Tokens,
}

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

#[derive(Clone)]
struct TrafficPoint {
    label: SharedString,
    requests: f64,
    cached: f64,
    errors: f64,
}

pub struct UsagePage {
    view: UsageView,
    range: usize,
    rows: Option<Vec<DailyUsage>>,
    rows_generation: u64,
    error: Option<SharedString>,
    loading: bool,
    chart_mode: UsageChartMode,
    traffic_open: bool,
    traffic_buckets: Option<Vec<UsageSeriesBucket>>,
    traffic_loading: bool,
    traffic_generation: u64,
    traffic_error: Option<SharedString>,
    /// Per-provider hourly buckets for the last 24h (feeds 5h mini bars & 24h spend).
    hourly_by_provider: Vec<UsageSeriesBucket>,
    hourly_loading: bool,
    limits_open: bool,
    show_all_providers: bool,
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
        let store = Store::global(cx);
        let store_subscription = cx.observe(&store, |_, _, cx| {
            cx.notify();
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
        // Refresh the 24h traffic series while Overview is visible.
        cx.spawn(async move |page, cx| {
            loop {
                cx.background_executor().timer(TRAFFIC_POLL_INTERVAL).await;
                let polled = page.update(cx, |page, cx| {
                    if page.active
                        && page.view == UsageView::Overview
                        && page.traffic_open
                        && RANGES[page.range] == 1
                    {
                        page.reload_traffic(cx);
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
            range: 2,
            rows: None,
            rows_generation: 0,
            error: None,
            loading: false,
            chart_mode: UsageChartMode::Traffic,
            traffic_open: true,
            traffic_buckets: None,
            traffic_loading: false,
            traffic_generation: 0,
            traffic_error: None,
            hourly_by_provider: Vec::new(),
            hourly_loading: false,
            limits_open: true,
            show_all_providers: false,
            turns: None,
            turns_error: None,
            turns_loading: false,
            active: true,
            follow: true,
            failed_only: false,
            selected_turn: None,
            fresh: HashSet::new(),
            search,
            _subscriptions: vec![search_subscription, store_subscription],
        };
        page.reload(cx);
        page.reload_traffic(cx);
        page.reload_hourly_by_provider(cx);
        page.request_missing_quotas(cx);
        page
    }

    /// Called by the console when it shows or leaves this page.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active {
            if self.view == UsageView::Turns {
                self.reload_turns(cx);
            } else {
                self.reload_traffic(cx);
                self.request_missing_quotas(cx);
            }
        }
    }

    fn request_missing_quotas(&self, cx: &mut Context<Self>) {
        let store = Store::global(cx);
        let missing: Vec<String> = store
            .read(cx)
            .providers
            .value
            .as_ref()
            .map(|list| {
                list.providers
                    .iter()
                    .filter(|provider| {
                        provider.enabled && !store.read(cx).quotas.contains_key(&provider.id)
                    })
                    .map(|provider| provider.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if missing.is_empty() {
            return;
        }
        store.update(cx, |store, cx| {
            for id in &missing {
                store.refresh_quota(id, cx);
            }
        });
    }

    fn refresh_all_quotas(&self, cx: &mut Context<Self>) {
        let store = Store::global(cx);
        let ids: Vec<String> = store
            .read(cx)
            .providers
            .value
            .as_ref()
            .map(|list| {
                list.providers
                    .iter()
                    .filter(|provider| provider.enabled)
                    .map(|provider| provider.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        store.update(cx, |store, cx| {
            for id in &ids {
                store.refresh_quota(id, cx);
            }
        });
    }

    fn reload_traffic(&mut self, cx: &mut Context<Self>) {
        self.traffic_generation = self.traffic_generation.wrapping_add(1);
        let generation = self.traffic_generation;
        self.traffic_loading = true;
        let window = range_series_window(RANGES[self.range]);
        let task = query::<Vec<UsageSeriesBucket>>(
            vec![
                "usage".into(),
                "--series".into(),
                "--window".into(),
                window.into(),
                "--json".into(),
            ],
            cx,
        );
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                if page.traffic_generation != generation {
                    return;
                }
                page.traffic_loading = false;
                match result {
                    Ok(buckets) => {
                        page.traffic_buckets = Some(buckets);
                        page.traffic_error = None;
                    }
                    Err(error) => page.traffic_error = Some(error.message.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn reload_hourly_by_provider(&mut self, cx: &mut Context<Self>) {
        if self.hourly_loading {
            return;
        }
        self.hourly_loading = true;
        let task = query::<Vec<UsageSeriesBucket>>(
            vec![
                "usage".into(),
                "--series".into(),
                "--window".into(),
                "24h".into(),
                "--by-provider".into(),
                "--json".into(),
            ],
            cx,
        );
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                page.hourly_loading = false;
                if let Ok(buckets) = result {
                    page.hourly_by_provider = buckets;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        self.reload_traffic(cx);
        self.reload_hourly_by_provider(cx);
        self.rows_generation = self.rows_generation.wrapping_add(1);
        let generation = self.rows_generation;
        self.loading = true;
        cx.notify();
        // Twice the range so the summary cards can compare with the prior period,
        // and at least 30 days so the Usage limits spend footer always has 7d/30d totals.
        let days = (RANGES[self.range] * 2).max(30);
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
                if page.rows_generation != generation {
                    return;
                }
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
        icon: IconName,
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
            .gap_2()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(colors.border)
            .bg(colors.group_box)
            .shadow_xs()
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .font_medium()
                    .text_color(colors.muted_foreground)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size_6()
                            .rounded_md()
                            .bg(colors.muted)
                            .child(Icon::new(icon).xsmall().text_color(colors.muted_foreground)),
                    )
                    .child(title),
            )
            .child(div().text_2xl().font_semibold().child(value))
            .child(delta)
    }
}

const NO_VALUE: &str = "\u{2014}";

fn traffic_x_label(bucket_start_ms: i64, window: &str) -> SharedString {
    let Some(utc) = chrono::DateTime::from_timestamp_millis(bucket_start_ms) else {
        return SharedString::default();
    };
    let local = utc.with_timezone(&chrono::Local);
    let formatted = match window {
        "10m" | "1h" => local.format("%H:%M").to_string(),
        "24h" => local.format("%H:00").to_string(),
        _ => local.format("%m-%d").to_string(),
    };
    formatted.into()
}

fn weekday_initial(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "M",
        chrono::Weekday::Tue => "T",
        chrono::Weekday::Wed => "W",
        chrono::Weekday::Thu => "T",
        chrono::Weekday::Fri => "F",
        chrono::Weekday::Sat => "S",
        chrono::Weekday::Sun => "S",
    }
}

impl UsagePage {
    fn render_chart_card(
        &mut self,
        days: i64,
        daily_bars: Vec<DayBar>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let window_key = range_series_window(RANGES[self.range]);
        let open = self.traffic_open;
        let mode = self.chart_mode;

        let header = h_flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                h_flex()
                    .id("usage-traffic-toggle")
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.traffic_open = !page.traffic_open;
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(colors.muted_foreground),
                    )
                    .child(
                        Icon::new(if mode == UsageChartMode::Traffic {
                            IconName::Activity
                        } else {
                            IconName::ChartColumn
                        })
                        .small()
                        .text_color(colors.foreground),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(colors.foreground)
                            .child(if mode == UsageChartMode::Traffic {
                                tr!("usage.traffic.title")
                            } else {
                                tr!("usage.daily_tokens")
                            }),
                    ),
            )
            .child(
                Segmented::new("usage-chart-mode")
                    .items([tr!("usage.traffic.title"), tr!("usage.traffic.tokens_tab")])
                    .selected(usize::from(mode == UsageChartMode::Tokens))
                    .on_change(cx.listener(|page, index: &usize, _, cx| {
                        let next = if *index == 1 {
                            UsageChartMode::Tokens
                        } else {
                            UsageChartMode::Traffic
                        };
                        if page.chart_mode != next {
                            page.chart_mode = next;
                            cx.notify();
                        }
                    })),
            );

        let legend_item = |color: Hsla, label: SharedString| {
            h_flex()
                .items_center()
                .gap_1p5()
                .child(div().w(px(12.0)).h(px(2.5)).rounded_full().bg(color))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child(label),
                )
        };

        let chart_body: AnyElement = match mode {
            UsageChartMode::Traffic => match &self.traffic_buckets {
                Some(buckets) if !buckets.is_empty() => {
                    let points: Vec<TrafficPoint> = buckets
                        .iter()
                        .map(|bucket| TrafficPoint {
                            label: traffic_x_label(bucket.bucket_start_ms, window_key),
                            requests: bucket.requests as f64,
                            cached: bucket.cached_requests as f64,
                            errors: bucket.errors as f64,
                        })
                        .collect();
                    let chart = AreaChart::new(points)
                        .id("usage-traffic-chart")
                        .x(|point: &TrafficPoint| point.label.clone())
                        .y(|point: &TrafficPoint| point.requests)
                        .stroke(colors.chart_1)
                        .fill(colors.chart_1.opacity(0.08))
                        .natural()
                        .name(tr!("usage.traffic.requests"))
                        .y(|point: &TrafficPoint| point.cached)
                        .stroke(colors.success)
                        .fill(colors.success.opacity(0.06))
                        .natural()
                        .name(tr!("usage.traffic.cached"))
                        .y(|point: &TrafficPoint| point.errors)
                        .stroke(colors.danger)
                        .fill(colors.danger.opacity(0.06))
                        .natural()
                        .name(tr!("usage.traffic.errors"))
                        .y_axis(true)
                        .y_tick_count(4)
                        .x_tick_count(6)
                        .grid_dashed(true)
                        .appear_key(window_key)
                        .y_tick_format(compact_number)
                        .tooltip_value(|_, _, value| SharedString::from(compact_number(value)));
                    div().h(px(210.0)).w_full().child(chart).into_any_element()
                }
                _ => {
                    let message = match &self.traffic_error {
                        Some(error) => error.clone(),
                        None if self.traffic_loading => tr!("usage.loading"),
                        None => tr!("usage.traffic.empty"),
                    };
                    h_flex()
                        .h(px(120.0))
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child(message)
                        .into_any_element()
                }
            },
            UsageChartMode::Tokens => {
                let bars: Vec<DayBar> = if days == 1 {
                    self.traffic_buckets
                        .as_ref()
                        .map(|buckets| {
                            buckets
                                .iter()
                                .map(|bucket| DayBar {
                                    label: traffic_x_label(bucket.bucket_start_ms, window_key),
                                    tokens: (bucket.input_tokens + bucket.output_tokens) as f64,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    daily_bars
                };
                if bars.is_empty() {
                    let message = match &self.traffic_error {
                        Some(error) => error.clone(),
                        None if self.traffic_loading => tr!("usage.loading"),
                        None => tr!("usage.empty"),
                    };
                    h_flex()
                        .h(px(120.0))
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child(message)
                        .into_any_element()
                } else {
                    let primary = colors.chart_1;
                    let tick_count = if days == 1 {
                        6
                    } else if days > 30 {
                        8
                    } else {
                        days as usize
                    };
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
                        .corner_radii(px(5.0))
                        .padding_inner(0.5)
                        .max_band_width(px(36.0))
                        .min_length(2.0)
                        .grid_dashed(true)
                        .value_tick_count(4)
                        .appear_key(window_key)
                        .value_axis(true)
                        .value_tick_format(compact_number)
                        .tooltip_value(|_, value| SharedString::from(compact_number(value)))
                        .band_tick_count(tick_count);
                    div().h(px(210.0)).w_full().child(chart).into_any_element()
                }
            }
        };

        v_flex()
            .gap_3()
            .p_5()
            .rounded_xl()
            .border_1()
            .border_color(colors.border)
            .bg(colors.group_box)
            .shadow_xs()
            .child(header)
            .when(open, |card| {
                card.when(mode == UsageChartMode::Traffic, |card| {
                    card.child(
                        h_flex()
                            .items_center()
                            .gap_4()
                            .child(legend_item(colors.chart_1, tr!("usage.traffic.requests")))
                            .child(legend_item(colors.success, tr!("usage.traffic.cached")))
                            .child(legend_item(colors.danger, tr!("usage.traffic.errors"))),
                    )
                })
                .child(chart_body)
            })
            .into_any_element()
    }

    fn build_provider_spend_and_mini(
        &self,
        rows: &[DailyUsage],
    ) -> (
        HashMap<String, ProviderSpendSummary>,
        HashMap<String, ProviderMiniSeries>,
    ) {
        let mut spend: HashMap<String, ProviderSpendSummary> = HashMap::new();
        let mut mini: HashMap<String, ProviderMiniSeries> = HashMap::new();

        // 24h spend & last-5-hours mini bars from hourly buckets.
        let now_ms = chrono::Utc::now().timestamp_millis();
        let hour_ms: i64 = 3_600_000;
        let current_hour_ms = now_ms - now_ms.rem_euclid(hour_ms);
        let start_5h_ms = current_hour_ms - 4 * hour_ms;
        let mut hourly_map: HashMap<String, BTreeMap<i64, u64>> = HashMap::new();
        for bucket in &self.hourly_by_provider {
            let Some(provider_id) = bucket.provider_id.as_ref() else {
                continue;
            };
            let entry = spend.entry(provider_id.clone()).or_default();
            entry.cost_24h += bucket.estimated_cost_usd;
            if bucket.bucket_start_ms >= start_5h_ms && bucket.bucket_start_ms <= current_hour_ms {
                let slot = bucket.bucket_start_ms - bucket.bucket_start_ms.rem_euclid(hour_ms);
                *hourly_map
                    .entry(provider_id.clone())
                    .or_default()
                    .entry(slot)
                    .or_default() += bucket.requests;
            }
        }
        for (provider_id, slots) in hourly_map {
            let bars: Vec<MiniActivityBar> = (0..5)
                .map(|offset| {
                    let slot_ms = start_5h_ms + offset * hour_ms;
                    let label = chrono::DateTime::from_timestamp_millis(slot_ms)
                        .map(|utc| format!("{:02}", utc.with_timezone(&chrono::Local).hour()))
                        .unwrap_or_default();
                    MiniActivityBar {
                        label: label.into(),
                        value: *slots.get(&slot_ms).unwrap_or(&0) as f64,
                        current: slot_ms == current_hour_ms,
                    }
                })
                .collect();
            mini.entry(provider_id).or_default().hourly_5h = bars;
        }

        // 7d / 30d spend & last-7-days mini bars from daily rows.
        let today_day = today();
        let start_7d = today_day - 6;
        let start_30d = today_day - 29;
        let mut daily_map: HashMap<String, BTreeMap<i64, u64>> = HashMap::new();
        for row in rows {
            if row.day < start_30d {
                continue;
            }
            let entry = spend.entry(row.provider_id.clone()).or_default();
            let cost = row.estimated_cost_usd.unwrap_or_default();
            entry.cost_30d += cost;
            entry.requests_30d += row.request_count;
            if row.day >= start_7d {
                entry.cost_7d += cost;
                *daily_map
                    .entry(row.provider_id.clone())
                    .or_default()
                    .entry(row.day)
                    .or_default() += row.request_count;
            }
        }
        for (provider_id, days_map) in daily_map {
            let bars: Vec<MiniActivityBar> = (start_7d..=today_day)
                .map(|day| {
                    let label = chrono::DateTime::from_timestamp(day * SECONDS_PER_DAY, 0)
                        .map(|utc| weekday_initial(utc.with_timezone(&chrono::Local).weekday()))
                        .unwrap_or("");
                    MiniActivityBar {
                        label: label.into(),
                        value: *days_map.get(&day).unwrap_or(&0) as f64,
                        current: day == today_day,
                    }
                })
                .collect();
            mini.entry(provider_id).or_default().daily_7d = bars;
        }

        (spend, mini)
    }

    fn render_usage_limits(&mut self, rows: &[DailyUsage], cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let open = self.limits_open;
        let show_all = self.show_all_providers;
        let (spend_map, mini_map) = self.build_provider_spend_and_mini(rows);

        let store = Store::global(cx);
        let store_ref = store.read(cx);
        let providers = store_ref
            .providers
            .value
            .as_ref()
            .map(|list| list.providers.clone())
            .unwrap_or_default();
        let mut quota_cards: Vec<AnyElement> = Vec::new();
        let any_quota_loading = store_ref.quotas.values().any(|slot| slot.loading);

        for provider in providers.into_iter().filter(|provider| provider.enabled) {
            let slot = store_ref.quotas.get(&provider.id);
            let entries = slot.and_then(|slot| slot.value.as_deref()).unwrap_or(&[]);
            let loading = slot.is_some_and(|slot| slot.loading);
            let spend = spend_map.get(&provider.id);
            let has_spend = spend.is_some_and(|summary| {
                summary.cost_24h > 0.0 || summary.cost_30d > 0.0 || summary.requests_30d > 0
            });
            if !show_all && entries.is_empty() && !has_spend && !loading {
                continue;
            }
            let mini = mini_map.get(&provider.id);
            quota_cards.push(render_quota_card(
                &provider,
                slot,
                QuotaCardDensity::Full,
                mini,
                spend,
                cx,
            ));
        }

        let filter_tooltip = if show_all {
            tr!("usage.limits.filter_active")
        } else {
            tr!("usage.limits.filter_all")
        };

        let header = h_flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                h_flex()
                    .id("usage-limits-toggle")
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.limits_open = !page.limits_open;
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(colors.muted_foreground),
                    )
                    .child(
                        Icon::new(IconName::Gauge)
                            .small()
                            .text_color(colors.foreground),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(colors.foreground)
                            .child(t!("usage.limits.title")),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id("usage-limits-filter-wrap")
                            .tooltip(move |window, cx| {
                                Tooltip::new(filter_tooltip.clone()).build(window, cx)
                            })
                            .child(
                                Button::new("usage-limits-filter")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Settings2)
                                    .on_click(cx.listener(|page, _, _, cx| {
                                        page.show_all_providers = !page.show_all_providers;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        Button::new("usage-limits-refresh")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .loading(any_quota_loading || self.hourly_loading)
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.refresh_all_quotas(cx);
                                page.reload_hourly_by_provider(cx);
                            })),
                    ),
            );

        v_flex()
            .gap_3()
            .child(header)
            .when(open, |section| {
                if quota_cards.is_empty() {
                    section.child(
                        div()
                            .p_5()
                            .rounded_xl()
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.group_box)
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(t!("usage.limits.empty")),
                    )
                } else {
                    section.child(div().grid().grid_cols(2).gap_4().children(quota_cards))
                }
            })
            .into_any_element()
    }

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
        let previous_start = start - days;
        let mut current = Totals::default();
        let mut previous = Totals::default();
        let mut per_day: BTreeMap<i64, f64> = (start..=today).map(|day| (day, 0.0)).collect();
        let mut per_model: BTreeMap<(String, String), (Totals, Option<f64>)> = BTreeMap::new();
        for row in &rows {
            if row.day < start {
                if row.day >= previous_start {
                    previous.add(row);
                }
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
                        .id(ElementId::Name(format!("usage-model-{index}").into()))
                        .px_5()
                        .h_11()
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(colors.border)
                        })
                        .hover(|style| style.bg(colors.table_hover))
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
                    .h_9()
                    .bg(colors.muted.opacity(0.5))
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
                IconName::Activity,
                tr!("usage.requests"),
                current.requests.to_string(),
                current.requests as f64,
                previous.requests as f64,
                false,
                cx,
            ))
            .child(Self::card(
                IconName::Coins,
                tr!("usage.tokens"),
                compact_number((current.input + current.output) as f64),
                (current.input + current.output) as f64,
                (previous.input + previous.output) as f64,
                false,
                cx,
            ))
            .child(Self::card(
                IconName::DollarSign,
                tr!("usage.cost"),
                format!("${:.2}", current.cost),
                current.cost,
                previous.cost,
                true,
                cx,
            ))
            .child(Self::card(
                IconName::Zap,
                tr!("usage.cache_hit"),
                cache_hit,
                current.cache_hit().unwrap_or_default(),
                previous.cache_hit().unwrap_or_default(),
                false,
                cx,
            ))
            .child(Self::card(
                IconName::Timer,
                tr!("usage.latency"),
                latency,
                current.latency().unwrap_or_default(),
                previous.latency().unwrap_or_default(),
                true,
                cx,
            ));
        let chart_card = self.render_chart_card(days, bars, cx);
        let usage_limits = self.render_usage_limits(&rows, cx);
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
                    .pt_4()
                    .pb_6()
                    .gap_4()
                    .child(cards)
                    .child(chart_card)
                    .child(usage_limits)
                    .child(
                        v_flex()
                            .rounded_xl()
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.group_box)
                            .shadow_xs()
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
                    .pt_3()
                    .gap_3()
                    .child(page_header(
                        IconName::ChartColumn,
                        t!("console.page.usage"),
                        None,
                        div(),
                        cx,
                    ))
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
                                    .items(RANGES.iter().map(|days| {
                                        if *days == 1 {
                                            tr!("usage.range_day")
                                        } else {
                                            tr!("usage.range", days = days)
                                        }
                                    }))
                                    .selected(self.range)
                                    .on_change(cx.listener(|page, index: &usize, _, cx| {
                                        if page.range != *index {
                                            page.range = *index;
                                            page.reload(cx);
                                        }
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
