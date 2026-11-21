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
    AnyWindowHandle, App, AppContext as _, Bounds, Context, ElementId, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, div, img, point, prelude::FluentBuilder as _, px,
    relative, size, svg,
};
#[cfg(target_os = "macos")]
use objc2::{MainThreadMarker, rc::Retained};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSScreen, NSWindow};
#[cfg(target_os = "macos")]
use objc2_foundation::NSPoint;
use rust_i18n::t;

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

/// The provider the user expanded, or None when the list is collapsed.
struct ExpandedProvider(Option<String>);

impl Global for ExpandedProvider {}

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
const QUOTA_PERCENT_WIDTH: Pixels = px(36.0);
const QUOTA_RESET_WIDTH: Pixels = px(86.0);

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

fn quota_line(entry: &QuotaEntry, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let remaining = entry.remaining_fraction();
    let low = remaining.is_some_and(|fraction| fraction <= QUOTA_WARNING);
    let reset = entry
        .reset_at
        .as_deref()
        .and_then(|reset| chrono::DateTime::parse_from_rfc3339(reset).ok())
        .map(|reset| {
            let reset = reset.with_timezone(&chrono::Local);
            let format = if reset.date_naive() == chrono::Local::now().date_naive() {
                "%H:%M"
            } else {
                "%m-%d %H:%M"
            };
            reset.format(format).to_string()
        });
    h_flex()
        .min_h(px(22.0))
        .gap_2()
        .text_size(px(12.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .when(entry.error.is_none(), |label| label.truncate())
                .child(entry.label.clone()),
        )
        .when_some(remaining, |row, fraction| {
            row.child(
                div()
                    .flex_none()
                    .w(px(50.0))
                    .h(px(3.0))
                    .rounded_full()
                    .bg(colors.muted_foreground.opacity(0.18))
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction))
                            .rounded_full()
                            .bg(if low { colors.danger } else { colors.primary }),
                    ),
            )
        })
        .child(
            div()
                .when(remaining.is_some(), |value| {
                    value.w(QUOTA_PERCENT_WIDTH).flex_none()
                })
                .when(entry.error.is_some(), |value| value.max_w(px(180.0)).py_1())
                .text_right()
                .text_color(if low || entry.error.is_some() {
                    colors.danger
                } else {
                    colors.foreground
                })
                .child(quota_value(entry)),
        )
        .child(
            div()
                .w(QUOTA_RESET_WIDTH)
                .flex_none()
                .text_right()
                .text_xs()
                .text_color(colors.muted_foreground)
                .child(reset.unwrap_or_else(|| "—".to_owned())),
        )
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
                    .h_8()
                    .gap_2()
                    .child(img("app/app-icon.png").size_6().flex_none())
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_semibold()
                            .child("Codex Mixin"),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(status_dot(dot))
                            .child(title),
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

    fn render_quotas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let store = Store::global(cx).read(cx);
        let mut groups = Vec::new();
        let selection = cx
            .try_global::<ExpandedProvider>()
            .and_then(|state| state.0.as_deref());
        for provider in store
            .providers
            .value
            .iter()
            .flat_map(|list| &list.providers)
            .filter(|provider| provider.enabled)
        {
            let Some(snapshot) = store.quotas.get(&provider.id) else {
                continue;
            };
            let entries = snapshot.value.as_deref().unwrap_or_default();
            if entries.is_empty() && !snapshot.loading && snapshot.error.is_none() {
                continue;
            }
            let balance = entries.first().filter(|entry| {
                entries.len() == 1
                    && entry.remaining.is_some()
                    && entry.remaining_fraction().is_none()
                    && entry.reset_at.is_none()
                    && entry.error.is_none()
            });
            let expandable = balance.is_none() && (!entries.is_empty() || snapshot.error.is_some());
            let expanded = expandable && selection == Some(provider.id.as_str());
            let lowest = entries
                .iter()
                .filter_map(QuotaEntry::remaining_fraction)
                .reduce(f32::min);
            let summary = if expanded {
                None
            } else if let Some(entry) = balance {
                Some(quota_value(entry))
            } else if let Some(fraction) = lowest {
                Some(tr!(
                    "panel.quota_lowest",
                    remaining = format!("{:.0}", fraction * 100.0)
                ))
            } else if snapshot.error.is_some() || entries.iter().any(|entry| entry.error.is_some())
            {
                Some(tr!("panel.quota_unavailable"))
            } else {
                None
            };
            let id = provider.id.clone();
            let details = expanded.then(|| {
                v_flex()
                    .ml_3()
                    .pl_2()
                    .pr_2()
                    .mb_1()
                    .border_l_1()
                    .border_color(colors.border)
                    .child(
                        h_flex()
                            .h_5()
                            .gap_2()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(div().flex_1())
                            .child(
                                div()
                                    .w(QUOTA_PERCENT_WIDTH)
                                    .text_right()
                                    .child(t!("panel.quota_left_heading")),
                            )
                            .child(
                                div()
                                    .w(QUOTA_RESET_WIDTH)
                                    .text_right()
                                    .child(t!("panel.quota_reset_heading")),
                            ),
                    )
                    .children(entries.iter().map(|entry| quota_line(entry, cx)))
                    .when_some(snapshot.error.clone(), |details, error| {
                        details.child(
                            div()
                                .py_1()
                                .text_xs()
                                .text_color(colors.danger)
                                .child(error),
                        )
                    })
            });
            groups.push(
                v_flex()
                    .flex_none()
                    .child(
                        h_flex()
                            .id(ElementId::Name(format!("quota-provider-{id}").into()))
                            .h(px(29.0))
                            .px_2()
                            .gap_2()
                            .rounded_md()
                            .when(expanded, |row| row.bg(colors.accent.opacity(0.7)))
                            .when(expandable, |row| {
                                row.cursor_pointer()
                                    .hover(|style| style.bg(colors.accent))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.set_global(ExpandedProvider(
                                            (!expanded).then(|| id.clone()),
                                        ));
                                        cx.notify();
                                    }))
                            })
                            .child(
                                svg()
                                    .path(provider_icon_path(&provider.icon))
                                    .size_3p5()
                                    .flex_none()
                                    .text_color(colors.muted_foreground),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(13.0))
                                    .font_medium()
                                    .truncate()
                                    .child(provider.display_name.clone()),
                            )
                            .when(snapshot.loading, |row| {
                                row.child(gpui_kit::component::spinner::Spinner::new().xsmall())
                            })
                            .when_some(summary, |row, summary| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(
                                            if lowest.is_some_and(|left| left <= QUOTA_WARNING) {
                                                colors.danger
                                            } else {
                                                colors.muted_foreground
                                            },
                                        )
                                        .child(summary),
                                )
                            })
                            .child(div().size_4().flex_none().when(expandable, |slot| {
                                slot.child(
                                    Icon::new(if expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .xsmall()
                                    .text_color(colors.muted_foreground),
                                )
                            })),
                    )
                    .children(details),
            );
        }
        v_flex()
            .when(groups.is_empty(), |region| {
                region.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child(t!("panel.no_quota")),
                )
            })
            .children(groups)
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
            .bg(colors.popover)
            .text_color(colors.popover_foreground)
            .when(!cfg!(target_os = "macos"), |panel| {
                panel.rounded_lg().border_1().border_color(colors.border)
            })
            .child(self.render_header(cx))
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .text_xs()
                    .font_medium()
                    .text_color(colors.muted_foreground)
                    .child(t!("panel.quota_heading")),
            )
            .child(quotas.child(self.render_quotas(cx)))
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
