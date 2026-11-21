//! Agents page: detected coding agents and whether Mixin manages them.

use std::collections::HashMap;
use std::sync::OnceLock;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
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
use crate::widgets::{logo_tile, page_header, status_dot};

#[derive(Clone, Debug, Default, Deserialize)]
struct AgentList {
    agents: Vec<Agent>,
}

#[derive(Deserialize)]
struct AgentVersions {
    agents: Vec<AgentVersionEntry>,
}

#[derive(Deserialize)]
struct AgentVersionEntry {
    id: String,
    cli: AgentVersion,
}

enum AgentRead {
    List(AgentList),
    Versions(AgentVersions),
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
    reload_pending: bool,
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
            reload_pending: false,
            show_undetected: false,
            revision: Store::global(cx).read(cx).revision,
            _subscriptions: vec![subscription],
        };
        page.reload(cx);
        page
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.load(false, cx);
    }

    fn check_versions(&mut self, cx: &mut Context<Self>) {
        self.load(true, cx);
    }

    fn load(&mut self, refresh_versions: bool, cx: &mut Context<Self>) {
        if self.loading {
            // A reload asked for mid-flight runs once the current read lands.
            self.reload_pending = true;
            return;
        }
        self.loading = true;
        cx.notify();
        let task = if refresh_versions {
            let versions = query::<AgentVersions>(
                ["agent", "versions", "--json", "--refresh"]
                    .map(str::to_owned)
                    .to_vec(),
                cx,
            );
            cx.spawn(async move |_, _| versions.await.map(AgentRead::Versions))
        } else {
            let list =
                query::<AgentList>(["agent", "list", "--json"].map(str::to_owned).to_vec(), cx);
            cx.spawn(async move |_, _| list.await.map(AgentRead::List))
        };
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                page.loading = false;
                match result {
                    Ok(AgentRead::List(list)) => {
                        page.agents = Some(list.agents);
                        page.error = None;
                    }
                    Ok(AgentRead::Versions(versions)) => page.merge_versions(versions.agents),
                    Err(error) => page.error = Some(error.message.into()),
                }
                if std::mem::take(&mut page.reload_pending) {
                    page.reload(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn merge_versions(&mut self, versions: Vec<AgentVersionEntry>) {
        self.error = None;
        let Some(current) = &mut self.agents else {
            return;
        };
        for version in versions {
            if let Some(agent) = current.iter_mut().find(|agent| agent.id == version.id) {
                agent.cli = version.cli;
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
        // Codex can keep the official account beside custom models. Each
        // mode is its own button, so dismissing the dialog connects nothing.
        window.open_dialog(cx, move |dialog, _, _| {
            let connect = |mode: &'static str| {
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                    let args = ["agent", "connect", "codex", "--mode", mode];
                    run_change(Change::new(t!("agents.connect_label"), args), cx);
                    window.close_dialog(cx);
                }
            };
            dialog
                .title(tr!("agents.codex_mode_title"))
                .w(gpui_kit::px(460.0))
                .child(div().text_sm().child(t!("agents.codex_mode_detail")))
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("codex-mode-cancel")
                                .ghost()
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("codex-mode-custom")
                                .secondary()
                                .label(t!("agents.codex_mode_custom"))
                                .on_click(connect("custom-only")),
                        )
                        .child(
                            Button::new("codex-mode-official")
                                .primary()
                                .label(t!("agents.codex_mode_official"))
                                .on_click(connect("official-account")),
                        ),
                )
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
            agent.cli.error.clone().map(|error| {
                if error
                    == format!(
                        "{} upgrade unavailable: no verified updater for this install",
                        agent.id
                    )
                {
                    t!("agents.update_via_installation").into_owned()
                } else {
                    t!("agents.version_failed", error = error).into_owned()
                }
            })
        });
        let dot = if connected {
            colors.success
        } else if agent.integration.state == "error" {
            colors.warning
        } else {
            colors.muted_foreground
        };
        // The group already says whether the agent is connected; the row
        // only spells out states that need attention or name a mode.
        let plain = matches!(agent.integration.state.as_str(), "managed" | "unmanaged")
            && agent.integration.mode.is_none();
        let show_state = agent.integration.state != "unsupported" && !plain;
        let missing_installer = connected && !agent.detected;
        let id = agent.id.clone();
        let name = agent.name.clone();
        let upgrade_id = agent.id.clone();
        h_flex()
            .id(SharedString::from(format!("agent-row-{}", agent.id)))
            .gap_3()
            .px_4()
            .py_2p5()
            .when(!first, |row| row.border_t_1().border_color(colors.border))
            .hover(|style| style.bg(colors.list_hover))
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
                            .children(versions.into_iter().map(|version| {
                                div()
                                    .px_1p5()
                                    .rounded_sm()
                                    .bg(colors.muted)
                                    .text_xs()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(colors.muted_foreground)
                                    .child(version)
                            })),
                    )
                    .when(show_state || missing_installer, |text| {
                        text.child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .when(show_state, |line| {
                                    line.child(status_dot(dot))
                                        .child(div().truncate().child(agent.integration.label()))
                                })
                                .when(missing_installer, |line| {
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
                        .secondary()
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
        let connected: Vec<&Agent> = agents
            .iter()
            .filter(|agent| agent.integration.connected())
            .collect();
        let available: Vec<&Agent> = agents
            .iter()
            .filter(|agent| agent.detected && !agent.integration.connected())
            .collect();
        let missing: Vec<&Agent> = agents
            .iter()
            .filter(|agent| !agent.detected && !agent.integration.connected())
            .collect();
        let group_title = |label: SharedString, count: usize| {
            h_flex()
                .gap_1p5()
                .px_1()
                .text_xs()
                .font_semibold()
                .text_color(colors.muted_foreground)
                .child(label)
                .child(div().font_normal().child(count.to_string()))
        };
        let mut groups = Vec::new();
        for (label, members) in [
            (tr!("agents.group.connected"), &connected),
            (tr!("agents.group.available"), &available),
        ] {
            if members.is_empty() {
                continue;
            }
            let rows: Vec<_> = members
                .iter()
                .enumerate()
                .map(|(index, agent)| self.render_agent(agent, index == 0, cx).into_any_element())
                .collect();
            groups.push(
                v_flex()
                    .gap_1p5()
                    .child(group_title(label, members.len()))
                    .child(
                        v_flex()
                            .rounded_xl()
                            .overflow_hidden()
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.group_box)
                            .shadow_xs()
                            .children(rows),
                    )
                    .into_any_element(),
            );
        }
        let missing_rows: Vec<_> = if self.show_undetected {
            missing
                .iter()
                .enumerate()
                .map(|(index, agent)| self.render_agent(agent, index == 0, cx).into_any_element())
                .collect()
        } else {
            Vec::new()
        };
        let missing_count = missing.len();
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
                    .pt_3()
                    .pb_6()
                    .gap_4()
                    .child(page_header(
                        IconName::Bot,
                        t!("console.page.agents"),
                        Some(tr!(
                            "agents.summary",
                            connected = connected.len(),
                            detected = agents.iter().filter(|agent| agent.detected).count()
                        )),
                        Button::new("check-agent-versions")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(t!("agents.check_versions"))
                            .disabled(self.loading)
                            .loading(self.loading)
                            .on_click(cx.listener(|page, _, _, cx| page.check_versions(cx))),
                        cx,
                    ))
                    .when_some(self.error.clone(), |body, error| {
                        body.child(div().text_xs().text_color(colors.danger).child(error))
                    })
                    .children(groups)
                    .when(missing_count > 0, |body| {
                        body.child(
                            v_flex()
                                .gap_1p5()
                                .child(
                                    h_flex()
                                        .id("toggle-undetected")
                                        .gap_1()
                                        .cursor_pointer()
                                        .child(group_title(
                                            tr!("agents.group.missing"),
                                            missing_count,
                                        ))
                                        .child(
                                            Icon::new(if self.show_undetected {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            })
                                            .xsmall()
                                            .text_color(colors.muted_foreground),
                                        )
                                        .on_click(cx.listener(|page, _, _, cx| {
                                            page.show_undetected = !page.show_undetected;
                                            cx.notify();
                                        })),
                                )
                                .when(!missing_rows.is_empty(), |group| {
                                    group.child(
                                        v_flex()
                                            .rounded_xl()
                                            .overflow_hidden()
                                            .border_1()
                                            .border_color(colors.border)
                                            .bg(colors.group_box)
                                            .shadow_xs()
                                            .children(missing_rows),
                                    )
                                }),
                        )
                    }),
            )
            .into_any_element()
    }
}
