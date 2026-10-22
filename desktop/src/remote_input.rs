//! Desktop-side remote keyboard/pointer injection.
//!
//! One ordered worker consumes input batches. On Wayland we use the
//! xdg-desktop-portal `RemoteDesktop` interface plus libei/EIS (the same
//! mechanism KDE Connect uses); this is the only unprivileged path that works
//! on GNOME and KDE. On X11, Windows and macOS we use enigo.
//!
//! Wayland portal approvals are persisted with `PersistMode::ExplicitlyRevoked`
//! and a restore token, so returning users are not prompted again. Revoking is
//! still possible from the desktop privacy settings.

use crate::state::{get_notifications_enabled_setting, get_remote_input_enabled_setting};
#[cfg(target_os = "linux")]
use crate::state::{get_remote_input_restore_token, set_remote_input_restore_token};
use connected_core::input::RemoteInputEvent;
#[cfg(target_os = "linux")]
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
#[cfg(target_os = "linux")]
use tracing::info;
use tracing::{debug, warn};

/// Commands for the single desktop input worker.
enum InputCommand {
    /// Prepare the injection backend before the first event arrives. This is
    /// sent when the phone opens its remote-input session.
    WarmUp {
        session_id: u64,
        from_device: String,
    },
    /// A batch of input events.
    Batch {
        session_id: u64,
        from_device: String,
        events: Vec<RemoteInputEvent>,
    },
    /// The phone's session ended or the desktop setting was disabled.
    Close,
}

static INPUT_TX: OnceLock<mpsc::Sender<InputCommand>> = OnceLock::new();

/// Stream id of the session currently permitted to inject input. Core replaces
/// a peer's stream when it reconnects; events queued by the replaced stream
/// must not leak into the new one.
static CURRENT_SESSION: AtomicU64 = AtomicU64::new(0);
/// Session that failed permission/EIS setup. Do not reopen the portal on every
/// event from that stream: a denial would otherwise prompt again every time the
/// cooldown expires. A fresh phone session clears it.
static BLOCKED_SESSION: AtomicU64 = AtomicU64::new(0);

fn current_session() -> u64 {
    CURRENT_SESSION.load(Ordering::Acquire)
}

fn session_blocked(session_id: u64) -> bool {
    session_id != 0 && BLOCKED_SESSION.load(Ordering::Acquire) == session_id
}

/// Whether this stream is still the current, permitted session. Backends check
/// this periodically as well as on queued commands: a `Close` can be dropped
/// when the bounded command channel is full, and the portal/EIS session must
/// still be torn down in that case.
fn session_is_active(session_id: u64) -> bool {
    session_id != 0
        && session_id == current_session()
        && !session_blocked(session_id)
        && get_remote_input_enabled_setting()
}

fn sender() -> Option<&'static mpsc::Sender<InputCommand>> {
    if let Some(tx) = INPUT_TX.get() {
        return Some(tx);
    }

    let (tx, rx) = mpsc::channel(64);
    if INPUT_TX.set(tx).is_err() {
        return INPUT_TX.get();
    }
    // The Wayland EIS event stream contains non-Send callbacks, so run the
    // worker on a dedicated thread with a current-thread Tokio runtime. This
    // also keeps enigo's synchronous X11/Windows/macOS calls off UI threads.
    std::thread::Builder::new()
        .name("connected-remote-input".to_string())
        .spawn(move || run_dispatcher(rx))
        .map_err(|error| warn!("Failed to start remote-input worker: {}", error))
        .ok();
    INPUT_TX.get()
}

fn queue(command: InputCommand) {
    let Some(tx) = sender() else {
        debug!("Remote-input worker unavailable: no Tokio runtime");
        return;
    };
    if let Err(error) = tx.try_send(command) {
        // Never log the event payload. A dropped motion sample under extreme
        // backpressure is safer than unbounded queue growth.
        debug!("Remote-input queue unavailable ({error}); dropping command");
    }
}

/// Called when a phone's remote-input stream handshake completes.
pub fn session_started(session_id: u64, from_device: &str) {
    if session_id == 0 {
        return;
    }
    let previous = CURRENT_SESSION.load(Ordering::Acquire);
    if previous != 0 && session_id < previous {
        // Ignore a delayed Started event from a stream that has already been
        // replaced. Session ids are globally monotonic.
        return;
    }
    BLOCKED_SESSION.store(0, Ordering::Release);
    let previous = CURRENT_SESSION.swap(session_id, Ordering::AcqRel);
    if previous != 0 && previous != session_id {
        // Belt-and-suspenders for a missed/stale end event: close any backend
        // session belonging to the stream this one replaced.
        queue(InputCommand::Close);
    }
    queue(InputCommand::WarmUp {
        session_id,
        from_device: from_device.to_string(),
    });
}

/// Called when the phone's remote-input stream ends. Stale end events from a
/// replaced stream are ignored.
pub fn session_ended(session_id: u64) {
    if CURRENT_SESSION
        .compare_exchange(session_id, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        if BLOCKED_SESSION.load(Ordering::Acquire) == session_id {
            BLOCKED_SESSION.store(0, Ordering::Release);
        }
        queue(InputCommand::Close);
    }
}

/// Queue a batch of remote events. The core layer already enforces trust and
/// the local enable gate; the backend re-checks the setting before injecting.
pub fn handle_events(session_id: u64, from_device: &str, events: Vec<RemoteInputEvent>) {
    if events.is_empty() || session_id == 0 {
        return;
    }
    let current = current_session();
    if session_id < current {
        // Event from a stream that has already been replaced.
        return;
    }
    if session_id != current {
        // The broadcast channel can drop a Started event under load. Recover
        // from the first authenticated batch rather than leaving every later
        // event for this stream ignored. Session ids are globally monotonic,
        // so an older id can never take over here.
        session_started(session_id, from_device);
    }
    if !session_is_active(session_id) {
        return;
    }
    queue(InputCommand::Batch {
        session_id,
        from_device: from_device.to_string(),
        events,
    });
}

/// Stop injecting input immediately, but keep the phone's logical session so
/// re-enabling the setting does not require closing and reopening the phone
/// panel. The next batch will start a fresh backend session (using the saved
/// portal token on Wayland).
pub fn disable() {
    BLOCKED_SESSION.store(0, Ordering::Release);
    queue(InputCommand::Close);
}

fn run_dispatcher(rx: mpsc::Receiver<InputCommand>) {
    #[cfg(target_os = "linux")]
    if is_wayland_session() {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                warn!("Failed to start Wayland remote-input runtime: {}", error);
                return;
            }
        };
        runtime.block_on(run_wayland_dispatcher(rx));
        return;
    }

    run_enigo_worker(rx);
}

fn notify(title: &str, body: &str) {
    if !get_notifications_enabled_setting() {
        return;
    }
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let title = title.to_string();
        let body = body.to_string();
        std::thread::spawn(move || {
            if let Err(error) = notify_rust::Notification::new()
                .summary(&title)
                .body(&body)
                .timeout(notify_rust::Timeout::Milliseconds(4000))
                .show()
            {
                debug!("Remote-input notification failed: {}", error);
            }
        });
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (title, body);
    }
}

// ============================================================================
// enigo backend (X11, Windows, macOS)
// ============================================================================

