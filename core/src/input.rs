//! Remote input protocol.
//!
//! Remote input travels on a dedicated, long-lived QUIC stream rather than the
//! control-message channel. That keeps high-frequency pointer events ordered,
//! lets the sender batch them, and — importantly — keeps keystrokes out of the
//! control-message debug logs.
//!
//! Frames are length-prefixed postcard payloads. The stream starts with a
//! [`RemoteInputMessage::Start`] handshake; the receiver answers `Started` or
//! `Denied` before any input events are accepted.

use crate::error::{ConnectedError, Result};
use serde::{Deserialize, Serialize};

/// Version of the remote-input stream protocol.
pub const INPUT_PROTOCOL_VERSION: u32 = 1;

/// Maximum encoded frame size accepted by either side. Pointer batches are
/// tiny; this bound mainly prevents a malicious peer from forcing a large
/// allocation on a trusted remote-input stream.
pub const MAX_INPUT_FRAME_SIZE: usize = 64 * 1024;

/// Maximum number of individual events in one batch.
pub const MAX_INPUT_EVENTS_PER_BATCH: usize = 256;

/// Maximum UTF-8 bytes in a single [`RemoteInputEvent::Text`] event.
pub const MAX_INPUT_TEXT_BYTES: usize = 4096;

/// Absolute bound for pointer/scroller deltas. Values outside this range are
/// treated as a protocol violation rather than being clamped silently.
pub const MAX_INPUT_DELTA: f32 = 100_000.0;

/// Linux `BTN_*` mouse button codes understood by every desktop backend.
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_SIDE: u16 = 0x113;
pub const BTN_EXTRA: u16 = 0x114;
pub const BTN_FORWARD: u16 = 0x115;
pub const BTN_BACK: u16 = 0x116;
pub const BTN_TASK: u16 = 0x117;

/// One input action from the phone.
///
/// This enum is private to the input stream: it never travels through the
/// control-message `Message` enum, so adding variants does not affect the
/// postcard compatibility contract used by older peers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RemoteInputEvent {
    /// Relative pointer motion in device-independent pixels.
    PointerMotion { dx: f32, dy: f32 },
    /// Mouse button state. `button` is a Linux `BTN_*` code.
    PointerButton { button: u16, pressed: bool },
    /// Relative scroll. Positive `dy` scrolls down on conventional layouts.
    Scroll { dx: f32, dy: f32 },
    /// Key press/release using an X11 keysym (not a keycode).
    Key { keysym: u32, pressed: bool },
    /// Type a UTF-8 string. Used for characters that don't map to a simple
    /// keysym on the phone's keyboard.
    Text { text: String },
}

/// Stream handshake and event envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RemoteInputMessage {
    Start { version: u32 },
    Started,
    Denied { reason: String },
    Events(Vec<RemoteInputEvent>),
    Stop,
}

/// Why a remote-input stream was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteInputError {
    Disabled,
    AlreadyActive,
    VersionUnsupported,
    Invalid(String),
    Unavailable(String),
}

impl std::fmt::Display for RemoteInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "Remote input is disabled"),
            Self::AlreadyActive => write!(f, "Another remote-input session is active"),
            Self::VersionUnsupported => write!(f, "Remote-input protocol version is unsupported"),
            Self::Invalid(reason) => write!(f, "Invalid remote input: {reason}"),
            Self::Unavailable(reason) => write!(f, "Remote input is unavailable: {reason}"),
        }
    }
}

pub fn is_valid_button(button: u16) -> bool {
    (BTN_LEFT..=BTN_TASK).contains(&button)
}

