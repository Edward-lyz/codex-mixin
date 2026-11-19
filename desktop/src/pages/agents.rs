//! Agents page: detected coding agents and whether Mixin manages them.

use std::collections::HashMap;
use std::sync::OnceLock;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _,
};
use rust_i18n::t;
use serde::Deserialize;

use super::{is_busy, placeholder, run_change};
use crate::store::{Change, Store, query};
use crate::tr;
use crate::widgets::{logo_tile, status_dot};

#[derive(Clone, Debug, Default, Deserialize)]
struct AgentList {
    agents: Vec<Agent>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Agent {
    id: String,
    name: String,
    detected: bool,
    #[serde(default)]
    cli: AgentVersion,
    integration: Integration,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct AgentVersion {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    app_version: Option<String>,
    #[serde(default)]
    latest: Option<String>,
    #[serde(default)]
    update: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Integration {
    state: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl Integration {
    fn connected(&self) -> bool {
        matches!(
            self.state.as_str(),
            "managed" | "drift" | "pending" | "pending_removal"
        )
    }

    fn label(&self) -> SharedString {
        let state = match self.state.as_str() {
            "managed" if self.mode.as_deref() == Some("native_models") => {
                t!("agents.state.native_managed")
            }
            "managed" => t!("agents.state.managed"),
            "unmanaged" => t!("agents.state.unmanaged"),
            "drift" => t!("agents.state.drift"),
            "error" => t!("agents.state.error"),
            "unsupported" => t!("agents.state.unsupported"),
            "pending" => t!("agents.state.pending"),
            "pending_removal" => t!("agents.state.pending_removal"),
            other => t!("agents.state.unknown", state = other),
        };
        let mode = match self.mode.as_deref() {
            Some("native_credentials") => Some(t!("agents.mode.native_credentials")),
            Some("native_runtime") => Some(t!("agents.mode.native_runtime")),
            Some("native_app") => Some(t!("agents.mode.native_app")),
            Some("confirmed_import") => Some(t!("agents.mode.confirmed_import")),
            _ => None,
        };
        match mode {
            Some(mode) => format!("{state} · {mode}").into(),
            None => state.into(),
        }
    }
}

#[derive(Deserialize)]
struct AgentArtwork {
    file: String,
    template: bool,
}

fn artwork() -> &'static HashMap<String, AgentArtwork> {
    static ARTWORK: OnceLock<HashMap<String, AgentArtwork>> = OnceLock::new();
    ARTWORK.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/agents/manifest.json")).unwrap_or_default()
    })
}

fn agent_logo(id: &str, cx: &App) -> AnyElement {
    match artwork().get(id) {
        Some(art) => logo_tile(
            format!("agents/{}", art.file).into(),
            art.template,
            true,
            cx,
        ),
        None => logo_tile("providers/custom.svg".into(), true, true, cx),
    }
}

pub struct AgentsPage {
    agents: Option<Vec<Agent>>,
    error: Option<SharedString>,
    loading: bool,
    show_undetected: bool,
    revision: u64,
    _subscriptions: Vec<Subscription>,
}

impl AgentsPage {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        // Reload after every change so connect/disconnect results show.
        let subscription = cx.observe(&store, |page, store: Entity<Store>, cx| {
            let revision = store.read(cx).revision;
            if revision != page.revision {
                page.revision = revision;
                page.reload(cx);
            }
        });
        let mut page = Self {
            agents: None,
            error: None,
            loading: false,
            show_undetected: false,
            revision: Store::global(cx).read(cx).revision,
            _subscriptions: vec![subscription],
        };
        page.reload(cx);
        page
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.load(vec!["agent".into(), "list".into(), "--json".into()], cx);
    }

    fn check_versions(&mut self, cx: &mut Context<Self>) {
        self.load(
            vec![
                "agent".into(),
                "versions".into(),
                "--json".into(),
                "--refresh".into(),
            ],
            cx,
        );
    }

