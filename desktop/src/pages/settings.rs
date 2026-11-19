//! Settings page: gateway service, official ECH proxy, configuration
//! backup, diagnostics, updates and about.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, Context, Entity, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Subscription, Window, div, img, prelude::FluentBuilder as _,
};
use rust_i18n::t;
use serde::Deserialize;

use super::{is_busy, run_change};
use crate::store::{Change, Store, query};
use crate::tr;
use crate::widgets::{row_divider, section, setting_row};

const RELEASES_URL: &str = "https://github.com/Edward-lyz/codex-mixin/releases/latest";
const REPOSITORY_URL: &str = "https://github.com/Edward-lyz/codex-mixin";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const WALLPAPERS: [&str; 3] = [
    "nasa-wallpapers/over-the-horizon.png",
    "nasa-wallpapers/crew-12-to-orbit.png",
    "nasa-wallpapers/waning-crescent-moon.png",
];

#[derive(Clone, Debug, Default, Deserialize)]
struct Autostart {
    enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct UpdateCheck {
    current_version: String,
    latest_version: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Doctor {
    ok: bool,
    checks: Vec<DoctorCheck>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct DoctorCheck {
    id: String,
    name: String,
    status: String,
    message: String,
}

pub struct SettingsPage {
    autostart: Option<bool>,
    update: Option<Result<UpdateCheck, SharedString>>,
    checking_update: bool,
    doctor: Option<Result<Doctor, SharedString>>,
    running_doctor: bool,
    wallpaper: usize,
    revision: u64,
    _subscriptions: Vec<Subscription>,
}

/// Compares dotted numeric versions; non-numeric parts compare as zero.
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |version: &str| -> Vec<u64> {
        version
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    parse(latest) > parse(current)
}

impl SettingsPage {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let subscription = cx.observe(&store, |page, store: Entity<Store>, cx| {
            let revision = store.read(cx).revision;
            if revision != page.revision {
                page.revision = revision;
                page.reload(cx);
            }
        });
        let mut page = Self {
            autostart: None,
            update: None,
            checking_update: false,
            doctor: None,
            running_doctor: false,
            wallpaper: chrono::Local::now().timestamp().unsigned_abs() as usize % WALLPAPERS.len(),
            revision: store.read(cx).revision,
            _subscriptions: vec![subscription],
        };
        page.reload(cx);
        page
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let task = query::<Autostart>(
            vec![
                "service".into(),
                "autostart".into(),
                "status".into(),
                "--json".into(),
            ],
            cx,
        );
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                page.autostart = result.ok().map(|status| status.enabled);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn check_update(&mut self, cx: &mut Context<Self>) {
        self.checking_update = true;
        cx.notify();
        let task =
            query::<UpdateCheck>(vec!["update".into(), "--check".into(), "--json".into()], cx);
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                page.checking_update = false;
                page.update = Some(result.map_err(|error| error.message.into()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_doctor(&mut self, cx: &mut Context<Self>) {
        self.running_doctor = true;
        cx.notify();
        let task = query::<Doctor>(vec!["doctor".into(), "--json".into()], cx);
        cx.spawn(async move |page, cx| {
            let result = task.await;
            page.update(cx, |page, cx| {
                page.running_doctor = false;
                page.doctor = Some(result.map_err(|error| error.message.into()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn export_backup(cx: &mut App) {
        let directory = dirs_home();
        let receiver = cx.prompt_for_new_path(&directory, Some("codex-mixin-backup.txt"));
        cx.spawn(async move |cx| {
            let Ok(Ok(Some(path))) = receiver.await else {
                return;
            };
            let path = path.to_string_lossy().into_owned();
            cx.update(|cx| {
                run_change(
                    Change::new(
                        t!("settings.export_label"),
                        ["config", "export", path.as_str()],
                    ),
                    cx,
                )
            });
        })
        .detach();
    }

    fn import_backup(cx: &mut App) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr!("settings.import_prompt")),
        });
        cx.spawn(async move |cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths
                .first()
                .map(|path| path.to_string_lossy().into_owned())
            else {
                return;
            };
            cx.update(|cx| {
                run_change(
                    Change::new(
                        t!("settings.import_label"),
                        ["config", "import", path.as_str()],
                    )
                    .applied(),
                    cx,
                )
            });
        })
        .detach();
    }
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}

impl SettingsPage {
    fn render_gateway(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx).read(cx);
        let status = store.service.value.clone();
        let running = status.as_ref().is_some_and(|status| status.is_running());
        let configured = status.as_ref().is_some_and(|status| status.configured);
        let endpoint = status.as_ref().and_then(|status| status.endpoint.clone());
        let version = status
            .as_ref()
            .and_then(|status| status.gateway_version.clone());
        let busy = is_busy(cx);
        let detail: SharedString = match (running, endpoint, version) {
            (true, Some(endpoint), Some(version)) => format!("{endpoint} · v{version}").into(),
            (true, Some(endpoint), None) => endpoint.into(),
            _ => tr!("panel.stopped_detail"),
        };
        let service = |command: &'static str, label: SharedString| {
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                run_change(
                    Change::new(label.clone(), ["service", command, "--managed", "--json"]),
                    cx,
                );
            }
        };
        let body = v_flex()
            .child(setting_row(
                if running {
                    t!("panel.running")
                } else {
                    t!("panel.stopped")
                },
                Some(detail),
                h_flex()
                    .gap_2()
                    .when(!running, |row| {
                        row.child(
                            Button::new("start-gateway")
                                .primary()
                                .small()
                                .label(t!("settings.start"))
                                .disabled(busy || !configured)
                                .on_click(service("start", tr!("panel.start_label"))),
                        )
                    })
                    .when(running, |row| {
                        row.child(
                            Button::new("restart-gateway")
                                .outline()
                                .small()
                                .label(t!("settings.restart"))
                                .disabled(busy)
                                .on_click(service("restart", tr!("settings.restart_label"))),
                        )
                        .child(
                            Button::new("stop-gateway")
                                .outline()
                                .small()
                                .label(t!("settings.stop"))
                                .disabled(busy)
                                .on_click(service("stop", tr!("panel.stop_label"))),
                        )
                    }),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("settings.autostart"),
                Some(tr!("settings.autostart_detail")),
                Switch::new("gateway-autostart")
                    .checked(self.autostart.unwrap_or(false))
                    .disabled(busy || self.autostart.is_none())
                    .on_change(|enable, _, cx| {
                        let command = if *enable { "enable" } else { "disable" };
                        run_change(
                            Change::new(
                                t!("settings.autostart"),
                                ["service", "autostart", command, "--json"],
                            ),
                            cx,
                        );
                    }),
                cx,
            ));
        section(t!("settings.gateway"), None, body, cx)
    }

    fn render_backup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = is_busy(cx);
        let body = v_flex()
            .child(setting_row(
                t!("settings.export"),
                Some(tr!("settings.export_detail")),
                Button::new("export-backup")
                    .outline()
                    .small()
                    .icon(IconName::Upload)
                    .label(t!("settings.export_button"))
                    .disabled(busy)
                    .on_click(|_, _, cx| Self::export_backup(cx)),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("settings.import"),
                Some(tr!("settings.import_detail")),
                Button::new("import-backup")
                    .outline()
                    .small()
                    .icon(IconName::Download)
                    .label(t!("settings.import_button"))
                    .disabled(busy)
                    .on_click(|_, _, cx| Self::import_backup(cx)),
                cx,
            ));
        section(
            t!("settings.backup"),
            Some(tr!("settings.backup_detail")),
            body,
            cx,
        )
    }

    fn render_diagnostics(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let results = match &self.doctor {
            None => None,
            Some(Err(error)) => Some(
                div()
                    .px_4()
                    .pb_3()
                    .text_xs()
                    .text_color(colors.danger)
                    .child(error.clone())
                    .into_any_element(),
            ),
            Some(Ok(doctor)) => Some(
                v_flex()
                    .px_4()
                    .pb_3()
                    .gap_1p5()
                    .children(doctor.checks.iter().map(|check| {
                        let (icon, color) = match check.status.as_str() {
                            "ok" => (IconName::CircleCheck, colors.success),
                            "warning" | "warn" => (IconName::TriangleAlert, colors.warning),
                            _ => (IconName::CircleX, colors.danger),
                        };
                        h_flex()
                            .id(SharedString::from(format!("doctor-{}", check.id)))
                            .gap_2()
                            .text_xs()
                            .child(Icon::new(icon).xsmall().text_color(color))
                            .child(
                                div()
                                    .w_40()
                                    .flex_none()
                                    .font_medium()
                                    .child(check.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(colors.muted_foreground)
                                    .child(check.message.clone()),
                            )
                    }))
                    .into_any_element(),
            ),
        };
        let summary: SharedString = match &self.doctor {
            Some(Ok(doctor)) if doctor.ok => tr!("settings.doctor_ok"),
            Some(Ok(_)) => tr!("settings.doctor_issues"),
            _ => tr!("settings.doctor_detail"),
        };
        let body = v_flex()
            .child(setting_row(
                t!("settings.doctor"),
                Some(summary),
                Button::new("run-doctor")
                    .outline()
                    .small()
                    .icon(IconName::Activity)
                    .label(t!("settings.doctor_run"))
                    .loading(self.running_doctor)
                    .on_click(cx.listener(|page, _, _, cx| page.run_doctor(cx))),
                cx,
            ))
            .children(results);
        section(t!("settings.diagnostics"), None, body, cx)
    }

    fn render_about(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let update_line: SharedString = match &self.update {
            None => tr!("settings.version", version = APP_VERSION),
            Some(Err(error)) => error.clone(),
            Some(Ok(check)) if is_newer(&check.latest_version, &check.current_version) => {
                tr!("settings.update_available", version = check.latest_version)
            }
            Some(Ok(check)) => tr!("settings.up_to_date", version = check.current_version),
        };
        let newer = matches!(&self.update, Some(Ok(check)) if is_newer(&check.latest_version, &check.current_version));
        div()
            .relative()
            .h_48()
            .rounded_xl()
            .overflow_hidden()
            .border_1()
            .border_color(colors.border)
            .child(
                img(WALLPAPERS[self.wallpaper])
                    .absolute()
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
            .child(
                v_flex()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .p_4()
                    .gap_3()
                    .bg(gpui_kit::black().opacity(0.45))
                    .text_color(gpui_kit::white())
                    .child(
                        h_flex()
                            .gap_3()
                            .child(img("app/app-icon.png").size_10())
                            .child(
                                v_flex()
                                    .flex_1()
                                    .child(div().text_lg().font_semibold().child("Codex Mixin"))
                                    .child(div().text_xs().opacity(0.85).child(update_line)),
                            )
                            .child(
                                Button::new("check-update")
                                    .small()
                                    .label(if newer {
                                        t!("settings.download")
                                    } else {
                                        t!("settings.check_update")
                                    })
                                    .loading(self.checking_update)
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        if newer {
                                            cx.open_url(RELEASES_URL);
                                        } else {
                                            page.check_update(cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("open-repository")
                                    .ghost()
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .tooltip(t!("settings.repository"))
                                    .on_click(|_, _, cx| cx.open_url(REPOSITORY_URL)),
                            ),
                    ),
            )
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .mx_auto()
                    .w_full()
                    .max_w(gpui_kit::rems(52.0))
                    .px_6()
                    .py_5()
                    .gap_6()
                    .child(self.render_about(cx))
                    .child(self.render_gateway(cx))
                    .child(self.render_backup(cx))
                    .child(self.render_diagnostics(cx)),
            )
    }
}
