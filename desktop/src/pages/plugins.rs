//! Manages the Pi packages and provider accounts exposed by the CLI.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _,
};
use rust_i18n::t;

use super::{is_busy, placeholder};
use crate::model::{MarketPackage, PluginCommand, PluginCommandOutput, PluginData, PluginStatus};
use crate::store::{Change, Snapshot, Store, plugin_data, query};
use crate::tr;
use crate::widgets::{Segmented, page_header, pill, row_divider, section, setting_row};

pub struct PluginsPage {
    status: Snapshot<PluginData>,
    market: Snapshot<Vec<MarketPackage>>,
    search: Entity<InputState>,
    package: Entity<InputState>,
    market_query: String,
    show_installed: bool,
    command_output: Option<(String, Result<PluginCommandOutput, SharedString>)>,
    _subscriptions: Vec<Subscription>,
}

impl PluginsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("plugins.search_hint")));
        let package =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("plugins.package_hint")));
        let search_subscription = cx.subscribe(&search, |page, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                page.search(cx);
            }
        });
        let package_subscription = cx.subscribe(&package, |page, _, event, cx| match event {
            InputEvent::PressEnter { .. } => page.install_manual(cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let store_subscription = cx.observe(&Store::global(cx), |_, _, cx| cx.notify());
        let mut page = Self {
            status: Snapshot::default(),
            market: Snapshot::default(),
            search,
            package,
            market_query: String::new(),
            show_installed: false,
            command_output: None,
            _subscriptions: vec![
                search_subscription,
                package_subscription,
                store_subscription,
            ],
        };
        page.reload_status(cx);
        page.load_market(cx);
        page
    }

    pub fn reload_status(&mut self, cx: &mut Context<Self>) {
        if !self.status.begin() {
            return;
        }
        let task = plugin_data(cx);
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |page, cx| {
                if page.status.finish(result) {
                    page.reload_status(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn load_market(&mut self, cx: &mut Context<Self>) {
        if !self.market.begin() {
            return;
        }
        let task = query::<Vec<MarketPackage>>(
            vec![
                "plugin".to_owned(),
                "search".to_owned(),
                format!("--query={}", self.market_query),
                "--json".to_owned(),
            ],
            cx,
        );
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |page, cx| {
                if page.market.finish(result) {
                    page.load_market(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        self.market_query = self.search.read(cx).value().trim().to_owned();
        self.load_market(cx);
    }

    fn apply(&mut self, change: Change, command_name: Option<String>, cx: &mut Context<Self>) {
        if is_busy(cx) {
            return;
        }
        let task = Store::global(cx).update(cx, |store, cx| store.run(change, cx));
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |page, cx| {
                if let Some(name) = command_name {
                    let output = result
                        .map_err(|error| error.message.into())
                        .and_then(|output| {
                            serde_json::from_str(&output).map_err(|error| {
                                tr!(
                                    "cli.invalid_json",
                                    command = "plugin run-command",
                                    error = error.to_string()
                                )
                            })
                        });
                    page.command_output = Some((name, output));
                } else {
                    page.load_market(cx);
                }
                page.reload_status(cx);
            });
        })
        .detach();
    }

    fn install_disabled(&self, cx: &App) -> bool {
        is_busy(cx)
            || !self
                .status
                .value
                .as_ref()
                .is_some_and(|data| data.status.node_available)
    }

    fn install(&mut self, spec: &str, cx: &mut Context<Self>) {
        if !spec.is_empty() && !self.install_disabled(cx) {
            self.apply(
                Change::new(
                    t!("plugins.install"),
                    ["plugin", "install", &format!("--spec={spec}"), "--json"],
                ),
                None,
                cx,
            );
        }
    }

    fn install_manual(&mut self, cx: &mut Context<Self>) {
        let spec = self.package.read(cx).value().trim().to_owned();
        self.install(&spec, cx);
    }

    fn open_command(&self, command: &PluginCommand, window: &mut Window, cx: &mut Context<Self>) {
        let args = cx.new(|cx| InputState::new(window, cx).placeholder(t!("plugins.arguments")));
        let view = cx.weak_entity();
        let name = command.name.clone();
        let description = command.description.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let run_view = view.clone();
            let run_args = args.clone();
            let run_name = name.clone();
            dialog
                .title(tr!("plugins.run_command_title", name = name))
                .w(gpui_kit::px(480.0))
                .child(
                    v_flex()
                        .gap_3()
                        .when(!description.is_empty(), |body| {
                            body.child(div().text_sm().child(description.clone()))
                        })
                        .child(Input::new(&args)),
                )
                .footer(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("plugin-command-cancel")
                                .ghost()
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("plugin-command-run")
                                .primary()
                                .label(t!("plugins.run"))
                                .disabled(is_busy(cx))
                                .on_click(move |_, window, cx| {
                                    let args = run_args.read(cx).value();
                                    let _ = run_view.update(cx, |page, cx| {
                                        page.apply(
                                            Change::new(
                                                t!("plugins.run"),
                                                [
                                                    "plugin",
                                                    "run-command",
                                                    &format!("--name={run_name}"),
                                                    &format!("--args={args}"),
                                                    "--json",
                                                ],
                                            ),
                                            Some(run_name.clone()),
                                            cx,
                                        );
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn runtime(&self, status: &PluginStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let can_update =
            status.node_available && !status.installed_packages.is_empty() && !is_busy(cx);
        let pi_status = status.pi_version.as_deref().map_or_else(
            || tr!("plugins.runtime_pending"),
            |version| tr!("plugins.pi_version", version = version),
        );
        let node_status = status.node_version.as_deref().map_or_else(
            || tr!("plugins.node_missing"),
            |version| tr!("plugins.node_version", version = version),
        );
        let models = if status.bridge_error.is_some() {
            tr!("plugins.models_unavailable")
        } else {
            tr!(
                "plugins.model_summary",
                providers = status.discovered_providers.len(),
                models = status.discovered_models.len()
            )
        };
        let body = v_flex()
            .child(setting_row(
                t!("plugins.runtime"),
                None,
                h_flex()
                    .gap_2()
                    .child(pill(
                        pi_status,
                        if status.runtime_initialized {
                            colors.success
                        } else {
                            colors.muted_foreground
                        },
                    ))
                    .child(pill(
                        node_status,
                        if status.node_available {
                            colors.muted_foreground
                        } else {
                            colors.warning
                        },
                    )),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("plugins.npm_registry"),
                None,
                pill(
                    status
                        .npm_registry
                        .trim_start_matches("https://")
                        .to_owned(),
                    colors.muted_foreground,
                ),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                models,
                None,
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("plugin-update-all")
                            .ghost()
                            .small()
                            .label(t!("plugins.update_all"))
                            .disabled(!can_update)
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.apply(
                                    Change::new(
                                        t!("plugins.update_all"),
                                        ["plugin", "update", "--json"],
                                    ),
                                    None,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("plugin-sync")
                            .secondary()
                            .small()
                            .label(t!("plugins.sync"))
                            .disabled(!can_update)
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.apply(
                                    Change::new(t!("plugins.sync"), ["plugin", "sync", "--json"]),
                                    None,
                                    cx,
                                );
                            })),
                    ),
                cx,
            ));
        v_flex()
            .gap_2()
            .child(section(t!("plugins.runtime_section"), None, body, cx))
            .when_some(status.bridge_error.clone(), |body, error| {
                body.child(div().text_sm().text_color(colors.danger).child(error))
            })
            .when(status.extension_errors != 0, |body| {
                body.child(div().text_sm().text_color(colors.warning).child(t!(
                    "plugins.extension_errors",
                    count = status.extension_errors
                )))
            })
    }

    fn installed(&self, status: &PluginStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = is_busy(cx) || !status.node_available;
        let colors = cx.theme().colors;
        let rows = status
            .installed_packages
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let update = format!("--spec={spec}");
                let remove = update.clone();
                v_flex()
                    .when(index != 0, |row| row.child(row_divider(cx)))
                    .child(setting_row(
                        spec.clone(),
                        None,
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new(("plugin-update", index))
                                    .ghost()
                                    .small()
                                    .label(t!("plugins.update"))
                                    .disabled(busy)
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.apply(
                                            Change::new(
                                                t!("plugins.update"),
                                                ["plugin", "update", &update, "--json"],
                                            ),
                                            None,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(("plugin-remove", index))
                                    .ghost()
                                    .small()
                                    .label(t!("plugins.remove"))
                                    .disabled(busy)
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.apply(
                                            Change::new(
                                                t!("plugins.remove"),
                                                ["plugin", "remove", &remove, "--json"],
                                            ),
                                            None,
                                            cx,
                                        );
                                    })),
                            ),
                        cx,
                    ))
            });
        let body = v_flex()
            .children(rows)
            .when(status.installed_packages.is_empty(), |body| {
                body.child(
                    div()
                        .px_4()
                        .py_3()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child(t!("plugins.none_installed")),
                )
            });
        section(t!("plugins.installed_section"), None, body, cx)
    }

    fn providers(&self, status: &PluginStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = is_busy(cx);
        let cli = Store::global(cx).read(cx).cli();
        let copy_notice = if cfg!(windows) {
            tr!("plugins.login_copied_powershell")
        } else {
            tr!("plugins.login_copied")
        };
        let rows = status
            .discovered_providers
            .iter()
            .enumerate()
            .map(|(index, provider)| {
                let argument = format!("--provider={provider}");
                let login = cli.plugin_login_command(provider);
                let notice = copy_notice.clone();
                v_flex()
                    .when(index != 0, |row| row.child(row_divider(cx)))
                    .child(setting_row(
                        provider.clone(),
                        None,
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new(("plugin-login", index))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Copy)
                                    .label(t!("plugins.copy_login"))
                                    .on_click(move |_, window, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            login.clone(),
                                        ));
                                        window.push_notification(
                                            Notification::success(notice.clone()),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                Button::new(("plugin-logout", index))
                                    .ghost()
                                    .small()
                                    .label(t!("plugins.logout"))
                                    .disabled(busy)
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.apply(
                                            Change::new(
                                                t!("plugins.logout"),
                                                ["plugin", "auth", "logout", &argument, "--json"],
                                            ),
                                            None,
                                            cx,
                                        );
                                    })),
                            ),
                        cx,
                    ))
            });
        section(
            t!("plugins.provider_section"),
            None,
            v_flex().children(rows),
            cx,
        )
    }

    fn commands(&self, data: &PluginData, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let busy = is_busy(cx);
        let rows = data.commands.iter().enumerate().map(|(index, command)| {
            let name = command.name.clone();
            v_flex()
                .when(index != 0, |row| row.child(row_divider(cx)))
                .child(setting_row(
                    name.clone(),
                    Some(command.description.clone().into()),
                    Button::new(("plugin-command", index))
                        .ghost()
                        .small()
                        .label(t!("plugins.run"))
                        .disabled(busy)
                        .on_click(cx.listener(move |page, _, window, cx| {
                            if let Some(command) = page
                                .status
                                .value
                                .iter()
                                .flat_map(|data| &data.commands)
                                .find(|command| command.name == name)
                            {
                                page.open_command(command, window, cx);
                            }
                        })),
                    cx,
                ))
        });
        let body = v_flex()
            .children(rows)
            .when_some(data.commands_error.as_deref(), |body, error| {
                body.child(
                    div()
                        .px_4()
                        .py_3()
                        .text_sm()
                        .text_color(colors.danger)
                        .child(error.to_owned()),
                )
            })
            .when_some(self.command_output.as_ref(), |body, (name, output)| {
                let result = v_flex().gap_1().px_4().py_3().child(
                    div()
                        .font_semibold()
                        .text_xs()
                        .child(t!("plugins.command_output", name = name)),
                );
                let result = match output {
                    Err(error) => result.child(
                        div()
                            .text_sm()
                            .text_color(colors.danger)
                            .child(error.clone()),
                    ),
                    Ok(output) if output.notifications.is_empty() => result.child(
                        div()
                            .text_sm()
                            .text_color(colors.muted_foreground)
                            .child(t!("plugins.command_no_output")),
                    ),
                    Ok(output) => {
                        result.children(output.notifications.iter().map(|notification| {
                            let color = match notification.kind.as_str() {
                                "error" => colors.danger,
                                "warning" => colors.warning,
                                _ => colors.foreground,
                            };
                            div()
                                .text_sm()
                                .text_color(color)
                                .child(notification.message.clone())
                        }))
                    }
                };
                body.when(!data.commands.is_empty(), |body| {
                    body.child(row_divider(cx))
                })
                .child(result)
            });
        section(t!("plugins.command_section"), None, body, cx)
    }

    fn marketplace(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let install_disabled = self.install_disabled(cx);
        let toolbar = v_flex()
            .gap_2()
            .px_4()
            .py_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div().min_w_0().flex_1().child(
                            Input::new(&self.search)
                                .small()
                                .cleanable(true)
                                .prefix(Icon::new(IconName::Search).small()),
                        ),
                    )
                    .child(
                        Button::new("plugin-search")
                            .secondary()
                            .small()
                            .label(t!("plugins.search"))
                            .disabled(self.market.loading)
                            .loading(self.market.loading)
                            .on_click(cx.listener(|page, _, _, cx| page.search(cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(Input::new(&self.package).small()),
                    )
                    .child(
                        Button::new("plugin-install")
                            .secondary()
                            .small()
                            .label(t!("plugins.install"))
                            .disabled(
                                install_disabled || self.package.read(cx).value().trim().is_empty(),
                            )
                            .on_click(cx.listener(|page, _, _, cx| page.install_manual(cx))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(t!("plugins.package_permissions")),
            );
        let packages = self.market.value.as_deref().unwrap_or_default();
        let rows = packages.iter().enumerate().map(|(index, package)| {
            let name = package.name.clone();
            v_flex().child(row_divider(cx)).child(
                h_flex()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .font_medium()
                                            .text_sm()
                                            .child(name.clone()),
                                    )
                                    .when_some(package.version.as_ref(), |row, version| {
                                        row.child(pill(version.clone(), colors.muted_foreground))
                                    }),
                            )
                            .when(!package.description.is_empty(), |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .line_clamp(2)
                                        .text_color(colors.muted_foreground)
                                        .child(package.description.clone()),
                                )
                            }),
                    )
                    .child(
                        Button::new(("plugin-market-install", index))
                            .ghost()
                            .small()
                            .label(if package.installed {
                                t!("plugins.already_installed")
                            } else {
                                t!("plugins.install")
                            })
                            .disabled(install_disabled || self.market.loading || package.installed)
                            .on_click(cx.listener(move |page, _, _, cx| page.install(&name, cx))),
                    ),
            )
        });
        let body = v_flex()
            .child(toolbar)
            .when_some(self.market.error.clone(), |body, error| {
                body.child(
                    div()
                        .px_4()
                        .pb_3()
                        .text_sm()
                        .text_color(colors.danger)
                        .child(error),
                )
            })
            .when(
                !self.market.loading && self.market.error.is_none() && packages.is_empty(),
                |body| {
                    body.child(
                        div()
                            .px_4()
                            .pb_3()
                            .text_sm()
                            .text_color(colors.muted_foreground)
                            .child(t!("plugins.no_results")),
                    )
                },
            )
            .children(rows);
        section(t!("plugins.market_section"), None, body, cx)
    }
}

