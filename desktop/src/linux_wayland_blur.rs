//! Native Wayland background blur for GNOME 51.
//!
//! GNOME 51's Mutter implements the staging Wayland protocol
//! `ext-background-effect-v1`: a client can ask the compositor to blur the
//! content behind (parts of) its own `wl_surface` ("frosted glass"). The blur
//! itself is rendered by the compositor subject to its own policy (Mutter
//! 51.rc defaults: radius 24, saturation 1.25, noise 0.015); the client only
//! describes *which* surface-local region should be blurred.
//!
//! Requirements for the effect to be visible:
//!
//! 1. The app must run as a **native Wayland client**. Under XWayland
//!    (`GDK_BACKEND=x11`) the window has no `wl_surface` and the protocol is
//!    unreachable. This is why the Dioxus desktop config must opt out of the
//!    legacy XWayland fallback via
//!    [`dioxus::desktop::Config::with_disable_dma_buf_on_wayland`] — see
//!    `main.rs`. That fallback predates GNOME 51 (unresponsive CSD buttons,
//!    broken move/resize, fractional-scale blur) and GNOME 51 removed the
//!    legacy NVIDIA interfaces it was working around.
//! 2. The window background must actually be translucent (alpha < 1) where the
//!    blur should show through. After the blur region is installed
//!    successfully, [`is_blur_active`] flips to `true` and the UI adds the
//!    `gnome-blur` CSS class (see `assets/styles.css`), which switches the
//!    shell from opaque to translucent surfaces. Without the compositor
//!    effect the previous opaque theme is kept (robust fallback).
//!
//! The implementation wraps GTK's *foreign* `wl_display` (the connection
//! owned by WebKitGTK/tao) with `wayland-backend` in guest mode and binds
//! `ext_background_effect_manager_v1` on that same connection, because only
//! that connection owns our `wl_surface`. The raw `wl_display` / `wl_surface`
//! pointers come from tao's `raw-window-handle` (v0.6) handles, which tao
//! itself derives via `gdk_wayland_window_get_wl_surface`.
//!
//! A deliberately oversized blur region (0,0,10000x10000) is used. The
//! protocol clips it to the surface size, so no resize tracking is needed.
//! The effect object is intentionally retained for the process lifetime:
//! destroying it would remove the effect on the next commit.

// The whole point of this module is FFI interop with GTK's Wayland objects;
// the unsafe blocks are documented at each site. The workspace lints
// `unsafe_code` as warn, which would otherwise flag every block here.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use tracing::{debug, warn};
use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_compositor, wl_region, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1, ext_background_effect_surface_v1,
};

/// Set once the compositor accepted a blur region for our surface.
///
/// The UI thread polls this to add the translucent `gnome-blur` theme class.
/// Defaults to `false` so every other platform / compositor keeps the proven
/// opaque theme.
static BLUR_ACTIVE: AtomicBool = AtomicBool::new(false);

/// True once [`enable_blur_async`] installed a blur region successfully.
pub fn is_blur_active() -> bool {
    BLUR_ACTIVE.load(Ordering::Acquire)
}

/// Are we in a session where native Wayland blur is even plausible?
///
/// Checks the usual session markers and honours an explicit `GDK_BACKEND`
/// override: if something forced `GDK_BACKEND=x11` we are on XWayland and
/// there is no `wl_surface` to blur.
pub fn is_wayland_session() -> bool {
    if std::env::var("GDK_BACKEND")
        .map(|v| v == "x11")
        .unwrap_or(false)
    {
        return false;
    }
    is_wayland_host_session()
}

/// Is the host session Wayland, regardless of `GDK_BACKEND` overrides?
///
/// Used for renderer workarounds: even under XWayland (`GDK_BACKEND=x11`)
/// the underlying session quirks (NVIDIA GBM, explicit sync) still apply.
pub fn is_wayland_host_session() -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return true;
    }
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
}

/// Is the NVIDIA proprietary driver in use?
///
/// WebKitGTK's DMA-BUF renderer is broken with it in several ways (GBM
/// allocation failures, missing explicit-sync acquire points that strict
/// compositors like Mutter kill with a protocol error, partial
/// presentation). Detected via the driver's proc entry, mirroring what
/// other WebKitGTK embedders do.
pub fn is_nvidia_proprietary() -> bool {
    std::path::Path::new("/proc/driver/nvidia/version").exists()
}

