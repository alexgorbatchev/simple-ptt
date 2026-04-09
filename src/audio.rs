use bytes::{BufMut, Bytes, BytesMut};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::MicConfig;
use crate::settings::LiveConfigStore;
use crate::state::AppState;
use crate::transcription::TranscriptionController;

const CLIP_DETECTION_THRESHOLD: f32 = 0.99;
const METER_MIN_DB: f32 = -42.0;
const METER_MAX_DB: f32 = -6.0;
const UNKNOWN_AUDIO_INPUT_DEVICE_LABEL: &str = "<unknown>";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioInputDeviceChoice {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableAudioInputDevices {
    pub default_device_name: Option<String>,
    pub choices: Vec<AudioInputDeviceChoice>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InputDeviceDescriptor {
    index: usize,
    name: Option<String>,
}

pub struct AudioController {
    active_stream: Mutex<Option<ActiveAudioStream>>,
    pending_config: Mutex<Option<MicConfig>>,
    config_store: LiveConfigStore,
    state: Arc<AppState>,
    transcription_controller: TranscriptionController,
}

struct ActiveAudioStream {
    configured_audio_device: Option<String>,
    actual_audio_device_name: Option<String>,
    requested_sample_rate: u32,
    _stream: Stream,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioConfigApplyEffect {
    AppliedNow,
    DeferredUntilRecordingStops,
}

#[derive(Debug)]
struct EncodedAudioChunk {
    clipped_sample_count: usize,
    mic_level: f32,
    mic_peak: f32,
    pcm_bytes: Bytes,
}

pub fn validate_mic_config(mic_config: &MicConfig) -> Result<(), String> {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, mic_config.audio_device.as_deref())?;
    let config = select_input_config(&device, mic_config.sample_rate)?;
    let stream_config = config.config();

    match config.sample_format() {
        SampleFormat::F32 => build_validation_stream::<f32>(&device, &stream_config)?,
        SampleFormat::I16 => build_validation_stream::<i16>(&device, &stream_config)?,
        SampleFormat::U16 => build_validation_stream::<u16>(&device, &stream_config)?,
        sample_format => {
            return Err(format!(
                "unsupported audio input sample format: {:?}",
                sample_format
            ));
        }
    };

    Ok(())
}

pub fn available_audio_input_devices() -> Result<AvailableAudioInputDevices, String> {
    let host = cpal::default_host();
    let default_device_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let devices = enumerate_input_devices(&host)?;

    Ok(AvailableAudioInputDevices {
        default_device_name,
        choices: build_audio_input_device_choices(&devices),
    })
}

pub fn print_input_devices() -> Result<(), String> {
    let host = cpal::default_host();
    let default_device_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let devices = enumerate_input_devices(&host)?;

    match default_device_name {
        Some(default_device_name) => println!("Default input device: {}", default_device_name),
        None => println!("Default input device: <none>"),
    }

    if devices.is_empty() {
        println!("Available input devices: <none>");
        return Ok(());
    }

    println!("Available input devices:");
    for device in devices {
        println!(
            "{}: {}",
            device.index,
            device
                .name
                .as_deref()
                .unwrap_or(UNKNOWN_AUDIO_INPUT_DEVICE_LABEL)
        );
    }

    Ok(())
}

impl AudioController {
    pub fn inactive(
        state: Arc<AppState>,
        transcription_controller: TranscriptionController,
        config_store: LiveConfigStore,
    ) -> Self {
        Self {
            active_stream: Mutex::new(None),
            pending_config: Mutex::new(None),
            config_store,
            state,
            transcription_controller,
        }
    }

    pub fn new(
        state: Arc<AppState>,
        transcription_controller: TranscriptionController,
        config_store: LiveConfigStore,
    ) -> (Self, Option<String>) {
        let mic_config = config_store.current().mic;
        let (active_stream, startup_error) = match build_input_stream(
            state.clone(),
            transcription_controller.clone(),
            config_store.clone(),
            &mic_config,
        ) {
            Ok((stream, actual_rate, actual_audio_device_name)) => {
                transcription_controller.set_sample_rate(actual_rate);
                (
                    Some(ActiveAudioStream {
                        configured_audio_device: mic_config.audio_device.clone(),
                        actual_audio_device_name,
                        requested_sample_rate: mic_config.sample_rate,
                        _stream: stream,
                    }),
                    None,
                )
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
            },
            startup_error,
        )
    }

    pub fn apply_mic_config(
        &self,
        mic_config: &MicConfig,
    ) -> Result<AudioConfigApplyEffect, String> {
        let active_stream = self
            .active_stream
            .lock()
            .map_err(|_| "audio stream lock poisoned".to_owned())?;

        // We must rebuild if settings changed, but also if it looks like the default device identity shifted.
        let needs_stream_rebuild = match active_stream.as_ref() {
            Some(active_stream) => {
                if active_stream.requested_sample_rate != mic_config.sample_rate
                    || active_stream.configured_audio_device != mic_config.audio_device
                {
                    true
                } else if normalized_configured_audio_device(mic_config.audio_device.as_deref())
                    .is_none()
                {
                    // Config says we're using "System default". Let's check if the default actually changed (e.g. plugged in new mic).
                    let host = cpal::default_host();
                    let current_default_name = host
                        .default_input_device()
                        .and_then(|device| device.name().ok());
                    current_default_name != active_stream.actual_audio_device_name
                } else {
                    false
                }
            }
            None => true,
        };
        drop(active_stream);

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
        let active_stream = self
            .active_stream
            .lock()
            .map_err(|_| "audio stream lock poisoned".to_owned())?;

        let mic_config = self.config_store.current().mic;

        let needs_rebuild = if let Some(active_stream) = &*active_stream {
            if normalized_configured_audio_device(mic_config.audio_device.as_deref()).is_none() {
                // Config says we're using "System default". Let's check if the default actually changed
                let host = cpal::default_host();
                let current_default_name = host
                    .default_input_device()
                    .and_then(|device| device.name().ok());
                current_default_name != active_stream.actual_audio_device_name
            } else {
                false
            }
        } else {
            true
        };

        drop(active_stream);

        if needs_rebuild {
            self.rebuild_stream(&mic_config)?;
            return Ok(true);
        }

        Ok(false)
    }

    pub fn apply_pending_if_idle(&self) {
        if self.state.is_recording() {
            return;
        }

        // Apply any pending config explicitly first
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

        // Even if there's no pending config, if the configured device is "System default", check if the system default actually changed.
        // E.g. pulling a mic out, or adding one while app is running.
        let needs_rebuild = {
            let active_stream = self.active_stream.lock().ok();
            let current_config_mic = self.config_store.current().mic;
            match active_stream.as_deref().and_then(|x| x.as_ref()) {
                Some(active)
                    if normalized_configured_audio_device(
                        current_config_mic.audio_device.as_deref(),
                    )
                    .is_none() =>
                {
                    let host = cpal::default_host();
                    let current_default_name = host
                        .default_input_device()
                        .and_then(|device| device.name().ok());
                    current_default_name != active.actual_audio_device_name
                }
                None => false,
                _ => false,
            }
        };

        if needs_rebuild {
            let current_config_mic = self.config_store.current().mic;
            if let Err(error) = self.rebuild_stream(&current_config_mic) {
                log::error!("failed to switch to new default audio device: {}", error);
            }
        }
    }

    fn rebuild_stream(&self, mic_config: &MicConfig) -> Result<(), String> {
        let (stream, actual_rate, actual_audio_device_name) = build_input_stream(
            self.state.clone(),
            self.transcription_controller.clone(),
            self.config_store.clone(),
            mic_config,
        )?;

        self.transcription_controller.set_sample_rate(actual_rate);
        let mut active_stream = self
            .active_stream
            .lock()
            .map_err(|_| "audio stream lock poisoned".to_owned())?;
        *active_stream = Some(ActiveAudioStream {
            configured_audio_device: mic_config.audio_device.clone(),
            actual_audio_device_name,
            requested_sample_rate: mic_config.sample_rate,
            _stream: stream,
        });
        Ok(())
    }
}

pub fn build_input_stream(
    state: Arc<AppState>,
    controller: TranscriptionController,
    config_store: LiveConfigStore,
    mic_config: &MicConfig,
) -> Result<(Stream, u32, Option<String>), String> {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, mic_config.audio_device.as_deref())?;

    let actual_audio_device_name = device.name().ok();
    log::info!(
        "audio input device: {}",
        actual_audio_device_name.as_deref().unwrap_or("<unknown>")
    );

    let config = select_input_config(&device, mic_config.sample_rate)?;
    let actual_rate = config.sample_rate().0;
    let channels = config.channels();

    log::info!(
        "selected input config: {} channels, {}Hz, {:?}",
        channels,
        actual_rate,
        config.sample_format()
    );

    let stream = match config.sample_format() {
        SampleFormat::F32 => {
            build_stream_for_format::<f32>(&device, &config, state, controller, config_store)?
        }
        SampleFormat::I16 => {
            build_stream_for_format::<i16>(&device, &config, state, controller, config_store)?
        }
        SampleFormat::U16 => {
            build_stream_for_format::<u16>(&device, &config, state, controller, config_store)?
        }
        sample_format => {
            return Err(format!(
                "unsupported audio input sample format: {:?}",
                sample_format
            ));
        }
    };

    stream
        .play()
        .map_err(|error| format!("failed to start audio stream: {}", error))?;
    log::info!("audio capture started ({}Hz, {} ch)", actual_rate, channels);

    Ok((stream, actual_rate, actual_audio_device_name))
}

fn build_validation_stream<T>(
    device: &cpal::Device,
    stream_config: &cpal::StreamConfig,
) -> Result<(), String>
where
    T: Sample + SizedSample + Send + 'static,
{
    let stream = device
        .build_input_stream(
            stream_config,
            |_data: &[T], _info: &cpal::InputCallbackInfo| {},
            |error| {
                log::error!("validation audio stream error: {}", error);
            },
            None,
        )
        .map_err(|error| format!("failed to validate audio input stream: {}", error))?;
    drop(stream);
    Ok(())
}

fn enumerate_input_devices(host: &cpal::Host) -> Result<Vec<InputDeviceDescriptor>, String> {
    let input_devices = host
        .input_devices()
        .map_err(|error| format!("failed to enumerate audio input devices: {}", error))?;

    Ok(input_devices
        .enumerate()
        .map(|(index, device)| InputDeviceDescriptor {
            index,
            name: device.name().ok(),
        })
        .collect())
}

fn build_audio_input_device_choices(
    devices: &[InputDeviceDescriptor],
) -> Vec<AudioInputDeviceChoice> {
    let mut duplicate_name_counts = HashMap::new();
    for device in devices {
        let Some(device_name) = device.name.as_ref() else {
            continue;
        };

        *duplicate_name_counts
            .entry(device_name.clone())
            .or_insert(0usize) += 1;
    }

    devices
        .iter()
        .map(|device| match device.name.as_deref() {
            Some(device_name)
                if duplicate_name_counts
                    .get(device_name)
                    .copied()
                    .unwrap_or_default()
                    == 1 =>
            {
                AudioInputDeviceChoice {
                    label: device_name.to_owned(),
                    value: device_name.to_owned(),
                }
            }
            Some(device_name) => AudioInputDeviceChoice {
                label: format!("{} ({})", device_name, device.index),
                value: device.index.to_string(),
            },
            None => AudioInputDeviceChoice {
                label: format!("{} ({})", UNKNOWN_AUDIO_INPUT_DEVICE_LABEL, device.index),
                value: device.index.to_string(),
            },
        })
        .collect()
}

fn resolve_input_device(
    host: &cpal::Host,
    configured_audio_device: Option<&str>,
) -> Result<cpal::Device, String> {
    let Some(requested_device) = normalized_configured_audio_device(configured_audio_device) else {
        return host
            .default_input_device()
            .ok_or_else(|| "no default audio input device".to_owned());
    };

    let input_devices = host
        .input_devices()
        .map_err(|error| format!("failed to enumerate audio input devices: {}", error))?;
    let devices: Vec<cpal::Device> = input_devices.collect();

    if let Ok(index) = requested_device.parse::<usize>() {
        if let Some(device) = devices.get(index) {
            log::info!("using configured audio device index {}", index);
            return Ok(device.clone());
        }
        return Err(format!(
            "configured audio_device index {} is out of range",
            index
        ));
    }

    let requested_device_lower = requested_device.to_lowercase();
    for device in devices {
        let Ok(device_name) = device.name() else {
            continue;
        };
        if device_name == requested_device || device_name.to_lowercase() == requested_device_lower {
            log::info!("using configured audio device '{}'", device_name);
            return Ok(device);
        }
    }

    Err(format!(
        "configured audio_device '{}' was not found",
        requested_device
    ))
}

fn normalized_configured_audio_device(configured_audio_device: Option<&str>) -> Option<&str> {
    let requested_device = configured_audio_device
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    if is_system_default_audio_device_value(requested_device) {
        return None;
    }

    Some(requested_device)
}

fn is_system_default_audio_device_value(value: &str) -> bool {
    let trimmed_value = value.trim();
    trimmed_value == "System default"
        || trimmed_value
            .strip_prefix("System default")
            .map(|suffix| suffix.starts_with(" (") && suffix.ends_with(')'))
            .unwrap_or(false)
}

fn select_input_config(
    device: &cpal::Device,
    preferred_sample_rate: u32,
) -> Result<SupportedStreamConfig, String> {
    let default_config = device
        .default_input_config()
        .map_err(|error| format!("no default input config: {}", error))?;

    let Ok(supported_configs) = device.supported_input_configs() else {
        log::warn!("failed to enumerate supported input configs; using device default");
        return Ok(default_config);
    };

    for supported_config in supported_configs {
        if let Some(config) =
            supported_config.try_with_sample_rate(cpal::SampleRate(preferred_sample_rate))
        {
            return Ok(config);
        }
    }

    log::warn!(
        "preferred sample rate {}Hz is unsupported; falling back to device default {}Hz",
        preferred_sample_rate,
        default_config.sample_rate().0
    );
    Ok(default_config)
}

fn build_stream_for_format<T>(
    device: &cpal::Device,
    config: &SupportedStreamConfig,
    state: Arc<AppState>,
    controller: TranscriptionController,
    config_store: LiveConfigStore,
) -> Result<Stream, String>
where
    T: Sample + SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let stream_config = config.config();
    let channels = usize::from(stream_config.channels);
    let meter_state = Arc::clone(&state);
    let mut smoothed_level = 0.0f32;
    let mut smoothed_peak = 0.0f32;
    let mut was_recording = false;
    let mut pcm_buffer = BytesMut::with_capacity(65536);

    device
        .build_input_stream(
            &stream_config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                let is_recording = meter_state.is_recording_or_transforming();
                if !is_recording {
                    if was_recording {
                        smoothed_level = 0.0;
                        smoothed_peak = 0.0;
                        was_recording = false;
                        pcm_buffer.clear();
                    }
                    meter_state.clear_mic_meter();
                    return;
                }

                if !was_recording {
                    smoothed_level = 0.0;
                    smoothed_peak = 0.0;
                    was_recording = true;
                    pcm_buffer.clear();
                }

                let current_gain = config_store.current().mic.gain;
                let encoded_chunk = encode_pcm_mono(data, channels, current_gain, &mut pcm_buffer);
                if encoded_chunk.pcm_bytes.is_empty() {
                    meter_state.clear_mic_meter();
                    return;
                }

                smoothed_level =
                    smooth_meter_value(smoothed_level, encoded_chunk.mic_level, 0.62, 0.18);
                smoothed_peak =
                    smooth_meter_value(smoothed_peak, encoded_chunk.mic_peak, 0.82, 0.10)
                        .max(smoothed_level);
                meter_state.set_mic_meter(
                    smoothed_level,
                    smoothed_peak,
                    encoded_chunk.clipped_sample_count > 0,
                );
                controller.send_audio(encoded_chunk.pcm_bytes);
            },
            |error| {
                log::error!("audio stream error: {}", error);
            },
            None,
        )
        .map_err(|error| format!("failed to build audio input stream: {}", error))
}

