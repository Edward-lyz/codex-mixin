use std::collections::VecDeque;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, prelude::FluentBuilder as _, px,
};
use rust_i18n::t;
use serde_json::{Value, json};
use smol::channel::Sender;

use crate::cli::CliError;
use crate::model::{
    PluginCommand, PluginCommandNotification, PluginEvent, PluginEventResult, PluginPrompt,
    PluginPromptKind,
};
use crate::store::Store;
use crate::tr;

use super::is_busy;

pub(crate) struct PluginCommandRun {
    pub(super) name: String,
    notifications: Vec<PluginCommandNotification>,
    state: CommandRunState,
}

enum CommandRunState {
    Running,
    Completed,
    Cancelled,
    Failed(SharedString),
}

pub(crate) fn render_command_output(output: &PluginCommandRun, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let name = &output.name;
    let result = v_flex()
        .gap_1()
        .px_4()
        .py_3()
        .child(
            div()
                .font_semibold()
                .text_xs()
                .child(t!("plugins.command_output", name = name)),
        )
        .children(output.notifications.iter().map(|notification| {
            let color = match notification.kind.as_str() {
                "error" => colors.danger,
                "warning" => colors.warning,
                _ => colors.foreground,
            };
            div()
                .text_sm()
                .text_color(color)
                .child(notification.message.clone())
        }));
    let (status, color) = match &output.state {
        CommandRunState::Running => (tr!("plugins.operation_running"), colors.muted_foreground),
        CommandRunState::Cancelled => (tr!("plugins.operation_cancelled"), colors.muted_foreground),
        CommandRunState::Completed if output.notifications.is_empty() => {
            (tr!("plugins.command_no_output"), colors.muted_foreground)
        }
        CommandRunState::Completed => (tr!("plugins.command_completed"), colors.muted_foreground),
        CommandRunState::Failed(error) => (error.clone(), colors.danger),
    };
    result.child(div().text_sm().text_color(color).child(status))
}

enum PromptInput {
    Single {
        state: Entity<InputState>,
        _enter: Subscription,
    },
    Multi(Entity<TextareaState>),
}

#[derive(Default)]
struct PluginInteraction {
    replies: Option<Sender<Value>>,
    prompts: VecDeque<(String, PluginPrompt)>,
    input: Option<PromptInput>,
    status: Option<String>,
    auth_url: Option<(String, Option<String>)>,
    device_code: Option<(String, String)>,
    error: Option<SharedString>,
    dismissed: bool,
}

impl PluginInteraction {
    fn close(&mut self) {
        self.dismissed = true;
        if let Some(replies) = self.replies.take() {
            replies.close();
        }
        self.input = None;
        self.prompts.clear();
    }

    fn show_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, prompt)) = self.prompts.front() else {
            self.input = None;
            return;
        };
        let placeholder = prompt.placeholder.clone().unwrap_or_default();
        let initial = prompt.prefill.clone().unwrap_or_default();
        self.input = match prompt.kind {
            PluginPromptKind::Confirm | PluginPromptKind::Select => None,
            PluginPromptKind::Editor => {
                let state = cx.new(|cx| {
                    let mut state = TextareaState::new(window, cx)
                        .placeholder(placeholder)
                        .rows(5);
                    state.set_value(initial, window, cx);
                    state.focus(window, cx);
                    state
                });
                Some(PromptInput::Multi(state))
            }
            kind => {
                let state = cx.new(|cx| {
                    let mut state = InputState::new(window, cx)
                        .placeholder(placeholder)
                        .masked(kind == PluginPromptKind::Secret);
                    state.set_value(initial, window, cx);
                    state.focus(window, cx);
                    state
                });
                let id = id.clone();
                let enter =
                    cx.subscribe_in(&state, window, move |session, state, event, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            let value = state.read(cx).value().to_string();
                            session.answer(&id, Some(value), window, cx);
                        }
                    });
                Some(PromptInput::Single {
                    state,
                    _enter: enter,
                })
            }
        };
    }

    fn answer(
        &mut self,
        id: &str,
        value: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .prompts
            .front()
            .is_some_and(|(pending, _)| pending == id)
        {
            return;
        }
        let Some(replies) = &self.replies else { return };
        let reply = match value {
            Some(value) => json!({ "id": id, "value": value }),
            None => json!({ "id": id, "cancel": true }),
        };
        if replies.try_send(reply).is_err() {
            self.error = Some(tr!("plugins.reply_failed"));
        } else {
            self.error = None;
            self.prompts.pop_front();
            self.show_prompt(window, cx);
        }
        window.refresh();
    }

    fn handle_event(
        &mut self,
        event: PluginEvent,
        output: Option<&Entity<PluginCommandRun>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dismissed {
            return;
        }
        match event {
            PluginEvent::AuthUrl { url, instructions } => {
                if is_web_url(&url) {
                    cx.open_url(&url);
                }
                self.auth_url = Some((url, instructions));
            }
            PluginEvent::DeviceCode {
                user_code,
                verification_uri,
            } => {
                self.device_code = Some((user_code, verification_uri));
            }
            PluginEvent::Progress { message } => self.status = Some(message),
            PluginEvent::Notify { level, message } => {
                if let Some(output) = output {
                    output.update(cx, |output, _| {
                        output.notifications.push(PluginCommandNotification {
                            kind: level,
                            message: message.into(),
                        });
                    });
                } else {
                    self.status = Some(message);
                }
            }
            PluginEvent::Prompt { id, prompt } => {
                self.prompts.push_back((id, prompt));
                if self.prompts.len() == 1 {
                    self.show_prompt(window, cx);
                }
            }
            PluginEvent::PromptCancelled { id } => {
                let active = self
                    .prompts
                    .front()
                    .is_some_and(|(pending, _)| *pending == id);
                self.prompts.retain(|(pending, _)| *pending != id);
                if active {
                    self.show_prompt(window, cx);
                }
            }
            PluginEvent::Result { .. } => {}
        }
    }
}

