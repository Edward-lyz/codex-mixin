//! Context page: the context engine switch and tuning, plus statistics for
//! recent sessions. Each setting saves immediately through the CLI.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _,
};
use rust_i18n::t;
use serde::Deserialize;

use super::usage::compact_number;
use super::{is_busy, placeholder, run_change};
use crate::store::{Change, Store, query};
use crate::tr;
use crate::widgets::{row_divider, section, setting_row};

const SESSION_LIMIT: usize = 30;

#[derive(Clone, Debug, Default, Deserialize)]
struct ContextConfig {
    context: ContextSettings,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ContextSettings {
    enabled: bool,
    guidance_only: bool,
    max_tier: u64,
    pressure_ratio: f64,
    protected_messages: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ContextStats {
    totals: StatsTotals,
    #[serde(default)]
    sessions: Vec<SessionStats>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct StatsTotals {
    sessions: u64,
    messages: u64,
    compressions: u64,
    saved_tokens: u64,
    view_tokens: u64,
    cache: CacheStats,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CacheStats {
    #[serde(default)]
    hit_percent: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SessionStats {
    session: String,
    #[serde(default)]
    display: String,
    compressions: u64,
    messages: u64,
    saved_tokens: u64,
    view_tokens: u64,
    updated_at: i64,
    cache: CacheStats,
}

pub struct ContextPage {
    config: Option<ContextSettings>,
    stats: Option<ContextStats>,
    error: Option<SharedString>,
    revision: u64,
    _subscriptions: Vec<Subscription>,
}

impl ContextPage {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let subscription = cx.observe(&store, |page, store: Entity<Store>, cx| {
            let revision = store.read(cx).revision;
            if revision != page.revision {
                page.revision = revision;
                page.reload(cx);
            }
        });
        Self {
            config: None,
            stats: None,
            error: None,
            revision: store.read(cx).revision,
            _subscriptions: vec![subscription],
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        // Statistics can take a while on large databases; settings show first.
        let config = query::<ContextConfig>(
            vec![
                "context".into(),
                "config".into(),
                "get".into(),
                "--json".into(),
            ],
            cx,
        );
        cx.spawn(async move |page, cx| {
            let config = config.await;
            page.update(cx, |page, cx| {
                match config {
                    Ok(config) => page.config = Some(config.context),
                    Err(error) => page.error = Some(error.message.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        let stats =
            query::<ContextStats>(vec!["context".into(), "stats".into(), "--json".into()], cx);
        cx.spawn(async move |page, cx| {
            let stats = stats.await;
            page.update(cx, |page, cx| {
                if let Ok(stats) = stats {
                    page.stats = Some(stats);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set(flag: &'static str, value: String, cx: &mut App) {
        let args = vec![
            "context".to_owned(),
            "config".to_owned(),
            "set".to_owned(),
            flag.to_owned(),
            value,
            "--json".to_owned(),
        ];
        run_change(Change::new(t!("context.save_label"), args), cx);
    }

    fn choice(
        id: &'static str,
        current: String,
        options: Vec<(String, SharedString)>,
        flag: &'static str,
        disabled: bool,
    ) -> AnyElement {
        let label = options
            .iter()
            .find(|(value, _)| *value == current)
            .map_or_else(
                || SharedString::from(current.clone()),
                |(_, label)| label.clone(),
            );
        Button::new(id)
            .outline()
            .small()
            .label(label)
            .dropdown_caret(true)
            .disabled(disabled)
            .dropdown_menu(move |menu, _, _| {
                options.iter().fold(menu, |menu, (value, label)| {
                    let value = value.clone();
                    menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(value == current)
                            .on_click(move |_, _, cx| Self::set(flag, value.clone(), cx)),
                    )
                })
            })
            .into_any_element()
    }

    fn render_settings(
        &self,
        settings: &ContextSettings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy = is_busy(cx);
        let off = !settings.enabled || busy;
        let body = v_flex()
            .child(setting_row(
                t!("context.enabled"),
                Some(tr!("context.enabled_detail")),
                Switch::new("context-enabled")
                    .checked(settings.enabled)
                    .disabled(busy)
                    .on_change(|value, _, cx| Self::set("--enabled", value.to_string(), cx)),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("context.guidance_only"),
                Some(tr!("context.guidance_only_detail")),
                Switch::new("context-guidance")
                    .checked(settings.guidance_only)
                    .disabled(off)
                    .on_change(|value, _, cx| Self::set("--guidance-only", value.to_string(), cx)),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("context.max_tier"),
                Some(tr!("context.max_tier_detail")),
                Self::choice(
                    "context-tier",
                    settings.max_tier.to_string(),
                    (1..=3)
                        .map(|tier| (tier.to_string(), tr!("context.tier", tier = tier)))
                        .collect(),
                    "--max-tier",
                    off,
                ),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("context.pressure"),
                Some(tr!("context.pressure_detail")),
                Self::choice(
                    "context-pressure",
                    format!("{:.2}", settings.pressure_ratio),
                    ["0.60", "0.70", "0.75", "0.80", "0.90"]
                        .into_iter()
                        .map(|ratio| {
                            (
                                ratio.to_owned(),
                                SharedString::from(format!(
                                    "{:.0}%",
                                    ratio.parse::<f64>().unwrap_or_default() * 100.0
                                )),
                            )
                        })
                        .collect(),
                    "--pressure-ratio",
                    off,
                ),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("context.protected"),
                Some(tr!("context.protected_detail")),
                Self::choice(
                    "context-protected",
                    settings.protected_messages.to_string(),
                    [1, 2, 3, 5, 8]
                        .into_iter()
                        .map(|count| (count.to_string(), tr!("context.messages", count = count)))
                        .collect(),
                    "--protected-messages",
                    off,
                ),
                cx,
            ));
        section(t!("context.engine"), None, body, cx)
    }

    fn tile(
        title: SharedString,
        value: String,
        note: Option<String>,
        icon: IconName,
        cx: &App,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
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
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(Icon::new(icon).xsmall())
                    .child(title),
            )
            .child(div().text_xl().font_semibold().child(value))
            .when_some(note, |tile, note| {
                tile.child(
                    div()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child(note),
                )
            })
    }

    fn render_stats(&self, stats: &ContextStats, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let totals = &stats.totals;
        let hit = |cache: &CacheStats| {
            cache
                .hit_percent
                .map_or_else(|| "\u{2014}".to_owned(), |hit| format!("{hit:.1}%"))
        };
        let mut sessions = stats.sessions.clone();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        sessions.truncate(SESSION_LIMIT);
        let rows = sessions.iter().enumerate().map(|(index, session)| {
            let title = if session.display.trim().is_empty() {
                session.session.clone()
            } else {
                session.display.clone()
            };
            let cell = |text: String| {
                div()
                    .w_24()
                    .text_right()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(text)
            };
            h_flex()
                .px_4()
                .h_10()
                .when(index % 2 == 1, |row| row.bg(colors.table_even))
                .child(div().flex_1().min_w_0().text_sm().truncate().child(title))
                .child(cell(session.messages.to_string()))
                .child(cell(session.compressions.to_string()))
                .child(cell(compact_number(session.saved_tokens as f64)))
                .child(cell(compact_number(session.view_tokens as f64)))
                .child(cell(hit(&session.cache)))
        });
        let head = |label: SharedString| {
            div()
                .w_24()
                .text_right()
                .text_xs()
                .font_medium()
                .text_color(colors.table_head_foreground)
                .child(label)
        };
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(Self::tile(
                        tr!("context.sessions"),
                        compact_number(totals.sessions as f64),
                        Some(
                            t!(
                                "context.archived",
                                count = compact_number(totals.messages as f64)
                            )
                            .into_owned(),
                        ),
                        IconName::MessagesSquare,
                        cx,
                    ))
                    .child(Self::tile(
                        tr!("context.compressions"),
                        compact_number(totals.compressions as f64),
                        None,
                        IconName::Layers,
                        cx,
                    ))
                    .child(Self::tile(
                        tr!("context.saved"),
                        compact_number(totals.saved_tokens as f64),
                        Some(
                            t!(
                                "context.view",
                                count = compact_number(totals.view_tokens as f64)
                            )
                            .into_owned(),
                        ),
                        IconName::Leaf,
                        cx,
                    ))
                    .child(Self::tile(
                        tr!("context.cache_hit"),
                        hit(&totals.cache),
                        None,
                        IconName::Zap,
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .rounded_lg()
                    .border_1()
                    .border_color(colors.border)
                    .overflow_hidden()
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
                                    .child(t!("context.column.session")),
                            )
                            .child(head(tr!("context.column.messages")))
                            .child(head(tr!("context.column.compressions")))
                            .child(head(tr!("context.column.saved")))
                            .child(head(tr!("context.column.view")))
                            .child(head(tr!("context.column.cache"))),
                    )
                    .when(sessions.is_empty(), |list| {
                        list.child(
                            div()
                                .p_6()
                                .text_sm()
                                .text_color(colors.muted_foreground)
                                .child(t!("context.no_sessions")),
                        )
                    })
                    .children(rows),
            )
    }
}

impl Render for ContextPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let Some(config) = self.config.clone() else {
            return match self.error.clone() {
                Some(error) => placeholder(
                    IconName::TriangleAlert,
                    t!("context.load_failed"),
                    Some(error),
                    None,
                    cx,
                )
                .into_any_element(),
                None => placeholder(
                    IconName::LoaderCircle,
                    t!("context.loading"),
                    None,
                    None,
                    cx,
                )
                .into_any_element(),
            };
        };
        let stats = self.stats.clone();
        div()
            .id("context-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .mx_auto()
                    .w_full()
                    .max_w(gpui_kit::rems(60.0))
                    .px_6()
                    .py_5()
                    .gap_5()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                v_flex()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_semibold()
                                            .child(t!("console.page.context")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors.muted_foreground)
                                            .child(t!("context.subtitle")),
                                    ),
                            )
                            .child(
                                Button::new("reload-context")
                                    .ghost()
                                    .small()
                                    .icon(IconName::RefreshCw)
                                    .on_click(cx.listener(|page, _, _, cx| page.reload(cx))),
                            ),
                    )
                    .child(self.render_settings(&config, cx))
                    .children(stats.map(|stats| self.render_stats(&stats, cx).into_any_element())),
            )
            .into_any_element()
    }
}
