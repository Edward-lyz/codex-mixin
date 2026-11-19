//! The menu bar (macOS), notification area (Windows) or StatusNotifierItem
//! (Linux) icon. A left click toggles the status panel; the context menu
//! offers the console and quit.

use gpui_kit::{App, Global, Pixels, Point, point, px};
use tray_icon::dpi::PhysicalPosition;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::console::Console;
use crate::panel;
use crate::store::Store;

#[cfg(target_os = "macos")]
const ICON_PIXELS: u32 = 44;
#[cfg(not(target_os = "macos"))]
const ICON_PIXELS: u32 = 32;

const OPEN_CONSOLE: &str = "open-console";
const QUIT: &str = "quit";

struct Tray {
    icon: TrayIcon,
    running: Option<bool>,
}

impl Global for Tray {}

enum TrayInput {
    Clicked(tray_icon::Rect, PhysicalPosition<f64>),
    Menu(String),
}

pub fn install(cx: &mut App) {
    let menu = Menu::new();
    let open = MenuItem::with_id(OPEN_CONSOLE, rust_i18n::t!("tray.open_console"), true, None);
    let quit = MenuItem::with_id(QUIT, rust_i18n::t!("tray.quit"), true, None);
    let built = menu
        .append_items(&[&open, &PredefinedMenuItem::separator(), &quit])
        .map_err(anyhow::Error::from)
        .and_then(|_| {
            TrayIconBuilder::new()
                .with_icon(render_icon(false)?)
                .with_tooltip("Codex Mixin")
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()
                .map_err(anyhow::Error::from)
        });
    let icon = match built {
        Ok(icon) => icon,
        Err(error) => {
            eprintln!("cannot create the tray icon: {error:#}");
            return;
        }
    };
    cx.set_global(Tray {
        icon,
        running: None,
    });

    // Tray callbacks arrive on the platform thread; forward them to GPUI's
    // foreground executor through a channel.
    let (sender, receiver) = smol::channel::unbounded::<TrayInput>();
    let click_sender = sender.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click {
            rect,
            position,
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = click_sender.try_send(TrayInput::Clicked(rect, position));
        }
    }));
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = sender.try_send(TrayInput::Menu(event.id.0));
    }));
    cx.spawn(async move |cx| {
        while let Ok(input) = receiver.recv().await {
            cx.update(|cx| match input {
                TrayInput::Clicked(rect, position) => panel::toggle(anchor(rect, position, cx), cx),
                TrayInput::Menu(id) if id == OPEN_CONSOLE => Console::open(None, cx),
                TrayInput::Menu(id) if id == QUIT => cx.quit(),
                TrayInput::Menu(_) => {}
            });
        }
    })
    .detach();

    let store = Store::global(cx);
    cx.observe(&store, |store, cx| {
        let running = store
            .read(cx)
            .service
            .value
            .as_ref()
            .map(|status| status.is_running());
        update_running(running.unwrap_or(false), cx);
    })
    .detach();
}

fn update_running(running: bool, cx: &mut App) {
    let Some(tray) = cx.try_global::<Tray>() else {
        return;
    };
    if tray.running == Some(running) {
        return;
    }
    let tooltip = if running {
        rust_i18n::t!("tray.tooltip_running")
    } else {
        rust_i18n::t!("tray.tooltip_stopped")
    };
    if let Ok(icon) = render_icon(running) {
        let _ = tray.icon.set_icon(Some(icon));
    }
    let _ = tray.icon.set_tooltip(Some(tooltip));
    cx.global_mut::<Tray>().running = Some(running);
}

fn render_icon(running: bool) -> anyhow::Result<Icon> {
    let path = if running {
        "tray/running.svg"
    } else {
        "tray/stopped.svg"
    };
    let source = crate::assets::product_bytes(path)
        .ok_or_else(|| anyhow::anyhow!("missing tray artwork {path}"))?;
    let tree = resvg::usvg::Tree::from_data(&source, &resvg::usvg::Options::default())?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(ICON_PIXELS, ICON_PIXELS)
        .ok_or_else(|| anyhow::anyhow!("cannot allocate the tray icon"))?;
    let scale = ICON_PIXELS as f32 / tree.size().width();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    Ok(Icon::from_rgba(rgba, ICON_PIXELS, ICON_PIXELS)?)
}

/// Where the panel's top-center should sit, in GPUI's logical screen space.
#[cfg(target_os = "macos")]
fn anchor(rect: tray_icon::Rect, _: PhysicalPosition<f64>, _: &App) -> Option<Point<Pixels>> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;

    let marker = MainThreadMarker::new()?;
    let screens = NSScreen::screens(marker);
    let main_height = screens.firstObject()?.frame().size.height;
    // tray-icon reports physical pixels from a top-left origin; find the
    // screen whose backing scale maps the icon inside its frame.
    screens.iter().find_map(|screen| {
        let scale = screen.backingScaleFactor();
        let frame = screen.frame();
        let x = rect.position.x / scale;
        let width = f64::from(rect.size.width) / scale;
        let bottom = (rect.position.y + f64::from(rect.size.height)) / scale;
        let top_of_screen = main_height - (frame.origin.y + frame.size.height);
        let inside = x >= frame.origin.x && x < frame.origin.x + frame.size.width;
        inside.then(|| {
            point(
                px((x + width / 2.0) as f32),
                px(bottom.max(top_of_screen) as f32),
            )
        })
    })
}

/// Windows keeps the notification area beside the taskbar; the panel opens
/// above it at the work area's corner.
#[cfg(windows)]
fn anchor(_: tray_icon::Rect, _: PhysicalPosition<f64>, cx: &App) -> Option<Point<Pixels>> {
    let display = cx.primary_display()?;
    let bounds = display.visible_bounds();
    Some(point(
        bounds.right() - px(panel::PANEL_WIDTH / 2.0 + 12.0),
        bounds.bottom() - px(panel::PANEL_HEIGHT + 12.0),
    ))
}

/// Linux panels sit on any screen edge and StatusNotifierItem reports only
/// the click point: the panel opens below a click in the top half of the
/// screen and above one in the bottom half, kept inside the work area.
#[cfg(not(any(target_os = "macos", windows)))]
fn anchor(_: tray_icon::Rect, click: PhysicalPosition<f64>, cx: &App) -> Option<Point<Pixels>> {
    let bounds = cx.primary_display()?.visible_bounds();
    let half_width = px(panel::PANEL_WIDTH / 2.0 + 12.0);
    let x = px(click.x as f32)
        .min(bounds.right() - half_width)
        .max(bounds.left() + half_width);
    let y = if px(click.y as f32) < bounds.center().y {
        bounds.top() + px(12.0)
    } else {
        bounds.bottom() - px(panel::PANEL_HEIGHT + 12.0)
    };
    Some(point(x, y))
}