pub(super) fn is_web_url(url: &str) -> bool {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with(['/', '\\']))
        && !url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn web_link(id: &'static str, url: &str) -> impl IntoElement {
    let target = url.to_owned();
    Button::new(id)
        .link()
        .small()
        .label(url.to_owned())
        .disabled(!is_web_url(url))
        .on_click(move |_, _, cx| cx.open_url(&target))
}

fn render_prompt(state: &Entity<PluginInteraction>, cx: &App) -> impl IntoElement {
    let session = state.read(cx);
    let Some((id, prompt)) = session.prompts.front() else {
        return v_flex();
    };
    let id = id.clone();
    let colors = cx.theme().colors;
    let body = v_flex().gap_2().child(
        div()
            .text_sm()
            .font_semibold()
            .child(prompt.message.clone()),
    );
    let answer = |button_id, label: SharedString, value: Option<String>, primary| {
        let state = state.clone();
        let id = id.clone();
        let button = Button::new(button_id).label(label);
        let button = if primary {
            button.primary()
        } else {
            button.secondary()
        };
        button.on_click(move |_, window, cx| {
            state.update(cx, |session, cx| {
                session.answer(&id, value.clone(), window, cx)
            });
        })
    };
    match prompt.kind {
        PluginPromptKind::Confirm => body.child(
            h_flex()
                .gap_2()
                .child(answer(
                    "plugin-prompt-no",
                    tr!("common.cancel"),
                    Some("false".into()),
                    false,
                ))
                .child(answer(
                    "plugin-prompt-yes",
                    tr!("plugins.confirm"),
                    Some("true".into()),
                    true,
                )),
        ),
        PluginPromptKind::Select => body
            .children(prompt.options.iter().enumerate().map(|(index, option)| {
                let state = state.clone();
                let id = id.clone();
                let value = option.id.clone();
                v_flex()
                    .gap_1()
                    .child(
                        Button::new(("plugin-prompt-option", index))
                            .secondary()
                            .label(option.label.clone())
                            .on_click(move |_, window, cx| {
                                state.update(cx, |session, cx| {
                                    session.answer(&id, Some(value.clone()), window, cx);
                                });
                            }),
                    )
                    .when_some(option.description.clone(), |row, description| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .child(description),
                        )
                    })
            }))
            .child(answer(
                "plugin-prompt-cancel",
                tr!("common.cancel"),
                None,
                false,
            )),
        _ => {
            let Some(input) = &session.input else {
                return body;
            };
            let input = match input {
                PromptInput::Single { state, .. } => Input::new(state).into_any_element(),
                PromptInput::Multi(state) => Textarea::new(state).h(px(120.)).into_any_element(),
            };
            let state = state.clone();
            body.child(input).child(
                h_flex()
                    .gap_2()
                    .child(answer(
                        "plugin-prompt-cancel",
                        tr!("common.cancel"),
                        None,
                        false,
                    ))
                    .child(
                        Button::new("plugin-prompt-submit")
                            .primary()
                            .label(t!("plugins.submit"))
                            .on_click(move |_, window, cx| {
                                state.update(cx, |session, cx| {
                                    let value = match &session.input {
                                        Some(PromptInput::Single { state, .. }) => {
                                            state.read(cx).value()
                                        }
                                        Some(PromptInput::Multi(state)) => state.read(cx).value(),
                                        None => return,
                                    };
                                    session.answer(&id, Some(value.to_string()), window, cx);
                                });
                            }),
                    ),
            )
        }
    }
}

