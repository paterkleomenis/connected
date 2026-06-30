#![allow(unsafe_code)]
//! Windows COM audio control. Isolated here because COM interop requires `unsafe`.

use connected_core::MediaCommand;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{MMDeviceEnumerator, eConsole, eRender};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoInitialize, CoUninitialize};

pub fn control_system_volume(cmd: MediaCommand) -> windows::core::Result<()> {
    // SAFETY: We initialize COM on this thread and ensure
    // it's uninitialized via RAII guard when the function exits.
    // All COM interface calls are within this unsafe block
    // and their validity is guaranteed by the Windows API.
    unsafe {
        let hr = CoInitialize(None);
        if hr.is_err() && hr.0 != -2147417850 {
            // S_FALSE (already initialized) is OK
            return Err(hr.into());
        }

        // Ensure COM is uninitialized on exit
        struct ComGuard;
        impl Drop for ComGuard {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _guard = ComGuard;

        // Get the default audio endpoint
        let enumerator = CoCreateInstance::<_, windows::Win32::Media::Audio::IMMDeviceEnumerator>(
            &MMDeviceEnumerator,
            None,
            CLSCTX_ALL,
        )?;

        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;

        let endpoint = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?;

        if matches!(cmd, MediaCommand::Mute) {
            let is_muted = endpoint.GetMute()?;
            endpoint.SetMute(!is_muted.as_bool(), &windows::core::GUID::zeroed())?;
        } else {
            let current_vol = endpoint.GetMasterVolumeLevelScalar()?;

            let step = 0.05; // 5% volume step
            let new_vol = match cmd {
                MediaCommand::VolumeUp => (current_vol + step).min(1.0),
                MediaCommand::VolumeDown => (current_vol - step).max(0.0),
                _ => current_vol,
            };

            endpoint.SetMasterVolumeLevelScalar(new_vol, &windows::core::GUID::zeroed())?;
        }
    }

    Ok(())
}
