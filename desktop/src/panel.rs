//! The status panel that drops from the tray icon: gateway switch, quota
//! dashboard and quick actions.

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Animation, AnimationExt as _, AnyWindowHandle, App, AppContext as _, Bounds, ClipboardItem,
    Context, ElementId, Global, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, img, point,
    prelude::FluentBuilder as _, px, relative, size,
};
use rust_i18n::t;

use crate::console::{Console, Page};
use crate::model::{QuotaEntry, ServiceStatus};
use crate::pages::run_change;
use crate::store::{Change, Store};
use crate::theme;
use crate::tr;
use crate::updater::{UpdateState, Updater};
use crate::widgets::{provider_logo, status_dot};

pub const PANEL_WIDTH: f32 = 340.0;
pub const PANEL_HEIGHT: f32 = 560.0;
/// Gap between the menu bar and the panel's top edge.
const PANEL_OFFSET: f32 = 6.0;

struct PanelWindow(AnyWindowHandle);

impl Global for PanelWindow {}

/// Opens the panel under `anchor`, or closes it when it is already open.
pub fn toggle(anchor: Option<Point<Pixels>>, cx: &mut App) {
    if let Some(handle) = cx.try_global::<PanelWindow>().map(|panel| panel.0)
        && cx.windows().contains(&handle)
    {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        cx.remove_global::<PanelWindow>();
        return;
    }
    let origin = anchor.map_or_else(
        || point(px(0.0), px(0.0)),
        |anchor| {
            point(
                anchor.x - px(PANEL_WIDTH / 2.0),
                anchor.y + px(PANEL_OFFSET),
            )
        },
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            origin,
            size(px(PANEL_WIDTH), px(PANEL_HEIGHT)),
        ))),
        titlebar: None,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        focus: true,
        show: true,
        // On macOS the system rounds and shadows the window frame and the
        // blur fills it; elsewhere the panel draws its own rounded card.
        window_background: if *theme::FROSTED {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Transparent
        },
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        theme::follow_system_appearance(window, cx);
        cx.new(|cx| StatusPanel::new(window, cx))
    });
    match opened {
        Ok((handle, _)) => {
            cx.set_global(PanelWindow(handle));
            cx.activate(true);
        }
        Err(error) => eprintln!("cannot open the status panel: {error:#}"),
    }
}

fn close(window: &mut Window, cx: &mut App) {
    window.remove_window();
    // A deferred close can run after a newer panel replaced this one.
    let current = cx
        .try_global::<PanelWindow>()
        .is_some_and(|panel| panel.0 == window.window_handle());
    if current {
        cx.remove_global::<PanelWindow>();
    }
}

pub struct StatusPanel {
    /// Providers whose quota windows are all shown.
    expanded: HashSet<String>,
    /// Whether the endpoint was copied since the panel opened.
    copied: bool,
    _subscriptions: Vec<Subscription>,
}

impl StatusPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let updater = cx.observe(&Updater::global(cx), |_, _, cx| cx.notify());
        let observe = cx.observe(&store, |_, store, cx| {
            request_missing_quotas(&store, cx);
            cx.notify();
        });
        // Clicking anywhere else dismisses the panel, like a menu.
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            // Closing from inside the activation change would free the
            // window while AppKit is still delivering it; wait one turn.
            if !window.is_window_active() {
                window.defer(cx, close);
            }
        });
        store.update(cx, |store, cx| {
            store.refresh_status(cx);
            store.quotas.clear();
        });
        request_missing_quotas(&store, cx);
        Self {
            expanded: HashSet::new(),
            copied: false,
            _subscriptions: vec![observe, activation, updater],
        }
    }
}

/// Asks for the quota of every enabled provider that has none yet; the
/// panel clears old quotas when it opens, so each opening reads fresh ones.
fn request_missing_quotas(store: &gpui_kit::Entity<Store>, cx: &mut App) {
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
    let store = store.clone();
    cx.defer(move |cx| {
        store.update(cx, |store, cx| {
            for id in &missing {
                store.refresh_quota(id, cx);
            }
        });
    });
}

/// Title and detail lines for the gateway, following the menu wording.
fn gateway_summary(
    status: Option<&ServiceStatus>,
    error: Option<&SharedString>,
) -> (SharedString, SharedString, bool) {
    let Some(status) = status else {
        let detail = error.cloned().unwrap_or_else(|| t!("panel.reading").into());
        return (t!("panel.status_unknown").into(), detail, false);
    };
    if !status.configured {
        return (
            t!("panel.waiting_config").into(),
            t!("panel.waiting_config_detail").into(),
            false,
        );
    }
    let running = status.is_running();
    let degraded = status.degraded_providers().count();
    let title: SharedString = if running && degraded > 0 {
        t!("panel.running_with_issues", count = degraded).into()
    } else if running && status.provider_readiness.as_deref() == Some("disabled") {
        t!("panel.running_no_provider").into()
    } else if running {
        t!("panel.running").into()
    } else {
        t!("panel.stopped").into()
    };
    let detail: SharedString = status
        .error
        .clone()
        .or_else(|| running.then(|| status.endpoint.clone()).flatten())
        .map_or_else(|| t!("panel.stopped_detail").into(), SharedString::from);
    (title, detail, running)
}

