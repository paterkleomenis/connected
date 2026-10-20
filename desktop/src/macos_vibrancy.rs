//! Native macOS vibrancy (`NSVisualEffectView`) behind the webview.
//!
//! Requires a transparent window and webview: `main.rs` enables
//! `with_transparent(true)` and Dioxus forwards that flag to wry. The opaque
//! CSS theme paints over the effect unless the `native-blur` class is active.
//!
//! `apply_vibrancy` touches AppKit, so it must run on the main thread. The
//! first post-init event-loop tick and the settings toggle both do; the effect
//! persists across hide/show, so it is only ever installed once.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::desktop::tao::window::Window;
use tracing::{debug, warn};
use window_vibrancy::{NSVisualEffectMaterial, NSVisualEffectState, apply_vibrancy};

static BLUR_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The AppKit call is only made from the UI thread. After one attempt there is
/// nothing useful a background retry could do (unsupported OS / missing view),
/// so never call into AppKit from the wakeup listener or poller threads.
static ATTEMPTED: AtomicBool = AtomicBool::new(false);

pub fn is_blur_active() -> bool {
    BLUR_ACTIVE.load(Ordering::Acquire)
}

pub fn enable_blur(window: &Arc<Window>) {
    if !crate::state::get_blur_enabled_setting() {
        debug!("macos-blur: disabled in settings");
        return;
    }
    if is_blur_active() || ATTEMPTED.swap(true, Ordering::AcqRel) {
        return;
    }

    match apply_vibrancy(
        window.as_ref(),
        NSVisualEffectMaterial::UnderWindowBackground,
        Some(NSVisualEffectState::Active),
        None,
    ) {
        Ok(()) => {
            BLUR_ACTIVE.store(true, Ordering::Release);
            debug!("macos-blur: vibrancy enabled");
        }
        Err(e) => warn!("macos-blur: {e}"),
    }
}
