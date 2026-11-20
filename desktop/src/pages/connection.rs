//! Provider connection settings, used inline for an existing provider and
//! inside the "Add provider" dialog for a new one. Fields follow the preset
//! the CLI describes in `interface --json`; validation stays in the CLI.

use crate::tr;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    AnimationExt as _, App, AppContext as _, Axis, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use rust_i18n::t;

use super::{is_busy, run_change, run_change_then};
use crate::cli::Secret;
use crate::model::{Provider, ProviderCatalog, ProviderPreset};
use crate::store::{Change, Store};
use crate::widgets::{provider_logo, row_divider, section, setting_row};

const BRIDGE_OPTIONS: [&str; 2] = ["disabled", "ducx_loopback"];

struct Inputs {
    id: Entity<InputState>,
    name: Entity<InputState>,
    base_url: Entity<InputState>,
    website: Entity<InputState>,
    api_path: Entity<InputState>,
    image_path: Entity<InputState>,
    key: Entity<InputState>,
    quota_username: Entity<InputState>,
    workspace: Entity<InputState>,
    cookie: Entity<InputState>,
    aws_access: Entity<InputState>,
    aws_secret: Entity<InputState>,
    aws_session: Entity<InputState>,
    aws_region: Entity<InputState>,
    oauth_timeout: Entity<InputState>,
}

pub struct ConnectionView {
    provider: Option<Provider>,
    preset: String,
    inputs: Inputs,
    protocol: Option<String>,
    bridge: String,
    report: bool,
    review_model: Option<String>,
    title_model: Option<String>,
    dirty: bool,
    advanced: bool,
    _subscriptions: Vec<Subscription>,
}