fn run_enigo_worker(mut rx: mpsc::Receiver<InputCommand>) {
    use enigo::{Enigo, Settings};

    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(enigo) => enigo,
        Err(error) => {
            warn!("Remote input unavailable (cannot initialize input backend): {error}");
            notify(
                "Remote input unavailable",
                "Could not initialize keyboard/mouse control on this desktop.",
            );
            return;
        }
    };
    let mut motion_accum = (0.0f32, 0.0f32);
    let mut scroll_accum = (0.0f32, 0.0f32);

    while let Some(command) = rx.blocking_recv() {
        match command {
            // Keep the backend and worker alive across sessions. A future
            // WarmUp must still be able to inject input after the phone
            // closes its panel and opens it again.
            InputCommand::Close => {
                motion_accum = (0.0, 0.0);
                scroll_accum = (0.0, 0.0);
                release_enigo_input_state(&mut enigo);
            }
            InputCommand::WarmUp {
                session_id,
                from_device,
            } => {
                // A replacement session must not inherit a held button from a
                // stream that disappeared without sending its release events.
                motion_accum = (0.0, 0.0);
                scroll_accum = (0.0, 0.0);
                release_enigo_input_state(&mut enigo);
                if !session_is_active(session_id) {
                    continue;
                }
                notify(
                    "Remote input active",
                    &format!(
                        "{from_device} can now control this computer. Disable it in Settings."
                    ),
                );
            }
            InputCommand::Batch {
                session_id, events, ..
            } => {
                if !session_is_active(session_id) {
                    motion_accum = (0.0, 0.0);
                    scroll_accum = (0.0, 0.0);
                    // The session may have been replaced or disabled. Never
                    // leave a modifier/button held from the stale stream.
                    release_enigo_input_state(&mut enigo);
                    continue;
                }
                apply_enigo_events(&mut enigo, &events, &mut motion_accum, &mut scroll_accum);
            }
        }
    }
    release_enigo_input_state(&mut enigo);
}

fn release_enigo_input_state(enigo: &mut enigo::Enigo) {
    use enigo::{Button, Direction, Keyboard, Mouse};
    for button in [
        Button::Left,
        Button::Right,
        Button::Middle,
        Button::Back,
        Button::Forward,
    ] {
        let _ = enigo.button(button, Direction::Release);
    }
    // Key presses can outlive a batch (Ctrl+C sends Ctrl down, C down/up,
    // Ctrl up). If the stream dies between those events, release everything
    // enigo still considers held, including letter keys.
    let (held_keys, _) = enigo.held();
    for key in held_keys {
        let _ = enigo.key(key, Direction::Release);
    }
}

fn apply_enigo_events(
    enigo: &mut enigo::Enigo,
    events: &[RemoteInputEvent],
    motion_accum: &mut (f32, f32),
    scroll_accum: &mut (f32, f32),
) {
    use enigo::{Axis, Coordinate, Direction, Keyboard, Mouse};

    // enigo accepts integer pixels while the touchpad sends fractional
    // deltas. Accumulate them so slow, precise movement is not rounded away.
    fn flush_motion(enigo: &mut enigo::Enigo, accum: &mut (f32, f32)) {
        let dx = accum.0.round() as i32;
        let dy = accum.1.round() as i32;
        if dx != 0 || dy != 0 {
            let _ = enigo.move_mouse(dx, dy, Coordinate::Rel);
            accum.0 -= dx as f32;
            accum.1 -= dy as f32;
        }
    }

    // Flush accumulated motion/scroll before an event whose ordering matters.
    fn flush_scroll(enigo: &mut enigo::Enigo, accum: &mut (f32, f32)) {
        let vertical = (accum.1 / 40.0).round() as i32;
        let horizontal = (accum.0 / 40.0).round() as i32;
        if vertical != 0 {
            let _ = enigo.scroll(vertical, Axis::Vertical);
            accum.1 -= vertical as f32 * 40.0;
        }
        if horizontal != 0 {
            let _ = enigo.scroll(horizontal, Axis::Horizontal);
            accum.0 -= horizontal as f32 * 40.0;
        }
    }

    for event in events {
        match event {
            RemoteInputEvent::PointerMotion { dx, dy } => {
                motion_accum.0 += dx;
                motion_accum.1 += dy;
            }
            RemoteInputEvent::Scroll { dx, dy } => {
                // enigo uses wheel clicks (15 degrees), while the protocol
                // carries logical pixels. Accumulate small touch deltas and
                // emit a notch every 40 px.
                scroll_accum.0 += dx;
                scroll_accum.1 += dy;
            }
            _ => {
                flush_motion(enigo, motion_accum);
                flush_scroll(enigo, scroll_accum);
                let result = match event {
                    RemoteInputEvent::PointerMotion { .. } => Ok(()),
                    RemoteInputEvent::PointerButton { button, pressed } => {
                        let direction = if *pressed {
                            Direction::Press
                        } else {
                            Direction::Release
                        };
                        match enigo_button(*button) {
                            Some(button) => enigo.button(button, direction),
                            None => Ok(()),
                        }
                    }
                    RemoteInputEvent::Key { keysym, pressed } => match enigo_key(*keysym) {
                        Some(key) => {
                            let direction = if *pressed {
                                Direction::Press
                            } else {
                                Direction::Release
                            };
                            enigo.key(key, direction)
                        }
                        None => Ok(()),
                    },
                    RemoteInputEvent::Text { text } => enigo.text(text),
                    RemoteInputEvent::Scroll { .. } => Ok(()),
                };
                if let Err(error) = result {
                    if matches!(event, RemoteInputEvent::Text { .. }) {
                        // Do not include an error value that might quote the
                        // input text or individual keys.
                        debug!("Remote-input text injection failed");
                    } else {
                        debug!("Remote-input injection failed: {}", error);
                    }
                }
            }
        }
    }
    flush_motion(enigo, motion_accum);
    flush_scroll(enigo, scroll_accum);
}