/// Spawn a background attempt to enable compositor blur for `window`.
///
/// Never blocks the caller and never panics: every failure path logs and
/// leaves [`is_blur_active`] at `false` (opaque fallback theme). The
/// `wl_surface` only exists after GDK maps the window, so creation is
/// retried for a few seconds.
///
/// Setting `CONNECTED_NO_BLUR=1` disables the attempt entirely (useful for
/// debugging and as an escape hatch on compositors with buggy
/// ext-background-effect-v1 implementations).
pub fn enable_blur_async(window: Arc<dioxus::desktop::tao::window::Window>) {
    if std::env::var("CONNECTED_NO_BLUR")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        debug!("wayland-blur: disabled via CONNECTED_NO_BLUR");
        return;
    }
    if !is_wayland_session() {
        debug!("wayland-blur: not a Wayland session, skipping");
        return;
    }
    if is_blur_active() {
        return;
    }
    std::thread::Builder::new()
        .name("wayland-blur".into())
        .spawn(move || {
            for attempt in 0..20 {
                if is_blur_active() {
                    return;
                }
                match try_enable_blur(&window) {
                    Ok(true) => {
                        BLUR_ACTIVE.store(true, Ordering::Release);
                        debug!("wayland-blur: background blur enabled (attempt {attempt})");
                        return;
                    }
                    Ok(false) => {
                        // Not ready yet (surface missing / manager not advertised).
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    Err(e) => {
                        warn!("wayland-blur: {e} (attempt {attempt})");
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
            debug!("wayland-blur: giving up, keeping opaque theme");
        })
        .ok();
}

/// One attempt. `Ok(true)` = blur installed. `Ok(false)` = retryable "not
/// ready". `Err` = unexpected failure (also retryable a few times).
fn try_enable_blur(window: &dioxus::desktop::tao::window::Window) -> Result<bool, String> {
    let display_handle = window
        .display_handle()
        .map_err(|e| format!("no display handle: {e:?}"))?;
    let window_handle = window
        .window_handle()
        .map_err(|e| format!("no window handle: {e:?}"))?;

    let (wl_display_ptr, wl_surface_ptr) = match (&display_handle.as_raw(), &window_handle.as_raw())
    {
        (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(surface)) => {
            (display.display.as_ptr(), surface.surface.as_ptr())
        }
        (display, surface) => {
            return Err(format!(
                "not a native Wayland window (display={display:?}, window={surface:?}); \
                 XWayland windows cannot use ext-background-effect-v1"
            ));
        }
    };
    if wl_display_ptr.is_null() || wl_surface_ptr.is_null() {
        return Ok(false);
    }

    // SAFETY: both pointers come from tao/GTK for the live window and stay
    // valid while the window lives. The backend is opened in guest mode: it
    // never closes the display. All retained proxies are intentionally leaked
    // below so they outlive this call for the process lifetime.
    unsafe { install_blur_region(wl_display_ptr, wl_surface_ptr) }
}

struct BlurState {
    blur_capability_seen: bool,
}

// Registry handling for the one-shot global discovery.
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for BlurState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for BlurState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_region::WlRegion, ()> for BlurState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_region::WlRegion,
        _event: wl_region::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for BlurState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_surface::WlSurface,
        _event: wl_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1, ()> for BlurState {
    fn event(
        state: &mut Self,
        _proxy: &ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The compositor announces its capabilities on bind and whenever they
        // change. Any announcement means the manager is live; Mutter 51
        // advertises the blur capability.
        let _ = &event;
        state.blur_capability_seen = true;
    }
}

impl Dispatch<ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1, ()> for BlurState {
    fn event(
        _state: &mut Self,
        _proxy: &ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
        _event: ext_background_effect_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

/// Everything that must stay alive for the blur to keep applying.
///
/// Intentionally leaked after a successful install: per the protocol,
/// destroying the effect object removes the effect on the next commit.
struct RetainedBlur {
    _conn: Connection,
    _manager: ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
    _surface: wl_surface::WlSurface,
    _effect: ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
}

/// Bind the manager on GTK's connection and set a full-surface blur region.
///
/// # Safety
///
/// `wl_display_ptr` must be GTK's live `wl_display`, `wl_surface_ptr` the
/// live `wl_surface` of our window, both on the same connection.
unsafe fn install_blur_region(
    wl_display_ptr: *mut std::ffi::c_void,
    wl_surface_ptr: *mut std::ffi::c_void,
) -> Result<bool, String> {
    use wayland_client::backend::Backend;

    // Wrap the foreign display. Guest mode: we never own/close it, and the
    // pointer stays valid as long as GTK's window lives (we only run while
    // the app window exists, and all proxies are leaked below).
    let backend = unsafe { Backend::from_foreign_display(wl_display_ptr as *mut _) };
    let conn = Connection::from_backend(backend);

    let mut state = BlurState {
        blur_capability_seen: false,
    };
    let (globals, mut queue) =
        registry_queue_init::<BlurState>(&conn).map_err(|e| format!("registry init: {e:?}"))?;
    // One roundtrip so the initial global list is populated.
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("registry roundtrip: {e:?}"))?;

    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 1..=6, ())
        .map_err(|e| format!("no wl_compositor global: {e:?}"))?;
    let manager: ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1 =
        globals.bind(&qh, 1..=1, ()).map_err(|_| {
            "compositor does not advertise ext_background_effect_manager_v1 (pre-51 Mutter?)"
                .to_string()
        })?;

    // Wrap GTK's existing wl_surface (created by GDK, not by us). The
    // interface check rejects stale/wrong pointers instead of corrupting
    // the connection.
    let surface_id =
        unsafe { ObjectId::from_ptr(wl_surface::WlSurface::interface(), wl_surface_ptr as *mut _) }
            .map_err(|_| "wl_surface pointer has unexpected interface".to_string())?;
    let surface = wl_surface::WlSurface::from_id(&conn, surface_id)
        .map_err(|_| "cannot wrap foreign wl_surface".to_string())?;

    let effect = manager.get_background_effect(&surface, &qh, ());

    // Oversized region; the compositor clips it to the surface, so window
    // resizes need no further handling.
    let region = compositor.create_region(&qh, ());
    const BLUR_EXTENT: i32 = 10_000;
    region.add(0, 0, BLUR_EXTENT, BLUR_EXTENT);
    effect.set_blur_region(Some(&region));
    region.destroy();

    queue.flush().map_err(|e| format!("wayland flush: {e:?}"))?;

    // The pending region applies on GTK's next wl_surface.commit (the UI
    // repaints continuously), so there is nothing left to do here.
    // Retain everything; dropping the effect would remove the blur. Leaking
    // avoids Send/Sync bounds entirely: the objects are never touched again.
    let retained = RetainedBlur {
        _conn: conn,
        _manager: manager,
        _surface: surface,
        _effect: effect,
    };
    let _ = Box::leak(Box::new(retained));
    // The event queue is no longer needed: further compositor events (e.g.
    // capability changes) are intentionally ignored.
    std::mem::forget(queue);

    debug!("wayland-blur: blur region committed for wl_surface");
    Ok(true)
}