fn text(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

impl ConnectionView {
    pub fn new(provider: Option<Provider>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let catalog = catalog(cx);
        let preset = provider
            .as_ref()
            .map(|provider| {
                provider
                    .preset_id
                    .clone()
                    .unwrap_or_else(|| "custom".to_owned())
            })
            .unwrap_or_else(|| catalog.default_preset.clone());
        let mut subscriptions = Vec::new();
        let mut input =
            |value: String, placeholder: SharedString, masked: bool, cx: &mut Context<Self>| {
                let state = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .masked(masked)
                        .default_value(value)
                });
                subscriptions.push(cx.subscribe(&state, |view, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        view.dirty = true;
                        cx.notify();
                    }
                }));
                state
            };
        let saved = provider.clone().unwrap_or_default();
        let keep: SharedString = if provider.is_some() {
            t!("connection.keep_secret").into()
        } else {
            "".into()
        };
        let inputs = Inputs {
            id: input(
                saved.id.clone(),
                t!("connection.id_placeholder").into(),
                false,
                cx,
            ),
            name: input(saved.display_name.clone(), "".into(), false, cx),
            base_url: input(
                text(&saved.base_url),
                t!("connection.base_url_placeholder").into(),
                false,
                cx,
            ),
            website: input(text(&saved.website_url), "https://".into(), false, cx),
            api_path: input(
                text(&saved.api_path),
                "/v1/chat/completions".into(),
                false,
                cx,
            ),
            image_path: input(
                text(&saved.image_generation_path),
                "/v1/images/generations".into(),
                false,
                cx,
            ),
            key: input(String::new(), keep.clone(), true, cx),
            quota_username: input(text(&saved.quota_username), "".into(), false, cx),
            workspace: input(text(&saved.quota_workspace_id), "".into(), false, cx),
            cookie: input(String::new(), keep.clone(), true, cx),
            aws_access: input(String::new(), keep.clone(), true, cx),
            aws_secret: input(String::new(), keep.clone(), true, cx),
            aws_session: input(String::new(), t!("connection.optional").into(), true, cx),
            aws_region: input(
                saved
                    .aws_region
                    .clone()
                    .unwrap_or_else(|| catalog.default_aws_region.clone()),
                "".into(),
                false,
                cx,
            ),
            oauth_timeout: input(
                catalog.oauth_timeout_seconds.to_string(),
                "".into(),
                false,
                cx,
            ),
        };
        Self {
            protocol: provider.as_ref().map(|provider| provider.protocol.clone()),
            bridge: saved
                .baidu_auth_bridge
                .clone()
                .unwrap_or_else(|| "disabled".to_owned()),
            report: saved.baidu_code_report,
            review_model: saved.auto_review_model.clone(),
            title_model: saved.codex_thread_title_model.clone(),
            provider,
            preset,
            inputs,
            dirty: false,
            advanced: false,
            _subscriptions: subscriptions,
        }
    }

    /// A saved provider snapshot arrived; a clean form shows it.
    pub fn sync(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if !self.dirty {
            self.protocol = Some(provider.protocol.clone());
            self.bridge = provider
                .baidu_auth_bridge
                .clone()
                .unwrap_or_else(|| "disabled".to_owned());
            self.report = provider.baidu_code_report;
            self.review_model = provider.auto_review_model.clone();
            self.title_model = provider.codex_thread_title_model.clone();
        }
        self.provider = Some(provider);
        cx.notify();
    }

    fn preset_definition(&self, cx: &App) -> Option<ProviderPreset> {
        catalog(cx)
            .presets
            .into_iter()
            .find(|preset| preset.id == self.preset)
    }

    fn value(state: &Entity<InputState>, cx: &App) -> String {
        state.read(cx).value().trim().to_owned()
    }

    /// Builds the `provider add|update` command, or `None` while the preset
    /// catalog is still loading.
    fn save_change(&self, cx: &App) -> Option<Change> {
        let preset = self.preset_definition(cx)?;
        let creating = self.provider.is_none();
        let mut args = vec![
            "provider".to_owned(),
            if creating { "add" } else { "update" }.to_owned(),
        ];
        let mut secrets = Vec::new();
        if creating {
            args.extend(["--preset".to_owned(), preset.id.clone()]);
            let id = Self::value(&self.inputs.id, cx);
            if !id.is_empty() && self.shows_id(&preset) {
                args.extend(["--id".to_owned(), id]);
            }
        } else if let Some(provider) = &self.provider {
            args.push(provider.id.clone());
        }
        let oauth = creating && preset.auth_mode == "local_oauth";
        if !oauth {
            for (flag, state) in [
                ("--display-name", &self.inputs.name),
                ("--base-url", &self.inputs.base_url),
                ("--website-url", &self.inputs.website),
                ("--image-generation-path", &self.inputs.image_path),
            ] {
                let value = Self::value(state, cx);
                if !value.is_empty() {
                    args.extend([flag.to_owned(), value]);
                }
            }
        }
        // An emptied optional field clears the saved value.
        if let Some(saved) = &self.provider {
            if saved.website_url.is_some() && Self::value(&self.inputs.website, cx).is_empty() {
                args.extend(["--website-url".to_owned(), String::new()]);
            }
            if saved.image_generation_path.is_some()
                && Self::value(&self.inputs.image_path, cx).is_empty()
            {
                args.push("--clear-image-generation".to_owned());
            }
            if saved.quota_workspace_id.is_some()
                && preset.quota_credentials_required
                && Self::value(&self.inputs.workspace, cx).is_empty()
            {
                args.push("--clear-quota-workspace-id".to_owned());
            }
        }
        if preset.custom_endpoint {
            if let Some(protocol) = &self.protocol {
                args.extend(["--protocol".to_owned(), protocol.clone()]);
            }
            let api_path = Self::value(&self.inputs.api_path, cx);
            if !api_path.is_empty() {
                args.extend(["--api-path".to_owned(), api_path]);
            }
        }
        let mut secret = |flag: &str,
                          variable: &'static str,
                          state: &Entity<InputState>,
                          args: &mut Vec<String>| {
            let value = state.read(cx).value().to_string();
            if value.is_empty() {
                return;
            }
            args.extend([flag.to_owned(), format!("@env:{variable}")]);
            secrets.push(Secret { variable, value });
        };
        if preset.auth_mode == "aws_sigv4" {
            secret(
                "--aws-access-key-id",
                "CM_AWS_ID",
                &self.inputs.aws_access,
                &mut args,
            );
            secret(
                "--aws-secret-access-key",
                "CM_AWS_SECRET",
                &self.inputs.aws_secret,
                &mut args,
            );
            secret(
                "--aws-session-token",
                "CM_AWS_SESSION",
                &self.inputs.aws_session,
                &mut args,
            );
            args.extend([
                "--aws-region".to_owned(),
                Self::value(&self.inputs.aws_region, cx),
            ]);
        } else if !oauth {
            secret("--key", "CM_API_KEY", &self.inputs.key, &mut args);
        }
        if oauth {
            let timeout = Self::value(&self.inputs.oauth_timeout, cx);
            if !timeout.is_empty() {
                args.extend(["--oauth-timeout-seconds".to_owned(), timeout]);
            }
        }
        if preset.quota_username_required {
            let username = Self::value(&self.inputs.quota_username, cx);
            if !username.is_empty() {
                args.extend(["--quota-username".to_owned(), username]);
            }
            args.extend(["--baidu-auth-bridge".to_owned(), self.bridge.clone()]);
            args.extend(["--baidu-code-report".to_owned(), self.report.to_string()]);
        }
        if preset.quota_credentials_required {
            let workspace = Self::value(&self.inputs.workspace, cx);
            if !workspace.is_empty() {
                args.extend(["--quota-workspace-id".to_owned(), workspace]);
            }
            secret(
                "--quota-auth-cookie",
                "CM_QUOTA_COOKIE",
                &self.inputs.cookie,
                &mut args,
            );
        }
        if !creating {
            match &self.review_model {
                Some(model) => args.extend(["--auto-review-model".to_owned(), model.clone()]),
                None => args.push("--clear-auto-review-model".to_owned()),
            }
            match &self.title_model {
                Some(model) => {
                    args.extend(["--codex-thread-title-model".to_owned(), model.clone()])
                }
                None => args.push("--clear-codex-thread-title-model".to_owned()),
            }
        }
        let label = if creating {
            t!("connection.add_label")
        } else {
            t!("connection.save_label")
        };
        let mut change = Change::new(label, args).applied();
        change.secrets = secrets;
        Some(change)
    }

    fn shows_id(&self, preset: &ProviderPreset) -> bool {
        !(preset.auth_mode == "local_oauth" && !preset.allow_multiple)
    }

    /// Saves the form; the draft stays dirty until the CLI confirms it, so a
    /// failed save keeps the user's input. Resolves to whether it succeeded.
    pub fn save(&mut self, cx: &mut Context<Self>) -> gpui_kit::Task<bool> {
        if is_busy(cx) {
            return gpui_kit::Task::ready(false);
        }
        let Some(change) = self.save_change(cx) else {
            return gpui_kit::Task::ready(false);
        };
        let saved = run_change_then(change, cx);
        cx.spawn(async move |view, cx| {
            let ok = saved.await;
            if ok {
                view.update(cx, |view, cx| {
                    view.dirty = false;
                    cx.notify();
                })
                .ok();
            }
            ok
        })
    }

    pub fn set_preset(&mut self, preset: String, cx: &mut Context<Self>) {
        if self.preset == preset {
            return;
        }
        self.preset = preset;
        self.protocol = None;
        cx.notify();
    }
}

