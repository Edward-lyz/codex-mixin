//! The status panel that drops from the tray icon: gateway switch, quota
//! dashboard and quick actions.

use gpui_kit::assets::IconName;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Animation, AnimationExt as _, AnyWindowHandle, App, AppContext as _, Bounds, ClipboardItem,
    Context, ElementId, Global, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, point,
    prelude::FluentBuilder as _, px, size,
};
use rust_i18n::t;

use crate::console::{Console, Page};
use crate::model::{QuotaEntry, ServiceStatus};
use crate::pages::run_change;
use crate::store::{Change, Store};
use crate::theme;
use crate::widgets::{logo_tile, provider_logo, status_dot};

pub const PANEL_WIDTH: f32 = 340.0;
pub const PANEL_HEIGHT: f32 = 540.0;
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
        window_background: WindowBackgroundAppearance::Transparent,
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
    if cx.has_global::<PanelWindow>() {
        cx.remove_global::<PanelWindow>();
    }
}

pub struct StatusPanel {
    _subscriptions: Vec<Subscription>,
}

impl StatusPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let observe = cx.observe(&store, |_, store, cx| {
            request_missing_quotas(&store, cx);
            cx.notify();
        });
        // Clicking anywhere else dismisses the panel, like a menu.
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                close(window, cx);
            }
        });
        store.update(cx, |store, cx| {
            store.refresh_status(cx);
            store.quotas.clear();
        });
        request_missing_quotas(&store, cx);
        Self {
            _subscriptions: vec![observe, activation],
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

fn quota_line(entry: &QuotaEntry, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let fraction = entry.used_fraction();
    let color = match fraction {
        Some(value) if value >= 0.9 => colors.danger,
        Some(value) if value >= 0.7 => colors.warning,
        _ => colors.primary,
    };
    let detail: SharedString = if let Some(error) = &entry.error {
        error.clone().into()
    } else if entry.unit.as_deref() == Some("percent") {
        t!(
            "panel.quota_percent",
            used = format!("{:.0}", entry.used.unwrap_or_default())
        )
        .into()
    } else {
        let currency = entry.currency.clone().unwrap_or_default();
        t!(
            "panel.quota_remaining",
            remaining = format!("{:.2}", entry.remaining.unwrap_or_default()),
            currency = currency
        )
        .into()
    };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .text_xs()
                .child(
                    div()
                        .text_color(colors.foreground)
                        .child(entry.label.clone()),
                )
                .child(div().text_color(colors.muted_foreground).child(detail)),
        )
        .when_some(fraction, |line, fraction| {
            line.child(
                Progress::new(ElementId::Name(
                    format!("quota-{}-{}", entry.provider_id, entry.label).into(),
                ))
                .value(fraction * 100.0)
                .color(color)
                .h_1p5(),
            )
        })
}

fn action_row(
    id: &'static str,
    icon: IconName,
    label: SharedString,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let colors = cx.theme().colors;
    h_flex()
        .id(id)
        .gap_2p5()
        .px_2()
        .h_8()
        .rounded_md()
        .text_sm()
        .cursor_pointer()
        .hover(|style| style.bg(colors.accent))
        .child(Icon::new(icon).small().text_color(colors.muted_foreground))
        .child(label)
}