impl Render for PluginsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let refresh = Button::new("plugin-refresh")
            .ghost()
            .small()
            .icon(IconName::RefreshCw)
            .label(t!("plugins.refresh"))
            .disabled(self.status.loading)
            .loading(self.status.loading)
            .on_click(cx.listener(|page, _, _, cx| {
                page.reload_status(cx);
                page.load_market(cx);
            }));
        let colors = cx.theme().colors;
        let busy = Store::global(cx).read(cx).busy();
        let body =
            v_flex()
                .mx_auto()
                .w_full()
                .max_w(gpui_kit::rems(56.0))
                .px_6()
                .pt_3()
                .pb_6()
                .gap_4()
                .child(page_header(
                    IconName::Puzzle,
                    t!("console.page.plugins"),
                    Some(tr!("plugins.subtitle")),
                    refresh,
                    cx,
                ))
                .when_some(busy, |body, (label, detail)| {
                    body.child(div().text_sm().text_color(colors.muted_foreground).child(
                        detail.unwrap_or_else(|| tr!("plugins.in_progress", action = label)),
                    ))
                });
        let Some(data) = &self.status.value else {
            let retry = Button::new("plugin-status-retry")
                .label(t!("plugins.refresh"))
                .on_click(cx.listener(|page, _, _, cx| page.reload_status(cx)));
            return v_flex()
                .size_full()
                .child(body)
                .child(div().flex_1().child(placeholder(
                    IconName::Puzzle,
                    if self.status.error.is_some() {
                        tr!("plugins.load_failed")
                    } else {
                        tr!("plugins.loading")
                    },
                    self.status.error.clone(),
                    self.status.error.as_ref().map(|_| retry),
                    cx,
                )))
                .into_any_element();
        };
        let tabs = Segmented::new("plugin-sections")
            .items([t!("plugins.market_tab"), t!("plugins.installed_tab")])
            .selected(usize::from(self.show_installed))
            .on_change(cx.listener(|page, selected, _, cx| {
                page.show_installed = *selected == 1;
                cx.notify();
            }));
        let content = if self.show_installed {
            v_flex()
                .gap_4()
                .child(self.installed(&data.status, cx))
                .when(!data.status.discovered_providers.is_empty(), |body| {
                    body.child(self.providers(&data.status, cx))
                })
                .when(
                    !data.commands.is_empty()
                        || data.commands_error.is_some()
                        || self.command_output.is_some(),
                    |body| body.child(self.commands(data, cx)),
                )
                .into_any_element()
        } else {
            self.marketplace(cx).into_any_element()
        };
        div()
            .id("plugins-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                body.when_some(self.status.error.clone(), |body, error| {
                    body.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.danger)
                            .child(error),
                    )
                })
                .child(self.runtime(&data.status, cx))
                .child(h_flex().child(tabs))
                .child(content),
            )
            .into_any_element()
    }
}