pub fn catalog(cx: &App) -> ProviderCatalog {
    Store::global(cx)
        .read(cx)
        .interface
        .value
        .as_ref()
        .map(|interface| interface.provider_catalog.clone())
        .unwrap_or_default()
}

fn field(label: impl Into<SharedString>, state: &Entity<InputState>) -> Field {
    Field::new()
        .label(label.into())
        .child(Input::new(state).small())
}

/// A dropdown that picks one of `options`, where `None` means "not set".
fn choice_button(
    id: &'static str,
    current: Option<String>,
    none_label: Option<SharedString>,
    options: Vec<(String, SharedString)>,
    on_pick: impl Fn(Option<String>, &mut App) + 'static,
) -> impl IntoElement {
    let label = current
        .as_ref()
        .and_then(|value| {
            options
                .iter()
                .find(|(id, _)| id == value)
                .map(|(_, label)| label.clone())
        })
        .or_else(|| current.clone().map(SharedString::from))
        .or_else(|| none_label.clone())
        .unwrap_or_default();
    let on_pick = std::rc::Rc::new(on_pick);
    Button::new(id)
        .secondary()
        .small()
        .label(label)
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu;
            if let Some(none_label) = &none_label {
                let on_pick = on_pick.clone();
                menu = menu.item(
                    PopupMenuItem::new(none_label.clone())
                        .checked(current.is_none())
                        .on_click(move |_, _, cx| on_pick(None, cx)),
                );
            }
            options.iter().fold(menu, |menu, (value, label)| {
                let on_pick = on_pick.clone();
                let value = value.clone();
                menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(current.as_deref() == Some(value.as_str()))
                        .on_click(move |_, _, cx| on_pick(Some(value.clone()), cx)),
                )
            })
        })
}

