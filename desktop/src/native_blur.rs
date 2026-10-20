//! Native window background blur, shared facade.
//!
//! Each desktop platform has its own mechanism:
//!
//! - **Linux/Wayland (GNOME 51+)**: Mutter's staging
//!   `ext-background-effect-v1` protocol, implemented in
//!   [`crate::linux_wayland_blur`].
//! - **Windows**: DWM backdrop via `window-vibrancy` (Mica on Windows 11,
//!   the legacy blur-behind effect on Windows 10), implemented in
//!   [`crate::windows_vibrancy`].
//! - **macOS**: `NSVisualEffectView` vibrancy behind the transparent webview,
//!   implemented in [`crate::macos_vibrancy`].
//!
//! The UI only consumes [`is_blur_active`] and the alpha helpers: once the
//! platform effect is active, the `native-blur` CSS class switches the shell
//! to translucent surfaces. Everywhere else the proven opaque theme stays,
//! so a failed/unsupported effect is invisible rather than broken.
//!
//! No platform exposes the blur radius to clients; the compositor/OS owns it.
//! The "strength" setting therefore controls how much of the blurred backdrop
//! shows through the frosted layer.

use std::sync::Arc;

use dioxus::desktop::tao::window::Window;

/// True once the current platform's native blur effect was installed.
pub fn is_blur_active() -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::linux_wayland_blur::is_blur_active()
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos_vibrancy::is_blur_active()
    }
    #[cfg(target_os = "windows")]
    {
        crate::windows_vibrancy::is_blur_active()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Install the platform blur effect if it is enabled in settings and has not
/// been installed yet. Safe to call repeatedly (e.g. when the user toggles the
/// setting back on).
pub fn enable_blur(window: &Arc<Window>) {
    #[cfg(target_os = "linux")]
    crate::linux_wayland_blur::enable_blur_async(window.clone());
    #[cfg(target_os = "macos")]
    crate::macos_vibrancy::enable_blur(window);
    #[cfg(target_os = "windows")]
    crate::windows_vibrancy::enable_blur(window);
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let _ = window;
}

/// Called once from the window-creation hook.
///
/// `visible` is false for `--autostart`, which creates the window hidden until
/// the user opens it from the tray. Linux needs that information because its
/// `wl_surface` does not exist while hidden; Windows installs its DWM backdrop
/// here. macOS waits for [`on_webview_ready`].
pub fn on_window_created(window: &Arc<Window>, visible: bool) {
    #[cfg(target_os = "linux")]
    {
        crate::linux_wayland_blur::set_window_visible(visible);
        crate::linux_wayland_blur::enable_blur_async(window.clone());
    }
    #[cfg(target_os = "windows")]
    {
        let _ = visible;
        crate::windows_vibrancy::enable_blur(window);
    }
    #[cfg(target_os = "macos")]
    let _ = (window, visible);
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let _ = (window, visible);
}

/// Called on macOS from the first event-loop tick after initialization.
///
/// wry replaces the `NSWindow`'s `contentView` while building the webview, so
/// an `NSVisualEffectView` added in the window hook would be discarded. The
/// first post-init event is the earliest main-thread point where the webview
/// (and its parent view) exists.
#[cfg(target_os = "macos")]
pub fn on_webview_ready(window: &Arc<Window>) {
    crate::macos_vibrancy::enable_blur(window);
}

/// Called whenever the tray/wakeup path shows the window. Linux re-arms the
/// installer (autostart windows were hidden when it first started); the DWM
/// and AppKit effects persist across hide/show.
pub fn on_window_shown(window: &Arc<Window>) {
    #[cfg(target_os = "linux")]
    crate::linux_wayland_blur::set_window_visible(true);
    enable_blur(window);
}

/// Called when the window is hidden to the tray.
pub fn on_window_hidden() {
    #[cfg(target_os = "linux")]
    crate::linux_wayland_blur::set_window_visible(false);
}

/// Maps the 0-100 blur-strength setting to the frosted-glass base alpha.
///
/// Higher strength = more transparent glass = more of the blurred backdrop
/// visible. `40` reproduces the original hardcoded `0.72` alpha.
pub fn blur_alpha(strength: u8) -> f32 {
    0.90 - 0.0045 * f32::from(strength.min(100))
}

/// Alpha for the lighter surfaces (sidebar/titlebar) that sit on the base.
pub fn blur_alpha_soft(strength: u8) -> f32 {
    (blur_alpha(strength) * (0.55 / 0.72)).clamp(0.15, 0.95)
}

#[cfg(test)]
mod tests {
    use super::{blur_alpha, blur_alpha_soft};

    #[test]
    fn default_strength_reproduces_previous_alphas() {
        assert!((blur_alpha(40) - 0.72).abs() < 1e-4);
        assert!((blur_alpha_soft(40) - 0.55).abs() < 1e-4);
    }

    #[test]
    fn strength_is_clamped_to_supported_range() {
        assert!((blur_alpha(0) - 0.90).abs() < 1e-4);
        assert!((blur_alpha(100) - 0.45).abs() < 1e-4);
        assert_eq!(blur_alpha(255), blur_alpha(100));
    }
}
