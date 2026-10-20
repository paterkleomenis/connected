//! Native Windows desktop backdrop.
//!
//! Tries Mica (Windows 11) first and falls back to the legacy blur-behind
//! effect on Windows 10 and Windows 11 21H2. Acrylic is deliberately not used:
//! on Windows 10 1903+ and Windows 11 22000 it makes dragging/resizing lag.
//!
//! Requires a transparent window and webview: `main.rs` enables
//! `with_transparent(true)` and Dioxus forwards that flag to wry. The opaque
//! CSS theme paints over the effect unless the `native-blur` class is active.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::desktop::tao::window::Window;
use tracing::{debug, warn};
use window_vibrancy::{apply_blur, apply_mica};

static BLUR_ACTIVE: AtomicBool = AtomicBool::new(false);
static ATTEMPTED: AtomicBool = AtomicBool::new(false);

pub fn is_blur_active() -> bool {
    BLUR_ACTIVE.load(Ordering::Acquire)
}

pub fn enable_blur(window: &Arc<Window>) {
    if !crate::state::get_blur_enabled_setting() {
        debug!("windows-blur: disabled in settings");
        return;
    }
    if is_blur_active() || ATTEMPTED.swap(true, Ordering::AcqRel) {
        return;
    }

    // `None` lets the system pick the Mica light/dark variant; the app's own
    // translucent theme supplies the tint on top.
    if apply_mica(window.as_ref(), None).is_ok() {
        BLUR_ACTIVE.store(true, Ordering::Release);
        debug!("windows-blur: Mica enabled");
        return;
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