impl ConnectionView {
    fn render_preset_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let presets = catalog(cx).presets;
        let rows = presets.into_iter().map(|preset| {
            let selected = preset.id == self.preset;
            let id = preset.id.clone();
            h_flex()
                .id(SharedString::from(format!("preset-{}", preset.id)))
                .gap_2()
                .h_11()
                .px_2()
                .flex_none()
                .rounded_lg()
                .border_1()
                .cursor_pointer()
                .border_color(if selected {
                    colors.primary
                } else {
                    colors.border
                })
                .when(selected, |card| card.bg(colors.primary.opacity(0.08)))
                .when(!selected, |card| card.hover(|style| style.bg(colors.muted)))
                .on_click(cx.listener(move |view, _, _, cx| view.set_preset(id.clone(), cx)))
                .child(provider_logo(&preset.icon, false, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_medium()
                        .child(preset.display_name),
                )
                .when(selected, |row| {
                    row.child(
                        Icon::new(IconName::Check)
                            .xsmall()
                            .text_color(colors.primary),
                    )
                })
        });
        v_flex()
            .w(px(232.0))
            .h_full()
            .flex_none()
            .p_2()
            .gap_2()
            .rounded_lg()
            .bg(colors.muted.opacity(0.5))
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .text_sm()
                    .font_semibold()
                    .child(t!("connection.presets")),
            )
            .child(
                v_flex()
                    .id("preset-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_1()
                    .children(rows),
            )
    }

