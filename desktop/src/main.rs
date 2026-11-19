//! Codex Mixin desktop shell: a tray (menu bar) app with a status panel and
//! a console window, built on GPUI for macOS and Windows alike.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod assets;
mod cli;
mod console;
mod model;
mod pages;
mod panel;
mod store;
mod theme;
mod tray;
mod widgets;

use gpui_kit::{ActivationPolicy, QuitMode};

rust_i18n::i18n!("locales", fallback = "en");

/// Translates a key into a `SharedString` for GPUI builders.
macro_rules! tr {
    ($($arg:tt)*) => {
        gpui_kit::SharedString::from(rust_i18n::t!($($arg)*))
    };
}
pub(crate) use tr;

fn main() {
    let locale = sys_locale::get_locale().unwrap_or_default();
    rust_i18n::set_locale(if locale.starts_with("zh") {
        "zh-CN"
    } else {
        "en"
    });

    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            if let Err(error) = theme::init(cx) {
                eprintln!("cannot load the product theme: {error:#}");
            }
            // A tray app stays alive with every window closed and stays out
            // of the Dock.
            cx.set_quit_mode(QuitMode::Explicit);
            cx.set_activation_policy(ActivationPolicy::Accessory);
            let cli = match cli::Cli::locate() {
                Ok(cli) => cli,
                Err(error) => {
                    eprintln!("{error:#}");
                    cx.quit();
                    return;
                }
            };
            store::Store::init(cli, cx);
            tray::install(cx);
            if std::env::args().all(|arg| arg != "--background") {
                console::Console::open(None, cx);
            }
        });
}
