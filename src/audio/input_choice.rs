//! Which microphone the input stream uses: a named device while it is
//! connected, otherwise the built-in microphone or the system default, and
//! the built-in microphone after the device in use disappears, until the
//! system default changes again.

use cpal::traits::DeviceTrait;
use objc2_av_foundation::{
    AVCaptureDeviceDiscoverySession, AVCaptureDevicePosition, AVCaptureDeviceTypeMicrophone,
    AVMediaTypeAudio,
};
use objc2_foundation::NSArray;

/// Shown when recording is asked for and no microphone is connected.
pub const NO_MICROPHONE_MESSAGE: &str =
    "No microphone is available. Connect one, or choose one in Settings > Microphone.";
/// Shown when the microphone disappears while recording.
pub const MICROPHONE_LOST_MESSAGE: &str =
    "The microphone disconnected while recording. Press the record shortcut to keep dictating; simple-ptt switches to another microphone.";

/// `kAudioDeviceTransportTypeBuiltIn` ('bltn', CoreAudio's
/// `AudioHardwareBase.h`), the transport type of the Mac's own microphone.
const BUILT_IN_TRANSPORT_TYPE: i32 = i32::from_be_bytes(*b"bltn");

/// Which of the available inputs is taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputChoice {
    /// The device named in `mic.audio_device`.
    Configured,
    /// The Mac's built-in microphone.
    BuiltIn,
    /// The system default input device.
    SystemDefault,
}

/// What is connected now. `configured` is `None` when `mic.audio_device`
/// follows the system default, else whether the named device is connected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputAvailability {
    pub configured: Option<bool>,
    pub built_in: bool,
    pub system_default: bool,
}

/// The input to use, or `None` when there is no microphone at all.
/// `prefer_built_in` is `BuiltInPreference::is_active`.
pub fn choose_input(available: InputAvailability, prefer_built_in: bool) -> Option<InputChoice> {
    match available.configured {
        Some(true) => Some(InputChoice::Configured),
        Some(false) if available.built_in => Some(InputChoice::BuiltIn),
        None if prefer_built_in && available.built_in => Some(InputChoice::BuiltIn),
        _ if available.system_default => Some(InputChoice::SystemDefault),
        _ if available.built_in => Some(InputChoice::BuiltIn),
        _ => None,
    }
}

/// Whether the built-in microphone is preferred over the system default:
/// from when the device in use disappears, for as long as the system default
/// stays the device macOS picked then (by its UID, or no default at all).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BuiltInPreference {
    default_when_lost: Option<Option<String>>,
}

impl BuiltInPreference {
    pub fn is_active(&self) -> bool {
        self.default_when_lost.is_some()
    }

    /// The preference after a look at the inputs: `device_in_use_lost` when
    /// the stream's device is no longer connected, `follows_system_default`
    /// when `mic.audio_device` names no device, `system_default` the UID of
    /// the default input now.
    pub fn next(&self, device_in_use_lost: bool, follows_system_default: bool, system_default: Option<&str>) -> Self {
        let system_default = system_default.map(str::to_owned);
        let default_when_lost = if !follows_system_default {
            None
        } else if device_in_use_lost {
            Some(system_default)
        } else {
            self.default_when_lost.clone().filter(|lost_default| *lost_default == system_default)
        };
        Self { default_when_lost }
    }
}

/// The Core Audio UID of `device` (`kAudioDevicePropertyDeviceUID`, which
/// cpal reports as the device ID on macOS).
pub fn device_uid(device: &cpal::Device) -> Option<String> {
    device.id().ok().map(|id| id.id().to_owned())
}

/// The UIDs of the Mac's built-in microphones: capture devices whose
/// transport type is built-in. `AVCaptureDevice.uniqueID` is the device's
/// Core Audio UID.
pub fn built_in_microphone_uids() -> Vec<String> {
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Vec::new();
    };
    let device_types = NSArray::from_slice(&[unsafe { AVCaptureDeviceTypeMicrophone }]);
    // SAFETY: the device types and media type are AVFoundation constants.
    let session = unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &device_types,
            Some(audio),
            AVCaptureDevicePosition::Unspecified,
        )
    };
    // SAFETY: `devices`, `transportType` and `uniqueID` are plain getters.
    unsafe { session.devices() }
        .iter()
        .filter(|device| unsafe { device.transportType() } == BUILT_IN_TRANSPORT_TYPE)
        .map(|device| unsafe { device.uniqueID() }.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(configured: Option<bool>, built_in: bool, system_default: bool) -> InputAvailability {
        InputAvailability { configured, built_in, system_default }
    }

    #[test]
    fn a_connected_named_device_is_used() {
        assert_eq!(choose_input(available(Some(true), true, true), false), Some(InputChoice::Configured));
        assert_eq!(choose_input(available(Some(true), true, true), true), Some(InputChoice::Configured));
    }

    #[test]
    fn a_missing_named_device_falls_back_to_the_built_in_mic_then_the_default() {
        assert_eq!(choose_input(available(Some(false), true, true), false), Some(InputChoice::BuiltIn));
        assert_eq!(choose_input(available(Some(false), false, true), false), Some(InputChoice::SystemDefault));
        assert_eq!(choose_input(available(Some(false), false, false), false), None);
    }

    #[test]
    fn the_system_default_is_followed_unless_the_built_in_mic_is_preferred() {
        assert_eq!(choose_input(available(None, true, true), false), Some(InputChoice::SystemDefault));
        assert_eq!(choose_input(available(None, true, true), true), Some(InputChoice::BuiltIn));
        // Preferred but absent: the default.
        assert_eq!(choose_input(available(None, false, true), true), Some(InputChoice::SystemDefault));
        assert_eq!(choose_input(available(None, false, false), true), None);
    }

    #[test]
    fn with_no_default_any_built_in_mic_is_used() {
        assert_eq!(choose_input(available(None, true, false), false), Some(InputChoice::BuiltIn));
    }

    #[test]
    fn losing_the_device_in_use_prefers_the_built_in_mic_while_the_default_stays() {
        let lost = BuiltInPreference::default().next(true, true, Some("lg-usb"));
        assert!(lost.is_active());
        // macOS keeps the monitor's mic as the default: stay on the built-in mic.
        let kept = lost.next(false, true, Some("lg-usb"));
        assert!(kept.is_active());
        // The default changes (the Bluetooth mic reconnects): follow it again.
        let followed = kept.next(false, true, Some("airpods"));
        assert!(!followed.is_active());
    }

    #[test]
    fn a_named_device_never_prefers_the_built_in_mic_over_itself() {
        let named = BuiltInPreference::default().next(true, false, Some("lg-usb"));
        assert!(!named.is_active());
    }

    #[test]
    fn the_default_disappearing_too_keeps_the_built_in_mic() {
        // Both the lost device and the next default are gone: no default now.
        let lost = BuiltInPreference::default().next(true, true, None);
        assert!(lost.is_active());
        assert!(lost.next(false, true, None).is_active());
        // A new default appears later: follow it.
        assert!(!lost.next(false, true, Some("airpods")).is_active());
    }
}