    fn render_fields(&self, preset: &ProviderPreset, cx: &mut Context<Self>) -> Form {
        let creating = self.provider.is_none();
        let advanced = !creating || self.advanced;
        let oauth = creating && preset.auth_mode == "local_oauth";
        let aws = preset.auth_mode == "aws_sigv4";
        let entity = cx.entity().downgrade();
        let key_label: SharedString = if preset.api_key_required {
            "API Key".into()
        } else if preset.auth_mode == "local_session" {
            t!("connection.key_local_session").into()
        } else {
            t!("connection.key_optional").into()
        };
        let protocols: Vec<(String, SharedString)> = catalog(cx)
            .protocols
            .into_iter()
            .map(|protocol| (protocol.id.clone(), protocol.display_name.clone().into()))
            .collect();
        let protocol_view = entity.clone();
        let bridge_view = entity.clone();
        let report_view = entity.clone();
        Form::new()
            .label_layout(Axis::Horizontal)
            .label_width(px(if creating { 124.0 } else { 150.0 }))
            .when(!oauth, |form| {
                form.when(!aws, |form| form.child(field(key_label, &self.inputs.key)))
                    .child(field(t!("connection.name"), &self.inputs.name))
                    .child(field(t!("connection.base_url"), &self.inputs.base_url))
            })
            .when(preset.custom_endpoint, |form| {
                form.child(
                    Field::new()
                        .label(tr!("connection.protocol"))
                        .child(choice_button(
                            "protocol",
                            self.protocol.clone(),
                            Some(t!("connection.protocol_auto").into()),
                            protocols,
                            move |protocol, cx| {
                                protocol_view
                                    .update(cx, |view, cx| {
                                        view.protocol = protocol;
                                        view.dirty = true;
                                        cx.notify();
                                    })
                                    .ok();
                            },
                        )),
                )
            })
            .when(advanced && creating && self.shows_id(preset), |form| {
                form.child(field(t!("connection.id"), &self.inputs.id))
            })
            .when(advanced && !oauth, |form| {
                form.child(field(t!("connection.website"), &self.inputs.website))
                    .child(field(t!("connection.image_path"), &self.inputs.image_path))
                    .when(preset.custom_endpoint, |form| {
                        form.child(field(t!("connection.api_path"), &self.inputs.api_path))
                    })
            })
            .when(aws, |form| {
                form.child(field("AWS Access Key ID", &self.inputs.aws_access))
                    .child(field("AWS Secret Access Key", &self.inputs.aws_secret))
                    .child(field("AWS Session Token", &self.inputs.aws_session))
                    .child(field("AWS Region", &self.inputs.aws_region))
            })
            .when(oauth, |form| {
                form.child(
                    field(t!("connection.oauth_timeout"), &self.inputs.oauth_timeout)
                        .description(tr!("connection.oauth_hint")),
                )
            })
            .when(preset.quota_username_required, |form| {
                form.child(field(
                    t!("connection.quota_username"),
                    &self.inputs.quota_username,
                ))
                .child(
                    Field::new()
                        .label(tr!("connection.bridge"))
                        .child(choice_button(
                            "bridge",
                            Some(self.bridge.clone()),
                            None,
                            BRIDGE_OPTIONS
                                .iter()
                                .map(|option| {
                                    let label = if *option == "disabled" {
                                        t!("connection.bridge_disabled")
                                    } else {
                                        t!("connection.bridge_ducx")
                                    };
                                    ((*option).to_owned(), label.into())
                                })
                                .collect(),
                            move |bridge, cx| {
                                bridge_view
                                    .update(cx, |view, cx| {
                                        view.bridge =
                                            bridge.unwrap_or_else(|| "disabled".to_owned());
                                        view.dirty = true;
                                        cx.notify();
                                    })
                                    .ok();
                            },
                        )),
                )
                .child(
                    Field::new().label_indent(true).child(
                        Checkbox::new("code-report")
                            .label(tr!("connection.code_report"))
                            .checked(self.report)
                            .on_change(move |checked, _, cx| {
                                report_view
                                    .update(cx, |view, cx| {
                                        view.report = *checked;
                                        view.dirty = true;
                                        cx.notify();
                                    })
                                    .ok();
                            }),
                    ),
                )
            })
            .when(preset.quota_credentials_required, |form| {
                form.child(field("OpenCode Go workspace ID", &self.inputs.workspace))
                    .child(field(t!("connection.cookie"), &self.inputs.cookie))
            })
            .when(!creating, |form| self.override_fields(form, cx))
    }

    fn override_fields(&self, form: Form, cx: &mut Context<Self>) -> Form {
        let models: Vec<(String, SharedString)> = self
            .provider
            .as_ref()
            .map(|provider| {
                provider
                    .selected_models
                    .iter()
                    .map(|model| (model.clone(), model.clone().into()))
                    .collect()
            })
            .unwrap_or_default();
        let review_view = cx.entity().downgrade();
        let title_view = review_view.clone();
        form.child(
            Field::new()
                .label(tr!("connection.review_model"))
                .description(tr!("connection.override_hint"))
                .child(choice_button(
                    "review-model",
                    self.review_model.clone(),
                    Some(t!("connection.follow_default").into()),
                    models.clone(),
                    move |model, cx| {
                        review_view
                            .update(cx, |view, cx| {
                                view.review_model = model;
                                view.dirty = true;
                                cx.notify();
                            })
                            .ok();
                    },
                )),
        )
        .child(
            Field::new()
                .label(tr!("connection.title_model"))
                .child(choice_button(
                    "title-model",
                    self.title_model.clone(),
                    Some(t!("connection.follow_default").into()),
                    models,
                    move |model, cx| {
                        title_view
                            .update(cx, |view, cx| {
                                view.title_model = model;
                                view.dirty = true;
                                cx.notify();
                            })
                            .ok();
                    },
                )),
        )
    }
}