/// Quota windows shown per provider before "Show All".
const PREVIEW_QUOTAS: usize = 2;
/// Used fraction from which a quota window is drawn as a warning.
const QUOTA_WARNING: f32 = 0.8;

fn quota_value(entry: &QuotaEntry) -> SharedString {
    if let Some(error) = &entry.error {
        return error.clone().into();
    }
    if entry.unit.as_deref() == Some("percent") {
        return t!(
            "panel.quota_percent",
            used = format!("{:.0}", entry.used.unwrap_or_default())
        )
        .into();
    }
    if entry.currency.is_none()
        && let (Some(used), Some(limit)) = (entry.used, entry.limit)
    {
        return format!("{used:.0} / {limit:.0}").into();
    }
    t!(
        "panel.quota_remaining",
        remaining = format!("{:.2}", entry.remaining.unwrap_or_default()),
        currency = entry.currency.clone().unwrap_or_default()
    )
    .into()
}

/// One quota window: name, a short level bar and the exact usage, aligned
/// on the same three columns for every provider.
fn quota_line(entry: &QuotaEntry, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let fraction = entry.used_fraction();
    let warning = fraction.is_some_and(|value| value >= QUOTA_WARNING);
    let color = if warning {
        colors.warning
    } else {
        colors.primary
    };
    h_flex()
        .h_6()
        .gap_2()
        .text_xs()
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().truncate().child(entry.label.clone()))
                .when(warning, |name| {
                    name.child(
                        Icon::new(IconName::TriangleAlert)
                            .xsmall()
                            .text_color(colors.warning),
                    )
                }),
        )
        .child(
            div()
                .w_16()
                .h_1()
                .flex_none()
                .rounded_full()
                .bg(colors.muted_foreground.opacity(0.18))
                .when_some(fraction, |track, fraction| {
                    track.child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(color)
                            .w(relative(fraction.max(0.04))),
                    )
                }),
        )
        .child(
            div()
                .w_20()
                .flex_none()
                .text_right()
                .text_color(if entry.error.is_some() {
                    colors.danger
                } else if warning {
                    colors.warning
                } else {
                    colors.muted_foreground
                })
                .truncate()
                .child(quota_value(entry)),
        )
}

fn shortcut_tile(
    id: &'static str,
    icon: IconName,
    label: SharedString,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let colors = cx.theme().colors;
    v_flex()
        .id(id)
        .flex_1()
        .items_center()
        .justify_center()
        .gap_1()
        .h(px(54.0))
        .rounded_lg()
        .bg(colors.muted.opacity(0.6))
        .text_xs()
        .cursor_pointer()
        .hover(|style| style.bg(colors.accent))
        .child(Icon::new(icon).small().text_color(colors.muted_foreground))
        .child(label)
}

fn quiet_action(
    id: &'static str,
    label: SharedString,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let colors = cx.theme().colors;
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .text_xs()
        .text_color(colors.muted_foreground)
        .cursor_pointer()
        .hover(|style| style.bg(colors.accent).text_color(colors.foreground))
        .child(label)
}

