//! Native Windows desktop backdrop.
//!
//! Uses the DWM system backdrop **Acrylic** (`DWMSBT_TRANSIENTWINDOW`) on
//! Windows 11 22H2+ so the desktop behind the window is actually blurred.
//! Older Windows 10/11 releases fall back to the legacy blur-behind effect.
//!
//! Acrylic is deliberately preferred over Mica: Mica samples only the
//! wallpaper (it is a tinted, non-blurring material), so it looked like a flat
//! grey sheet instead of the frosted glass GNOME/macOS produce.
//!
//! On Windows 11 the window must use `WS_EX_NOREDIRECTIONBITMAP`; otherwise
//! tao's transparent-window shim (`DwmEnableBlurBehindWindow` with an empty
//! region) owns the backdrop and the system acrylic is silently ignored.
//! `main.rs` enables that style only when the OS supports the backdrop API.
//!
//! The webview background is transparent (`with_transparent(true)`), so the
//! `native-blur` CSS theme supplies the tint on top of DWM's blur.

// Win32 calls below only pass live HWNDs owned by tao.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::desktop::tao::platform::windows::WindowExtWindows;
use dioxus::desktop::tao::window::{Theme, Window};
use tracing::{debug, warn};
use window_vibrancy::{apply_acrylic, apply_blur};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::{
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};

static BLUR_ACTIVE: AtomicBool = AtomicBool::new(false);
static ATTEMPTED: AtomicBool = AtomicBool::new(false);
static SHOW_ON_READY: AtomicBool = AtomicBool::new(false);

pub fn is_blur_active() -> bool {
    BLUR_ACTIVE.load(Ordering::Acquire)
}

/// True on Windows 11 22H2+ (build 22523), where `DWMWA_SYSTEMBACKDROP_TYPE`
/// provides the real Acrylic backdrop. `main.rs` also uses this to pick the
/// no-redirection-bitmap window style the system backdrop requires.
pub fn supports_system_backdrop() -> bool {
    windows_version::OsVersion::current().build >= 22523
}

/// Whether either native backdrop path exists on this Windows release.
///
/// The legacy blur-behind path requires Windows 10 1809 (build 17763).
pub fn supports_blur() -> bool {
    windows_version::OsVersion::current().build >= 17763
}

pub fn enable_blur(window: &Arc<Window>) {
    if !crate::state::get_blur_enabled_setting() {
        debug!("windows-blur: disabled in settings");
        return;
    }
    if is_blur_active() || ATTEMPTED.swap(true, Ordering::AcqRel) {
        return;
    }

    if supports_system_backdrop() {
        match apply_acrylic(window.as_ref(), None) {
            Ok(()) => {
                BLUR_ACTIVE.store(true, Ordering::Release);
                debug!("windows-blur: acrylic enabled");
                return;
            }
            Err(e) => warn!("windows-blur: acrylic failed ({e}); trying legacy blur"),
        }
    }

    // Windows 10 (1809+) / Windows 11 21H2 fallback.
    match apply_blur(window.as_ref(), None) {
        Ok(()) => {
            BLUR_ACTIVE.store(true, Ordering::Release);
            debug!("windows-blur: blur-behind enabled");
        }
        Err(e) => warn!("windows-blur: {e}"),
    }
}

/// Create the window hidden and reveal it on the first UI tick.
///
/// Showing after the webview is ready avoids the white flash of an unpainted
/// webview, and lets the dark titlebar attribute settle before the frame is
/// first composited (DWM otherwise keeps the initial light titlebar until the
/// window is hidden and shown again).
pub fn defer_show_until_ready(show: bool) {
    SHOW_ON_READY.store(show, Ordering::Release);
}

/// Called once the UI is ready. Applies the titlebar theme before revealing the
/// window when a deferred show was requested.
pub fn on_ui_ready(window: &Arc<Window>) {
    sync_window_chrome(window);
    if SHOW_ON_READY.swap(false, Ordering::AcqRel) && !window.is_visible() {
        window.set_visible(true);
        window.set_focus();
    }
}

/// Keep the native titlebar in sync with the app theme.
///
/// The DWM backdrop also paints the non-client area, so without this a dark
/// app would get a bright titlebar. "System" follows the Windows setting, the
/// same way the in-app System theme does.
pub fn sync_window_chrome(window: &Arc<Window>) {
    let dark = match crate::state::get_theme_mode_setting() {
        crate::state::ThemeModeSetting::System => window.theme() == Theme::Dark,
        crate::state::ThemeModeSetting::Light => false,
        crate::state::ThemeModeSetting::Dark => true,
    };

    let hwnd = HWND(window.hwnd() as *mut _);
    let dark = i32::from(dark);
    // DWMWA_USE_IMMERSIVE_DARK_MODE. `Window::set_theme` caches the requested
    // theme and skips the DWM call when it is unchanged, so a window created
    // while hidden can keep a light titlebar after it is shown. Calling DWM
    // directly and forcing a frame change makes the initial paint correct.
    unsafe {
        _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&dark).cast(),
            std::mem::size_of_val(&dark) as u32,
        );
        _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}