impl ConnectionView {
    fn render_management(
        &self,
        provider: &Provider,
        preset: Option<&ProviderPreset>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy = is_busy(cx);
        let id = provider.id.clone();
        let enabled = provider.enabled;
        let toggle_id = id.clone();
        let reauthorize = preset
            .filter(|preset| {
                preset.browser_oauth && (preset.id == "openai" || !provider.api_key_configured)
            })
            .map(|preset| {
                let preset_id = preset.id.clone();
                let id = id.clone();
                Button::new("reauthorize")
                    .secondary()
                    .small()
                    .label(tr!("connection.reauthorize"))
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        let args = [
                            "providers",
                            "add",
                            "--preset",
                            preset_id.as_str(),
                            "--id",
                            id.as_str(),
                            "--reauthorize",
                        ];
                        run_change(
                            Change::new(t!("connection.reauthorize"), args).applied(),
                            cx,
                        );
                    })
            });
        let clear_flag = if provider.preset_id.as_deref() == Some("aws-bedrock") {
            "--clear-aws-credentials"
        } else {
            "--clear-key"
        };
        let clear_id = id.clone();
        let remove_id = id.clone();
        let name = provider.display_name.clone();
        let body = v_flex()
            .child(setting_row(
                t!("connection.enabled"),
                Some(t!("connection.enabled_detail").into()),
                Switch::new("provider-enabled")
                    .checked(enabled)
                    .disabled(busy)
                    .on_change(move |enable, _, cx| {
                        let command = if *enable { "enable" } else { "disable" };
                        let label = if *enable {
                            t!("connection.enable_label")
                        } else {
                            t!("connection.disable_label")
                        };
                        run_change(
                            Change::new(label, ["provider", command, toggle_id.as_str()]).applied(),
                            cx,
                        );
                    }),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("connection.credentials"),
                Some(t!("connection.credentials_detail").into()),
                h_flex().gap_2().children(reauthorize).child(
                    Button::new("clear-key")
                        .secondary()
                        .small()
                        .label(tr!("connection.clear_key"))
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            run_change(
                                Change::new(
                                    t!("connection.clear_key"),
                                    ["provider", "update", clear_id.as_str(), clear_flag],
                                )
                                .applied(),
                                cx,
                            );
                        }),
                ),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("connection.remove"),
                Some(t!("connection.remove_detail").into()),
                Button::new("remove-provider")
                    .danger()
                    .small()
                    .label(tr!("connection.remove"))
                    .disabled(busy)
                    .on_click(move |_, window, cx| {
                        let id = remove_id.clone();
                        let name = name.clone();
                        window.open_alert_dialog(cx, move |dialog, _, _| {
                            let id = id.clone();
                            dialog
                                .title(SharedString::from(t!(
                                    "connection.remove_title",
                                    name = name
                                )))
                                .description(SharedString::from(t!("connection.remove_confirm")))
                                .confirm()
                                .ok_text(t!("connection.remove"))
                                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                                .cancel_text(t!("common.cancel"))
                                .on_ok(move |_, _, cx| {
                                    run_change(
                                        Change::new(
                                            t!("connection.remove_label"),
                                            ["provider", "remove", id.as_str()],
                                        )
                                        .applied(),
                                        cx,
                                    );
                                    true
                                })
                        });
                    }),
                cx,
            ));
        section(t!("connection.manage"), None, body, cx)
    }

    fn render_official(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx).read(cx);
        let ech = store
            .service
            .value
            .as_ref()
            .and_then(|status| status.official_ech.clone())
            .unwrap_or_default();
        let busy = is_busy(cx);
        let body = v_flex()
            .child(setting_row(
                t!("connection.official_auth"),
                Some(t!("connection.official_auth_detail").into()),
                div(),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("connection.ech"),
                Some(
                    ech.fallback_reason
                        .clone()
                        .map_or_else(|| t!("connection.ech_detail").into(), SharedString::from),
                ),
                Switch::new("official-ech")
                    .checked(ech.enabled)
                    .disabled(busy)
                    .on_change(|enable, _, cx| {
                        let command = if *enable { "enable" } else { "disable" };
                        run_change(Change::new(t!("connection.ech"), ["ech", command]), cx);
                    }),
                cx,
            ));
        section(t!("connection.official"), None, body, cx)
    }

    fn render_save_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        h_flex()
            .absolute()
            .bottom_6()
            .left_0()
            .right_0()
            .justify_center()
            .child(
                h_flex()
                    .gap_3()
                    .pl_4()
                    .pr_2()
                    .py_2()
                    .rounded_xl()
                    .bg(colors.popover)
                    .border_1()
                    .border_color(colors.border)
                    .shadow_lg()
                    .child(div().text_sm().child(t!("connection.unsaved")))
                    .child(
                        Button::new("save-connection")
                            .primary()
                            .small()
                            .label(tr!("common.apply"))
                            .loading(is_busy(cx))
                            .disabled(is_busy(cx))
                            .on_click(cx.listener(|view, _, _, cx| view.save(cx).detach())),
                    ),
            )
            .with_animation(
                "connection-save-bar",
                gpui_kit::Animation::new(crate::theme::MOVE_DURATION)
                    .with_easing(crate::theme::curve(crate::theme::MOVE_CURVE)),
                |bar, progress| bar.opacity(progress).mb(px(-16.0 * (1.0 - progress))),
            )
    }
}