impl StatusPanel {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let store = Store::global(cx).read(cx);
        let status = store.service.value.as_ref();
        let (title, detail, running) = gateway_summary(status, store.service.error.as_ref());
        let configured = status.is_some_and(|status| status.configured);
        let endpoint = status
            .filter(|status| status.is_running())
            .and_then(|status| status.endpoint.clone());
        let busy = store.busy();
        let dot = if !configured {
            colors.warning
        } else if running {
            colors.success
        } else {
            colors.muted_foreground
        };
        let detail = busy
            .as_ref()
            .map(|(label, progress)| progress.clone().unwrap_or_else(|| label.clone()))
            .or_else(|| endpoint.is_none().then_some(detail));
        let copied = self.copied;
        v_flex()
            .gap_2()
            .px_1()
            .child(
                h_flex()
                    .gap_3()
                    .child(img("app/app-icon.png").size_9().flex_none())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().font_semibold().child("Codex Mixin"))
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(status_dot(dot))
                                    .child(div().truncate().child(title)),
                            ),
                    )
                    .child(
                        Switch::new("gateway-switch")
                            .checked(running)
                            .disabled(!configured || busy.is_some())
                            .on_change(|start, _, cx| {
                                let (command, label) = if *start {
                                    ("start", t!("panel.start_label"))
                                } else {
                                    ("stop", t!("panel.stop_label"))
                                };
                                run_change(
                                    Change::new(label, ["service", command, "--managed", "--json"]),
                                    cx,
                                );
                            }),
                    ),
            )
            .when_some(detail, |header, detail| {
                header.child(
                    div()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .line_clamp(2)
                        .child(detail),
                )
            })
            .when_some(endpoint, |header, endpoint| {
                header.child(
                    h_flex()
                        .h_8()
                        .pl_2p5()
                        .pr_1()
                        .gap_2()
                        .rounded_lg()
                        .bg(colors.muted.opacity(0.6))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .font_family("IBM Plex Mono")
                                .child(endpoint.clone()),
                        )
                        .child(
                            Button::new("copy-endpoint")
                                .ghost()
                                .xsmall()
                                .label(if copied {
                                    t!("panel.copied")
                                } else {
                                    t!("panel.copy")
                                })
                                .on_click(cx.listener(move |panel, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        endpoint.clone(),
                                    ));
                                    panel.copied = true;
                                    cx.notify();
                                })),
                        ),
                )
            })
    }

    fn render_quotas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let store = Store::global(cx).read(cx);
        let providers: Vec<_> = store
            .providers
            .value
            .as_ref()
            .map(|list| {
                list.providers
                    .iter()
                    .filter(|provider| provider.enabled)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let mut groups = Vec::new();
        for provider in providers {
            let Some(snapshot) = store.quotas.get(&provider.id) else {
                continue;
            };
            let entries = snapshot.value.clone().unwrap_or_default();
            if entries.is_empty() && !snapshot.loading && snapshot.error.is_none() {
                continue;
            }
            // A single balance without a limit reads best beside the name.
            let inline = (entries.len() == 1 && entries[0].used_fraction().is_none())
                .then(|| quota_value(&entries[0]));
            let expanded = self.expanded.contains(&provider.id);
            let shown: Vec<&QuotaEntry> = if inline.is_some() {
                Vec::new()
            } else if expanded || entries.len() <= PREVIEW_QUOTAS {
                entries.iter().collect()
            } else {
                // Collapsed, the windows closest to their limit stay visible,
                // in their original order.
                let mut fullest: Vec<usize> = (0..entries.len()).collect();
                fullest.sort_by(|a, b| {
                    let used = |index: &usize| entries[*index].used_fraction().unwrap_or(0.0);
                    used(b).total_cmp(&used(a))
                });
                fullest.truncate(PREVIEW_QUOTAS);
                fullest.sort_unstable();
                fullest.into_iter().map(|index| &entries[index]).collect()
            };
            let toggle = (entries.len() > PREVIEW_QUOTAS).then(|| {
                let id = provider.id.clone();
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(t!("panel.quota_windows", count = entries.len())),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("quota-toggle-{id}").into()))
                            .ghost()
                            .xsmall()
                            .label(if expanded {
                                t!("panel.collapse")
                            } else {
                                t!("panel.show_all")
                            })
                            .icon(if expanded {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                if !panel.expanded.remove(&id) {
                                    panel.expanded.insert(id.clone());
                                }
                                cx.notify();
                            })),
                    )
            });
            let lines = v_flex()
                .children(shown.into_iter().map(|entry| quota_line(entry, cx)))
                .with_animation(
                    ElementId::Name(format!("quota-lines-{}-{expanded}", provider.id).into()),
                    Animation::new(theme::ENTER_DURATION)
                        .with_easing(theme::curve(theme::ENTER_CURVE)),
                    |lines, progress| lines.opacity(progress).mt(px(-4.0 * (1.0 - progress))),
                );
            groups.push(
                v_flex()
                    .gap_1()
                    .py_2()
                    .child(
                        h_flex()
                            .h_7()
                            .gap_2()
                            .child(provider_logo(&provider.icon, false, cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_medium()
                                    .truncate()
                                    .child(provider.display_name.clone()),
                            )
                            .when(snapshot.loading, |row| {
                                row.child(gpui_kit::component::spinner::Spinner::new().xsmall())
                            })
                            .when_some(inline, |row, value| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.muted_foreground)
                                        .child(value),
                                )
                            })
                            .children(toggle),
                    )
                    .child(lines)
                    .when_some(
                        snapshot.error.clone().filter(|_| entries.is_empty()),
                        |group, error| {
                            group.child(div().text_xs().text_color(colors.danger).child(error))
                        },
                    ),
            );
        }
        let count = groups.len();
        v_flex()
            .px_1()
            .children(groups.into_iter().enumerate().map(|(index, group)| {
                group.when(index + 1 < count, |group| {
                    group.border_b_1().border_color(colors.border.opacity(0.6))
                })
            }))
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let paths = Store::global(cx)
            .read(cx)
            .interface
            .value
            .as_ref()
            .map(|interface| interface.paths.clone());
        let log = paths.as_ref().map(|paths| paths.gateway_log.clone());
        let state = paths.map(|paths| paths.state);
        let update = match Updater::global(cx).read(cx).state.clone() {
            UpdateState::Available { version } => {
                Some(tr!("panel.update_available", version = version))
            }
            UpdateState::Ready { version } => Some(tr!("panel.update_ready", version = version)),
            _ => None,
        };
        v_flex()
            .gap_2()
            .when_some(update, |actions, label| {
                actions.child(
                    Button::new("update")
                        .ghost()
                        .small()
                        .w_full()
                        .icon(IconName::CircleArrowUp)
                        .label(label)
                        .on_click(|_, window, cx| {
                            let updater = Updater::global(cx);
                            let ready = matches!(updater.read(cx).state, UpdateState::Ready { .. });
                            if ready {
                                updater.update(cx, |updater, cx| updater.restart(cx));
                                return;
                            }
                            updater.update(cx, |updater, cx| updater.download(cx));
                            close(window, cx);
                            Console::open(Some(Page::Settings), cx);
                        }),
                )
            })
            .child(
                Button::new("open-console")
                    .primary()
                    .w_full()
                    .icon(IconName::AppWindow)
                    .label(t!("panel.open_console"))
                    .on_click(|_, window, cx| {
                        close(window, cx);
                        Console::open(Some(Page::Providers), cx);
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        shortcut_tile(
                            "open-log",
                            IconName::FileText,
                            t!("panel.open_log").into(),
                            cx,
                        )
                        .when_some(log, |tile, log| {
                            tile.on_click(move |_, window, cx| {
                                cx.reveal_path(std::path::Path::new(&log));
                                close(window, cx);
                            })
                        }),
                    )
                    .child(
                        shortcut_tile(
                            "open-state",
                            IconName::Folder,
                            t!("panel.open_state").into(),
                            cx,
                        )
                        .when_some(state, |tile, state| {
                            tile.on_click(move |_, window, cx| {
                                cx.reveal_path(std::path::Path::new(&state));
                                close(window, cx);
                            })
                        }),
                    )
                    .child(
                        shortcut_tile(
                            "open-settings",
                            IconName::Settings,
                            t!("panel.settings").into(),
                            cx,
                        )
                        .on_click(|_, window, cx| {
                            close(window, cx);
                            Console::open(Some(Page::Settings), cx);
                        }),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .pt_1()
                    .border_t_1()
                    .border_color(colors.border.opacity(0.6))
                    .child(
                        quiet_action("lightweight", t!("panel.lightweight").into(), cx)
                            .on_click(|_, _, cx| cx.quit()),
                    )
                    .child(quiet_action("quit", t!("panel.quit").into(), cx).on_click(
                        |_, _, cx| {
                            let change = Change::new(
                                t!("panel.quit"),
                                [
                                    "service",
                                    "stop",
                                    "--managed",
                                    "--allow-codex-disconnect",
                                    "--json",
                                ],
                            );
                            let task =
                                Store::global(cx).update(cx, |store, cx| store.run(change, cx));
                            cx.spawn(async move |cx| {
                                let _ = task.await;
                                cx.update(|cx| cx.quit());
                            })
                            .detach();
                        },
                    )),
            )
    }
}

impl Render for StatusPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .size_full()
            .when_else(
                *theme::FROSTED,
                |surface| surface.bg(colors.sidebar.opacity(theme::FROSTED_TINT)),
                |surface| surface.p_1(),
            )
            .child(
                v_flex()
                    .size_full()
                    .gap_3()
                    .p_3()
                    .text_color(colors.popover_foreground)
                    .when(!*theme::FROSTED, |card| {
                        card.rounded_xl()
                            .bg(colors.popover)
                            .border_1()
                            .border_color(colors.border)
                            .shadow_lg()
                    })
                    .child(self.render_header(cx))
                    .child(
                        div()
                            .id("panel-quotas")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(self.render_quotas(cx)),
                    )
                    .child(self.render_actions(cx))
                    .with_animation(
                        "panel-enter",
                        Animation::new(theme::ENTER_DURATION)
                            .with_easing(theme::curve(theme::ENTER_CURVE)),
                        |panel, progress| panel.opacity(progress).mt(px(-8.0 * (1.0 - progress))),
                    ),
            )
    }
}
