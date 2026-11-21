//! The status panel that drops from the tray icon: gateway switch, quota
//! dashboard and quick actions.

#[cfg(target_os = "macos")]
use gpui_kit::WeakEntity;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    Animation, AnimationExt as _, AnyWindowHandle, App, AppContext as _, Bounds, Context,
    ElementId, Global, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
    canvas, div, img, point, prelude::FluentBuilder as _, px, size, svg,
};
#[cfg(target_os = "macos")]
use objc2::{MainThreadMarker, rc::Retained};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSScreen, NSWindow};
#[cfg(target_os = "macos")]
use objc2_foundation::NSPoint;
use rust_i18n::t;
use std::time::Duration;

use crate::console::{Console, Page};
use crate::model::{QuotaEntry, ServiceStatus};
use crate::store::{Change, Store};
use crate::theme;
use crate::tr;
use crate::widgets::{provider_icon_path, status_dot};

pub const PANEL_WIDTH: f32 = 360.0;
#[cfg(not(target_os = "macos"))]
pub const PANEL_HEIGHT: f32 = 380.0;
#[cfg(target_os = "macos")]
const PANEL_HEIGHT: f32 = 224.0;
#[cfg(target_os = "macos")]
const MAX_QUOTA_HEIGHT: Pixels = px(265.0);
/// Gap between the menu bar and the panel's top edge.
const PANEL_OFFSET: f32 = 6.0;
#[cfg(target_os = "macos")]
const PANEL_TITLE: &str = "Codex Mixin Provider Usage";

struct PanelWindow {
    handle: AnyWindowHandle,
    #[cfg(target_os = "macos")]
    native: Option<Retained<NSWindow>>,
    #[cfg(target_os = "macos")]
    view: WeakEntity<StatusPanel>,
}

impl Global for PanelWindow {}

/// The provider whose quotas the panel shows; None picks the first one.
struct SelectedProvider(Option<String>);

impl Global for SelectedProvider {}

/// Opens the panel under `anchor`, or closes it when it is already open.
pub fn toggle(anchor: Option<Point<Pixels>>, cx: &mut App) {
    let origin = anchor.map_or_else(
        || point(px(0.0), px(0.0)),
        |anchor| {
            point(
                anchor.x - px(PANEL_WIDTH / 2.0),
                anchor.y + px(PANEL_OFFSET),
            )
        },
    );
    if let Some(panel) = cx.try_global::<PanelWindow>() {
        let handle = panel.handle;
        #[cfg(target_os = "macos")]
        let view = panel.view.clone();
        #[cfg(target_os = "macos")]
        if let Some(native) = panel.native.clone()
            && !native.isVisible()
            && handle
                .update(cx, |_, window, cx| {
                    let _ = view.update(cx, |panel, cx| {
                        panel.action_error = None;
                        cx.notify();
                    });
                    if anchor.is_some()
                        && let Some(main) = MainThreadMarker::new()
                            .and_then(|marker| NSScreen::screens(marker).firstObject())
                    {
                        native.setFrameTopLeftPoint(NSPoint::new(
                            origin.x.into(),
                            main.frame().size.height - f64::from(origin.y),
                        ));
                    }
                    let store = Store::global(cx);
                    store.update(cx, |store, cx| {
                        store.refresh_status(cx);
                        let ids: Vec<_> = store.quotas.keys().cloned().collect();
                        for id in ids {
                            store.refresh_quota(&id, cx);
                        }
                    });
                    request_missing_quotas(&store, cx);
                    window.activate_window();
                    cx.activate(true);
                })
                .is_ok()
        {
            return;
        }
        if handle.update(cx, |_, window, cx| close(window, cx)).is_ok() {
            return;
        }
    }
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
        window_background: if cfg!(target_os = "macos") {
            WindowBackgroundAppearance::Opaque
        } else {
            WindowBackgroundAppearance::Transparent
        },
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        #[cfg(target_os = "macos")]
        window.set_window_title(PANEL_TITLE);
        theme::follow_system_appearance(window, cx);
        cx.new(|cx| StatusPanel::new(window, cx))
    });
    match opened {
        Ok((handle, _view)) => {
            #[cfg(target_os = "macos")]
            let native = MainThreadMarker::new().and_then(|marker| {
                NSApplication::sharedApplication(marker)
                    .windows()
                    .iter()
                    .find(|window| window.title().to_string() == PANEL_TITLE)
                    .inspect(|window| window.setExcludedFromWindowsMenu(true))
            });
            cx.set_global(PanelWindow {
                handle,
                #[cfg(target_os = "macos")]
                native,
                #[cfg(target_os = "macos")]
                view: _view.downgrade(),
            });
            #[cfg(target_os = "macos")]
            let _ = handle.update(cx, |_, window, _| window.refresh());
            cx.activate(true);
        }
        Err(error) => eprintln!("cannot open the status panel: {error:#}"),
    }
}

