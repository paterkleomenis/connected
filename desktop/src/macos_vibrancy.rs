//! Native macOS vibrancy (`NSVisualEffectView`) behind the webview.
//!
//! Requires a transparent window and webview: `main.rs` enables
//! `with_transparent(true)` and Dioxus forwards that flag to wry. The opaque
//! CSS theme paints over the effect unless the `native-blur` class is active.
//!
//! wry replaces the window's `contentView` with its own parent view while
//! building the webview, so tao's cached `ns_view` is detached by the time we
//! install the effect. We therefore always attach to the window's *current*
//! content view, which is the only view that is actually on screen.
//!
//! The window itself stays clear; the `native-blur` theme paints the tinted
//! glass (and the titlebar strip) so the effect stays in sync with the blur
//! strength slider.

// Objective-C FFI requires unsafe; every block below is scoped to the main
// thread and only uses live AppKit objects owned by tao/the window.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus::desktop::tao::platform::macos::WindowExtMacOS;
use dioxus::desktop::tao::window::Window;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSColor, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSWindow, NSWindowOrderingMode,
};
use tracing::debug;
use window_vibrancy::NSVisualEffectViewTagged;

const BLUR_VIEW_TAG: isize = 91376255;

static BLUR_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn is_blur_active() -> bool {
    BLUR_ACTIVE.load(Ordering::Acquire)
}

fn window_from_ptr(window: &Window) -> Option<&NSWindow> {
    let ptr = window.ns_window().cast::<NSWindow>();
    if ptr.is_null() {
        return None;
    }
    // The pointer is a live NSWindow owned by tao for the duration of the call.
    Some(unsafe { &*ptr })
}

pub fn enable_blur(window: &Arc<Window>) {
    if !crate::state::get_blur_enabled_setting() {
        return;
    }
    if BLUR_ACTIVE.load(Ordering::Acquire) {
        return;
    }

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(ns_window) = window_from_ptr(window) else {
        return;
    };
    let Some(view) = ns_window.contentView() else {
        return;
    };

    if view.viewWithTag(BLUR_VIEW_TAG).is_none() {
        let effect = unsafe {
            NSVisualEffectViewTagged::initWithFrame(mtm.alloc(), view.bounds(), BLUR_VIEW_TAG)
        };
        unsafe {
            // HUDWindow keeps the backdrop visible instead of painting the
            // almost-solid grey of UnderWindowBackground in dark appearance.
            effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
            effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            effect.setState(NSVisualEffectState::Active);
        }
        unsafe {
            effect.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
        }
        view.addSubview_positioned_relativeTo(
            &effect,
            NSWindowOrderingMode::Below,
            None::<&NSView>,
        );
        BLUR_ACTIVE.store(true, Ordering::Release);
        debug!("macos-blur: vibrancy enabled");
    } else {
        BLUR_ACTIVE.store(true, Ordering::Release);
    }

    sync_window_chrome(window);
}

pub fn sync_window_chrome(window: &Arc<Window>) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let Some(ns_window) = window_from_ptr(window) else {
        return;
    };

    let (aqua, dark_aqua) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let theme = crate::state::get_theme_mode_setting();
    let appearance = match theme {
        crate::state::ThemeModeSetting::System => None,
        crate::state::ThemeModeSetting::Light => NSAppearance::appearanceNamed(aqua),
        crate::state::ThemeModeSetting::Dark => NSAppearance::appearanceNamed(dark_aqua),
    };
    ns_window.setAppearance(appearance.as_deref());

    let name = ns_window.effectiveAppearance().name();
    let dark = name.isEqualToString(dark_aqua);

    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));

    debug!("macos-blur: window chrome synced (dark={dark})");
}
