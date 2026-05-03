#![allow(unsafe_code)]

use block2::RcBlock;
use connected_core::{MediaCommand, MediaState};
use core_foundation::base::{CFGetTypeID, TCFType};
use core_foundation::dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef};
use core_foundation::number::{CFNumber, CFNumberGetTypeID, CFNumberRef};
use core_foundation::string::{CFString, CFStringGetTypeID, CFStringRef};
use libloading::Library;
use std::ffi::c_void;
use std::process::Command;
use std::ptr;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

const MEDIA_REMOTE_PATH: &str =
    "/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote";
const DISPATCH_QUEUE_PRIORITY_DEFAULT: isize = 0;

const MR_COMMAND_PLAY: u32 = 0;
const MR_COMMAND_PAUSE: u32 = 1;
const MR_COMMAND_TOGGLE_PLAY_PAUSE: u32 = 2;
const MR_COMMAND_STOP: u32 = 3;
const MR_COMMAND_NEXT_TRACK: u32 = 4;
const MR_COMMAND_PREVIOUS_TRACK: u32 = 5;

type MRMediaRemoteSendCommand = unsafe extern "C" fn(u32, *const c_void);
type MRMediaRemoteGetNowPlayingInfo = unsafe extern "C" fn(*mut c_void, *mut c_void);

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
}

struct MediaRemote {
    _library: Library,
    send_command: MRMediaRemoteSendCommand,
    get_now_playing_info: MRMediaRemoteGetNowPlayingInfo,
}

impl MediaRemote {
    fn load() -> Result<Self, String> {
        // MediaRemote is private and lives in the dyld shared cache on modern macOS,
        // so keep it dynamically loaded instead of linking the app against it.
        let library = unsafe { Library::new(MEDIA_REMOTE_PATH) }
            .map_err(|e| format!("failed to load MediaRemote: {e}"))?;
        let send_command = unsafe {
            *library
                .get::<MRMediaRemoteSendCommand>(b"MRMediaRemoteSendCommand\0")
                .map_err(|e| format!("missing MRMediaRemoteSendCommand: {e}"))?
        };
        let get_now_playing_info = unsafe {
            *library
                .get::<MRMediaRemoteGetNowPlayingInfo>(b"MRMediaRemoteGetNowPlayingInfo\0")
                .map_err(|e| format!("missing MRMediaRemoteGetNowPlayingInfo: {e}"))?
        };

        Ok(Self {
            _library: library,
            send_command,
            get_now_playing_info,
        })
    }
}

static MEDIA_REMOTE: OnceLock<Result<MediaRemote, String>> = OnceLock::new();

fn media_remote() -> Result<&'static MediaRemote, String> {
    MEDIA_REMOTE
        .get_or_init(MediaRemote::load)
        .as_ref()
        .map_err(Clone::clone)
}

pub fn control_media(command: MediaCommand) -> Result<(), String> {
    match command {
        MediaCommand::VolumeUp | MediaCommand::VolumeDown | MediaCommand::Mute => {
            control_system_volume(command)
        }
        command => {
            let mr_command = match command {
                MediaCommand::Play => MR_COMMAND_PLAY,
                MediaCommand::Pause => MR_COMMAND_PAUSE,
                MediaCommand::PlayPause => MR_COMMAND_TOGGLE_PLAY_PAUSE,
                MediaCommand::Next => MR_COMMAND_NEXT_TRACK,
                MediaCommand::Previous => MR_COMMAND_PREVIOUS_TRACK,
                MediaCommand::Stop => MR_COMMAND_STOP,
                MediaCommand::VolumeUp | MediaCommand::VolumeDown | MediaCommand::Mute => {
                    unreachable!()
                }
            };

            let media_remote = media_remote()?;
            unsafe { (media_remote.send_command)(mr_command, ptr::null()) };
            Ok(())
        }
    }
}

pub fn current_media_state() -> Option<MediaState> {
    let media_remote = media_remote().ok()?;
    let (tx, rx) = mpsc::channel();

    let block = RcBlock::new(move |info: *const c_void| {
        let state = unsafe { media_state_from_dictionary(info as CFDictionaryRef) };
        let _ = tx.send(state);
    });

    let queue = unsafe { dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0) };
    if queue.is_null() {
        return None;
    }

    unsafe {
        (media_remote.get_now_playing_info)(queue, RcBlock::as_ptr(&block).cast());
    }

    rx.recv_timeout(Duration::from_millis(750)).ok().flatten()
}

fn control_system_volume(command: MediaCommand) -> Result<(), String> {
    let script = match command {
        MediaCommand::VolumeUp => {
            r#"
set currentVolume to output volume of (get volume settings)
if currentVolume is missing value then error "No output device"
set newVolume to currentVolume + 5
if newVolume > 100 then set newVolume to 100
set volume output volume newVolume
"#
        }
        MediaCommand::VolumeDown => {
            r#"
set currentVolume to output volume of (get volume settings)
if currentVolume is missing value then error "No output device"
set newVolume to currentVolume - 5
if newVolume < 0 then set newVolume to 0
set volume output volume newVolume
"#
        }
        MediaCommand::Mute => {
            r#"
if output muted of (get volume settings) then
    set volume without output muted
else
    set volume with output muted
end if
"#
        }
        _ => return Ok(()),
    };

    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("failed to run osascript: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err("osascript volume command failed".to_string())
        } else {
            Err(stderr)
        }
    }
}

unsafe fn media_state_from_dictionary(info: CFDictionaryRef) -> Option<MediaState> {
    if info.is_null() {
        return None;
    }

    let title = unsafe { dictionary_string(info, "kMRMediaRemoteNowPlayingInfoTitle") };
    let artist = unsafe { dictionary_string(info, "kMRMediaRemoteNowPlayingInfoArtist") };
    let album = unsafe { dictionary_string(info, "kMRMediaRemoteNowPlayingInfoAlbum") };
    let playing = unsafe { dictionary_number(info, "kMRMediaRemoteNowPlayingInfoPlaybackRate") }
        .map(|rate| rate > 0.0)
        .unwrap_or(false);

    if title.is_none() && artist.is_none() && album.is_none() && !playing {
        return None;
    }

    Some(MediaState {
        title,
        artist,
        album,
        playing,
    })
}

unsafe fn dictionary_string(info: CFDictionaryRef, key: &'static str) -> Option<String> {
    let value = unsafe { dictionary_value(info, key) }?;
    if unsafe { CFGetTypeID(value as _) } != unsafe { CFStringGetTypeID() } {
        return None;
    }

    let value = unsafe { CFString::wrap_under_get_rule(value as CFStringRef) }.to_string();
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

unsafe fn dictionary_number(info: CFDictionaryRef, key: &'static str) -> Option<f64> {
    let value = unsafe { dictionary_value(info, key) }?;
    if unsafe { CFGetTypeID(value as _) } != unsafe { CFNumberGetTypeID() } {
        return None;
    }

    unsafe { CFNumber::wrap_under_get_rule(value as CFNumberRef) }.to_f64()
}

unsafe fn dictionary_value(info: CFDictionaryRef, key: &'static str) -> Option<*const c_void> {
    let key = CFString::from_static_string(key);
    let mut value = ptr::null();
    let found = unsafe {
        CFDictionaryGetValueIfPresent(info, key.as_CFTypeRef() as *const c_void, &mut value)
    };

    if found == 0 || value.is_null() {
        None
    } else {
        Some(value)
    }
}
