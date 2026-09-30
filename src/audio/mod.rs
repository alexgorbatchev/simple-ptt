pub mod devices;
pub mod stream;

pub use devices::*;
pub use stream::*;

use cpal::traits::{HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
static REGISTER_LISTENER: Once = Once::new();

use crate::config::MicConfig;
use crate::settings::LiveConfigStore;
use crate::state::AppState;
use crate::transcription::TranscriptionController;

const STREAM_STALL_TIMEOUT: Duration = Duration::from_millis(1500);
const REBUILD_RETRY_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Clone, Debug, PartialEq, Eq)]
enum PreviewDeviceState {
    Disabled,
    Override(Option<String>),
}

pub struct AudioController {
    active_stream: Mutex<Option<ActiveAudioStream>>,
    pending_config: Mutex<Option<MicConfig>>,
    config_store: LiveConfigStore,
    state: Arc<AppState>,
    transcription_controller: TranscriptionController,
    last_rebuild_attempt: Mutex<Option<Instant>>,
    preview_audio_device: Mutex<PreviewDeviceState>,
}

struct ActiveAudioStream {
    configured_audio_device: Option<String>,
    actual_device: cpal::Device,
    actual_audio_device_name: Option<String>,
    requested_sample_rate: u32,
    _stream: Stream,
    healthy: Arc<AtomicBool>,
    last_callback_millis: Arc<AtomicU64>,
}