fn close(window: &mut Window, cx: &mut App) {
    // A deferred close can run after a newer panel replaced this one.
    let current = cx
        .try_global::<PanelWindow>()
        .filter(|panel| panel.handle == window.window_handle());
    if current.is_some() {
        #[cfg(target_os = "macos")]
        if let Some(native) = current.and_then(|panel| panel.native.as_ref()) {
            // Reuse the window so GPUI does not tear down AppKit's observed view.
            native.orderOut(None);
            return;
        }
        cx.remove_global::<PanelWindow>();
    }
    window.remove_window();
}

pub struct StatusPanel {
    action_error: Option<SharedString>,
    authorizing_startup: bool,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
enum AfterGatewayAction {
    Lightweight,
    Quit,
}

impl StatusPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let observe = cx.observe(&store, |_, store, cx| {
            request_missing_quotas(&store, cx);
            cx.notify();
        });
        // Clicking anywhere else dismisses the panel, like a menu.
        let activation = cx.observe_window_activation(window, |panel, window, cx| {
            // Closing from inside the activation change would free the
            // window while AppKit is still delivering it; wait one turn.
            if !window.is_window_active() && !panel.authorizing_startup {
                window.defer(cx, |window, cx| {
                    if !window.is_window_active() {
                        close(window, cx);
                    }
                });
            }
        });
        store.update(cx, |store, cx| {
            store.refresh_status(cx);
            store.quotas.clear();
        });
        request_missing_quotas(&store, cx);
        Self {
            action_error: None,
            authorizing_startup: false,
            _subscriptions: vec![observe, activation],
        }
    }

    fn gateway_action(
        &mut self,
        change: Change,
        after: AfterGatewayAction,
        cx: &mut Context<Self>,
    ) {
        self.action_error = None;
        self.authorizing_startup = change.repair_startup_on_denied;
        let task = Store::global(cx).update(cx, |store, cx| store.run(change, cx));
        cx.spawn(async move |panel, cx| {
            let outcome = task.await.map_err(|error| error.message).and_then(|_| {
                if matches!(after, AfterGatewayAction::Lightweight) {
                    crate::startup::enter_lightweight().map_err(|error| {
                        t!(
                            "panel.lightweight_preference_failed",
                            error = error.to_string()
                        )
                        .into_owned()
                    })?;
                }
                Ok(())
            });
            match outcome {
                Ok(()) => cx.update(|cx| cx.quit()),
                Err(message) => {
                    let _ = panel.update(cx, |panel, cx| {
                        panel.action_error = Some(message.into());
                        panel.authorizing_startup = false;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn confirm_lightweight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.entity().downgrade();
        #[cfg(windows)]
        window.open_dialog(cx, move |dialog, _, _| {
            let session_panel = panel.clone();
            let login_panel = panel.clone();
            dialog
                .title(tr!("panel.lightweight"))
                .w(px(346.0))
                .child(
                    div()
                        .text_sm()
                        .child(tr!("panel.lightweight_windows_detail")),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .justify_end()
                        .child(
                            Button::new("cancel-lightweight")
                                .ghost()
                                .small()
                                .label(tr!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("lightweight-session")
                                .secondary()
                                .small()
                                .label(tr!("panel.lightweight_session"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = session_panel.update(cx, |panel, cx| {
                                        panel.gateway_action(
                                            Change::new(
                                                t!("panel.lightweight_session"),
                                                ["service", "start", "--managed", "--json"],
                                            ),
                                            AfterGatewayAction::Quit,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .child(
                            Button::new("lightweight-login")
                                .primary()
                                .small()
                                .label(tr!("panel.lightweight_login"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = login_panel.update(cx, |panel, cx| {
                                        panel.start_lightweight_at_login(true, cx);
                                    });
                                }),
                        ),
                )
        });
        #[cfg(not(windows))]
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let panel = panel.clone();
            dialog
                .title(tr!("panel.lightweight"))
                .description(tr!("panel.lightweight_detail"))
                .confirm()
                .ok_text(t!("panel.lightweight"))
                .cancel_text(t!("common.cancel"))
                .on_ok(move |_, _, cx| {
                    let _ =
                        panel.update(cx, |panel, cx| panel.start_lightweight_at_login(false, cx));
                    true
                })
        });
    }

    fn start_lightweight_at_login(&mut self, repair: bool, cx: &mut Context<Self>) {
        let mut change = Change::new(
            t!("panel.lightweight"),
            ["service", "autostart", "enable", "--json"],
        );
        change.repair_startup_on_denied = repair;
        self.gateway_action(change, AfterGatewayAction::Lightweight, cx);
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
pub(crate) fn gateway_summary(
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
    let degraded = status.degraded_providers();
    let title: SharedString = if running && degraded > 0 {
        t!("panel.running_with_issues", count = degraded).into()
    } else if running && status.provider_readiness.as_deref() == Some("disabled") {
        t!("panel.running_no_provider").into()
    } else if running {
        t!("panel.running").into()
    } else if status.gateway.as_deref() == Some("unresponsive") {
        t!("panel.unresponsive").into()
    } else {
        t!("panel.stopped").into()
    };
    let detail = error
        .cloned()
        .or_else(|| {
            status
                .error
                .as_ref()
                .filter(|_| status.gateway.as_deref() != Some("stopped"))
                .map(|message| SharedString::from(message.clone()))
        })
        .unwrap_or_else(|| tr!("panel.stopped_detail"));
    (title, detail, running)
}

/// Warn at or below 20 percent remaining.
const QUOTA_WARNING: f32 = 0.2;
/// Outer diameter of the activity rings, their stroke and the gap between them.
const RING_SIZE: f32 = 104.0;
const RING_STROKE: f32 = 9.0;
const RING_GAP: f32 = 2.5;
/// Ring colours, outermost first, after the Apple Fitness rings.
const RING_COLORS: [u32; 3] = [0x0a84ff, 0x34c759, 0x5ac8fa];

fn quota_value(entry: &QuotaEntry) -> SharedString {
    if let Some(error) = &entry.error {
        return error.clone().into();
    }
    if let Some(fraction) = entry.remaining_fraction() {
        return format!("{:.0}%", fraction * 100.0).into();
    }
    entry.remaining.map_or_else(
        || "—".into(),
        |remaining| {
            tr!(
                "panel.quota_remaining",
                remaining = format!("{remaining:.2}"),
                currency = entry.currency.as_deref().unwrap_or_default()
            )
        },
    )
}

/// One legend line beside the rings: the ring colour, the quota name and
/// what is left, with the reset time and spent amount underneath.
fn quota_line(entry: &QuotaEntry, ring: Option<Hsla>, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let low = entry
        .remaining_fraction()
        .is_some_and(|fraction| fraction <= QUOTA_WARNING);
    // Amounts are shown for metered quotas such as a monthly budget; window
    // quotas reported only as percentages already say everything in the ring.
    let usage = entry
        .used
        .zip(entry.limit)
        .filter(|_| entry.unit.as_deref() != Some("percent") && entry.error.is_none())
        .map(|(used, limit)| {
            let amount = |value: f64| match entry.currency.as_deref() {
                Some("CNY") => format!("¥{value:.2}"),
                Some("USD") => format!("${value:.2}"),
                Some(code) => format!("{value:.2} {code}"),
                None => crate::pages::usage::compact_number(value),
            };
            tr!(
                "panel.quota_usage",
                used = amount(used),
                limit = amount(limit)
            )
        });
    let reset = entry
        .reset_at
        .as_deref()
        .and_then(|reset| chrono::DateTime::parse_from_rfc3339(reset).ok())
        .map(|reset| (reset.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_minutes())
        // A reset already in the past is stale until the next refresh.
        .filter(|minutes| *minutes >= 0)
        .map(|minutes| {
            let label: SharedString = if minutes < 60 {
                t!("panel.reset_in_minutes", count = minutes.max(1)).into()
            } else if minutes < 48 * 60 {
                t!("panel.reset_in_hours", count = minutes / 60).into()
            } else {
                t!("panel.reset_in_days", count = minutes / (24 * 60)).into()
            };
            label
        });
    let detail = [reset, usage]
        .into_iter()
        .flatten()
        .map(|part| part.to_string())
        .collect::<Vec<_>>()
        .join(" · ");
    v_flex()
        .min_w_0()
        .gap_0p5()
        .child(
            h_flex()
                .gap_2()
                .text_size(px(13.0))
                .when_some(ring, |line, ring| {
                    line.child(div().flex_none().size_2().rounded_full().bg(ring))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_medium()
                        .truncate()
                        .child(entry.label.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .when(entry.error.is_some(), |value| {
                            value.max_w(px(150.0)).truncate()
                        })
                        .font_semibold()
                        .text_color(if low || entry.error.is_some() {
                            colors.danger
                        } else {
                            colors.foreground
                        })
                        .child(quota_value(entry)),
                ),
        )
        .when(!detail.is_empty(), |line| {
            line.child(
                div()
                    .when(ring.is_some(), |detail| detail.pl_4())
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(colors.muted_foreground)
                    .child(detail),
            )
        })
}

impl StatusPanel {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let store = Store::global(cx).read(cx);
        let status = store.service.value.as_ref();
        let (title, detail, running) = gateway_summary(status, store.service.error.as_ref());
        let configured = status.is_some_and(|status| status.configured);
        let busy = store.busy();
        let unresponsive =
            status.is_some_and(|status| status.gateway.as_deref() == Some("unresponsive"));
        let dot = if !configured || unresponsive {
            colors.warning
        } else if running {
            colors.success
        } else {
            colors.muted_foreground
        };
        let detail = self.action_error.clone().or_else(|| {
            busy.as_ref()
                .map(|(label, progress)| progress.clone().unwrap_or_else(|| label.clone()))
                .or_else(|| (unresponsive || store.service.error.is_some()).then_some(detail))
        });

        v_flex()
            .gap_1()
            .px_1()
            .pb_2()
            .border_b_1()
            .border_color(colors.border.opacity(0.6))
            .child(
                h_flex()
                    .gap_2p5()
                    .child(img("app/app-icon.png").size_8().flex_none())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(15.0))
                                    .font_semibold()
                                    .child("Codex Mixin"),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(status_dot(dot))
                                    .child(div().truncate().child(title)),
                            ),
                    ),
            )
            .when_some(detail, |header, detail| {
                let is_error =
                    self.action_error.is_some() || store.service.error.is_some() || unresponsive;
                let summary = if is_error && crate::cli::startup_task_access_denied(&detail) {
                    tr!("panel.startup_access_denied")
                } else {
                    SharedString::from(
                        detail
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or_default()
                            .trim()
                            .to_owned(),
                    )
                };
                header.child(
                    div()
                        .id("gateway-detail")
                        .truncate()
                        .text_xs()
                        .text_color(if is_error {
                            colors.danger
                        } else {
                            colors.muted_foreground
                        })
                        .child(summary)
                        .when(is_error, |summary| {
                            summary.tooltip(move |window, cx| {
                                Tooltip::new(detail.clone()).build(window, cx)
                            })
                        }),
                )
            })
    }

    /// Provider icons to switch between, then the selected provider's quotas
    /// as concentric activity rings with a legend beside them.
    fn render_quotas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let store = Store::global(cx).read(cx);
        let providers: Vec<_> = store
            .providers
            .value
            .iter()
            .flat_map(|list| &list.providers)
            .filter(|provider| provider.enabled)
            .filter_map(|provider| {
                let snapshot = store.quotas.get(&provider.id)?;
                let empty = snapshot.value.as_deref().unwrap_or_default().is_empty();
                (!empty || snapshot.loading || snapshot.error.is_some())
                    .then_some((provider, snapshot))
            })
            .collect();
        let chosen = cx
            .try_global::<SelectedProvider>()
            .and_then(|state| state.0.as_deref());
        let Some(&(selected, snapshot)) = providers
            .iter()
            .find(|(provider, _)| Some(provider.id.as_str()) == chosen)
            .or_else(|| providers.first())
        else {
            return v_flex().child(
                div()
                    .px_2()
                    .py_3()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(t!("panel.no_quota")),
            );
        };
        let tabs = providers.iter().map(|(provider, snapshot)| {
            let active = provider.id == selected.id;
            let entries = snapshot.value.as_deref().unwrap_or_default();
            let low = snapshot.error.is_some()
                || entries.iter().any(|entry| {
                    entry.error.is_some()
                        || entry
                            .remaining_fraction()
                            .is_some_and(|fraction| fraction <= QUOTA_WARNING)
                });
            let id = provider.id.clone();
            let name = provider.display_name.clone();
            div()
                .id(ElementId::Name(format!("quota-tab-{id}").into()))
                .relative()
                .size(px(30.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .when(active, |tab| {
                    tab.bg(colors.accent)
                        .border_1()
                        .border_color(colors.border.opacity(0.7))
                })
                .when(!active, |tab| {
                    tab.hover(|style| style.bg(colors.accent.opacity(0.6)))
                })
                .child(
                    svg()
                        .path(provider_icon_path(&provider.icon))
                        .size(px(17.0))
                        .text_color(if active {
                            colors.foreground
                        } else {
                            colors.muted_foreground
                        }),
                )
                .when(low, |tab| {
                    tab.child(
                        div()
                            .absolute()
                            .top(px(3.0))
                            .right(px(3.0))
                            .size(px(6.0))
                            .rounded_full()
                            .bg(colors.danger),
                    )
                })
                .tooltip(move |window, cx| Tooltip::new(name.clone()).build(window, cx))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.set_global(SelectedProvider(Some(id.clone())));
                    cx.notify();
                }))
        });
        let tab_row = h_flex()
            .gap_2()
            .px_1()
            .child(
                h_flex()
                    .id("quota-tabs")
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .when(snapshot.loading, |row| {
                row.child(gpui_kit::component::spinner::Spinner::new().xsmall())
            })
            .child(
                div()
                    .max_w(px(150.0))
                    .truncate()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(selected.display_name.clone()),
            );

        let entries = snapshot.value.as_deref().unwrap_or_default();
        let ring_count = entries
            .iter()
            .filter(|entry| entry.remaining_fraction().is_some())
            .count()
            .min(RING_COLORS.len());
        let mut ring_index = 0;
        let mut rings = Vec::new();
        let legend = entries.iter().map(|entry| {
            let ring = entry
                .remaining_fraction()
                .filter(|_| ring_index < ring_count)
                .map(|fraction| {
                    let color: Hsla = gpui_kit::rgb(RING_COLORS[ring_index]).into();
                    rings.push((fraction, color));
                    ring_index += 1;
                    color
                });
            quota_line(entry, ring, cx).into_any_element()
        });
        let legend = v_flex()
            .flex_1()
            .min_w_0()
            .gap_2p5()
            .children(legend.collect::<Vec<_>>())
            .when_some(snapshot.error.clone(), |legend, error| {
                legend.child(div().text_xs().text_color(colors.danger).child(error))
            });
        let lowest = rings.iter().map(|(fraction, _)| *fraction).reduce(f32::min);
        let track_color = colors.muted_foreground.opacity(0.14);
        let ring_view = lowest.map(|lowest| {
            div()
                .relative()
                .flex_none()
                .size(px(RING_SIZE))
                .with_animation(
                    ElementId::Name(format!("quota-rings-{}", selected.id).into()),
                    Animation::new(Duration::from_millis(720))
                        .with_easing(theme::curve(theme::ENTER_CURVE)),
                    move |view, progress| {
                        let rings = rings.clone();
                        view.child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    let center = bounds.center();
                                    let at = |radius: f32, turn: f32| {
                                        let angle = (turn - 0.25) * std::f32::consts::TAU;
                                        point(
                                            center.x + px(radius * angle.cos()),
                                            center.y + px(radius * angle.sin()),
                                        )
                                    };
                                    let dot =
                                        |position: Point<Pixels>, window: &mut Window, color| {
                                            let radius = px(RING_STROKE / 2.0);
                                            let mut cap = PathBuilder::fill();
                                            cap.move_to(point(position.x - radius, position.y));
                                            cap.arc_to(
                                                point(radius, radius),
                                                px(0.0),
                                                false,
                                                true,
                                                point(position.x + radius, position.y),
                                            );
                                            cap.arc_to(
                                                point(radius, radius),
                                                px(0.0),
                                                false,
                                                true,
                                                point(position.x - radius, position.y),
                                            );
                                            if let Ok(cap) = cap.build() {
                                                window.paint_path(cap, color);
                                            }
                                        };
                                    for (index, (fraction, color)) in rings.iter().enumerate() {
                                        let radius = RING_SIZE / 2.0
                                            - RING_STROKE / 2.0
                                            - index as f32 * (RING_STROKE + RING_GAP);
                                        let sweep = fraction * progress;
                                        // Arcs are drawn as fine polylines; 96 steps per turn look round.
                                        let steps = ((sweep * 96.0).ceil() as usize).max(1);
                                        let mut track = PathBuilder::stroke(px(RING_STROKE));
                                        let mut arc = PathBuilder::stroke(px(RING_STROKE));
                                        track.move_to(at(radius, 0.0));
                                        arc.move_to(at(radius, 0.0));
                                        for step in 1..=96 {
                                            track.line_to(at(radius, step as f32 / 96.0));
                                        }
                                        for step in 1..=steps {
                                            arc.line_to(at(
                                                radius,
                                                sweep * step as f32 / steps as f32,
                                            ));
                                        }
                                        if let Ok(track) = track.build() {
                                            window.paint_path(track, track_color);
                                        }
                                        if sweep > 0.0 {
                                            if let Ok(arc) = arc.build() {
                                                window.paint_path(arc, *color);
                                            }
                                            dot(at(radius, 0.0), window, *color);
                                            dot(at(radius, sweep), window, *color);
                                        }
                                    }
                                },
                            )
                            .absolute()
                            .inset_0(),
                        )
                    },
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(19.0))
                        .font_semibold()
                        .text_color(if lowest <= QUOTA_WARNING {
                            colors.danger
                        } else {
                            colors.foreground
                        })
                        .child(format!("{:.0}%", lowest * 100.0)),
                )
        });
        v_flex().gap_3().child(tab_row).child(
            h_flex()
                .px_1()
                .pb_1()
                .gap_4()
                .children(ring_view)
                .child(legend),
        )
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let store = Store::global(cx).read(cx);
        let configured = store
            .service
            .value
            .as_ref()
            .is_some_and(|status| status.configured);
        let busy = store.busy().is_some();
        let actions = [
            (
                Button::new("open-settings").on_click(|_, window, cx| {
                    close(window, cx);
                    Console::open(Some(Page::Settings), cx);
                }),
                IconName::Settings,
                tr!("panel.settings"),
            ),
            (
                Button::new("lightweight")
                    .disabled(!configured || busy)
                    .on_click(cx.listener(|panel, _, window, cx| {
                        panel.confirm_lightweight(window, cx);
                    })),
                IconName::Leaf,
                tr!("panel.lightweight"),
            ),
            (
                Button::new("quit")
                    .disabled(busy)
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.gateway_action(
                            Change::new(
                                t!("panel.quit"),
                                [
                                    "service",
                                    "stop",
                                    "--managed",
                                    "--allow-codex-disconnect",
                                    "--json",
                                ],
                            ),
                            AfterGatewayAction::Quit,
                            cx,
                        );
                    })),
                IconName::Power,
                tr!("panel.quit"),
            ),
        ];
        v_flex()
            .pt_1()
            .border_t_1()
            .border_color(colors.border.opacity(0.6))
            .children(actions.into_iter().map(|(button, icon, label)| {
                button
                    .ghost()
                    .small()
                    .w_full()
                    .h(px(30.0))
                    .text_size(px(13.5))
                    .accessibility_label(label.clone())
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(Icon::new(icon).small().text_color(colors.muted_foreground))
                            .child(label),
                    )
            }))
    }
}

impl Render for StatusPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let quotas = div().id("panel-quotas").overflow_y_scroll();
        #[cfg(target_os = "macos")]
        let quotas = quotas.flex_none().max_h(MAX_QUOTA_HEIGHT);
        #[cfg(not(target_os = "macos"))]
        let quotas = quotas.flex_1().min_h_0();
        let content = v_flex()
            .w_full()
            .when(!cfg!(target_os = "macos"), |panel| panel.h_full())
            .p_3()
            .gap_1()
            // Tabular figures keep percentages and reset times aligned.
            .font_features(gpui_kit::FontFeatures(std::sync::Arc::new(vec![(
                "tnum".into(),
                1,
            )])))
            .bg(colors.popover)
            .text_color(colors.popover_foreground)
            .when(!cfg!(target_os = "macos"), |panel| {
                panel.rounded_lg().border_1().border_color(colors.border)
            })
            .child(self.render_header(cx))
            .child(quotas.py_2().child(self.render_quotas(cx)))
            .child(self.render_actions(cx));
        let panel = div().size_full();
        #[cfg(target_os = "macos")]
        let panel = panel
            .bg(colors.popover)
            .on_children_prepainted(|bounds, window, cx| {
                let [content] = bounds.as_slice() else {
                    return;
                };
                let Some(native) = cx
                    .try_global::<PanelWindow>()
                    .filter(|panel| panel.handle == window.window_handle())
                    .and_then(|panel| panel.native.clone())
                else {
                    return;
                };
                let height = f64::from(content.size.height).ceil();
                if (native.frame().size.height - height).abs() < 0.5 {
                    return;
                }
                // Resize after painting, preserving the top edge under the menu bar.
                cx.foreground_executor()
                    .spawn(async move {
                        let mut frame = native.frame();
                        frame.origin.y += frame.size.height - height;
                        frame.size.height = height;
                        native.setFrame_display(frame, true);
                    })
                    .detach();
            });
        panel.child(content)
    }
}
