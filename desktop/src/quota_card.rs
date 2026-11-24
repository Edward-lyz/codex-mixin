//! Shared quota limit card used by both the Usage page (2-column full cards
//! with mini activity bars and local spend footer) and the menu bar / tray
//! panel (compact cards).

use gpui_kit::assets::IconName;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, img,
    prelude::FluentBuilder as _, px, relative, svg,
};
use rust_i18n::t;

use crate::model::{Provider, QuotaEntry};
use crate::pages::usage::compact_number;
use crate::store::Snapshot;
use crate::widgets::provider_icon_path;

/// Display density of [`render_quota_card`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaCardDensity {
    /// Usage page grid card: includes mini activity bars and spend footer.
    Full,
    /// Menu bar / tray popover card: tighter spacing, omits mini bars.
    Compact,
}

/// One bar inside the compact activity sparkline beside a 5h or 7d quota bar.
#[derive(Clone, Debug)]
pub struct MiniActivityBar {
    pub label: SharedString,
    pub value: f64,
    pub current: bool,
}

/// Local gateway spend summary for one provider across 24h, 7d and 30d.
#[derive(Clone, Debug, Default)]
pub struct ProviderSpendSummary {
    pub cost_24h: f64,
    pub cost_7d: f64,
    pub cost_30d: f64,
    pub requests_30d: u64,
}

impl ProviderSpendSummary {
    pub fn has_activity(&self) -> bool {
        self.requests_30d > 0 || self.cost_30d > 0.0 || self.cost_24h > 0.0
    }
}

/// Activity bar buckets attached to a provider card (5h hourly and 7d daily).
#[derive(Clone, Debug, Default)]
pub struct ProviderMiniSeries {
    pub hourly_5h: Vec<MiniActivityBar>,
    pub daily_7d: Vec<MiniActivityBar>,
}

const TEMPLATE_PROVIDER_ICONS: &[&str] = &["openai", "baidu", "custom"];

fn provider_glyph(icon: &str, size_px: f32, cx: &App) -> AnyElement {
    let path = provider_icon_path(icon);
    if TEMPLATE_PROVIDER_ICONS.contains(&icon) {
        svg()
            .path(path)
            .size(px(size_px))
            .flex_none()
            .text_color(cx.theme().colors.foreground)
            .into_any_element()
    } else {
        img(path).size(px(size_px)).flex_none().into_any_element()
    }
}

/// Picks semantic status colour for a used fraction relative to time pace.
fn status_color(used: f32, pace: Option<f32>, cx: &App) -> Hsla {
    let colors = cx.theme().colors;
    if used >= 0.85 {
        return colors.danger;
    }
    if let Some(pace) = pace {
        if used > pace + 0.15 && used >= 0.25 {
            colors.danger
        } else if used > pace + 0.04 && used >= 0.20 {
            colors.warning
        } else {
            colors.success
        }
    } else if used >= 0.70 {
        colors.warning
    } else {
        colors.success
    }
}

/// Picks the text colour of the percentage figure: normal foreground when on
/// pace, semantic warning/danger when running ahead of pace.
fn percent_text_color(used: f32, pace: Option<f32>, cx: &App) -> Hsla {
    let colors = cx.theme().colors;
    let bar = status_color(used, pace, cx);
    if bar == colors.danger {
        colors.danger
    } else if bar == colors.warning {
        colors.warning
    } else {
        colors.foreground
    }
}

