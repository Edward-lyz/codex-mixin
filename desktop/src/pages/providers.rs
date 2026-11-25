//! Providers page: the provider list on the left and the selected
//! provider's models or connection settings on the right.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _,
};
use rust_i18n::t;

use super::connection::{ConnectionView, confirm_remove_provider, open_add_provider};
use super::is_busy;
use super::models::ModelsView;
use super::placeholder;
use super::plugins::{
    PluginCommandRun, open_plugin_command_dialog, open_plugin_login_dialog, plugin_auth_badge,
    render_command_output,
};
use crate::console::Console;
use crate::model::{PluginAuthStatus, PluginCommand, PluginData, Provider};
use crate::store::{Change, Snapshot, Store, plugin_data};
use crate::tr;
use crate::widgets::{
    Segmented, pill, provider_logo_with_badge, row_divider, section, setting_row, status_dot,
};

const MODELS_TAB: usize = 0;
const PLUGIN_TAB: usize = 1;
const CONNECTION_TAB: usize = 2;

pub struct ProvidersPage {
    store: Entity<Store>,
    selected: Option<String>,
    tab: usize,
    models: HashMap<String, Entity<ModelsView>>,
    connections: HashMap<String, Entity<ConnectionView>>,
    plugin_data: Snapshot<PluginData>,
    plugin_command_output: Option<Entity<PluginCommandRun>>,
    _subscriptions: Vec<Subscription>,
}

