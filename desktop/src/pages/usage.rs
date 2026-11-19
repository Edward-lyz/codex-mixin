//! Usage page: daily totals from `usage --daily`, compared with the period
//! before, plus a per-model breakdown.

use std::collections::BTreeMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::chart::BarChart;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    App, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use rust_i18n::t;
use serde::Deserialize;

use super::placeholder;
use crate::store::query;
use crate::tr;
use crate::widgets::Segmented;

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

#[derive(Clone)]
struct DayBar {
    label: SharedString,
    tokens: f64,
}

pub struct UsagePage {
    range: usize,
    rows: Option<Vec<DailyUsage>>,
    error: Option<SharedString>,
    loading: bool,
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
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            range: 0,
            rows: None,
            error: None,
            loading: false,
        };
        page.reload(cx);
        page
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
            .gap_1()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(colors.border)
            .bg(colors.group_box)
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(title),
            )
            .child(div().text_xl().font_semibold().child(value))
            .child(delta)
    }
}

const NO_VALUE: &str = "\u{2014}";

impl Render for UsagePage {
    fn render(&mut self, _: &mut Window, cx: &mut gpui_kit::Context<Self>) -> impl IntoElement {
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
            .fill(move |_, _, _, _| primary)
            .corner_radii(px(4.0))
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
                        .px_4()
                        .h_10()
                        .when(index % 2 == 1, |row| row.bg(colors.table_even))
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
                    .px_4()
                    .h_9()
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
        let range_labels: Vec<SharedString> = RANGES
            .iter()
            .map(|days| tr!("usage.range", days = days))
            .collect();
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
                    .py_5()
                    .gap_5()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .text_lg()
                                    .font_semibold()
                                    .child(t!("console.page.usage")),
                            )
                            .child(
                                Segmented::new("usage-range")
                                    .items(range_labels)
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
                            ),
                    )
                    .child(cards)
                    .child(
                        v_flex()
                            .gap_2()
                            .p_4()
                            .rounded_lg()
                            .border_1()
                            .border_color(colors.border)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Icon::new(IconName::ChartColumn)
                                            .small()
                                            .text_color(colors.muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .child(t!("usage.daily_tokens")),
                                    ),
                            )
                            .child(div().h_56().child(chart)),
                    )
                    .child(
                        v_flex()
                            .rounded_lg()
                            .border_1()
                            .border_color(colors.border)
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