/// Horizontal progress bar with an optional vertical pace tick mark.
fn pace_progress_bar(used: f32, pace: Option<f32>, bar_color: Hsla, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let clamped_used = used.clamp(0.0, 1.0);
    let track_bg = bar_color.opacity(0.18);
    let tick = pace.filter(|value| *value > 0.02 && *value < 0.98);
    div()
        .relative()
        .w_full()
        .h(px(6.0))
        .rounded_full()
        .bg(track_bg)
        .when(clamped_used > 0.0, |track| {
            track.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(relative(clamped_used))
                    .min_w(px(6.0))
                    .rounded_full()
                    .bg(bar_color),
            )
        })
        .when_some(tick, |track, pace_pos| {
            track.child(
                div()
                    .absolute()
                    .top(px(-1.5))
                    .bottom(px(-1.5))
                    .left(relative(pace_pos))
                    .w(px(2.0))
                    .rounded_full()
                    .bg(colors.foreground.opacity(0.55)),
            )
        })
}

/// Compact neutral progress bar for metered API quotas without a time pace.
fn neutral_progress_bar(used: f32, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let clamped = used.clamp(0.0, 1.0);
    let fill = if clamped >= 0.90 {
        colors.danger
    } else if clamped >= 0.75 {
        colors.warning
    } else {
        colors.muted_foreground.opacity(0.65)
    };
    div()
        .relative()
        .w_full()
        .h(px(5.0))
        .rounded_full()
        .bg(colors.muted)
        .when(clamped > 0.0, |track| {
            track.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(relative(clamped))
                    .min_w(px(5.0))
                    .rounded_full()
                    .bg(fill),
            )
        })
}

/// Small 5-bar (hourly) or 7-bar (daily) activity sparkline beside a window bar.
fn mini_activity_bars(bars: &[MiniActivityBar], cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let max_val = bars
        .iter()
        .map(|bar| bar.value)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    h_flex()
        .flex_none()
        .items_end()
        .gap(px(3.0))
        .children(bars.iter().map(|bar| {
            let ratio = (bar.value / max_val).clamp(0.0, 1.0) as f32;
            let bar_height = if bar.value > 0.0 {
                (ratio * 14.0).max(3.0)
            } else {
                2.0
            };
            let fill = if bar.current {
                colors.foreground.opacity(0.70)
            } else if bar.value > 0.0 {
                colors.muted_foreground.opacity(0.40)
            } else {
                colors.muted_foreground.opacity(0.16)
            };
            v_flex()
                .items_center()
                .gap(px(2.0))
                .w(px(14.0))
                .child(
                    div()
                        .w_full()
                        .h(px(14.0))
                        .flex()
                        .items_end()
                        .justify_center()
                        .child(
                            div()
                                .w(px(10.0))
                                .h(px(bar_height))
                                .rounded_t(px(2.0))
                                .bg(fill),
                        ),
                )
                .child(
                    div()
                        .text_size(px(9.5))
                        .line_height(px(11.0))
                        .when(bar.current, |el| {
                            el.font_semibold().text_color(colors.foreground)
                        })
                        .when(!bar.current, |el| el.text_color(colors.muted_foreground))
                        .child(bar.label.clone()),
                )
        }))
}

fn format_currency_amount(value: f64, currency: Option<&str>) -> String {
    match currency {
        Some("CNY") => format!("\u{00a5}{value:.2}"),
        Some("USD") => format!("${value:.2}"),
        Some(code) => format!("{value:.2} {code}"),
        None => compact_number(value),
    }
}

fn format_usd(value: f64) -> String {
    format!("${value:.2}")
}

fn format_count(value: u64) -> String {
    let raw = value.to_string();
    let mut out = String::with_capacity(raw.len() + raw.len() / 3);
    for (index, ch) in raw.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn reset_badge(countdown: Option<String>, cx: &App) -> Option<AnyElement> {
    let countdown = countdown?;
    let colors = cx.theme().colors;
    Some(
        h_flex()
            .flex_none()
            .gap_1()
            .text_size(px(11.5))
            .text_color(colors.muted_foreground)
            .child(Icon::new(IconName::Clock).size(px(11.0)))
            .child(countdown)
            .into_any_element(),
    )
}

fn pick_mini_bars<'a>(
    entry: &QuotaEntry,
    series: Option<&'a ProviderMiniSeries>,
) -> Option<&'a [MiniActivityBar]> {
    let series = series?;
    let secs = entry.inferred_window_seconds()?;
    if secs <= 6 * 3_600 {
        (!series.hourly_5h.is_empty()).then_some(series.hourly_5h.as_slice())
    } else if secs <= 8 * 86_400 {
        (!series.daily_7d.is_empty()).then_some(series.daily_7d.as_slice())
    } else {
        None
    }
}