impl StatusPanel {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let store = Store::global(cx).read(cx);
        let status = store.service.value.as_ref();
        let (title, detail, running) = gateway_summary(status, store.service.error.as_ref());
        let configured = status.is_some_and(|status| status.configured);
        let busy = store.busy.clone();
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
            .unwrap_or(detail);
        h_flex()
            .gap_3()
            .p_3()
            .rounded_lg()
            .bg(colors.muted)
            .child(logo_tile("app/app-icon.png".into(), false, true, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(status_dot(dot))
                            .child(div().text_sm().font_semibold().truncate().child(title)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .truncate()
                            .child(detail),
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
            )
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
        let cards = providers.into_iter().filter_map(|provider| {
            let snapshot = store.quotas.get(&provider.id)?;
            let entries = snapshot.value.clone().unwrap_or_default();
            if entries.is_empty() && !snapshot.loading && snapshot.error.is_none() {
                return None;
            }
            Some(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(colors.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(provider_logo(&provider.icon, false, cx))
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_medium()
                                    .truncate()
                                    .child(provider.display_name.clone()),
                            )
                            .when(snapshot.loading, |row| {
                                row.child(gpui_kit::component::spinner::Spinner::new().xsmall())
                            }),
                    )
                    .children(entries.iter().map(|entry| quota_line(entry, cx)))
                    .when_some(
                        snapshot.error.clone().filter(|_| entries.is_empty()),
                        |card, error| {
                            card.child(div().text_xs().text_color(colors.danger).child(error))
                        },
                    ),
            )
        });
        v_flex().gap_2().children(cards)
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let endpoint = Store::global(cx)
            .read(cx)
            .service
            .value
            .as_ref()
            .and_then(|status| status.endpoint.clone());
        let paths = Store::global(cx)
            .read(cx)
            .interface
            .value
            .as_ref()
            .map(|interface| interface.paths.clone());
        let log = paths.as_ref().map(|paths| paths.gateway_log.clone());
        let state = paths.map(|paths| paths.state);
        v_flex()
            .gap_0p5()
            .child(
                action_row(
                    "open-console",
                    IconName::AppWindow,
                    t!("panel.open_console").into(),
                    cx,
                )
                .on_click(|_, window, cx| {
                    close(window, cx);
                    Console::open(Some(Page::Providers), cx);
                }),
            )
            .child(
                action_row(
                    "copy-endpoint",
                    IconName::Link,
                    t!("panel.copy_endpoint").into(),
                    cx,
                )
                .when_some(endpoint, |row, endpoint| {
                    row.on_click(move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(endpoint.clone()));
                        close(window, cx);
                    })
                }),
            )
            .child(
                action_row(
                    "open-log",
                    IconName::FileText,
                    t!("panel.open_log").into(),
                    cx,
                )
                .when_some(log, |row, log| {
                    row.on_click(move |_, window, cx| {
                        cx.reveal_path(std::path::Path::new(&log));
                        close(window, cx);
                    })
                }),
            )
            .child(
                action_row(
                    "open-state",
                    IconName::Folder,
                    t!("panel.open_state").into(),
                    cx,
                )
                .when_some(state, |row, state| {
                    row.on_click(move |_, window, cx| {
                        cx.open_url(&format!("file://{state}"));
                        close(window, cx);
                    })
                }),
            )
            .child(
                action_row(
                    "open-settings",
                    IconName::Settings,
                    t!("panel.settings").into(),
                    cx,
                )
                .on_click(|_, window, cx| {
                    close(window, cx);
                    Console::open(Some(Page::Settings), cx);
                }),
            )
            .child(div().my_1().mx_2().h_px().bg(colors.border))
            .child(
                action_row(
                    "lightweight",
                    IconName::Leaf,
                    t!("panel.lightweight").into(),
                    cx,
                )
                .on_click(|_, _, cx| cx.quit()),
            )
            .child(
                action_row("quit", IconName::Power, t!("panel.quit").into(), cx).on_click(
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
                        let task = Store::global(cx).update(cx, |store, cx| store.run(change, cx));
                        cx.spawn(async move |cx| {
                            let _ = task.await;
                            cx.update(|cx| cx.quit());
                        })
                        .detach();
                    },
                ),
            )
    }
}

impl Render for StatusPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div().size_full().p_1().child(
            v_flex()
                .size_full()
                .gap_3()
                .p_2()
                .rounded_xl()
                .bg(colors.popover)
                .text_color(colors.popover_foreground)
                .border_1()
                .border_color(colors.border)
                .shadow_lg()
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