/// Validate a single event. No strings are logged here or by callers, since
/// they may contain typed text.
pub fn validate_event(event: &RemoteInputEvent) -> std::result::Result<(), String> {
    match event {
        RemoteInputEvent::PointerMotion { dx, dy } => validate_delta(*dx, *dy),
        RemoteInputEvent::Scroll { dx, dy } => validate_delta(*dx, *dy),
        RemoteInputEvent::PointerButton { button, .. } => {
            if is_valid_button(*button) {
                Ok(())
            } else {
                Err(format!("unsupported mouse button 0x{button:02x}"))
            }
        }
        RemoteInputEvent::Key { keysym, .. } => {
            if *keysym == 0 || *keysym > 0x1fff_ffff {
                Err(format!("invalid keysym 0x{keysym:08x}"))
            } else {
                Ok(())
            }
        }
        RemoteInputEvent::Text { text } => {
            if text.is_empty() {
                return Err("empty text event".to_string());
            }
            if text.len() > MAX_INPUT_TEXT_BYTES {
                return Err("text event too large".to_string());
            }
            if text.contains('\0') {
                return Err("text event contains NUL".to_string());
            }
            Ok(())
        }
    }
}

fn validate_delta(dx: f32, dy: f32) -> std::result::Result<(), String> {
    if !dx.is_finite() || !dy.is_finite() {
        return Err("non-finite pointer delta".to_string());
    }
    if dx.abs() > MAX_INPUT_DELTA || dy.abs() > MAX_INPUT_DELTA {
        return Err("pointer delta out of range".to_string());
    }
    Ok(())
}

/// Validate a batch before it enters the sender channel or is emitted to the
/// injection backend.
pub fn validate_events(events: &[RemoteInputEvent]) -> std::result::Result<(), String> {
    if events.is_empty() {
        return Err("empty input batch".to_string());
    }
    if events.len() > MAX_INPUT_EVENTS_PER_BATCH {
        return Err(format!(
            "input batch exceeds {MAX_INPUT_EVENTS_PER_BATCH} events"
        ));
    }
    for event in events {
        validate_event(event)?;
    }
    Ok(())
}

/// Encode a length-prefixed frame.
pub fn encode_frame(message: &RemoteInputMessage) -> Result<Vec<u8>> {
    let payload = postcard::to_stdvec(message).map_err(|error| {
        ConnectedError::Protocol(format!("remote-input encode failed: {error}"))
    })?;
    if payload.is_empty() || payload.len() > MAX_INPUT_FRAME_SIZE {
        return Err(ConnectedError::Protocol(format!(
            "remote-input frame exceeds {MAX_INPUT_FRAME_SIZE} bytes"
        )));
    }
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode a payload (without the length prefix).
pub fn decode_payload(payload: &[u8]) -> Result<RemoteInputMessage> {
    if payload.is_empty() || payload.len() > MAX_INPUT_FRAME_SIZE {
        return Err(ConnectedError::Protocol(
            "remote-input frame has invalid length".to_string(),
        ));
    }
    let (message, remaining) = postcard::take_from_bytes(payload).map_err(|error| {
        ConnectedError::Protocol(format!("remote-input decode failed: {error}"))
    })?;
    if !remaining.is_empty() {
        return Err(ConnectedError::Protocol(
            "remote-input frame contains trailing bytes".to_string(),
        ));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let message = RemoteInputMessage::Events(vec![
            RemoteInputEvent::PointerMotion { dx: 1.25, dy: -2.5 },
            RemoteInputEvent::Text {
                text: "hello".to_string(),
            },
        ]);
        let frame = encode_frame(&message).unwrap();
        assert_eq!(frame.len(), 4 + frame.len() - 4);
        let len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
        assert_eq!(len, frame.len() - 4);
        assert_eq!(decode_payload(&frame[4..]).unwrap(), message);
    }

    #[test]
    fn rejects_bad_events() {
        assert!(validate_events(&[]).is_err());
        assert!(
            validate_events(&[RemoteInputEvent::PointerMotion {
                dx: f32::NAN,
                dy: 0.0
            }])
            .is_err()
        );
        assert!(
            validate_events(&[RemoteInputEvent::PointerButton {
                button: 0xff,
                pressed: true
            }])
            .is_err()
        );
        assert!(
            validate_events(&[RemoteInputEvent::Text {
                text: "\0".to_string()
            }])
            .is_err()
        );
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut payload = postcard::to_stdvec(&RemoteInputMessage::Stop).unwrap();
        payload.push(0);
        assert!(decode_payload(&payload).is_err());
    }
}