fn render_quota_row(
    provider_id: &str,
    index: usize,
    entry: &QuotaEntry,
    density: QuotaCardDensity,
    series: Option<&ProviderMiniSeries>,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors;
    let compact = density == QuotaCardDensity::Compact;
    if let Some(error) = &entry.error {
        let error_text: SharedString = error.clone().into();
        let id = ElementId::Name(format!("quota-err-{provider_id}-{index}").into());
        return h_flex()
            .id(id)
            .gap_2()
            .justify_between()
            .text_size(px(if compact { 12.0 } else { 12.5 }))
            .child(
                div()
                    .flex_none()
                    .text_color(colors.muted_foreground)
                    .child(entry.label.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_right()
                    .truncate()
                    .text_color(colors.danger)
                    .child(error_text.clone()),
            )
            .tooltip(move |window, cx| Tooltip::new(error_text.clone()).build(window, cx))
            .into_any_element();
    }

    let used_fraction = entry.used_fraction();
    let pace = entry.pace_fraction();
    let countdown = entry.reset_countdown();

    if entry.is_percent_window()
        && let Some(used) = used_fraction
    {
        let bar_color = status_color(used, pace, cx);
        let pct_color = percent_text_color(used, pace, cx);
        let mini = (!compact).then(|| pick_mini_bars(entry, series)).flatten();
        return h_flex()
            .items_end()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(if compact { 4.0 } else { 5.0 }))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(if compact { 12.0 } else { 12.5 }))
                                    .text_color(colors.muted_foreground)
                                    .child(entry.label.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(if compact { 12.5 } else { 13.0 }))
                                    .font_semibold()
                                    .text_color(pct_color)
                                    .child(format!("{:.0}%", used * 100.0)),
                            )
                            .children(reset_badge(countdown, cx)),
                    )
                    .child(pace_progress_bar(used, pace, bar_color, cx)),
            )
            .when_some(mini, |row, bars| row.child(mini_activity_bars(bars, cx)))
            .into_any_element();
    }

    if let (Some(used), Some(limit)) = (entry.used, entry.limit)
        && limit > 0.0
    {
        let frac = (used / limit).clamp(0.0, 1.0) as f32;
        let amount_text = format!(
            "{} / {}",
            format_currency_amount(used, entry.currency.as_deref()),
            format_currency_amount(limit, entry.currency.as_deref()),
        );
        return h_flex()
            .gap_2p5()
            .items_center()
            .child(
                div()
                    .flex_none()
                    .max_w(px(if compact { 96.0 } else { 132.0 }))
                    .truncate()
                    .text_size(px(if compact { 12.0 } else { 12.5 }))
                    .text_color(colors.muted_foreground)
                    .child(entry.label.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(36.0))
                    .child(neutral_progress_bar(frac, cx)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(if compact { 12.0 } else { 12.5 }))
                    .font_semibold()
                    .text_color(if frac >= 0.90 {
                        colors.danger
                    } else {
                        colors.foreground
                    })
                    .child(amount_text),
            )
            .children(reset_badge(countdown, cx))
            .into_any_element();
    }

    let balance_text = entry.remaining.map_or_else(
        || "\u{2014}".to_owned(),
        |remaining| format_currency_amount(remaining, entry.currency.as_deref()),
    );
    h_flex()
        .gap_2()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(if compact { 12.0 } else { 12.5 }))
                .text_color(colors.muted_foreground)
                .child(entry.label.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(if compact { 12.0 } else { 12.5 }))
                .font_semibold()
                .text_color(colors.foreground)
                .child(balance_text),
        )
        .children(reset_badge(countdown, cx))
        .into_any_element()
}