fn encode_pcm_mono<T>(
    data: &[T],
    channels: usize,
    gain: f32,
    pcm_buffer: &mut BytesMut,
) -> EncodedAudioChunk
where
    T: Sample,
    f32: FromSample<T>,
{
    if channels == 0 {
        return EncodedAudioChunk {
            clipped_sample_count: 0,
            mic_level: 0.0,
            mic_peak: 0.0,
            pcm_bytes: Bytes::new(),
        };
    }

    let frame_count = data.len() / channels;
    let required_capacity = frame_count * 2;

    // BytesMut::reserve will allocate if capacity is less than required,
    // but by reserving a larger chunk we amortize allocations.
    if pcm_buffer.capacity() < required_capacity {
        pcm_buffer.reserve(required_capacity.max(65536));
    }

    let mut clipped_sample_count = 0usize;
    let mut peak_amplitude = 0.0f32;
    let mut squared_sum = 0.0f32;

    for frame in data.chunks(channels) {
        let mono_sample = frame
            .iter()
            .fold(0.0f32, |sum, sample| sum + f32::from_sample(*sample))
            / channels as f32;
        let gained_sample = mono_sample * gain;
        if gained_sample.abs() >= CLIP_DETECTION_THRESHOLD {
            clipped_sample_count += 1;
        }

        let amplified_sample = gained_sample.clamp(-1.0, 1.0);
        peak_amplitude = peak_amplitude.max(amplified_sample.abs());
        squared_sum += amplified_sample * amplified_sample;

        let linear16_sample = (amplified_sample * i16::MAX as f32) as i16;
        pcm_buffer.put_i16_le(linear16_sample);
    }

    let rms_amplitude = if frame_count == 0 {
        0.0
    } else {
        (squared_sum / frame_count as f32).sqrt()
    };

    EncodedAudioChunk {
        clipped_sample_count,
        mic_level: normalize_meter_amplitude(rms_amplitude),
        mic_peak: normalize_meter_amplitude(peak_amplitude),
        pcm_bytes: pcm_buffer.split().freeze(),
    }
}