impl ProvidersPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let subscription =
            cx.observe_in(&store, window, |page, _, window, cx| page.sync(window, cx));
        let mut page = Self {
            store,
            selected: None,
            tab: MODELS_TAB,
            models: HashMap::new(),
            connections: HashMap::new(),
            plugin_data: Snapshot::default(),
            plugin_command_output: None,
            _subscriptions: vec![subscription],
        };
        page.sync(window, cx);
        page
    }

    pub fn focus_provider(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.select(id, window, cx);
        if self
            .selected_provider(cx)
            .is_some_and(|provider| provider.plugin_managed)
        {
            self.ensure_plugin_data(cx);
        }
    }

    fn ensure_plugin_data(&mut self, cx: &mut Context<Self>) {
        if self.plugin_data.value.is_some() || self.plugin_data.loading {
            return;
        }
        self.reload_plugin_data(cx);
    }

    fn reload_plugin_data(&mut self, cx: &mut Context<Self>) {
        if !self.plugin_data.begin() {
            return;
        }
        let task = plugin_data(cx);
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |page, cx| {
                if page.plugin_data.finish(result) {
                    page.reload_plugin_data(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn run_plugin_action(&mut self, change: Change, cx: &mut Context<Self>) {
        if is_busy(cx) {
            return;
        }
        let task = Store::global(cx).update(cx, |store, cx| store.run(change, cx));
        cx.spawn(async move |view, cx| {
            let _ = task.await;
            let _ = view.update(cx, |page, cx| page.reload_plugin_data(cx));
        })
        .detach();
    }

    fn open_plugin_command(
        &self,
        command: &PluginCommand,
        quick_args: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.weak_entity();
        open_plugin_command_dialog(
            command,
            quick_args,
            move |output, finished, cx| {
                let _ = view.update(cx, |page, cx| {
                    page.plugin_command_output = output;
                    if finished {
                        page.reload_plugin_data(cx);
                    }
                    cx.notify();
                });
            },
            window,
            cx,
        );
    }

    fn providers<'a>(&self, cx: &'a gpui_kit::App) -> &'a [Provider] {
        self.store
            .read(cx)
            .providers
            .value
            .as_ref()
            .map_or(&[], |list| list.providers.as_slice())
    }

    /// Keeps the selection valid and hands fresh snapshots to retained views.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let providers = self.providers(cx).to_vec();
        let still_listed = self
            .selected
            .as_ref()
            .is_some_and(|id| providers.iter().any(|provider| &provider.id == id));
        if !still_listed {
            self.selected = providers.first().map(|provider| provider.id.clone());
        }
        self.models
            .retain(|id, _| providers.iter().any(|provider| &provider.id == id));
        self.connections
            .retain(|id, _| providers.iter().any(|provider| &provider.id == id));
        for provider in providers {
            if let Some(view) = self.models.get(&provider.id) {
                view.update(cx, |view, cx| view.sync(provider.clone(), window, cx));
            }
            if let Some(view) = self.connections.get(&provider.id) {
                view.update(cx, |view, cx| view.sync(provider.clone(), cx));
            }
        }
        self.ensure_views(window, cx);
        cx.notify();
    }

    fn ensure_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.selected_provider(cx).cloned() else {
            return;
        };
        if !provider.plugin_managed && self.tab == PLUGIN_TAB {
            self.tab = CONNECTION_TAB;
        }
        if self.tab == MODELS_TAB && !self.models.contains_key(&provider.id) {
            let id = provider.id.clone();
            let view = cx.new(|cx| ModelsView::new(provider, window, cx));
            self.models.insert(id, view);
        } else if self.tab == PLUGIN_TAB && provider.plugin_managed {
            self.ensure_plugin_data(cx);
        } else if self.tab == CONNECTION_TAB && !self.connections.contains_key(&provider.id) {
            let id = provider.id.clone();
            let view = cx.new(|cx| ConnectionView::new(Some(provider), window, cx));
            self.connections.insert(id, view);
        }
    }

    fn selected_provider<'a>(&self, cx: &'a gpui_kit::App) -> Option<&'a Provider> {
        let selected = self.selected.as_ref()?;
        self.providers(cx)
            .iter()
            .find(|provider| &provider.id == selected)
    }

    fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.plugin_command_output = None;
        self.ensure_views(window, cx);
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        // The official provider is built in; only added providers can be removed.
        let removable = self
            .selected_provider(cx)
            .filter(|provider| !provider.is_official())
            .map(|provider| (provider.id.clone(), provider.display_name.clone()));
        let can_remove = removable.is_some() && !is_busy(cx);
        let rows = self.providers(cx).iter().map(|provider| {
            let selected = self.selected.as_ref() == Some(&provider.id);
            let id = provider.id.clone();
            let (dot, state) = readiness(provider, cx);
            let enabled = provider.enabled;
            h_flex()
                .id(ElementId::Name(format!("provider-{}", provider.id).into()))
                .gap_2p5()
                .px_2()
                .py_1p5()
                .rounded_lg()
                .border_1()
                .border_color(gpui_kit::transparent_black())
                .cursor_pointer()
                .when(selected, |row| {
                    row.bg(colors.background)
                        .border_color(colors.border)
                        .shadow_xs()
                })
                .when(!selected, |row| {
                    row.hover(|style| style.bg(colors.list_hover))
                })
                .on_click(
                    cx.listener(move |page, _, window, cx| page.select(id.clone(), window, cx)),
                )
                .child(div().when(!enabled, |logo| logo.opacity(0.5)).child(
                    provider_logo_with_badge(&provider.icon, provider.plugin_managed, false, cx),
                ))
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .truncate()
                                .when(!enabled, |name| name.text_color(colors.muted_foreground))
                                .child(provider.display_name.clone()),
                        )
                        .child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .child(status_dot(dot))
                                .child(div().truncate().child(t!(
                                    "providers.row_detail",
                                    state = state,
                                    selected = provider.selected_models.len(),
                                    total = provider.cached_models.len()
                                ))),
                        ),
                )
        });
        v_flex()
            .w_64()
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.muted.opacity(0.35))
            .child(
                h_flex()
                    .pl_4()
                    .pr_2()
                    .pt_2()
                    .pb_1()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(colors.muted_foreground)
                            .child(t!("console.page.providers")),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .rounded_md()
                            .border_1()
                            .border_color(colors.input)
                            .bg(colors.background)
                            .shadow_xs()
                            .overflow_hidden()
                            .child(
                                Button::new("add-provider")
                                    .ghost()
                                    .small()
                                    .rounded_none()
                                    .icon(IconName::Plus)
                                    .tooltip(tr!("providers.add"))
                                    .on_click(|_, window, cx| open_add_provider(window, cx)),
                            )
                            .child(div().w_px().h_4().bg(colors.border))
                            .child(
                                Button::new("remove-selected-provider")
                                    .ghost()
                                    .small()
                                    .rounded_none()
                                    .icon(IconName::Minus)
                                    .tooltip(tr!("connection.remove"))
                                    .disabled(!can_remove)
                                    .on_click(move |_, window, cx| {
                                        if let Some((id, name)) = &removable {
                                            confirm_remove_provider(id, name, window, cx);
                                        }
                                    }),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("provider-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .gap_1()
                    .children(rows),
            )
    }

    fn render_header(&self, provider: &Provider, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (state_color, state) = readiness(provider, cx);
        let state_label: SharedString = if provider.is_healthy() {
            t!("providers.ready").into()
        } else {
            state
        };
        h_flex()
            .px_6()
            .pt_2()
            .pb_4()
            .gap_3()
            .child(provider_logo_with_badge(
                &provider.icon,
                provider.plugin_managed,
                true,
                cx,
            ))
            .child(
                v_flex()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .truncate()
                                    .child(provider.display_name.clone()),
                            )
                            .when(provider.is_official(), |title| {
                                title.child(pill(t!("providers.official"), colors.muted_foreground))
                            })
                            .when(provider.plugin_managed, |title| {
                                title.child(pill(t!("providers.plugin_managed"), colors.primary))
                            })
                            .child(pill(state_label, state_color)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .truncate()
                            .child(if provider.plugin_managed {
                                provider
                                    .pi_plugin_package
                                    .clone()
                                    .or_else(|| provider.pi_provider_id.clone())
                                    .unwrap_or_else(|| provider.id.clone())
                            } else {
                                provider
                                    .base_url
                                    .clone()
                                    .unwrap_or_else(|| provider.id.clone())
                            }),
                    ),
            )
            .child(div().flex_1())
            .child(if provider.plugin_managed {
                Segmented::new("provider-tabs-plugin")
                    .items([
                        t!("providers.tab.models"),
                        t!("providers.tab.plugin"),
                        t!("providers.tab.connection"),
                    ])
                    .selected(self.tab)
                    .on_change(cx.listener(|page, tab: &usize, window, cx| {
                        page.tab = *tab;
                        page.ensure_views(window, cx);
                        cx.notify();
                    }))
            } else {
                let selected_idx = usize::from(self.tab == CONNECTION_TAB);
                Segmented::new("provider-tabs")
                    .items([t!("providers.tab.models"), t!("providers.tab.connection")])
                    .selected(selected_idx)
                    .on_change(cx.listener(|page, tab: &usize, window, cx| {
                        page.tab = if *tab == 0 {
                            MODELS_TAB
                        } else {
                            CONNECTION_TAB
                        };
                        page.ensure_views(window, cx);
                        cx.notify();
                    }))
            })
    }

    fn render_plugin_panel(&self, provider: &Provider, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let busy = is_busy(cx);
        let package = provider.pi_plugin_package.clone();
        let pi_provider = provider
            .pi_provider_id
            .clone()
            .unwrap_or_else(|| provider.id.clone());
        let auth_status = self
            .plugin_data
            .value
            .as_ref()
            .and_then(|data| {
                data.status
                    .provider_details
                    .iter()
                    .find(|detail| detail.id == pi_provider)
            })
            .map_or(PluginAuthStatus::Unknown, |detail| detail.auth_status);
        let login_provider = pi_provider.clone();
        let logout_arg = format!("--provider={pi_provider}");

        let overview = section(
            t!("providers.plugin_section"),
            None,
            v_flex()
                .child(setting_row(
                    t!("providers.plugin_source"),
                    Some(
                        package
                            .as_deref()
                            .map(|pkg| pkg.strip_prefix("npm:").unwrap_or(pkg).to_owned())
                            .unwrap_or_else(|| pi_provider.clone())
                            .into(),
                    ),
                    h_flex()
                        .gap_2()
                        .when_some(package.clone(), |actions, pkg| {
                            actions.child(
                                Button::new("provider-open-plugin")
                                    .secondary()
                                    .small()
                                    .label(t!("providers.open_plugin_detail"))
                                    .on_click(move |_, _, cx| {
                                        Console::open_plugin(pkg.clone(), cx);
                                    }),
                            )
                        })
                        .child(
                            Button::new("provider-plugin-sync")
                                .secondary()
                                .small()
                                .label(t!("plugins.sync"))
                                .disabled(busy)
                                .on_click(cx.listener(|page, _, _, cx| {
                                    page.run_plugin_action(
                                        Change::new(
                                            t!("plugins.sync"),
                                            ["plugin", "sync", "--json"],
                                        ),
                                        cx,
                                    );
                                })),
                        ),
                    cx,
                ))
                .child(row_divider(cx))
                .child(setting_row(
                    t!("providers.plugin_auth"),
                    Some(pi_provider.clone().into()),
                    h_flex()
                        .gap_2()
                        .child(plugin_auth_badge(auth_status, cx))
                        .when(auth_status != PluginAuthStatus::Configured, |actions| {
                            actions.child(
                                Button::new("provider-plugin-login")
                                    .secondary()
                                    .small()
                                    .label(t!("plugins.login"))
                                    .disabled(busy)
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        let view = cx.weak_entity();
                                        open_plugin_login_dialog(
                                            login_provider.clone(),
                                            move |cx| {
                                                let _ = view.update(cx, |page, cx| {
                                                    page.reload_plugin_data(cx)
                                                });
                                            },
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                        })
                        .when(auth_status == PluginAuthStatus::Configured, |actions| {
                            actions.child(
                                Button::new("provider-plugin-logout")
                                    .ghost()
                                    .small()
                                    .label(t!("plugins.logout"))
                                    .disabled(busy)
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.run_plugin_action(
                                            Change::new(
                                                t!("plugins.logout"),
                                                ["plugin", "auth", "logout", &logout_arg, "--json"],
                                            ),
                                            cx,
                                        );
                                    })),
                            )
                        }),
                    cx,
                )),
            cx,
        );

        let commands: Vec<PluginCommand> = self
            .plugin_data
            .value
            .as_ref()
            .map(|data| {
                data.commands
                    .iter()
                    .filter(|cmd| match (&package, &cmd.package) {
                        (Some(pkg), Some(cmd_pkg)) => cmd_pkg == pkg,
                        (None, _) => true,
                        _ => false,
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        let command_rows = commands.iter().enumerate().map(|(idx, command)| {
            let cmd_for_dialog = command.clone();
            let quick_args = command
                .arguments
                .iter()
                .take(4)
                .enumerate()
                .map(|(aidx, arg)| {
                    let command = command.clone();
                    let arg_val = arg.value.clone();
                    Button::new(("provider-cmd-quick", idx * 32 + aidx))
                        .ghost()
                        .xsmall()
                        .label(arg.value.clone())
                        .disabled(busy)
                        .on_click(cx.listener(move |page, _, window, cx| {
                            page.open_plugin_command(&command, Some(arg_val.clone()), window, cx);
                        }))
                });
            let desc = if command.description.is_empty() {
                None
            } else {
                Some(command.description.clone().into())
            };
            v_flex()
                .when(idx != 0, |row| row.child(row_divider(cx)))
                .child(setting_row(
                    format!("/{}", command.name),
                    desc,
                    h_flex().gap_1p5().children(quick_args).child(
                        Button::new(("provider-cmd-run", idx))
                            .secondary()
                            .small()
                            .label(t!("plugins.run"))
                            .disabled(busy)
                            .on_click(cx.listener(move |page, _, window, cx| {
                                page.open_plugin_command(&cmd_for_dialog, None, window, cx);
                            })),
                    ),
                    cx,
                ))
        });

        let commands_body = v_flex()
            .children(command_rows)
            .when(commands.is_empty() && !self.plugin_data.loading, |body| {
                body.child(
                    div()
                        .px_4()
                        .py_3()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child(t!("plugins.no_commands")),
                )
            })
            .when(self.plugin_data.loading && commands.is_empty(), |body| {
                body.child(
                    div()
                        .px_4()
                        .py_3()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child(t!("plugins.loading")),
                )
            })
            .when_some(self.plugin_command_output.as_ref(), |body, output| {
                body.when(!commands.is_empty(), |body| body.child(row_divider(cx)))
                    .child(render_command_output(output.read(cx), cx))
            });

        v_flex()
            .id("provider-plugin-scroll")
            .size_full()
            .overflow_y_scroll()
            .px_6()
            .pb_6()
            .gap_4()
            .child(overview)
            .child(section(
                t!("providers.plugin_commands_section"),
                None,
                commands_body,
                cx,
            ))
    }
}

/// Status color and label for a provider row.
pub fn readiness(provider: &Provider, cx: &gpui_kit::App) -> (gpui_kit::Hsla, SharedString) {
    let colors = cx.theme().colors;
    if !provider.enabled {
        return (
            colors.muted_foreground,
            t!("providers.state.disabled").into(),
        );
    }
    match provider.readiness.as_str() {
        "healthy" => (colors.success, t!("providers.state.healthy").into()),
        "degraded" => (colors.warning, t!("providers.state.degraded").into()),
        other => (colors.warning, other.to_owned().into()),
    }
}

impl Render for ProvidersPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        if store.providers.value.is_none() {
            let content = match store.providers.error.clone() {
                Some(error) => placeholder(
                    IconName::TriangleAlert,
                    t!("providers.load_failed"),
                    Some(error),
                    None,
                    cx,
                )
                .into_any_element(),
                None => placeholder(
                    IconName::LoaderCircle,
                    t!("providers.loading"),
                    None,
                    None,
                    cx,
                )
                .into_any_element(),
            };
            return div().size_full().child(content).into_any_element();
        }
        let Some(provider) = self.selected_provider(cx).cloned() else {
            let add = Button::new("first-provider")
                .label(t!("providers.add"))
                .on_click(|_, window, cx| open_add_provider(window, cx));
            return h_flex()
                .size_full()
                .child(self.render_sidebar(cx))
                .child(placeholder(
                    IconName::Plug,
                    t!("providers.empty_title"),
                    Some(t!("providers.empty_detail").into()),
                    Some(add),
                    cx,
                ))
                .into_any_element();
        };
        let body = if self.tab == MODELS_TAB {
            self.models
                .get(&provider.id)
                .cloned()
                .map(IntoElement::into_any_element)
        } else if self.tab == PLUGIN_TAB && provider.plugin_managed {
            Some(self.render_plugin_panel(&provider, cx).into_any_element())
        } else {
            self.connections
                .get(&provider.id)
                .cloned()
                .map(IntoElement::into_any_element)
        };
        h_flex()
            .size_full()
            .child(self.render_sidebar(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_header(&provider, cx))
                    .child(div().flex_1().min_h_0().children(body)),
            )
            .into_any_element()
    }
}