fn render_spend_footer(spend: &ProviderSpendSummary, compact: bool, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    let metric = |window_label: &'static str, value: f64| {
        h_flex()
            .gap_1()
            .items_baseline()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.muted_foreground)
                    .child(window_label),
            )
            .child(
                div()
                    .text_size(px(if compact { 11.5 } else { 12.5 }))
                    .font_semibold()
                    .text_color(colors.foreground)
                    .child(format_usd(value)),
            )
    };
    v_flex()
        .gap_1()
        .pt(px(if compact { 6.0 } else { 10.0 }))
        .mt(px(if compact { 2.0 } else { 4.0 }))
        .border_t_1()
        .border_color(colors.border.opacity(0.75))
        .child(
            h_flex()
                .justify_between()
                .gap_2()
                .text_size(px(11.0))
                .text_color(colors.muted_foreground)
                .child(t!("usage.spend_via"))
                .child(t!(
                    "usage.spend_requests",
                    count = format_count(spend.requests_30d)
                )),
        )
        .child(
            h_flex()
                .gap_3()
                .child(metric("24h", spend.cost_24h))
                .child(metric("7d", spend.cost_7d))
                .child(metric("30d", spend.cost_30d)),
        )
        .into_any_element()
}

/// Renders one provider's Usage limits card in either full or compact density.
pub fn render_quota_card(
    provider: &Provider,
    snapshot: Option<&Snapshot<Vec<QuotaEntry>>>,
    density: QuotaCardDensity,
    series: Option<&ProviderMiniSeries>,
    spend: Option<&ProviderSpendSummary>,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors;
    let compact = density == QuotaCardDensity::Compact;
    let entries = snapshot
        .and_then(|snap| snap.value.as_deref())
        .unwrap_or_default();
    let loading = snapshot.is_some_and(|snap| snap.loading);
    let snapshot_error = snapshot.and_then(|snap| snap.error.clone());

    let header = h_flex()
        .gap_2()
        .items_center()
        .child(provider_glyph(
            &provider.icon,
            if compact { 15.0 } else { 17.0 },
            cx,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(if compact { 13.0 } else { 14.0 }))
                .font_semibold()
                .text_color(colors.foreground)
                .child(provider.display_name.clone()),
        )
        .when(loading, |row| row.child(Spinner::new().xsmall()));

    let rows: Vec<AnyElement> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| render_quota_row(&provider.id, index, entry, density, series, cx))
        .collect();

    let empty_note = (rows.is_empty() && snapshot_error.is_none() && !loading).then(|| {
        div()
            .text_size(px(12.0))
            .text_color(colors.muted_foreground)
            .child(t!("usage.limits_empty_provider"))
            .into_any_element()
    });

    let footer = spend
        .filter(|summary| summary.has_activity())
        .map(|summary| render_spend_footer(summary, compact, cx));

    v_flex()
        .min_w_0()
        .gap(px(if compact { 8.0 } else { 12.0 }))
        .p(px(if compact { 10.0 } else { 16.0 }))
        .rounded(px(if compact { 10.0 } else { 12.0 }))
        .border_1()
        .border_color(colors.border)
        .bg(colors.group_box)
        .when(!compact, |card| card.shadow_xs())
        .font_features(gpui_kit::FontFeatures(std::sync::Arc::new(vec![(
            "tnum".into(),
            1,
        )])))
        .child(header)
        .when(!rows.is_empty(), |card| {
            card.child(
                v_flex()
                    .gap(px(if compact { 8.0 } else { 11.0 }))
                    .children(rows),
            )
        })
        .children(empty_note)
        .when_some(snapshot_error, |card, error| {
            card.child(div().text_xs().text_color(colors.danger).child(error))
        })
        .children(footer)
        .into_any_element()
}
