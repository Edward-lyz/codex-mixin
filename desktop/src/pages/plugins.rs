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
use crate::console::Console;
use crate::model::{
    MarketPackage, PluginCommand, PluginCommandOutput, PluginData, PluginProvider, PluginStatus,
};
use crate::store::{Change, Snapshot, Store, plugin_data, query};
use crate::tr;
use crate::widgets::{Segmented, page_header, pill, row_divider, section, setting_row};

pub struct PluginsPage {
    status: Snapshot<PluginData>,
    market: Snapshot<Vec<MarketPackage>>,
    search: Entity<InputState>,
    market_query: String,
    show_installed: bool,
    selected_package: Option<String>,
    command_output: Option<(String, Result<PluginCommandOutput, SharedString>)>,
    _subscriptions: Vec<Subscription>,
}

pub fn normalize_pkg_source(spec: &str) -> String {
    let trimmed = spec.trim();
    if trimmed.starts_with("npm:") {
        trimmed.to_owned()
    } else {
        format!("npm:{trimmed}")
    }
}

fn strip_npm_prefix(spec: &str) -> &str {
    spec.strip_prefix("npm:").unwrap_or(spec)
}

fn looks_like_npm_spec(input: &str) -> bool {
    let raw = strip_npm_prefix(input.trim());
    if raw.is_empty() || raw.len() > 256 || raw.chars().any(char::is_whitespace) {
        return false;
    }
    let name = raw
        .get(1..)
        .and_then(|rest| rest.find('@'))
        .map_or(raw, |idx| &raw[..idx + 1]);
    if let Some(stripped) = name.strip_prefix('@') {
        let Some((scope, pkg)) = stripped.split_once('/') else {
            return false;
        };
        valid_npm_segment(scope) && valid_npm_segment(pkg)
    } else {
        valid_npm_segment(name) && (name.contains('-') || input.trim().starts_with("npm:"))
    }
}

fn valid_npm_segment(seg: &str) -> bool {
    !seg.is_empty()
        && !seg.starts_with('.')
        && !seg.starts_with('_')
        && seg
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-._".contains(&b))
}

