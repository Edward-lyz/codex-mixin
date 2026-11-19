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
mod updater;
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
    if updater::run_helper_if_requested() {
        return;
    }
    updater::clean_up_after_update();
    #[cfg(target_os = "macos")]
    disable_touch_bar();
    let locale = sys_locale::get_locale().unwrap_or_default();
    rust_i18n::set_locale(if locale.starts_with("zh") {
        "zh-CN"
    } else {
        "en"
    });

    let application = gpui_kit::application().with_assets(assets::Assets);
    // Launching the app again from Finder or the Dock shows the console.
    application.on_reopen(|cx| console::Console::open(None, cx));
    application.run(|cx| {
        gpui_kit::init(cx);
        if let Err(error) = theme::init(cx) {
            eprintln!("cannot load the product theme: {error:#}");
        }
        // A tray app stays alive with every window closed and stays out
        // of the Dock until the console opens.
        cx.set_quit_mode(QuitMode::Explicit);
        cx.set_activation_policy(ActivationPolicy::Accessory);
        console::init(cx);
        let cli = match cli::Cli::locate() {
            Ok(cli) => cli,
            Err(error) => {
                eprintln!("{error:#}");
                cx.quit();
                return;
            }
        };
        store::Store::init(cli, cx);
        updater::Updater::init(cx);
        tray::install(cx);
        if std::env::args().all(|arg| arg != "--background") {
            console::Console::open(None, cx);
        }
    });
}

/// AppKit's Touch Bar finder keeps key-value observers on window views and
/// removes them one display cycle after a window closes. GPUI frees the view
/// in the same turn, so closing the status panel aborts the app with
/// `NSRangeException` (zed-industries/zed#64819). The app has no Touch Bar
/// items; turning the API off removes the finder and its observers.
#[cfg(target_os = "macos")]
fn disable_touch_bar() {
    objc2_foundation::NSUserDefaults::standardUserDefaults().setBool_forKey(
        false,
        objc2_foundation::ns_string!("NSFunctionBarAPIEnabled"),
    );
}