fn enigo_button(button: u16) -> Option<enigo::Button> {
    use connected_core::input::{BTN_BACK, BTN_FORWARD, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
    use enigo::Button;
    Some(match button {
        BTN_LEFT => Button::Left,
        BTN_RIGHT => Button::Right,
        BTN_MIDDLE => Button::Middle,
        BTN_BACK => Button::Back,
        BTN_FORWARD => Button::Forward,
        _ => return None,
    })
}

#[allow(non_upper_case_globals)]
fn enigo_key(keysym: u32) -> Option<enigo::Key> {
    use enigo::Key;
    use xkb_keysyms::*;
    Some(match keysym {
        KEY_BackSpace => Key::Backspace,
        KEY_Tab => Key::Tab,
        KEY_Return => Key::Return,
        KEY_Escape => Key::Escape,
        KEY_Delete => Key::Delete,
        KEY_Insert => Key::Insert,
        KEY_Home => Key::Home,
        KEY_End => Key::End,
        KEY_Page_Up => Key::PageUp,
        KEY_Page_Down => Key::PageDown,
        KEY_Left => Key::LeftArrow,
        KEY_Right => Key::RightArrow,
        KEY_Up => Key::UpArrow,
        KEY_Down => Key::DownArrow,
        KEY_Shift_L | KEY_Shift_R => Key::Shift,
        KEY_Control_L | KEY_Control_R => Key::Control,
        KEY_Alt_L | KEY_Alt_R => Key::Alt,
        KEY_Super_L | KEY_Super_R => Key::Meta,
        KEY_Caps_Lock => Key::CapsLock,
        KEY_space => Key::Space,
        KEY_F1 => Key::F1,
        KEY_F2 => Key::F2,
        KEY_F3 => Key::F3,
        KEY_F4 => Key::F4,
        KEY_F5 => Key::F5,
        KEY_F6 => Key::F6,
        KEY_F7 => Key::F7,
        KEY_F8 => Key::F8,
        KEY_F9 => Key::F9,
        KEY_F10 => Key::F10,
        KEY_F11 => Key::F11,
        KEY_F12 => Key::F12,
        // Printable ASCII keysyms are used for shortcuts (Ctrl+C, etc.).
        // `Key::Unicode` resolves the character through the active layout.
        _ if (0x20..=0x7e).contains(&keysym) => Key::Unicode(keysym as u8 as char),
        _ => return None,
    })
}

/// X11 keysym constants. Kept local so the enigo path does not need to depend
/// on the Wayland-only xkbcommon crate.
#[allow(non_upper_case_globals)]
mod xkb_keysyms {
    pub const KEY_BackSpace: u32 = 0xff08;
    pub const KEY_Tab: u32 = 0xff09;
    pub const KEY_Return: u32 = 0xff0d;
    pub const KEY_Escape: u32 = 0xff1b;
    pub const KEY_Delete: u32 = 0xffff;
    pub const KEY_Insert: u32 = 0xff63;
    pub const KEY_Home: u32 = 0xff50;
    pub const KEY_End: u32 = 0xff57;
    pub const KEY_Page_Up: u32 = 0xff55;
    pub const KEY_Page_Down: u32 = 0xff56;
    pub const KEY_Left: u32 = 0xff51;
    pub const KEY_Up: u32 = 0xff52;
    pub const KEY_Right: u32 = 0xff53;
    pub const KEY_Down: u32 = 0xff54;
    pub const KEY_Shift_L: u32 = 0xffe1;
    pub const KEY_Shift_R: u32 = 0xffe2;
    pub const KEY_Control_L: u32 = 0xffe3;
    pub const KEY_Control_R: u32 = 0xffe4;
    pub const KEY_Caps_Lock: u32 = 0xffe5;
    pub const KEY_Alt_L: u32 = 0xffe9;
    pub const KEY_Alt_R: u32 = 0xffea;
    pub const KEY_Super_L: u32 = 0xffeb;
    pub const KEY_Super_R: u32 = 0xffec;
    pub const KEY_space: u32 = 0x0020;
    pub const KEY_F1: u32 = 0xffbe;
    pub const KEY_F2: u32 = 0xffbf;
    pub const KEY_F3: u32 = 0xffc0;
    pub const KEY_F4: u32 = 0xffc1;
    pub const KEY_F5: u32 = 0xffc2;
    pub const KEY_F6: u32 = 0xffc3;
    pub const KEY_F7: u32 = 0xffc4;
    pub const KEY_F8: u32 = 0xffc5;
    pub const KEY_F9: u32 = 0xffc6;
    pub const KEY_F10: u32 = 0xffc7;
    pub const KEY_F11: u32 = 0xffc8;
    pub const KEY_F12: u32 = 0xffc9;
}

// ============================================================================
// Wayland portal + libei backend
// ============================================================================

#[cfg(target_os = "linux")]
fn is_wayland_session() -> bool {
    // Trust the session type first: some X11 sessions inherit a stale
    // WAYLAND_DISPLAY from a parent process, which would send us down the
    // portal path on a machine where enigo already works.
    match std::env::var("XDG_SESSION_TYPE") {
        Ok(value) if value.eq_ignore_ascii_case("wayland") => true,
        Ok(value) if value.eq_ignore_ascii_case("x11") => false,
        _ => std::env::var_os("WAYLAND_DISPLAY").is_some(),
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
enum WaylandSessionError {
    /// No usable RemoteDesktop portal on this compositor. Safe to try the
    /// ydotool compatibility path, which is gated by the same local setting
    /// and trust checks.
    PortalUnavailable(String),
    /// The portal exists but permission, EIS setup, or the session failed.
    /// Never bypass a denial with an external injector.
    Failed(String),
}

#[cfg(target_os = "linux")]
fn classify_portal_error(context: &str, error: ashpd::Error) -> WaylandSessionError {
    match error {
        ashpd::Error::PortalNotFound(_) | ashpd::Error::RequiresVersion(..) => {
            WaylandSessionError::PortalUnavailable(format!("{context}: {error}"))
        }
        other => WaylandSessionError::Failed(format!("{context}: {other}")),
    }
}

#[cfg(target_os = "linux")]
async fn run_wayland_dispatcher(mut rx: mpsc::Receiver<InputCommand>) {
    let mut cooldown_until = Instant::now();
    // Once a compositor is known to lack the RemoteDesktop portal, don't spend
    // a DBus round-trip (and a visible delay) on every new session.
    let mut portal_unavailable = false;
    // Commands that arrived just as a session was ending. A WarmUp for the new
    // session must survive cleanup of stale events from the old one.
    let mut deferred: VecDeque<InputCommand> = VecDeque::new();

    loop {
        let command = if let Some(command) = deferred.pop_front() {
            command
        } else {
            let Some(command) = rx.recv().await else {
                return;
            };
            command
        };

        let (session_id, from_device) = match command {
            InputCommand::Close => continue,
            InputCommand::WarmUp {
                session_id,
                from_device,
            } => (session_id, from_device),
            InputCommand::Batch {
                session_id,
                from_device,
                ..
            } => (session_id, from_device),
        };

        if !session_is_active(session_id) {
            continue;
        }
        if Instant::now() < cooldown_until {
            continue;
        }

        let result = if portal_unavailable {
            Err(WaylandSessionError::PortalUnavailable(
                "the RemoteDesktop portal was unavailable earlier in this session".to_string(),
            ))
        } else {
            run_wayland_session(&mut rx, &mut deferred, session_id, &from_device).await
        };

        match result {
            Ok(()) => {
                // Session ended normally (phone closed it). Be ready for the
                // next session immediately; the persisted token suppresses a
                // second permission dialog.
                cooldown_until = Instant::now();
            }
            Err(WaylandSessionError::PortalUnavailable(reason)) => {
                portal_unavailable = true;
                debug!(
                    "RemoteDesktop portal unavailable ({}); trying ydotool fallback",
                    reason
                );
                match run_ydotool_session(&mut rx, &mut deferred, session_id, &from_device).await {
                    Ok(()) => {
                        cooldown_until = Instant::now();
                    }
                    Err(error) => {
                        warn!("Remote-input fallback unavailable: {}", error);
                        notify(
                            "Remote input unavailable",
                            "This Wayland compositor has no RemoteDesktop portal and the ydotool fallback is not ready. Install ydotool and start ydotoold, or use a desktop with portal support.",
                        );
                        cooldown_until = Instant::now() + Duration::from_secs(30);
                    }
                }
            }
            Err(WaylandSessionError::Failed(error)) => {
                warn!("Remote input unavailable on Wayland: {}", error);
                notify(
                    "Remote input unavailable",
                    "Allow remote control in the system permission dialog, or check your desktop's remote-desktop portal.",
                );
                // The stream is still open on the phone. Stop considering it
                // until the phone opens a fresh session, otherwise a denied
                // permission prompt would reappear every cooldown interval.
                BLOCKED_SESSION.store(session_id, Ordering::Release);
                cooldown_until = Instant::now();
            }
        }

        // Discard events that accumulated while the portal dialog or old
        // backend was open; replaying an old swipe or click after approval is
        // surprising and can activate the wrong UI. Keep only a replacement
        // session's WarmUp so the next backend starts without waiting for a
        // new touch. Any Batch left in `deferred` would otherwise be consumed
        // by the next backend before it could ever be applied.
        let current = current_session();
        let mut keep_warmup = None;
        for command in deferred.drain(..) {
            if let InputCommand::WarmUp {
                session_id,
                from_device,
            } = command
                && session_id == current
                && !session_blocked(session_id)
            {
                keep_warmup = Some(InputCommand::WarmUp {
                    session_id,
                    from_device,
                });
            }
        }
        while let Ok(pending) = rx.try_recv() {
            if let InputCommand::WarmUp {
                session_id,
                from_device,
            } = pending
                && session_id == current
                && !session_blocked(session_id)
            {
                keep_warmup = Some(InputCommand::WarmUp {
                    session_id,
                    from_device,
                });
            }
        }
        if let Some(warmup) = keep_warmup {
            deferred.push_back(warmup);
        }
    }
}

#[cfg(target_os = "linux")]
async fn run_wayland_session(
    rx: &mut mpsc::Receiver<InputCommand>,
    deferred: &mut VecDeque<InputCommand>,
    session_id: u64,
    from_device: &str,
) -> Result<(), WaylandSessionError> {
    use ashpd::desktop::remote_desktop::{
        ConnectToEISOptions, DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions,
    };
    use ashpd::desktop::{PersistMode, Session};
    use futures_util::StreamExt;

    // If the portal service itself cannot be reached, this is a compositor
    // without RemoteDesktop support (or a broken installation), not a user
    // denial. The caller may use the external fallback.
    let portal = RemoteDesktop::new().await.map_err(|error| {
        WaylandSessionError::PortalUnavailable(format!(
            "xdg-desktop-portal RemoteDesktop unavailable: {error}"
        ))
    })?;

    let session: Session<RemoteDesktop> = portal
        .create_session(Default::default())
        .await
        .map_err(|error| classify_portal_error("could not create portal session", error))?;

    let restore_token = get_remote_input_restore_token();
    portal
        .select_devices(
            &session,
            SelectDevicesOptions::default()
                .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(restore_token.as_deref()),
        )
        .await
        .map_err(|error| classify_portal_error("could not select input devices", error))?;

    let selected = portal
        .start(&session, None, StartOptions::default())
        .await
        .map_err(|error| classify_portal_error("portal start failed", error))?
        .response()
        .map_err(|error| {
            WaylandSessionError::Failed(format!(
                "remote-desktop permission was not granted: {error}"
            ))
        })?;

    let granted = selected.devices();
    if !granted.contains(DeviceType::Keyboard) || !granted.contains(DeviceType::Pointer) {
        return Err(WaylandSessionError::Failed(
            "the portal did not grant keyboard and pointer access".to_string(),
        ));
    }

    if let Some(token) = selected.restore_token()
        && restore_token.as_deref() != Some(token)
    {
        set_remote_input_restore_token(Some(token.to_string()));
        debug!("Stored remote-input portal restore token");
    }

    // The user may have switched the setting off (or the phone may have
    // replaced this stream) while the permission dialog was open. Do not keep
    // an approved portal session alive for a session that is already gone.
    if !session_is_active(session_id) {
        return Ok(());
    }

    let eis_fd = portal
        .connect_to_eis(&session, ConnectToEISOptions::default())
        .await
        .map_err(|error| classify_portal_error("could not connect to the EIS socket", error))?;

    let stream = std::os::unix::net::UnixStream::from(eis_fd);
    let context = reis::ei::Context::new(stream).map_err(|error| {
        WaylandSessionError::Failed(format!("could not create libei context: {error}"))
    })?;
    let (connection, event_stream) = context
        .handshake_tokio(
            "Connected Remote Input",
            reis::ei::handshake::ContextType::Sender,
        )
        .await
        .map_err(|error| WaylandSessionError::Failed(format!("libei handshake failed: {error}")))?;

    // Keep `session` alive for the entire EIS stream lifetime. Dropping it
    // closes the portal session and tears down the virtual devices.
    let _session_guard = session;

    let mut event_stream = Box::pin(event_stream);
    let mut state = WaylandState::new();
    let mut notified = false;
    // Safety net for a `Close` dropped under channel backpressure, and for a
    // stream replacement that does not get its WarmUp delivered. This is also
    // what closes the portal session promptly when the desktop setting is
    // switched off.
    let mut lifecycle = tokio::time::interval(Duration::from_millis(200));
    lifecycle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Drop input that arrived while the permission dialog was open. Preserve a
    // replacement session's WarmUp so a quick close/reopen can start its
    // backend without waiting for another touch, but never replay the old
    // session's input after a prompt.
    if !drain_stale_commands(rx, deferred, session_id) {
        return Ok(());
    }

    let result = loop {
        tokio::select! {
            _ = lifecycle.tick() => {
                if !session_is_active(session_id) {
                    break Ok(());
                }
            }
            event = event_stream.next() => {
                match event {
                    Some(Ok(reis::event::EiEvent::SeatAdded(added))) => {
                        use reis::event::DeviceCapability;
                        added.seat.bind_capabilities(
                            DeviceCapability::Keyboard
                                | DeviceCapability::Pointer
                                | DeviceCapability::Button
                                | DeviceCapability::Scroll
                                | DeviceCapability::Text,
                        );
                        // libei requests are buffered; without the flush the
                        // server never sees the capability bind and no input
                        // devices are created.
                        let _ = connection.flush();
                    }
                    Some(Ok(reis::event::EiEvent::DeviceAdded(added))) => {
                        state.add_device(&added.device);
                        // ei_device v3 requires an explicit ready request in
                        // response to done before the EIS server resumes the
                        // device. Without this modern libei servers never send
                        // DeviceResumed, so no input is ever injected.
                        if added.device.device().version() >= 3 {
                            added.device.device().ready();
                            let _ = connection.flush();
                        }
                    }
                    Some(Ok(reis::event::EiEvent::DeviceResumed(resumed))) => {
                        state.resume_device(&resumed.device, resumed.serial, &connection);
                    }
                    Some(Ok(reis::event::EiEvent::DevicePaused(paused))) => {
                        state.pause_device(&paused.device, paused.serial, &connection);
                    }
                    Some(Ok(reis::event::EiEvent::DeviceRemoved(removed))) => {
                        state.remove_device(&removed.device);
                    }
                    Some(Ok(reis::event::EiEvent::Disconnected(_))) | None => break Ok(()),
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        break Err(WaylandSessionError::Failed(format!(
                            "EIS event stream failed: {error}"
                        )));
                    }
                }

                if !notified && state.any_ready() {
                    notified = true;
                    info!("Remote-input EIS backend ready for {}", from_device);
                    notify(
                        "Remote input active",
                        &format!("{from_device} can now control this computer. Disable it in Settings."),
                    );
                }
            }
            command = rx.recv() => {
                match command {
                    None | Some(InputCommand::Close) => break Ok(()),
                    Some(InputCommand::WarmUp {
                        session_id: command_session_id,
                        from_device,
                    }) => {
                        // A replacement session can start while this one is
                        // already injecting. Hand its WarmUp back to the
                        // dispatcher instead of consuming it here.
                        if command_session_id == current_session()
                            && !session_blocked(command_session_id)
                            && command_session_id != session_id
                        {
                            deferred.push_back(InputCommand::WarmUp {
                                session_id: command_session_id,
                                from_device,
                            });
                            break Ok(());
                        }
                    }
                    Some(InputCommand::Batch {
                        session_id: command_session_id,
                        events,
                        ..
                    }) => {
                        if command_session_id == current_session()
                            && !session_blocked(command_session_id)
                            && command_session_id != session_id
                        {
                            // The replacement's WarmUp was dropped under
                            // backpressure. Stop this backend rather than
                            // injecting the replacement's first batch into the
                            // previous session.
                            break Ok(());
                        }
                        if command_session_id != current_session() {
                            continue;
                        }
                        if !get_remote_input_enabled_setting() {
                            break Ok(());
                        }
                        if state.any_ready() {
                            state.apply_events(&events, &connection);
                        } else {
                            debug!("Dropping remote-input batch before EIS devices are ready");
                        }
                    }
                }
            }
        }
    };

    state.release_all(&connection);
    result
}

/// Drop input that accumulated while the portal prompt was open. Returns false
/// if this session must stop. A `Close` is stale when a replacement session has
/// already been announced; that replacement's WarmUp is moved into `deferred`
/// so the dispatcher can start it after this backend closes. Batches are never
/// replayed after a prompt.
#[cfg(target_os = "linux")]
fn drain_stale_commands(
    rx: &mut mpsc::Receiver<InputCommand>,
    deferred: &mut VecDeque<InputCommand>,
    session_id: u64,
) -> bool {
    while let Ok(command) = rx.try_recv() {
        match command {
            InputCommand::Close => {
                // session_started queues Close before WarmUp when replacing a
                // stream. Only a Close with no current session means stop.
                if current_session() == 0 {
                    return false;
                }
            }
            InputCommand::WarmUp {
                session_id: command_session_id,
                from_device,
            } => {
                if command_session_id == current_session()
                    && !session_blocked(command_session_id)
                    && command_session_id != session_id
                {
                    deferred.push_back(InputCommand::WarmUp {
                        session_id: command_session_id,
                        from_device,
                    });
                    return false;
                }
            }
            InputCommand::Batch {
                session_id: command_session_id,
                ..
            } => {
                if command_session_id == current_session()
                    && !session_blocked(command_session_id)
                    && command_session_id != session_id
                {
                    // The replacement's WarmUp was dropped under backpressure.
                    // Stop this backend; the next event will start the new one.
                    // Do not replay this stale batch after a permission prompt.
                    return false;
                }
            }
        }
    }
    // The channel may have dropped a Close under backpressure, but the phone
    // session was still cleared. Never keep a portal/EIS session alive for it.
    current_session() != 0
}

// ============================================================================
// Wayland fallback: ydotool compatibility backend
// ============================================================================
//
// Compositors that do not implement xdg-desktop-portal's RemoteDesktop
// interface (most wlroots-based desktops without a remote-desktop portal)
// cannot be reached through libei. ydotool uses /dev/uinput instead. It needs
// a running ydotoold with permission to open /dev/uinput, usually provided by
// a udev rule or a system service.
//
// This path is deliberately only tried when the portal is absent, never when
// the user denied a portal prompt.

#[cfg(target_os = "linux")]
fn ydotool_available() -> bool {
    use std::process::{Command, Stdio};
    Command::new("ydotool")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

#[cfg(target_os = "linux")]
fn run_ydotool_command(args: &[&str]) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let output = Command::new("ydotool")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("could not run ydotool: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "ydotool {} failed: {}",
            args.first().copied().unwrap_or("command"),
            detail.trim()
        ))
    }
}