pub fn open_plugin_command_dialog(
    command: &PluginCommand,
    on_run: impl Fn(String, String, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let args = cx.new(|cx| InputState::new(window, cx).placeholder(t!("plugins.arguments")));
    let name = command.name.clone();
    let description = command.description.clone();
    let arguments = command.arguments.clone();
    let on_run = std::rc::Rc::new(on_run);
    window.open_dialog(cx, move |dialog, _, cx| {
        let run_args = args.clone();
        let run_name = name.clone();
        let run_cb = on_run.clone();
        let colors = cx.theme().colors;
        let arg_chips = arguments.iter().enumerate().map(|(idx, arg)| {
            let fill_args = args.clone();
            let value = arg.value.clone();
            let label = if let Some(desc) = arg.description.as_deref() {
                format!("{} · {desc}", arg.label)
            } else if !arg.label.is_empty() {
                arg.label.clone()
            } else {
                arg.value.clone()
            };
            Button::new(("plugin-arg-chip", idx))
                .ghost()
                .xsmall()
                .label(label)
                .on_click(move |_, window, cx| {
                    let next = value.clone();
                    fill_args.update(cx, |input, cx| input.set_value(next, window, cx));
                })
        });
        dialog
            .title(tr!("plugins.run_command_title", name = name))
            .w(gpui_kit::px(520.0))
            .child(
                v_flex()
                    .gap_3()
                    .when(!description.is_empty(), |body| {
                        body.child(div().text_sm().child(description.clone()))
                    })
                    .when(!arguments.is_empty(), |body| {
                        body.child(
                            v_flex()
                                .gap_1p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.muted_foreground)
                                        .child(t!("plugins.arguments_hint")),
                                )
                                .child(h_flex().flex_wrap().gap_1p5().children(arg_chips)),
                        )
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
                                let args = run_args.read(cx).value().to_string();
                                (run_cb)(run_name.clone(), args, cx);
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}

pub fn render_command_output(
    name: &str,
    output: &Result<PluginCommandOutput, SharedString>,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors;
    let result = v_flex().gap_1().px_4().py_3().child(
        div()
            .font_semibold()
            .text_xs()
            .child(t!("plugins.command_output", name = name)),
    );
    match output {
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
        Ok(output) => result.children(output.notifications.iter().map(|notification| {
            let color = match notification.kind.as_str() {
                "error" => colors.danger,
                "warning" => colors.warning,
                _ => colors.foreground,
            };
            div()
                .text_sm()
                .text_color(color)
                .child(notification.message.clone())
        })),
    }
}

impl PluginsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("plugins.search_hint")));
        let search_subscription = cx.subscribe(&search, |page, _, event, cx| match event {
            InputEvent::PressEnter { .. } => page.search(cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let store_subscription = cx.observe(&Store::global(cx), |_, _, cx| cx.notify());
        let mut page = Self {
            status: Snapshot::default(),
            market: Snapshot::default(),
            search,
            market_query: String::new(),
            show_installed: false,
            selected_package: None,
            command_output: None,
            _subscriptions: vec![search_subscription, store_subscription],
        };
        page.reload_status(cx);
        page.load_market(cx);
        page
    }

    pub fn open_package_detail(&mut self, package: String, cx: &mut Context<Self>) {
        self.show_installed = true;
        self.selected_package = Some(normalize_pkg_source(&package));
        self.reload_status(cx);
        cx.notify();
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
                if let Some(selected) = page.selected_package.as_deref()
                    && let Some(data) = &page.status.value
                    && !data
                        .status
                        .installed_packages
                        .iter()
                        .any(|pkg| pkg == selected)
                {
                    page.selected_package = None;
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

    fn open_command(&self, command: &PluginCommand, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        open_plugin_command_dialog(
            command,
            move |run_name, args, cx| {
                let _ = view.update(cx, |page, cx| {
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
                        Some(run_name),
                        cx,
                    );
                });
            },
            window,
            cx,
        );
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

    fn installed(&self, data: &PluginData, cx: &mut Context<Self>) -> impl IntoElement {
        let status = &data.status;
        let busy = is_busy(cx) || !status.node_available;
        let colors = cx.theme().colors;
        let rows = status
            .installed_packages
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let update = format!("--spec={spec}");
                let remove = update.clone();
                let open_spec = spec.clone();
                let provider_count = status
                    .provider_details
                    .iter()
                    .filter(|p| p.package.as_deref() == Some(spec.as_str()))
                    .count();
                let model_count: usize = status
                    .provider_details
                    .iter()
                    .filter(|p| p.package.as_deref() == Some(spec.as_str()))
                    .map(|p| p.model_count)
                    .sum();
                let command_count = data
                    .commands
                    .iter()
                    .filter(|c| c.package.as_deref() == Some(spec.as_str()))
                    .count();
                let summary = tr!(
                    "plugins.package_summary",
                    providers = provider_count,
                    models = model_count,
                    commands = command_count
                );
                v_flex()
                    .when(index != 0, |row| row.child(row_divider(cx)))
                    .child(setting_row(
                        strip_npm_prefix(spec).to_owned(),
                        Some(summary),
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new(("plugin-manage", index))
                                    .secondary()
                                    .small()
                                    .label(t!("plugins.manage"))
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.selected_package = Some(open_spec.clone());
                                        page.command_output = None;
                                        cx.notify();
                                    })),
                            )
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

    fn render_provider_rows(
        &self,
        title: SharedString,
        providers: &[PluginProvider],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy = is_busy(cx);
        let colors = cx.theme().colors;
        let cli = Store::global(cx).read(cx).cli();
        let copy_notice = if cfg!(windows) {
            tr!("plugins.login_copied_powershell")
        } else {
            tr!("plugins.login_copied")
        };
        let rows = providers.iter().enumerate().map(|(index, provider)| {
            let argument = format!("--provider={}", provider.id);
            let login = cli.plugin_login_command(&provider.id);
            let notice = copy_notice.clone();
            let mixin_id = provider.mixin_provider_id.clone();
            let mut detail = format!(
                "{} · {}",
                provider.id,
                t!("plugins.model_count", count = provider.model_count)
            );
            if provider.auth_status == "not_configured" {
                detail.push_str(&format!(" · {}", t!("plugins.not_signed_in")));
            }
            let detail: SharedString = detail.into();
            v_flex()
                .when(index != 0, |row| row.child(row_divider(cx)))
                .child(setting_row(
                    provider.display_name.clone(),
                    Some(detail),
                    h_flex()
                        .gap_2()
                        .when(!mixin_id.is_empty(), |actions| {
                            actions.child(
                                Button::new(("plugin-open-provider", index))
                                    .secondary()
                                    .small()
                                    .label(t!("plugins.open_provider"))
                                    .on_click(move |_, _, cx| {
                                        Console::open_provider(mixin_id.clone(), cx);
                                    }),
                            )
                        })
                        .child(
                            Button::new(("plugin-login", index))
                                .ghost()
                                .small()
                                .icon(IconName::Copy)
                                .label(t!("plugins.copy_login"))
                                .on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(login.clone()));
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
        let body = v_flex().children(rows).when(providers.is_empty(), |body| {
            body.child(
                div()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(t!("plugins.no_providers")),
            )
        });
        section(title, None, body, cx)
    }

    fn render_command_rows(
        &self,
        title: SharedString,
        commands: &[PluginCommand],
        commands_error: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let busy = is_busy(cx);
        let rows = commands.iter().enumerate().map(|(index, command)| {
            let command_for_click = command.clone();
            let arg_summary = if command.arguments.is_empty() {
                None
            } else {
                let list = command
                    .arguments
                    .iter()
                    .take(8)
                    .map(|a| a.value.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ");
                Some(list)
            };
            let desc_text = match (command.description.is_empty(), arg_summary) {
                (false, Some(args)) => Some(format!("{} — {}", command.description, args).into()),
                (false, None) => Some(command.description.clone().into()),
                (true, Some(args)) => Some(args.into()),
                (true, None) => None,
            };
            v_flex()
                .when(index != 0, |row| row.child(row_divider(cx)))
                .child(setting_row(
                    format!("/{}", command.name),
                    desc_text,
                    Button::new(("plugin-command", index))
                        .secondary()
                        .small()
                        .label(t!("plugins.run"))
                        .disabled(busy)
                        .on_click(cx.listener(move |page, _, window, cx| {
                            page.open_command(&command_for_click, window, cx);
                        })),
                    cx,
                ))
        });
        let body = v_flex()
            .children(rows)
            .when(commands.is_empty() && commands_error.is_none(), |body| {
                body.child(
                    div()
                        .px_4()
                        .py_3()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child(t!("plugins.no_commands")),
                )
            })
            .when_some(commands_error, |body, error| {
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
                body.when(!commands.is_empty(), |body| body.child(row_divider(cx)))
                    .child(render_command_output(name, output, cx))
            });
        section(title, None, body, cx)
    }

    fn package_detail(
        &self,
        package: &str,
        data: &PluginData,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let busy = is_busy(cx) || !data.status.node_available;
        let update_arg = format!("--spec={package}");
        let remove_arg = update_arg.clone();
        let pkg_providers: Vec<PluginProvider> = data
            .status
            .provider_details
            .iter()
            .filter(|p| p.package.as_deref() == Some(package))
            .cloned()
            .collect();
        let pkg_commands: Vec<PluginCommand> = data
            .commands
            .iter()
            .filter(|c| c.package.as_deref() == Some(package))
            .cloned()
            .collect();
        let header = h_flex()
            .gap_3()
            .items_center()
            .child(
                Button::new("plugin-detail-back")
                    .ghost()
                    .small()
                    .icon(IconName::ArrowLeft)
                    .label(t!("plugins.back"))
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.selected_package = None;
                        page.command_output = None;
                        cx.notify();
                    })),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .text_base()
                            .font_semibold()
                            .truncate()
                            .child(strip_npm_prefix(package).to_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(package.to_owned()),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("plugin-detail-sync")
                            .secondary()
                            .small()
                            .label(t!("plugins.sync"))
                            .disabled(busy)
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.apply(
                                    Change::new(t!("plugins.sync"), ["plugin", "sync", "--json"]),
                                    None,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("plugin-detail-update")
                            .ghost()
                            .small()
                            .label(t!("plugins.update"))
                            .disabled(busy)
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.apply(
                                    Change::new(
                                        t!("plugins.update"),
                                        ["plugin", "update", &update_arg, "--json"],
                                    ),
                                    None,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("plugin-detail-remove")
                            .ghost()
                            .small()
                            .label(t!("plugins.remove"))
                            .disabled(busy)
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.apply(
                                    Change::new(
                                        t!("plugins.remove"),
                                        ["plugin", "remove", &remove_arg, "--json"],
                                    ),
                                    None,
                                    cx,
                                );
                            })),
                    ),
            );
        v_flex()
            .gap_4()
            .child(header)
            .child(self.render_provider_rows(
                t!("plugins.provider_section").into(),
                &pkg_providers,
                cx,
            ))
            .child(self.render_command_rows(
                t!("plugins.command_section").into(),
                &pkg_commands,
                data.commands_error.as_deref(),
                cx,
            ))
    }

    fn marketplace(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let install_disabled = self.install_disabled(cx);
        let typed = self.search.read(cx).value().trim().to_owned();
        let already_installed = self.status.value.as_ref().is_some_and(|data| {
            let normalized = normalize_pkg_source(&typed);
            data.status
                .installed_packages
                .iter()
                .any(|pkg| pkg == &normalized || strip_npm_prefix(pkg) == strip_npm_prefix(&typed))
        });
        let direct_spec = (looks_like_npm_spec(&typed) && !already_installed).then_some(typed);
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
            .when_some(direct_spec, |bar, spec| {
                let label = tr!("plugins.direct_install", spec = strip_npm_prefix(&spec));
                bar.child(
                    h_flex().justify_end().child(
                        Button::new("plugin-direct-install")
                            .primary()
                            .small()
                            .label(label)
                            .disabled(install_disabled)
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.install(&spec, cx);
                            })),
                    ),
                )
            })
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
                if !page.show_installed {
                    page.selected_package = None;
                }
                cx.notify();
            }));
        let content = if self.show_installed {
            if let Some(package) = self.selected_package.clone() {
                self.package_detail(&package, data, cx).into_any_element()
            } else {
                let orphan_providers: Vec<PluginProvider> = data
                    .status
                    .provider_details
                    .iter()
                    .filter(|p| p.package.is_none())
                    .cloned()
                    .collect();
                let orphan_commands: Vec<PluginCommand> = data
                    .commands
                    .iter()
                    .filter(|c| c.package.is_none())
                    .cloned()
                    .collect();
                v_flex()
                    .gap_4()
                    .child(self.installed(data, cx))
                    .when(!orphan_providers.is_empty(), |body| {
                        body.child(self.render_provider_rows(
                            t!("plugins.unattributed_providers").into(),
                            &orphan_providers,
                            cx,
                        ))
                    })
                    .when(!orphan_commands.is_empty(), |body| {
                        body.child(self.render_command_rows(
                            t!("plugins.unattributed_commands").into(),
                            &orphan_commands,
                            data.commands_error.as_deref(),
                            cx,
                        ))
                    })
                    .into_any_element()
            }
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