fn interaction_dialog(
    dialog: Dialog,
    state: &Entity<PluginInteraction>,
    output: Option<&Entity<PluginCommandRun>>,
    cx: &App,
) -> Dialog {
    let colors = cx.theme().colors;
    let session = state.read(cx);
    let output = output
        .map(|output| output.read(cx))
        .filter(|output| !output.notifications.is_empty());
    let status = session.status.clone().map(SharedString::from).or_else(|| {
        (session.replies.is_some() && output.is_none()).then(|| tr!("plugins.operation_running"))
    });
    let error = session.error.clone().filter(|_| {
        !output.is_some_and(|output| matches!(output.state, CommandRunState::Failed(_)))
    });
    let close = state.clone();
    let cancel = state.clone();
    dialog
        .w(px(520.))
        .overlay_closable(false)
        .on_close(move |_, _, cx| close.update(cx, |session, _| session.close()))
        .child(
            div()
                .id("plugin-interaction-scroll")
                .max_h(px(420.))
                .overflow_y_scroll()
                .child(
                    v_flex()
                        .gap_3()
                        .when_some(status, |body, status| {
                            body.child(div().text_sm().child(status))
                        })
                        .when_some(session.auth_url.as_ref(), |body, (url, instructions)| {
                            body.when_some(instructions.clone(), |body, instructions| {
                                body.child(div().text_sm().child(instructions))
                            })
                            .child(web_link("plugin-auth-url", url))
                        })
                        .when_some(session.device_code.as_ref(), |body, (code, url)| {
                            let code = code.clone();
                            body.child(web_link("plugin-device-url", url)).child(
                                h_flex()
                                    .gap_2()
                                    .child(div().font_semibold().child(code.clone()))
                                    .child(
                                        Button::new("plugin-copy-device-code")
                                            .ghost()
                                            .small()
                                            .label(t!("plugins.copy_code"))
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    code.clone(),
                                                ));
                                            }),
                                    ),
                            )
                        })
                        .when_some(error, |body, error| {
                            body.child(div().text_sm().text_color(colors.danger).child(error))
                        })
                        .child(render_prompt(state, cx))
                        .when_some(output, |body, output| {
                            body.child(render_command_output(output, cx))
                        }),
                ),
        )
        .footer(
            h_flex().justify_end().child(
                Button::new("plugin-interaction-close")
                    .ghost()
                    .label(if session.replies.is_some() {
                        t!("plugins.stop_operation")
                    } else {
                        t!("plugins.close")
                    })
                    .on_click(move |_, window, cx| {
                        cancel.update(cx, |session, _| session.close());
                        window.close_dialog(cx);
                    }),
            ),
        )
}

type RunChanged = Rc<dyn Fn(Option<Entity<PluginCommandRun>>, bool, &mut App)>;