impl Render for ConnectionView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let preset = self.preset_definition(cx);
        let creating = self.provider.is_none();
        let official = self.provider.as_ref().is_some_and(Provider::is_official);
        if creating {
            let colors = cx.theme().colors;
            let advanced = self.advanced;
            return h_flex()
                .h_full()
                .w_full()
                .items_start()
                .gap_4()
                .child(self.render_preset_picker(cx))
                .when_some(preset, |layout, preset| {
                    let show_advanced = preset.auth_mode != "local_oauth" || self.shows_id(&preset);
                    layout.child(
                        v_flex()
                            .id("new-provider-fields")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .gap_4()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(colors.border)
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_start()
                                    .child(provider_logo(&preset.icon, true, cx))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_base()
                                                    .font_semibold()
                                                    .child(preset.display_name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(colors.muted_foreground)
                                                    .child(preset.description.clone()),
                                            ),
                                    ),
                            )
                            .child(row_divider(cx))
                            .child(self.render_fields(&preset, cx))
                            .when(show_advanced, |pane| {
                                pane.child(
                                    Button::new("provider-advanced")
                                        .ghost()
                                        .small()
                                        .icon(if advanced {
                                            IconName::ChevronUp
                                        } else {
                                            IconName::ChevronDown
                                        })
                                        .label(tr!("connection.advanced"))
                                        .on_click(cx.listener(|view, _, _, cx| {
                                            view.advanced = !view.advanced;
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                })
                .into_any_element();
        }
        let mut body = v_flex().gap_6().w_full().max_w(gpui_kit::rems(46.0));
        if official {
            body = body.child(self.render_official(cx));
        } else if let Some(preset) = &preset {
            let form = self.render_fields(preset, cx);
            body = body.child(section(
                t!("connection.settings"),
                Some(tr!("connection.edit_hint")),
                div().p_4().child(form),
                cx,
            ));
        }
        if let Some(provider) = self
            .provider
            .clone()
            .filter(|provider| !provider.is_official())
        {
            body = body.child(self.render_management(&provider, preset.as_ref(), cx));
        }
        let content = div()
            .id("connection-scroll")
            .size_full()
            .overflow_y_scroll()
            .px_6()
            .pt_2()
            .pb_24()
            .child(body);
        div()
            .relative()
            .size_full()
            .child(content)
            .when(self.dirty, |view| view.child(self.render_save_bar(cx)))
            .into_any_element()
    }
}

/// Opens the "Add provider" dialog over the current window.
pub fn open_add_provider(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| ConnectionView::new(None, window, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let save_view = view.clone();
        let ready = view.read(cx).preset_definition(cx).is_some() && !is_busy(cx);
        dialog
            .title(SharedString::from(t!("connection.add_title")))
            .w(px(730.0))
            .child(div().h(px(370.0)).child(view.clone()))
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-add-provider")
                            .ghost()
                            .label(tr!("common.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("confirm-add-provider")
                            .primary()
                            .disabled(!ready)
                            .icon(IconName::Plus)
                            .label(tr!("connection.add_confirm"))
                            .on_click(move |_, window, cx| {
                                // The dialog stays open on failure so nothing typed is lost.
                                let saved = save_view.update(cx, |view, cx| view.save(cx));
                                window
                                    .spawn(cx, async move |cx| {
                                        if saved.await {
                                            cx.update(|window, cx| window.close_dialog(cx)).ok();
                                        }
                                    })
                                    .detach();
                            }),
                    ),
            )
    });
}