impl ActiveAudioStream {
    fn new(handle: InputStreamHandle, mic_config: &MicConfig) -> Self {
        Self {
            configured_audio_device: mic_config.audio_device.clone(),
            actual_device: handle.device,
            actual_audio_device_name: handle.device_name,
            requested_sample_rate: mic_config.sample_rate,
            _stream: handle.stream,
            healthy: handle.healthy,
            last_callback_millis: handle.last_callback_millis,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioConfigApplyEffect {
    AppliedNow,
    DeferredUntilRecordingStops,
}

pub fn validate_mic_config(mic_config: &MicConfig) -> Result<(), String> {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, mic_config.audio_device.as_deref())?;
    let config = select_input_config(&device, mic_config.sample_rate)?;
    let stream_config = config.config();

    match config.sample_format() {
        cpal::SampleFormat::F32 => build_validation_stream::<f32>(&device, stream_config)?,
        cpal::SampleFormat::I16 => build_validation_stream::<i16>(&device, stream_config)?,
        cpal::SampleFormat::U16 => build_validation_stream::<u16>(&device, stream_config)?,
        sample_format => {
            return Err(format!(
                "unsupported audio input sample format: {:?}",
                sample_format
            ));
        }
    };

    Ok(())
}

impl AudioController {
    pub fn inactive(
        state: Arc<AppState>,
        transcription_controller: TranscriptionController,
        config_store: LiveConfigStore,
    ) -> Self {
        #[cfg(target_os = "macos")]
        REGISTER_LISTENER.call_once(|| {
            core_audio_listener::register_hardware_listeners();
        });

        Self {
            active_stream: Mutex::new(None),
            pending_config: Mutex::new(None),
            config_store,
            state,
            transcription_controller,
            last_rebuild_attempt: Mutex::new(None),
            preview_audio_device: Mutex::new(PreviewDeviceState::Disabled),
        }
    }

    pub fn new(
        state: Arc<AppState>,
        transcription_controller: TranscriptionController,
        config_store: LiveConfigStore,
    ) -> (Self, Option<String>) {
        #[cfg(target_os = "macos")]
        REGISTER_LISTENER.call_once(|| {
            core_audio_listener::register_hardware_listeners();
        });

        let mic_config = config_store.current().mic;
        let (active_stream, startup_error) = match build_input_stream(
            state.clone(),
            transcription_controller.clone(),
            config_store.clone(),
            &mic_config,
        ) {
            Ok(handle) => {
                transcription_controller.set_sample_rate(handle.sample_rate);
                (Some(ActiveAudioStream::new(handle, &mic_config)), None)
            }
            Err(error) => {
                log::error!("failed to initialize audio input stream: {}", error);
                (None, Some(error))
            }
        };

        (
            Self {
                active_stream: Mutex::new(active_stream),
                pending_config: Mutex::new(None),
                config_store,
                state,
                transcription_controller,
                last_rebuild_attempt: Mutex::new(None),
                preview_audio_device: Mutex::new(PreviewDeviceState::Disabled),
            },
            startup_error,
        )
    }

    pub fn set_preview_audio_device(&self, device: Option<String>) {
        if let Ok(mut preview_guard) = self.preview_audio_device.lock() {
            *preview_guard = PreviewDeviceState::Override(device);
        }
    }

    pub fn clear_preview_audio_device(&self) {
        if let Ok(mut preview_guard) = self.preview_audio_device.lock() {
            *preview_guard = PreviewDeviceState::Disabled;
        }
    }

    pub fn effective_mic_config(&self) -> MicConfig {
        let mut mic_config = self.config_store.current().mic;
        if let Ok(preview_guard) = self.preview_audio_device.lock() {
            if let PreviewDeviceState::Override(ref preview_device) = *preview_guard {
                mic_config.audio_device = preview_device.clone();
            }
        }
        mic_config
    }

    pub fn should_rebuild_stream(&self) -> bool {
        let active_stream = match self.active_stream.lock() {
            Ok(guard) => guard,
            Err(_) => return true,
        };

        let mic_config = self.effective_mic_config();
        let Some(active) = active_stream.as_ref() else {
            return true;
        };

        if !active.healthy.load(Ordering::SeqCst) {
            return true;
        }

        if active.requested_sample_rate != mic_config.sample_rate
            || active.configured_audio_device != mic_config.audio_device
        {
            return true;
        }

        let is_recording = self.state.is_recording();
        let is_preview = self.state.is_settings_window_visible();
        let should_play = mic_config.always_on || is_recording || is_preview;
        if should_play {
            let last_ms = active.last_callback_millis.load(Ordering::Relaxed);
            if last_ms > 0 {
                static PROCESS_START: std::sync::LazyLock<Instant> =
                    std::sync::LazyLock::new(Instant::now);
                let now_ms = PROCESS_START.elapsed().as_millis() as u64;
                if now_ms.saturating_sub(last_ms) > STREAM_STALL_TIMEOUT.as_millis() as u64 {
                    log::warn!(
                        "audio stream stalled (no audio callbacks for >1.5s); marking unhealthy"
                    );
                    active.healthy.store(false, Ordering::SeqCst);
                    return true;
                }
            }
        }

        #[cfg(target_os = "macos")]
        let hardware_changed = core_audio_listener::HARDWARE_CHANGED.load(Ordering::SeqCst);
        #[cfg(not(target_os = "macos"))]
        let hardware_changed = false;

        if let Some(configured_name) =
            normalized_configured_audio_device(mic_config.audio_device.as_deref())
        {
            let actual_name = active.actual_audio_device_name.as_deref().unwrap_or("");
            if actual_name != configured_name
                && actual_name.to_lowercase() != configured_name.to_lowercase()
            {
                return true;
            }
            if hardware_changed {
                return true;
            }
        } else {
            if hardware_changed {
                return true;
            }
            let host = cpal::default_host();
            if host.default_input_device().as_ref() != Some(&active.actual_device) {
                return true;
            }
        }

        false
    }

    pub fn apply_mic_config(
        &self,
        mic_config: &MicConfig,
    ) -> Result<AudioConfigApplyEffect, String> {
        let needs_stream_rebuild = self.should_rebuild_stream();

        if !needs_stream_rebuild {
            return Ok(AudioConfigApplyEffect::AppliedNow);
        }

        if self.state.is_recording() {
            if let Ok(mut pending_config) = self.pending_config.lock() {
                *pending_config = Some(mic_config.clone());
            }
            return Ok(AudioConfigApplyEffect::DeferredUntilRecordingStops);
        }

        self.rebuild_stream(mic_config)?;
        Ok(AudioConfigApplyEffect::AppliedNow)
    }

    pub fn ensure_input_stream_ready(&self) -> Result<bool, String> {
        if self.should_rebuild_stream() {
            let mic_config = self.effective_mic_config();
            self.rebuild_stream(&mic_config)?;
            return Ok(true);
        }

        Ok(false)
    }

    pub fn sync_stream_state(&self) {
        let mic_config = self.effective_mic_config();
        let is_recording = self.state.is_recording();
        let is_preview = self.state.is_settings_window_visible();
        let should_play = mic_config.always_on || is_recording || is_preview;

        let mut active_stream = match self.active_stream.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        if let Some(active) = active_stream.as_mut() {
            if should_play {
                if let Err(error) = active._stream.play() {
                    log::error!("failed to play audio stream: {}", error);
                    active.healthy.store(false, Ordering::SeqCst);
                }
            } else {
                if let Err(error) = active._stream.pause() {
                    log::error!("failed to pause audio stream: {}", error);
                    active.healthy.store(false, Ordering::SeqCst);
                }
            }
        }
    }

    pub fn apply_pending_if_idle(&self) {
        if self.state.is_recording() {
            return;
        }

        let pending_config = self
            .pending_config
            .lock()
            .ok()
            .and_then(|mut pending_config| pending_config.take());

        if let Some(pending_config) = pending_config {
            if let Err(error) = self.rebuild_stream(&pending_config) {
                log::error!("failed to apply deferred audio config: {}", error);
                if let Ok(mut retry_config) = self.pending_config.lock() {
                    *retry_config = Some(pending_config);
                }
            }
            return;
        }

        if !self.should_rebuild_stream() {
            return;
        }

        let now = Instant::now();
        if let Ok(mut last_attempt_guard) = self.last_rebuild_attempt.lock() {
            if let Some(last_attempt) = *last_attempt_guard {
                if now.duration_since(last_attempt) < REBUILD_RETRY_INTERVAL {
                    return;
                }
            }
            *last_attempt_guard = Some(now);
        }

        let current_config_mic = self.effective_mic_config();
        match self.rebuild_stream(&current_config_mic) {
            Ok(()) => {
                log::info!("successfully rebuilt audio input stream");
            }
            Err(error) => {
                log::error!("failed to switch or reconnect audio input device: {}", error);
            }
        }
    }

    fn rebuild_stream(&self, mic_config: &MicConfig) -> Result<(), String> {
        let handle = build_input_stream(
            self.state.clone(),
            self.transcription_controller.clone(),
            self.config_store.clone(),
            mic_config,
        )?;

        #[cfg(target_os = "macos")]
        core_audio_listener::HARDWARE_CHANGED.store(false, Ordering::SeqCst);

        self.transcription_controller.set_sample_rate(handle.sample_rate);
        let mut active_stream = self
            .active_stream
            .lock()
            .map_err(|_| "audio stream lock poisoned".to_owned())?;
        *active_stream = Some(ActiveAudioStream::new(handle, mic_config));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_audio_input_device_choices, encode_pcm_mono, is_system_default_audio_device_value,
        config_with_preferred_rate, normalize_meter_amplitude, normalized_configured_audio_device,
        smooth_meter_level_db, smooth_meter_value, stream_error_response, AudioController, AudioInputDeviceChoice,
        InputDeviceDescriptor, StreamErrorResponse,
    };
    use cpal::{ErrorKind, SampleFormat, SupportedBufferSize, SupportedStreamConfigRange};

    #[test]
    fn stream_errors_that_keep_the_stream_running_are_ignored() {
        for kind in [
            ErrorKind::Xrun,
            ErrorKind::DeviceChanged,
            ErrorKind::RealtimeDenied,
        ] {
            assert_eq!(
                stream_error_response(&kind.into()),
                StreamErrorResponse::Ignore,
                "{:?}",
                kind
            );
        }
    }

    #[test]
    fn invalidated_stream_is_rebuilt_without_reporting_an_error() {
        assert_eq!(
            stream_error_response(&ErrorKind::StreamInvalidated.into()),
            StreamErrorResponse::Rebuild
        );
    }

    #[test]
    fn stream_failures_are_rebuilt_and_reported() {
        for kind in [
            ErrorKind::DeviceNotAvailable,
            ErrorKind::BackendError,
            ErrorKind::Other,
        ] {
            assert_eq!(
                stream_error_response(&kind.into()),
                StreamErrorResponse::RebuildAndReport,
                "{:?}",
                kind
            );
        }
    }

    #[test]
    fn config_with_preferred_rate_picks_first_range_containing_rate() {
        let ranges = [
            SupportedStreamConfigRange::new(
                1,
                44_100,
                44_100,
                SupportedBufferSize::Unknown,
                SampleFormat::F32,
            ),
            SupportedStreamConfigRange::new(
                2,
                8_000,
                48_000,
                SupportedBufferSize::Unknown,
                SampleFormat::F32,
            ),
            SupportedStreamConfigRange::new(
                1,
                16_000,
                16_000,
                SupportedBufferSize::Unknown,
                SampleFormat::F32,
            ),
        ];

        let selected = config_with_preferred_rate(ranges.clone(), 16_000).unwrap();
        assert_eq!(selected.sample_rate(), 16_000);
        assert_eq!(selected.channels(), 2);
        assert_eq!(selected.sample_format(), SampleFormat::F32);

        assert!(config_with_preferred_rate(ranges, 96_000).is_none());
    }

    #[test]
    fn decorated_system_default_audio_device_value_is_detected() {
        assert!(is_system_default_audio_device_value(
            "System default (MacBook Pro Microphone)"
        ));
        assert!(is_system_default_audio_device_value("System default"));
    }

    #[test]
    fn decorated_system_default_audio_device_value_normalizes_to_none() {
        assert_eq!(
            normalized_configured_audio_device(Some("System default (MacBook Pro Microphone)")),
            None
        );
        assert_eq!(
            normalized_configured_audio_device(Some("System default")),
            None
        );
        assert_eq!(
            normalized_configured_audio_device(Some("Shure MV7")),
            Some("Shure MV7")
        );
    }

    #[test]
    fn build_audio_input_device_choices_prefers_names_for_unique_devices() {
        let choices = build_audio_input_device_choices(&[
            InputDeviceDescriptor {
                index: 0,
                name: Some("External Microphone".to_owned()),
            },
            InputDeviceDescriptor {
                index: 1,
                name: Some("MacBook Pro Microphone".to_owned()),
            },
        ]);

        assert_eq!(
            choices,
            vec![
                AudioInputDeviceChoice {
                    label: "External Microphone".to_owned(),
                    value: "0".to_owned(),
                },
                AudioInputDeviceChoice {
                    label: "MacBook Pro Microphone".to_owned(),
                    value: "1".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn build_audio_input_device_choices_uses_indexes_for_duplicates_and_unknown_devices() {
        let choices = build_audio_input_device_choices(&[
            InputDeviceDescriptor {
                index: 0,
                name: Some("Virtual Cable".to_owned()),
            },
            InputDeviceDescriptor {
                index: 1,
                name: Some("Virtual Cable".to_owned()),
            },
            InputDeviceDescriptor {
                index: 2,
                name: None,
            },
        ]);

        assert_eq!(
            choices,
            vec![
                AudioInputDeviceChoice {
                    label: "Virtual Cable (#1)".to_owned(),
                    value: "0".to_owned(),
                },
                AudioInputDeviceChoice {
                    label: "Virtual Cable (#2)".to_owned(),
                    value: "1".to_owned(),
                },
                AudioInputDeviceChoice {
                    label: "<unknown> (#3)".to_owned(),
                    value: "2".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn encode_pcm_mono_counts_post_gain_clipped_samples() {
        let input = vec![0.8f32, -0.8f32];
        let (bytes, clipped_count, level_db, peak_db) = encode_pcm_mono(&input, 1, 1.5);

        assert_eq!(bytes.len(), 4);
        assert_eq!(clipped_count, 2);
        assert!(level_db > -2.0);
        assert!(peak_db >= 0.0);
    }

    #[test]
    fn normalize_meter_amplitude_clamps_silence_and_hot_input() {
        assert_eq!(normalize_meter_amplitude(-100.0), 0.0);
        assert_eq!(normalize_meter_amplitude(-42.0), 0.0);
        assert_eq!(normalize_meter_amplitude(-6.0), 1.0);
        assert_eq!(normalize_meter_amplitude(0.0), 1.0);
    }

    #[test]
    fn normalize_meter_amplitude_is_monotonic() {
        assert!(normalize_meter_amplitude(-24.0) > normalize_meter_amplitude(-30.0));
    }

    #[test]
    fn meter_level_db_starts_at_the_first_reading_then_smooths() {
        assert_eq!(smooth_meter_level_db(None, -45.0), -45.0);
        assert_eq!(smooth_meter_level_db(Some(-45.0), -35.0), smooth_meter_value(-45.0, -35.0));
    }

    #[test]
    fn smooth_meter_value_uses_attack_and_release_paths() {
        assert_eq!(smooth_meter_value(0.0, 1.0), 0.4);
        assert_eq!(smooth_meter_value(1.0, 0.0), 0.85);
    }

    #[test]
    fn none_active_stream_requests_rebuild() {
        let state = crate::state::AppState::new();
        let config = crate::config::Config::default();
        let config_store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from("/tmp/config.toml"),
        );
        let transcription_controller =
            crate::transcription::spawn_transcription_thread(state.clone(), config_store.clone());
        let controller = AudioController::inactive(state, transcription_controller, config_store);

        assert!(controller.should_rebuild_stream());
    }

    #[test]
    fn config_change_requests_rebuild() {
        let state = crate::state::AppState::new();
        let mut config = crate::config::Config::default();
        config.mic.audio_device = Some("Original Mic".to_owned());
        let config_store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config.clone(),
            std::path::PathBuf::from("/tmp/config.toml"),
        );
        let transcription_controller =
            crate::transcription::spawn_transcription_thread(state.clone(), config_store.clone());
        let controller = AudioController::inactive(state, transcription_controller, config_store.clone());

        assert!(controller.should_rebuild_stream());

        let mut new_config = config.clone();
        new_config.mic.audio_device = Some("New Mic".to_owned());
        config_store.replace(new_config.clone(), new_config);

        assert!(controller.should_rebuild_stream());
    }

    #[test]
    fn preview_audio_device_overrides_configured_device_for_rebuild() {
        let state = crate::state::AppState::new();
        let config = crate::config::Config::default();
        let config_store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from("/tmp/config.toml"),
        );
        let transcription_controller =
            crate::transcription::spawn_transcription_thread(state.clone(), config_store.clone());
        let controller = AudioController::inactive(state, transcription_controller, config_store);

        controller.set_preview_audio_device(Some("Preview Mic".to_owned()));
        assert_eq!(
            controller.effective_mic_config().audio_device,
            Some("Preview Mic".to_owned())
        );

        controller.clear_preview_audio_device();
        assert_eq!(controller.effective_mic_config().audio_device, None);
    }
}

#[cfg(target_os = "macos")]
#[allow(non_snake_case)]
mod core_audio_listener {
    use std::sync::atomic::{AtomicBool, Ordering};

    pub static HARDWARE_CHANGED: AtomicBool = AtomicBool::new(false);

    pub fn register_hardware_listeners() {
        extern "C" {
            fn AudioObjectAddPropertyListener(
                inObjectID: u32,
                inAddress: *const AudioObjectPropertyAddress,
                inListener: AudioObjectPropertyListenerProc,
                inClientData: *mut std::ffi::c_void,
            ) -> i32;
        }

        #[repr(C)]
        struct AudioObjectPropertyAddress {
            mSelector: u32,
            mScope: u32,
            mElement: u32,
        }

        type AudioObjectPropertyListenerProc = extern "C" fn(
            inObjectID: u32,
            inNumberAddresses: u32,
            inAddresses: *const AudioObjectPropertyAddress,
            inClientData: *mut std::ffi::c_void,
        ) -> i32;

        extern "C" fn hardware_property_listener(
            _in_object_id: u32,
            _in_number_addresses: u32,
            _in_addresses: *const AudioObjectPropertyAddress,
            _in_client_data: *mut std::ffi::c_void,
        ) -> i32 {
            HARDWARE_CHANGED.store(true, Ordering::SeqCst);
            0
        }

        const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
        const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = 0x676c6f62; // 'glob'
        const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;
        const K_AUDIO_HARDWARE_PROPERTY_DEVICES: u32 = 0x64657673; // 'devs'
        const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE: u32 = 0x64696e69; // 'dini'

        let devices_address = AudioObjectPropertyAddress {
            mSelector: K_AUDIO_HARDWARE_PROPERTY_DEVICES,
            mScope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            mElement: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };

        let default_input_address = AudioObjectPropertyAddress {
            mSelector: K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE,
            mScope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            mElement: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };

        unsafe {
            let _ = AudioObjectAddPropertyListener(
                K_AUDIO_OBJECT_SYSTEM_OBJECT,
                &devices_address,
                hardware_property_listener,
                std::ptr::null_mut(),
            );
            let _ = AudioObjectAddPropertyListener(
                K_AUDIO_OBJECT_SYSTEM_OBJECT,
                &default_input_address,
                hardware_property_listener,
                std::ptr::null_mut(),
            );
        }
    }
}