fn normalize_meter_amplitude(amplitude: f32) -> f32 {
    let clamped_amplitude = amplitude.clamp(0.0, 1.0);
    if clamped_amplitude <= f32::EPSILON {
        return 0.0;
    }

    let decibels = 20.0 * clamped_amplitude.log10();
    ((decibels - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB)).clamp(0.0, 1.0)
}

fn smooth_meter_value(previous: f32, current: f32, attack: f32, release: f32) -> f32 {
    let smoothing_factor = if current >= previous { attack } else { release };
    previous + ((current - previous) * smoothing_factor)
}

#[cfg(test)]
mod tests {
    use super::{
        build_audio_input_device_choices, encode_pcm_mono, is_system_default_audio_device_value,
        normalize_meter_amplitude, normalized_configured_audio_device, smooth_meter_value,
        AudioInputDeviceChoice, InputDeviceDescriptor,
    };
    use bytes::BytesMut;

    #[test]
    fn build_audio_input_device_choices_prefers_names_for_unique_devices() {
        let devices = vec![
            InputDeviceDescriptor {
                index: 0,
                name: Some("MacBook Pro Microphone".to_owned()),
            },
            InputDeviceDescriptor {
                index: 1,
                name: Some("Shure MV7".to_owned()),
            },
        ];

        let choices = build_audio_input_device_choices(&devices);

        assert_eq!(
            choices,
            vec![
                AudioInputDeviceChoice {
                    label: "MacBook Pro Microphone".to_owned(),
                    value: "MacBook Pro Microphone".to_owned(),
                },
                AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn build_audio_input_device_choices_uses_indexes_for_duplicates_and_unknown_devices() {
        let devices = vec![
            InputDeviceDescriptor {
                index: 0,
                name: Some("USB Audio Codec".to_owned()),
            },
            InputDeviceDescriptor {
                index: 1,
                name: Some("USB Audio Codec".to_owned()),
            },
            InputDeviceDescriptor {
                index: 2,
                name: None,
            },
        ];

        let choices = build_audio_input_device_choices(&devices);

        assert_eq!(
            choices,
            vec![
                AudioInputDeviceChoice {
                    label: "USB Audio Codec (0)".to_owned(),
                    value: "0".to_owned(),
                },
                AudioInputDeviceChoice {
                    label: "USB Audio Codec (1)".to_owned(),
                    value: "1".to_owned(),
                },
                AudioInputDeviceChoice {
                    label: "<unknown> (2)".to_owned(),
                    value: "2".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn normalize_meter_amplitude_clamps_silence_and_hot_input() {
        assert_eq!(normalize_meter_amplitude(0.0), 0.0);
        assert_eq!(normalize_meter_amplitude(1.0), 1.0);
        assert_eq!(normalize_meter_amplitude(2.0), 1.0);
    }

    #[test]
    fn normalize_meter_amplitude_is_monotonic() {
        let quiet = normalize_meter_amplitude(0.02);
        let conversational = normalize_meter_amplitude(0.12);
        let loud = normalize_meter_amplitude(0.55);

        assert!(quiet < conversational);
        assert!(conversational < loud);
    }

    #[test]
    fn smooth_meter_value_uses_attack_and_release_paths() {
        let attacked = smooth_meter_value(0.2, 0.8, 0.5, 0.1);
        let released = smooth_meter_value(0.8, 0.2, 0.5, 0.1);

        assert_eq!(attacked, 0.5);
        assert_eq!(released, 0.74);
    }

    #[test]
    fn encode_pcm_mono_counts_post_gain_clipped_samples() {
        let mut pcm_buffer = BytesMut::new();
        let encoded_chunk = encode_pcm_mono(&[0.2f32, 0.5, -0.7, 0.1], 1, 2.0, &mut pcm_buffer);

        assert_eq!(encoded_chunk.clipped_sample_count, 2);
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
    fn decorated_system_default_audio_device_value_is_detected() {
        assert!(is_system_default_audio_device_value(
            "System default (MacBook Pro Microphone)"
        ));
        assert!(is_system_default_audio_device_value("System default"));
        assert!(!is_system_default_audio_device_value("0"));
    }
}