#[cfg(target_os = "linux")]
fn ydotool_type(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Feed text on stdin so it never appears in the process list or a command
    // line visible to other local users.
    let mut child = Command::new("ydotool")
        .args(["type", "--file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not run ydotool type: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|error| format!("could not write ydotool input: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("ydotool type failed: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        // Never include stderr here: ydotool could quote the text being typed.
        Err(format!("ydotool type failed ({})", output.status))
    }
}

#[cfg(target_os = "linux")]
fn flush_ydotool_motion(motion: &mut (f32, f32), scroll: &mut (f32, f32)) -> Result<(), String> {
    let dx = motion.0.round() as i32;
    let dy = motion.1.round() as i32;
    if dx != 0 || dy != 0 {
        let dx_arg = dx.to_string();
        let dy_arg = dy.to_string();
        run_ydotool_command(&["mousemove", "-x", &dx_arg, "-y", &dy_arg])?;
        // Keep sub-pixel motion for the next flush instead of losing it.
        motion.0 -= dx as f32;
        motion.1 -= dy as f32;
    }

    // Linux REL_WHEEL is positive upwards; the phone's finger delta is
    // positive downwards, like a touch scroll.
    let sx = (scroll.0 / 40.0).round() as i32;
    let sy = -((scroll.1 / 40.0).round() as i32);
    if sx != 0 || sy != 0 {
        let sx_arg = sx.to_string();
        let sy_arg = sy.to_string();
        run_ydotool_command(&["mousemove", "--wheel", "-x", &sx_arg, "-y", &sy_arg])?;
        // Keep the sub-notch remainder, otherwise very slow scrolling never
        // reaches a full wheel step.
        scroll.0 -= sx as f32 * 40.0;
        scroll.1 += sy as f32 * 40.0;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn release_ydotool_state(buttons: &mut Vec<u16>, keys: &mut Vec<u32>) {
    while let Some(button) = buttons.pop() {
        if let Some(base) = ydotool_button(button) {
            let value = format!("0x{:02X}", base | 0x80);
            let _ = run_ydotool_command(&["click", &value]);
        }
    }
    while let Some(keysym) = keys.pop() {
        if let Some(code) = ydotool_keycode(keysym) {
            let value = format!("{code}:0");
            let _ = run_ydotool_command(&["key", &value]);
        }
    }
}

#[cfg(target_os = "linux")]
async fn run_ydotool_session(
    rx: &mut mpsc::Receiver<InputCommand>,
    deferred: &mut VecDeque<InputCommand>,
    session_id: u64,
    from_device: &str,
) -> Result<(), String> {
    if !ydotool_available() {
        return Err("ydotool is not installed".to_string());
    }
    if !session_is_active(session_id) {
        return Ok(());
    }

    notify(
        "Remote input active",
        &format!(
            "{from_device} is using the ydotool compatibility backend. Install a RemoteDesktop portal for native permission controls."
        ),
    );

    let mut motion = (0.0f32, 0.0f32);
    let mut scroll = (0.0f32, 0.0f32);
    let mut held_buttons = Vec::new();
    let mut held_keys = Vec::new();
    let mut last_flush = Instant::now();

    loop {
        let command = match tokio::time::timeout(Duration::from_millis(16), rx.recv()).await {
            Ok(Some(command)) => command,
            Ok(None) => {
                let flush = flush_ydotool_motion(&mut motion, &mut scroll);
                release_ydotool_state(&mut held_buttons, &mut held_keys);
                flush?;
                return Ok(());
            }
            Err(_) => {
                if let Err(error) = flush_ydotool_motion(&mut motion, &mut scroll) {
                    release_ydotool_state(&mut held_buttons, &mut held_keys);
                    return Err(error);
                }
                last_flush = Instant::now();
                // `Close` can be dropped when the bounded channel is full.
                // Poll the lifecycle so a portal/stream replacement or a
                // setting change still tears this backend down promptly.
                if !session_is_active(session_id) {
                    release_ydotool_state(&mut held_buttons, &mut held_keys);
                    return Ok(());
                }
                continue;
            }
        };

        match command {
            InputCommand::Close => {
                if let Err(error) = flush_ydotool_motion(&mut motion, &mut scroll) {
                    release_ydotool_state(&mut held_buttons, &mut held_keys);
                    return Err(error);
                }
                release_ydotool_state(&mut held_buttons, &mut held_keys);
                return Ok(());
            }
            InputCommand::WarmUp {
                session_id: command_session_id,
                from_device,
            } => {
                if command_session_id == current_session()
                    && !session_blocked(command_session_id)
                    && command_session_id != session_id
                {
                    deferred.push_back(InputCommand::WarmUp {
                        session_id: command_session_id,
                        from_device,
                    });
                    if let Err(error) = flush_ydotool_motion(&mut motion, &mut scroll) {
                        release_ydotool_state(&mut held_buttons, &mut held_keys);
                        return Err(error);
                    }
                    release_ydotool_state(&mut held_buttons, &mut held_keys);
                    return Ok(());
                }
            }
            InputCommand::Batch {
                session_id: command_session_id,
                events,
                ..
            } => {
                if command_session_id != session_id || !session_is_active(session_id) {
                    release_ydotool_state(&mut held_buttons, &mut held_keys);
                    return Ok(());
                }

                for event in &events {
                    match event {
                        RemoteInputEvent::PointerMotion { dx, dy } => {
                            motion.0 += dx;
                            motion.1 += dy;
                        }
                        RemoteInputEvent::Scroll { dx, dy } => {
                            scroll.0 += dx;
                            scroll.1 += dy;
                        }
                        RemoteInputEvent::PointerButton { button, pressed } => {
                            if let Err(error) = flush_ydotool_motion(&mut motion, &mut scroll) {
                                release_ydotool_state(&mut held_buttons, &mut held_keys);
                                return Err(error);
                            }
                            let Some(base) = ydotool_button(*button) else {
                                continue;
                            };
                            let value = if *pressed {
                                format!("0x{:02X}", base | 0x40)
                            } else {
                                format!("0x{:02X}", base | 0x80)
                            };
                            if let Err(error) = run_ydotool_command(&["click", &value]) {
                                release_ydotool_state(&mut held_buttons, &mut held_keys);
                                return Err(error);
                            }
                            if *pressed {
                                held_buttons.push(*button);
                            } else {
                                held_buttons.retain(|held| held != button);
                            }
                        }
                        RemoteInputEvent::Key { keysym, pressed } => {
                            if let Err(error) = flush_ydotool_motion(&mut motion, &mut scroll) {
                                release_ydotool_state(&mut held_buttons, &mut held_keys);
                                return Err(error);
                            }
                            let Some(code) = ydotool_keycode(*keysym) else {
                                continue;
                            };
                            let value = format!("{code}:{}", if *pressed { 1 } else { 0 });
                            if let Err(error) = run_ydotool_command(&["key", &value]) {
                                release_ydotool_state(&mut held_buttons, &mut held_keys);
                                return Err(error);
                            }
                            if *pressed {
                                held_keys.push(*keysym);
                            } else {
                                held_keys.retain(|held| held != keysym);
                            }
                        }
                        RemoteInputEvent::Text { text } => {
                            if let Err(error) = flush_ydotool_motion(&mut motion, &mut scroll) {
                                release_ydotool_state(&mut held_buttons, &mut held_keys);
                                return Err(error);
                            }
                            if let Err(error) = ydotool_type(text) {
                                release_ydotool_state(&mut held_buttons, &mut held_keys);
                                return Err(error);
                            }
                        }
                    }
                }

                if last_flush.elapsed() >= Duration::from_millis(16) {
                    if let Err(error) = flush_ydotool_motion(&mut motion, &mut scroll) {
                        release_ydotool_state(&mut held_buttons, &mut held_keys);
                        return Err(error);
                    }
                    last_flush = Instant::now();
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn ydotool_button(button: u16) -> Option<u8> {
    use connected_core::input::{
        BTN_BACK, BTN_EXTRA, BTN_FORWARD, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE, BTN_TASK,
    };
    Some(match button {
        BTN_LEFT => 0x00,
        BTN_RIGHT => 0x01,
        BTN_MIDDLE => 0x02,
        BTN_SIDE => 0x03,
        BTN_EXTRA => 0x04,
        BTN_FORWARD => 0x05,
        BTN_BACK => 0x06,
        BTN_TASK => 0x07,
        _ => return None,
    })
}

/// Map X11 keysyms to Linux evdev keycodes for ydotool. Text uses
/// `ydotool type` (layout-aware); this table is only for shortcuts, modifiers
/// and special keys, and assumes a conventional US layout for letters.
#[cfg(target_os = "linux")]
#[allow(non_upper_case_globals)]
fn ydotool_keycode(keysym: u32) -> Option<u32> {
    use xkb_keysyms::*;
    let code = match keysym {
        KEY_BackSpace => 14,
        KEY_Tab => 15,
        KEY_Return => 28,
        KEY_Escape => 1,
        KEY_Delete => 111,
        KEY_Insert => 110,
        KEY_Home => 102,
        KEY_End => 107,
        KEY_Page_Up => 104,
        KEY_Page_Down => 109,
        KEY_Left => 105,
        KEY_Up => 103,
        KEY_Right => 106,
        KEY_Down => 108,
        KEY_Shift_L => 42,
        KEY_Shift_R => 54,
        KEY_Control_L => 29,
        KEY_Control_R => 97,
        KEY_Alt_L => 56,
        KEY_Alt_R => 100,
        KEY_Super_L => 125,
        KEY_Super_R => 126,
        KEY_Caps_Lock => 58,
        KEY_space => 57,
        KEY_F1 => 59,
        KEY_F2 => 60,
        KEY_F3 => 61,
        KEY_F4 => 62,
        KEY_F5 => 63,
        KEY_F6 => 64,
        KEY_F7 => 65,
        KEY_F8 => 66,
        KEY_F9 => 67,
        KEY_F10 => 68,
        KEY_F11 => 87,
        KEY_F12 => 88,
        _ if (0x20..=0x7e).contains(&keysym) => return ydotool_ascii(keysym as u8),
        _ => return None,
    };
    Some(code)
}

#[cfg(target_os = "linux")]
fn ydotool_ascii(byte: u8) -> Option<u32> {
    const LETTERS: [u32; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17,
        45, 21, 44,
    ];
    let lower = byte.to_ascii_lowercase();
    Some(match lower {
        b'a'..=b'z' => LETTERS[(lower - b'a') as usize],
        b'0' => 11,
        b'1'..=b'9' => (lower - b'1') as u32 + 2,
        b' ' => 57,
        b'-' => 12,
        b'=' => 13,
        b'[' => 26,
        b']' => 27,
        b'\\' => 43,
        b';' => 39,
        b'\'' => 40,
        b'`' => 41,
        b',' => 51,
        b'.' => 52,
        b'/' => 53,
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct KeyMapping {
    /// Linux evdev keycode (XKB keycode minus 8).
    code: u32,
    /// XKB shift level: 0 = base, 1 = Shift, 2 = AltGr, 3 = Shift+AltGr.
    level: u8,
}

#[cfg(target_os = "linux")]
struct KeymapData {
    keysyms: std::collections::HashMap<u32, KeyMapping>,
    chars: std::collections::HashMap<char, KeyMapping>,
    shift_code: Option<u32>,
    altgr_code: Option<u32>,
}

#[cfg(target_os = "linux")]
struct WaylandDevice {
    device: reis::ei::Device,
    pointer: Option<reis::ei::Pointer>,
    button: Option<reis::ei::Button>,
    scroll: Option<reis::ei::Scroll>,
    keyboard: Option<reis::ei::Keyboard>,
    text: Option<reis::ei::Text>,
    keymap: Option<KeymapData>,
    serial: u32,
    sequence: u32,
    resumed: bool,
}

#[cfg(target_os = "linux")]
struct WaylandState {
    devices: Vec<WaylandDevice>,
    /// Buttons pressed across event batches (e.g. a drag). Released when the
    /// session closes so a dropped phone cannot leave a button stuck down.
    held_buttons: Vec<u16>,
    /// Keycodes (including auto-pressed modifiers) currently held down.
    held_keys: Vec<u32>,
}

#[cfg(target_os = "linux")]
impl WaylandState {
    fn new() -> Self {
        Self {
            devices: Vec::new(),
            held_buttons: Vec::new(),
            held_keys: Vec::new(),
        }
    }

    fn add_device(&mut self, device: &reis::event::Device) {
        let pointer = device.interface::<reis::ei::Pointer>();
        let button = device.interface::<reis::ei::Button>();
        let scroll = device.interface::<reis::ei::Scroll>();
        let keyboard = device.interface::<reis::ei::Keyboard>();
        let text = device.interface::<reis::ei::Text>();
        if pointer.is_none()
            && button.is_none()
            && scroll.is_none()
            && keyboard.is_none()
            && text.is_none()
        {
            return;
        }

        // Build a plain-Rust keymap immediately and drop xkbcommon's keymap:
        // its raw pointer type is not Send, and this async future must remain
        // Send to run on the Tokio runtime.
        let keymap = if keyboard.is_some() {
            build_keymap(device)
        } else {
            None
        };

        self.devices.push(WaylandDevice {
            device: device.device().clone(),
            pointer,
            button,
            scroll,
            keyboard,
            text,
            keymap,
            serial: 0,
            sequence: 0,
            resumed: false,
        });
        debug!(
            "Remote-input EIS device added: type={:?}",
            device.device_type()
        );
    }

    fn resume_device(
        &mut self,
        device: &reis::event::Device,
        serial: u32,
        connection: &reis::event::Connection,
    ) {
        if let Some(state) = self
            .devices
            .iter_mut()
            .find(|state| state.device == *device.device())
        {
            state.serial = serial;
            state.resumed = true;
            state.device.start_emulating(serial, state.sequence);
            state.sequence = state.sequence.wrapping_add(1);
            let _ = connection.flush();
        }
    }

    fn pause_device(
        &mut self,
        device: &reis::event::Device,
        serial: u32,
        connection: &reis::event::Connection,
    ) {
        if let Some(state) = self
            .devices
            .iter_mut()
            .find(|state| state.device == *device.device())
            && state.resumed
        {
            state.device.stop_emulating(serial);
            state.resumed = false;
            let _ = connection.flush();
        }
    }

    fn remove_device(&mut self, device: &reis::event::Device) {
        self.devices
            .retain(|state| state.device != *device.device());
    }

    fn any_ready(&self) -> bool {
        self.devices.iter().any(|device| {
            device.resumed
                && (device.pointer.is_some()
                    || device.button.is_some()
                    || device.scroll.is_some()
                    || (device.keyboard.is_some() && device.keymap.is_some())
                    || device.text.is_some())
        })
    }

    #[allow(unsafe_code)]
    fn timestamp(&self) -> u64 {
        // The EIS timestamp is documented as CLOCK_MONOTONIC microseconds.
        // `Instant` only gives an opaque relative value, so query the clock
        // directly. If the call ever fails, zero is still a valid timestamp
        // and the frame is accepted; ordering within this client is preserved.
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: clock_gettime writes a `timespec` to the pointer we pass.
        let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        if result == 0 {
            now.tv_sec as u64 * 1_000_000 + now.tv_nsec as u64 / 1_000
        } else {
            0
        }
    }

    fn apply_events(&mut self, events: &[RemoteInputEvent], connection: &reis::event::Connection) {
        for event in events {
            match event {
                RemoteInputEvent::PointerMotion { dx, dy } => {
                    if let Some(index) = self
                        .devices
                        .iter()
                        .position(|device| device.resumed && device.pointer.is_some())
                    {
                        let device = &self.devices[index];
                        if let Some(pointer) = &device.pointer {
                            pointer.motion_relative(*dx, *dy);
                        }
                        self.frame_and_flush(index, connection);
                    }
                }
                RemoteInputEvent::PointerButton { button, pressed } => {
                    if let Some(index) = self
                        .devices
                        .iter()
                        .position(|device| device.resumed && device.button.is_some())
                    {
                        let device = &self.devices[index];
                        if let Some(button_iface) = &device.button {
                            let state = if *pressed {
                                reis::ei::button::ButtonState::Press
                            } else {
                                reis::ei::button::ButtonState::Released
                            };
                            button_iface.button(u32::from(*button), state);
                        }
                        self.frame_and_flush(index, connection);
                        if *pressed {
                            if !self.held_buttons.contains(button) {
                                self.held_buttons.push(*button);
                            }
                        } else {
                            self.held_buttons.retain(|held| held != button);
                        }
                    }
                }
                RemoteInputEvent::Scroll { dx, dy } => {
                    if let Some(index) = self
                        .devices
                        .iter()
                        .position(|device| device.resumed && device.scroll.is_some())
                    {
                        let device = &self.devices[index];
                        if let Some(scroll) = &device.scroll {
                            // libei's positive vertical scroll is up, while the
                            // phone sends positive dy for "scroll down" (the
                            // same convention as enigo and REL_WHEEL below).
                            scroll.scroll(*dx, -*dy);
                        }
                        self.frame_and_flush(index, connection);
                    }
                }
                RemoteInputEvent::Key { keysym, pressed } => {
                    self.apply_key(*keysym, *pressed, connection);
                }
                RemoteInputEvent::Text { text } => {
                    self.apply_text(text, connection);
                }
            }
        }
    }

    fn apply_key(&mut self, keysym: u32, pressed: bool, connection: &reis::event::Connection) {
        let Some(index) = self
            .devices
            .iter()
            .position(|device| device.resumed && device.keyboard.is_some())
        else {
            return;
        };
        let Some((code, modifiers)) = self.devices[index].keymap.as_ref().and_then(|keymap| {
            let mapping = keymap.keysyms.get(&keysym).copied()?;
            let modifiers = keymap.modifiers_for(mapping.level)?;
            Some((mapping.code, modifiers))
        }) else {
            return;
        };
        let Some(keyboard) = self.devices[index].keyboard.as_ref() else {
            return;
        };

        if pressed {
            for modifier in &modifiers {
                keyboard.key(*modifier, reis::ei::keyboard::KeyState::Press);
            }
            keyboard.key(code, reis::ei::keyboard::KeyState::Press);
        } else {
            keyboard.key(code, reis::ei::keyboard::KeyState::Released);
            for modifier in modifiers.iter().rev() {
                keyboard.key(*modifier, reis::ei::keyboard::KeyState::Released);
            }
        }
        self.frame_and_flush(index, connection);

        if pressed {
            if !self.held_keys.contains(&code) {
                self.held_keys.push(code);
            }
            for modifier in modifiers {
                if !self.held_keys.contains(&modifier) {
                    self.held_keys.push(modifier);
                }
            }
        } else {
            self.held_keys.retain(|held| *held != code);
            for modifier in modifiers {
                self.held_keys.retain(|held| *held != modifier);
            }
        }
    }

    fn apply_text(&self, text: &str, connection: &reis::event::Connection) {
        // Prefer the EIS text interface. Unlike keycodes it is independent of
        // the desktop layout, modifier state (CapsLock/AltGr) and can carry
        // arbitrary Unicode. KDE Connect uses the same path for plain text.
        if let Some(index) = self
            .devices
            .iter()
            .position(|device| device.resumed && device.text.is_some())
        {
            let device = &self.devices[index];
            if let Some(text_iface) = &device.text {
                let mut remaining = text;
                while !remaining.is_empty() {
                    // The protocol allows at most 254 characters/255 bytes
                    // including NUL. Chunk by UTF-8 byte length so multi-byte
                    // characters are never split.
                    let mut len = remaining.len().min(254);
                    while len > 0 && !remaining.is_char_boundary(len) {
                        len -= 1;
                    }
                    if len == 0 {
                        break;
                    }
                    let (chunk, rest) = remaining.split_at(len);
                    text_iface.utf8(chunk);
                    // One ei_text.utf8 request per frame is required.
                    self.frame_and_flush(index, connection);
                    remaining = rest;
                }
                return;
            }
        }

        let Some(index) = self
            .devices
            .iter()
            .position(|device| device.resumed && device.keyboard.is_some())
        else {
            return;
        };
        let device = &self.devices[index];
        let (Some(keyboard), Some(keymap)) = (&device.keyboard, &device.keymap) else {
            return;
        };

        let mut skipped = false;
        for character in text.chars() {
            let keysym = match character {
                '\n' => xkbcommon::xkb::keysyms::KEY_Return,
                '\t' => xkbcommon::xkb::keysyms::KEY_Tab,
                _ => xkbcommon::xkb::Keysym::from_char(character).raw(),
            };
            let mapping = keymap
                .chars
                .get(&character)
                .copied()
                .or_else(|| keymap.keysyms.get(&keysym).copied());
            let Some(mapping) = mapping else {
                skipped = true;
                continue;
            };
            let Some(modifiers) = keymap.modifiers_for(mapping.level) else {
                skipped = true;
                continue;
            };

            for modifier in &modifiers {
                keyboard.key(*modifier, reis::ei::keyboard::KeyState::Press);
            }
            keyboard.key(mapping.code, reis::ei::keyboard::KeyState::Press);
            // EIS forbids more than one key request for the same key inside a
            // single frame, so press and release must be in separate frames.
            self.frame_and_flush(index, connection);

            keyboard.key(mapping.code, reis::ei::keyboard::KeyState::Released);
            for modifier in modifiers.iter().rev() {
                keyboard.key(*modifier, reis::ei::keyboard::KeyState::Released);
            }
            self.frame_and_flush(index, connection);
        }

        if skipped {
            // Never log the text itself (it is sensitive input).
            debug!(
                "Remote-input text: some characters were not representable in the current layout"
            );
        }
    }

    fn frame_and_flush(&self, index: usize, connection: &reis::event::Connection) {
        if let Some(device) = self.devices.get(index) {
            device.device.frame(device.serial, self.timestamp());
        }
        let _ = connection.flush();
    }

    /// Release any state that outlived its press event. Called when the phone
    /// disconnects so a lost session cannot leave the desktop with a held
    /// mouse button or modifier.
    fn release_all(&mut self, connection: &reis::event::Connection) {
        let buttons = std::mem::take(&mut self.held_buttons);
        let keys = std::mem::take(&mut self.held_keys);
        for index in 0..self.devices.len() {
            if !self.devices[index].resumed {
                continue;
            }
            let mut changed = false;
            if let Some(button_iface) = &self.devices[index].button {
                for button in &buttons {
                    button_iface
                        .button(u32::from(*button), reis::ei::button::ButtonState::Released);
                    changed = true;
                }
            }
            if let Some(keyboard) = &self.devices[index].keyboard {
                for code in &keys {
                    keyboard.key(*code, reis::ei::keyboard::KeyState::Released);
                    changed = true;
                }
            }
            if changed {
                self.frame_and_flush(index, connection);
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl KeymapData {
    fn modifiers_for(&self, level: u8) -> Option<Vec<u32>> {
        let mut modifiers = Vec::with_capacity(2);
        if level & 1 != 0 {
            modifiers.push(self.shift_code?);
        }
        if level & 2 != 0 {
            modifiers.push(self.altgr_code?);
        }
        Some(modifiers)
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn build_keymap(device: &reis::event::Device) -> Option<KeymapData> {
    use xkbcommon::xkb;

    let keymap = device.keymap()?;
    let fd = keymap.fd.try_clone().ok()?;
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    // SAFETY: `fd` is a freshly duplicated, valid memfd for the keymap that
    // the EIS server just sent us, and `size` is its advertised length.
    let compiled = unsafe {
        xkb::Keymap::new_from_fd(
            &context,
            fd,
            keymap.size as usize,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
    }
    .ok()
    .flatten()?;

    let mut keysyms = std::collections::HashMap::new();
    let mut chars = std::collections::HashMap::new();
    let mut shift_code = None;
    let mut altgr_code = None;

    let min = compiled.min_keycode().raw();
    let max = compiled.max_keycode().raw();
    for code in min..=max {
        // xkbcommon keycodes are evdev codes + 8. Valid evdev codes start at
        // 1, so anything below 9 has no representable libei keycode.
        let Some(evdev) = code.checked_sub(8) else {
            continue;
        };
        if evdev == 0 {
            continue;
        }

        for level in 0..=3u32 {
            for keysym in compiled.key_get_syms_by_level(xkb::Keycode::new(code), 0, level) {
                if level == 0 {
                    if keysym.raw() == xkb::keysyms::KEY_Shift_L
                        || keysym.raw() == xkb::keysyms::KEY_Shift_R
                    {
                        shift_code = Some(evdev);
                    }
                    if keysym.raw() == xkb::keysyms::KEY_ISO_Level3_Shift
                        || keysym.raw() == xkb::keysyms::KEY_Alt_R
                    {
                        altgr_code = Some(evdev);
                    }
                }

                let mapping = KeyMapping {
                    code: evdev,
                    level: level as u8,
                };
                keysyms.entry(keysym.raw()).or_insert(mapping);
                if let Some(character) = keysym.key_char() {
                    chars.entry(character).or_insert(mapping);
                }
            }
        }
    }

    if shift_code.is_none() {
        debug!("Remote-input keymap has no Shift key; shifted characters may not work");
    }
    Some(KeymapData {
        keysyms,
        chars,
        shift_code,
        altgr_code,
    })
}