    fn load(&mut self, args: Vec<String>, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        let task = query::<AgentList>(args, cx);
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                page.loading = false;
                match result {
                    // `agent versions` omits integration details; merge versions
                    // into the listed agents instead of replacing them.
                    Ok(list) => page.merge(list.agents),
                    Err(error) => page.error = Some(error.message.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn merge(&mut self, agents: Vec<Agent>) {
        self.error = None;
        let Some(current) = &mut self.agents else {
            self.agents = Some(agents);
            return;
        };
        for agent in agents {
            match current.iter_mut().find(|existing| existing.id == agent.id) {
                Some(existing) if agent.integration.state.is_empty() => existing.cli = agent.cli,
                Some(existing) => *existing = agent,
                None => current.push(agent),
            }
        }
    }

    fn toggle(id: String, name: String, connect: bool, window: &mut Window, cx: &mut App) {
        if !connect {
            window.open_alert_dialog(cx, move |dialog, _, _| {
                let id = id.clone();
                dialog
                    .title(tr!("agents.disconnect_title", name = name))
                    .description(tr!("agents.disconnect_detail"))
                    .confirm()
                    .ok_text(t!("agents.disconnect"))
                    .cancel_text(t!("common.cancel"))
                    .on_ok(move |_, _, cx| {
                        run_change(
                            Change::new(
                                t!("agents.disconnect_label"),
                                ["agent", "disconnect", id.as_str()],
                            ),
                            cx,
                        );
                        true
                    })
            });
            return;
        }
        if id != "codex" {
            run_change(
                Change::new(
                    t!("agents.connect_label"),
                    ["agent", "connect", id.as_str()],
                ),
                cx,
            );
            return;
        }
        // Codex can keep the official account beside custom models.
        window.open_alert_dialog(cx, move |dialog, _, _| {
            dialog
                .title(tr!("agents.codex_mode_title"))
                .description(tr!("agents.codex_mode_detail"))
                .confirm()
                .ok_text(t!("agents.codex_mode_official"))
                .cancel_text(t!("agents.codex_mode_custom"))
                .on_ok(|_, _, cx| {
                    let args = ["agent", "connect", "codex", "--mode", "official-account"];
                    run_change(Change::new(t!("agents.connect_label"), args), cx);
                    true
                })
                .on_cancel(|_, _, cx| {
                    let args = ["agent", "connect", "codex", "--mode", "custom-only"];
                    run_change(Change::new(t!("agents.connect_label"), args), cx);
                    true
                })
        });
    }

    fn render_agent(&self, agent: &Agent, first: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let connected = agent.integration.connected();
        let versions: Vec<String> = [
            agent.cli.version.as_ref().map(|version| {
                if agent.cli.app_version.is_some() {
                    format!("CLI {version}")
                } else {
                    version.clone()
                }
            }),
            agent
                .cli
                .app_version
                .as_ref()
                .map(|version| format!("App {version}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let problem = agent.integration.error.clone().or_else(|| {
            agent
                .cli
                .error
                .clone()
                .map(|error| t!("agents.version_failed", error = error).into_owned())
        });
        let dot = if connected {
            colors.success
        } else if agent.integration.state == "error" {
            colors.warning
        } else {
            colors.muted_foreground
        };
        let id = agent.id.clone();
        let name = agent.name.clone();
        let upgrade_id = agent.id.clone();
        h_flex()
            .gap_3()
            .px_4()
            .py_3()
            .when(!first, |row| row.border_t_1().border_color(colors.border))
            .child(agent_logo(&agent.id, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_sm().font_semibold().child(agent.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(versions.join("  ")),
                            ),
                    )
                    .when(agent.integration.state != "unsupported", |text| {
                        text.child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .child(status_dot(dot))
                                .child(div().truncate().child(agent.integration.label()))
                                .when(connected && !agent.detected, |line| {
                                    line.child(t!("agents.not_detected"))
                                }),
                        )
                    })
                    .when_some(problem, |text, detail| {
                        text.child(
                            div()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .line_clamp(2)
                                .child(detail),
                        )
                    }),
            )
            .when(agent.cli.update, |row| {
                row.child(
                    Button::new(SharedString::from(format!("upgrade-{}", agent.id)))
                        .outline()
                        .small()
                        .icon(IconName::Download)
                        .label(t!(
                            "agents.upgrade_to",
                            version = agent.cli.latest.clone().unwrap_or_default()
                        ))
                        .disabled(is_busy(cx))
                        .on_click(move |_, _, cx| {
                            run_change(
                                Change::new(
                                    t!("agents.upgrade_label"),
                                    ["agent", "upgrade", upgrade_id.as_str(), "--json"],
                                ),
                                cx,
                            );
                        }),
                )
            })
            .child(
                Switch::new(SharedString::from(format!("connect-{}", agent.id)))
                    .checked(connected)
                    .disabled(agent.integration.state == "unsupported" || is_busy(cx))
                    .on_change(move |connect, window, cx| {
                        Self::toggle(id.clone(), name.clone(), *connect, window, cx)
                    }),
            )
    }
}

impl Render for AgentsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let Some(agents) = self.agents.clone() else {
            return match self.error.clone() {
                Some(error) => placeholder(
                    IconName::TriangleAlert,
                    t!("agents.load_failed"),
                    Some(error),
                    None,
                    cx,
                )
                .into_any_element(),
                None => placeholder(IconName::LoaderCircle, t!("agents.loading"), None, None, cx)
                    .into_any_element(),
            };
        };
        let (visible, hidden): (Vec<_>, Vec<_>) = agents
            .iter()
            .partition(|agent| agent.detected || agent.integration.connected());
        let connected = agents
            .iter()
            .filter(|agent| agent.integration.connected())
            .count();
        let detected = agents.iter().filter(|agent| agent.detected).count();
        let list = |rows: Vec<gpui_kit::AnyElement>| {
            v_flex()
                .rounded_lg()
                .border_1()
                .border_color(colors.border)
                .bg(colors.group_box)
                .children(rows)
        };
        let mut visible_rows = Vec::new();
        for (index, agent) in visible.iter().enumerate() {
            visible_rows.push(self.render_agent(agent, index == 0, cx).into_any_element());
        }
        let mut hidden_rows = Vec::new();
        if self.show_undetected {
            for (index, agent) in hidden.iter().enumerate() {
                hidden_rows.push(self.render_agent(agent, index == 0, cx).into_any_element());
            }
        }
        let visible_list = list(visible_rows);
        let hidden_list = self.show_undetected.then(|| list(hidden_rows));
        div()
            .id("agents-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .mx_auto()
                    .w_full()
                    .max_w(gpui_kit::rems(56.0))
                    .px_6()
                    .py_5()
                    .gap_4()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                v_flex()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_semibold()
                                            .child(t!("console.page.agents")),
                                    )
                                    .child(
                                        div().text_xs().text_color(colors.muted_foreground).child(
                                            t!(
                                                "agents.summary",
                                                connected = connected,
                                                detected = detected
                                            ),
                                        ),
                                    ),
                            )
                            .child(
                                Button::new("check-agent-versions")
                                    .ghost()
                                    .small()
                                    .icon(IconName::RefreshCw)
                                    .label(t!("agents.check_versions"))
                                    .loading(self.loading)
                                    .on_click(
                                        cx.listener(|page, _, _, cx| page.check_versions(cx)),
                                    ),
                            ),
                    )
                    .when_some(self.error.clone(), |body, error| {
                        body.child(div().text_xs().text_color(colors.danger).child(error))
                    })
                    .child(visible_list)
                    .when(!hidden.is_empty(), |body| {
                        body.child(
                            Button::new("toggle-undetected")
                                .ghost()
                                .small()
                                .icon(if self.show_undetected {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .label(t!("agents.show_undetected", count = hidden.len()))
                                .on_click(cx.listener(|page, _, _, cx| {
                                    page.show_undetected = !page.show_undetected;
                                    cx.notify();
                                })),
                        )
                    })
                    .children(hidden_list),
            )
            .into_any_element()
    }
}
