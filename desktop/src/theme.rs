//! Product theme and motion. Colors live in `themes/codex-mixin.json`; the
//! app follows the system light/dark appearance and uses one family of
//! cubic Bezier curves for every transition.

use std::sync::LazyLock;
use std::time::Duration;

use gpui_kit::base::motion::{Easing, Transition};
use gpui_kit::component::{Theme, ThemeRegistry};
use gpui_kit::{App, Window};

const THEME_SOURCE: &str = include_str!("../themes/codex-mixin.json");
const LIGHT_THEME: &str = "Codex Mixin Light";
const DARK_THEME: &str = "Codex Mixin Dark";

/// Decelerating curve shared by sliding indicators and panel moves; close to
/// the system curve AppKit uses for segmented-control selection.
pub const MOVE_CURVE: (f32, f32, f32, f32) = (0.32, 0.72, 0.0, 1.0);
/// Entrance curve for content that appears: fast start, long settle.
pub const ENTER_CURVE: (f32, f32, f32, f32) = (0.16, 1.0, 0.3, 1.0);
/// Exit curve for content that leaves.
pub const EXIT_CURVE: (f32, f32, f32, f32) = (0.4, 0.0, 1.0, 1.0);

pub const MOVE_DURATION: Duration = Duration::from_millis(340);
pub const ENTER_DURATION: Duration = Duration::from_millis(260);

/// Whether windows sit on the system's frosted-glass material: an
/// `NSVisualEffectView` on macOS and acrylic on Windows. On Linux GPUI asks
/// Wayland compositors for blur, which KWin reliably provides; X11 and
/// compositors without blur would show the desktop unblurred, so they stay
/// opaque.
pub static FROSTED: LazyLock<bool> = LazyLock::new(|| {
    if cfg!(any(target_os = "macos", windows)) {
        return true;
    }
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.contains("KDE"))
});
/// Opacity of the sidebar tint over the frosted material.
pub const FROSTED_TINT: f32 = 0.72;

pub fn bezier(curve: (f32, f32, f32, f32)) -> Easing {
    let (x1, y1, x2, y2) = curve;
    Easing::cubic_bezier(x1, y1, x2, y2).unwrap_or(Easing::Linear)
}

/// The same curve as an easing function for GPUI element animations.
pub fn curve(points: (f32, f32, f32, f32)) -> impl Fn(f32) -> f32 {
    gpui_kit::base::animation::cubic_bezier(points.0, points.1, points.2, points.3)
}

/// Timing for values that slide between positions, such as tab indicators.
pub fn move_transition() -> Transition {
    Transition::new(MOVE_DURATION).easing(bezier(MOVE_CURVE))
}

pub fn init(cx: &mut App) -> anyhow::Result<()> {
    ThemeRegistry::global_mut(cx).load_themes_from_str(THEME_SOURCE)?;
    let registry = ThemeRegistry::global(cx).themes();
    let light = registry
        .get(LIGHT_THEME)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("theme {LIGHT_THEME} is missing"))?;
    let dark = registry
        .get(DARK_THEME)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("theme {DARK_THEME} is missing"))?;
    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
        theme.motion.easing_move = bezier(MOVE_CURVE);
        theme.motion.easing_enter = bezier(ENTER_CURVE);
        theme.motion.easing_exit = bezier(EXIT_CURVE);
        theme.motion.duration_normal = ENTER_DURATION;
        theme.motion.duration_slow = MOVE_DURATION;
    });
    Theme::sync_system_appearance(None, cx);
    Ok(())
}

/// Keeps a window's theme in step with the system appearance.
pub fn follow_system_appearance(window: &mut Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
    window
        .observe_window_appearance(|window, cx| Theme::sync_system_appearance(Some(window), cx))
        .detach();
}