fn start_interaction(
    state: &Entity<PluginInteraction>,
    output: Option<Entity<PluginCommandRun>>,
    label: SharedString,
    args: Vec<String>,
    on_change: RunChanged,
    window: &mut Window,
    cx: &mut App,
) {
    let (replies, events, task) =
        Store::global(cx).update(cx, |store, cx| store.run_plugin_events(label, args, cx));
    state.update(cx, |session, _| session.replies = Some(replies));
    window.refresh();
    let state = state.downgrade();
    window
        .spawn(cx, async move |cx| {
            let _ = cx.update(|_, cx| on_change(output.clone(), false, cx));
            while let Ok(event) = events.recv().await {
                let _ = cx.update(|window, cx| {
                    if let Some(state) = state.upgrade() {
                        state.update(cx, |session, cx| {
                            session.handle_event(event, output.as_ref(), window, cx);
                        });
                        on_change(output.clone(), false, cx);
                        window.refresh();
                    }
                });
            }
            let result = task.await;
            let _ = cx.update(|window, cx| {
                let state = state.upgrade();
                let dismissed = state.as_ref().is_none_or(|state| state.read(cx).dismissed);
                let result = match (&output, result) {
                    (
                        None,
                        Ok(PluginEventResult::Login {
                            provider,
                            synced_models,
                        }),
                    ) => Ok(Some(tr!(
                        "plugins.login_done",
                        provider = provider,
                        count = synced_models
                    ))),
                    (
                        Some(output),
                        Ok(PluginEventResult::Command {
                            name,
                            notifications,
                        }),
                    ) => {
                        output.update(cx, |output, _| {
                            output.name = name;
                            output.notifications = notifications;
                        });
                        Ok(None)
                    }
                    (_, Err(error)) => Err(error),
                    (_, Ok(_)) => Err(CliError::new(t!("plugins.missing_result"))),
                };
                if let Some(output) = &output {
                    output.update(cx, |output, _| {
                        output.state = match &result {
                            Ok(_) => CommandRunState::Completed,
                            Err(error) if dismissed && error.committed != Some(true) => {
                                CommandRunState::Cancelled
                            }
                            Err(error) => CommandRunState::Failed(error.message.clone().into()),
                        };
                    });
                }
                on_change(output, true, cx);
                let Some(state) = state.filter(|_| !dismissed) else {
                    return;
                };
                state.update(cx, |session, _| {
                    session.replies.take();
                    session.input = None;
                    session.prompts.clear();
                });
                match result {
                    Ok(notice) => {
                        window.close_dialog(cx);
                        if let Some(notice) = notice {
                            window.push_notification(Notification::success(notice), cx);
                        }
                    }
                    Err(error) => state.update(cx, |session, _| {
                        session.error = Some(error.message.into());
                        session.status = None;
                    }),
                }
                window.refresh();
            });
        })
        .detach();
}

pub(crate) fn open_plugin_login_dialog(
    provider: String,
    on_finish: impl Fn(&mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    if is_busy(cx) {
        return;
    }
    let state = cx.new(|_| PluginInteraction::default());
    let title = tr!("plugins.login_title", provider = provider);
    let dialog_state = state.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        interaction_dialog(dialog.title(title.clone()), &dialog_state, None, cx)
    });
    start_interaction(
        &state,
        None,
        t!("plugins.login").into(),
        vec![
            "plugin".into(),
            "auth".into(),
            "login".into(),
            format!("--provider={provider}"),
        ],
        Rc::new(move |_, finished, cx| {
            if finished {
                on_finish(cx);
            }
        }),
        window,
        cx,
    );
}

pub(crate) fn open_plugin_command_dialog(
    command: &PluginCommand,
    quick_args: Option<String>,
    on_change: impl Fn(Option<Entity<PluginCommandRun>>, bool, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    if is_busy(cx) {
        return;
    }
    let args = cx.new(|cx| InputState::new(window, cx).placeholder(t!("plugins.arguments")));
    let name = command.name.clone();
    let description = command.description.clone();
    let arguments = command.arguments.clone();
    let title = tr!("plugins.run_command_title", name = name);
    let state = cx.new(|_| PluginInteraction::default());
    let output = cx.new(|_| PluginCommandRun {
        name: name.clone(),
        notifications: Vec::new(),
        state: CommandRunState::Running,
    });
    let start = {
        let state = state.clone();
        let output = output.clone();
        let on_change: RunChanged = Rc::new(on_change);
        Rc::new(move |args: String, window: &mut Window, cx: &mut App| {
            if !is_busy(cx) {
                start_interaction(
                    &state,
                    Some(output.clone()),
                    t!("plugins.run").into(),
                    vec![
                        "plugin".into(),
                        "run-command".into(),
                        format!("--name={name}"),
                        format!("--args={args}"),
                    ],
                    on_change.clone(),
                    window,
                    cx,
                );
            }
        })
    };
    let start_quick = start.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let dialog = dialog.title(title.clone());
        let session = state.read(cx);
        if session.replies.is_some() || session.error.is_some() || session.dismissed {
            return interaction_dialog(dialog, &state, Some(&output), cx);
        }
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
                    fill_args.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                })
        });
        let run_args = args.clone();
        let start = start.clone();
        dialog
            .w(px(520.))
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
                                start(run_args.read(cx).value().to_string(), window, cx);
                            }),
                    ),
            )
    });
    if let Some(args) = quick_args {
        start_quick(args, window, cx);
    }
}
