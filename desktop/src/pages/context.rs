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
    AnyElement, App, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, div, prelude::FluentBuilder as _,
};
use rust_i18n::t;
use serde::Deserialize;

use super::usage::compact_number;
use super::{is_busy, placeholder, run_change};
use crate::store::{Change, Store, query};
use crate::tr;
use crate::widgets::{page_header, row_divider, section, setting_row};

const SESSION_LIMIT: &str = "10";
type SessionCursor = (i64, String);

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
struct ContextSummary {
    totals: StatsTotals,
}

#[derive(Debug, Deserialize)]
struct ContextSessions {
    sessions: Vec<SessionStats>,
    next_cursor: Option<SessionCursor>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct StatsTotals {
    sessions: u64,
    messages: u64,
    compressions: u64,
    saved_tokens: u64,
    cache: CacheStats,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CacheStats {
    #[serde(default)]
    hit_percent: Option<f64>,
}

impl CacheStats {
    fn display(&self) -> String {
        self.hit_percent
            .map_or_else(|| "\u{2014}".to_owned(), |hit| format!("{hit:.1}%"))
    }
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
    cache: CacheStats,
}

pub struct ContextPage {
    config: Option<ContextSettings>,
    totals: Option<StatsTotals>,
    sessions: Option<ContextSessions>,
    settings_error: Option<SharedString>,
    summary_error: Option<SharedString>,
    sessions_error: Option<(SharedString, Option<SessionCursor>)>,
    settings_task: Option<Task<()>>,
    loading_sessions: bool,
    active: bool,
    generation: u64,
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
                if page.active {
                    page.reload_settings(cx);
                }
            }
        });
        Self {
            config: None,
            totals: None,
            sessions: None,
            settings_error: None,
            summary_error: None,
            sessions_error: None,
            settings_task: None,
            loading_sessions: false,
            active: false,
            generation: 0,
            revision: store.read(cx).revision,
            _subscriptions: vec![subscription],
        }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active {
            self.reload(cx);
        }
    }

    fn reload_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_error = None;
        let config = query::<ContextConfig>(
            vec![
                "context".into(),
                "config".into(),
                "get".into(),
                "--json".into(),
            ],
            cx,
        );
        self.settings_task = Some(cx.spawn(async move |page, cx| {
            let config = config.await;
            page.update(cx, |page, cx| {
                match config {
                    Ok(config) => page.config = Some(config.context),
                    Err(error) => page.settings_error = Some(error.message.into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_settings(cx);
        self.generation += 1;
        let generation = self.generation;
        self.summary_error = None;
        self.sessions_error = None;
        self.loading_sessions = true;
        let summary = query::<ContextSummary>(
            ["context", "stats", "--summary", "--json"]
                .map(str::to_owned)
                .into(),
            cx,
        );
        cx.spawn(async move |page, cx| {
            let summary = summary.await;
            page.update(cx, |page, cx| {
                if generation != page.generation {
                    return;
                }
                match summary {
                    Ok(summary) => page.totals = Some(summary.totals),
                    Err(error) => page.summary_error = Some(error.message.into()),
                }
                if page.active {
                    page.load_sessions(None, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn load_sessions(&mut self, cursor: Option<SessionCursor>, cx: &mut Context<Self>) {
        let mut args = ["context", "stats", "--limit", SESSION_LIMIT, "--json"]
            .map(str::to_owned)
            .to_vec();
        if let Some(cursor) = &cursor {
            args.extend(["--before".into(), serde_json::json!(cursor).to_string()]);
        }
        let generation = self.generation;
        self.loading_sessions = true;
        self.sessions_error = None;
        let task = query::<ContextSessions>(args, cx);
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                if generation != page.generation {
                    return;
                }
                page.loading_sessions = false;
                match result {
                    Ok(mut result) => {
                        if let Some(current) = page.sessions.as_mut().filter(|_| cursor.is_some()) {
                            current.sessions.append(&mut result.sessions);
                            current.next_cursor = result.next_cursor;
                        } else {
                            page.sessions = Some(result);
                        }
                    }
                    Err(error) => page.sessions_error = Some((error.message.into(), cursor)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
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
            .secondary()
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
            .min_w_0()
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
            .when_some(note, |tile, note| {
                tile.child(
                    div()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child(note),
                )
            })
    }

    fn render_sessions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let sessions = self
            .sessions
            .as_ref()
            .map_or(&[][..], |page| page.sessions.as_slice());
        let cursor = self
            .sessions
            .as_ref()
            .and_then(|page| page.next_cursor.clone());
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
                .id(ElementId::Name(format!("context-session-{index}").into()))
                .px_4()
                .h_9()
                .when(index > 0, |row| {
                    row.border_t_1().border_color(colors.border)
                })
                .hover(|style| style.bg(colors.table_hover))
                .child(div().flex_1().min_w_0().text_sm().truncate().child(title))
                .child(cell(session.messages.to_string()))
                .child(cell(session.compressions.to_string()))
                .child(cell(compact_number(session.saved_tokens as f64)))
                .child(cell(compact_number(session.view_tokens as f64)))
                .child(cell(session.cache.display()))
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
            .rounded_xl()
            .border_1()
            .border_color(colors.border)
            .bg(colors.group_box)
            .shadow_xs()
            .overflow_hidden()
            .child(
                h_flex()
                    .px_4()
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
                            .child(t!("context.column.session")),
                    )
                    .child(head(tr!("context.column.messages")))
                    .child(head(tr!("context.column.compressions")))
                    .child(head(tr!("context.column.saved")))
                    .child(head(tr!("context.column.view")))
                    .child(head(tr!("context.column.cache"))),
            )
            .when(
                sessions.is_empty() && !self.loading_sessions && self.sessions_error.is_none(),
                |list| {
                    list.child(
                        div()
                            .p_6()
                            .text_sm()
                            .text_color(colors.muted_foreground)
                            .child(t!("context.no_sessions")),
                    )
                },
            )
            .children(rows)
            .when_some(self.sessions_error.as_ref(), |list, (error, _)| {
                list.child(
                    div()
                        .px_4()
                        .pt_3()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child(error.clone()),
                )
            })
            .when(
                self.loading_sessions || cursor.is_some() || self.sessions_error.is_some(),
                |list| {
                    list.child(
                        h_flex().justify_center().p_3().child(
                            Button::new("more-context-sessions")
                                .secondary()
                                .small()
                                .label(if self.sessions_error.is_some() {
                                    tr!("context.retry_sessions")
                                } else if self.loading_sessions {
                                    tr!("context.loading_sessions")
                                } else {
                                    tr!("context.load_more")
                                })
                                .loading(self.loading_sessions)
                                .disabled(self.loading_sessions)
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    if !page.loading_sessions {
                                        let cursor = page.sessions_error.as_ref().map_or_else(
                                            || cursor.clone(),
                                            |(_, cursor)| cursor.clone(),
                                        );
                                        page.load_sessions(cursor, cx);
                                    }
                                })),
                        ),
                    )
                },
            )
    }

    fn render_summary(totals: &StatsTotals, cx: &App) -> impl IntoElement {
        div()
            .grid()
            .grid_cols(4)
            .gap_3()
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
                None,
                IconName::Leaf,
                cx,
            ))
            .child(Self::tile(
                tr!("context.cache_hit"),
                totals.cache.display(),
                None,
                IconName::Zap,
                cx,
            ))
    }
}

impl Render for ContextPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let config = self.config.clone();
        let totals = self.totals.clone();
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
                    .pt_3()
                    .pb_6()
                    .gap_5()
                    .child(page_header(
                        IconName::Layers,
                        t!("console.page.context"),
                        Some(tr!("context.subtitle")),
                        Button::new("reload-context")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .on_click(cx.listener(|page, _, _, cx| page.reload(cx))),
                        cx,
                    ))
                    .when_some(totals, |body, totals| {
                        body.child(Self::render_summary(&totals, cx))
                    })
                    .when_some(self.summary_error.clone(), |body, error| {
                        body.child(placeholder(
                            IconName::TriangleAlert,
                            t!("context.summary_failed"),
                            Some(error),
                            None,
                            cx,
                        ))
                    })
                    .when_some(config, |body, config| {
                        body.child(self.render_settings(&config, cx))
                    })
                    .when_some(self.settings_error.clone(), |body, error| {
                        body.child(placeholder(
                            IconName::TriangleAlert,
                            t!("context.load_failed"),
                            Some(error),
                            None,
                            cx,
                        ))
                    })
                    .child(self.render_sessions(cx)),
            )
            .into_any_element()
    }
}
