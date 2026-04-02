use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig};
use std::sync::Arc;

use crate::state::AppState;
use crate::transcription::TranscriptionController;

const CLIP_DETECTION_THRESHOLD: f32 = 0.99;
const METER_MIN_DB: f32 = -42.0;
const METER_MAX_DB: f32 = -6.0;

#[derive(Debug)]
struct EncodedAudioChunk {
    clipped_sample_count: usize,
    mic_level: f32,
    mic_peak: f32,
    pcm_bytes: Vec<u8>,
}

pub fn print_input_devices() -> Result<(), String> {
    let host = cpal::default_host();
    let default_device_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let input_devices = host
        .input_devices()
        .map_err(|error| format!("failed to enumerate audio input devices: {}", error))?;
    let devices: Vec<cpal::Device> = input_devices.collect();

    match default_device_name {
        Some(default_device_name) => println!("Default input device: {}", default_device_name),
        None => println!("Default input device: <none>"),
    }

    if devices.is_empty() {
        println!("Available input devices: <none>");
        return Ok(());
    }

    println!("Available input devices:");
    for (index, device) in devices.iter().enumerate() {
        let device_name = device.name().unwrap_or_else(|_| "<unknown>".into());
        println!("{}: {}", index, device_name);
    }

    Ok(())
}

pub fn build_input_stream(
    state: Arc<AppState>,
    controller: TranscriptionController,
    preferred_sample_rate: u32,
    gain: f32,
    audio_device: Option<&str>,
) -> (Stream, u32) {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, audio_device);

    log::info!(
        "audio input device: {}",
        device.name().unwrap_or_else(|_| "<unknown>".into())
    );

    let config = select_input_config(&device, preferred_sample_rate);
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
            build_stream_for_format::<f32>(&device, &config, state, controller, gain)
        }
        SampleFormat::I16 => {
            build_stream_for_format::<i16>(&device, &config, state, controller, gain)
        }
        SampleFormat::U16 => {
            build_stream_for_format::<u16>(&device, &config, state, controller, gain)
        }
        sample_format => panic!("unsupported audio input sample format: {:?}", sample_format),
    };

    stream.play().expect("failed to start audio stream");
    log::info!("audio capture started ({}Hz, {} ch)", actual_rate, channels);

    (stream, actual_rate)
}

fn resolve_input_device(host: &cpal::Host, configured_audio_device: Option<&str>) -> cpal::Device {
    let Some(requested_device) = configured_audio_device
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return host
            .default_input_device()
            .expect("no default audio input device");
    };

    let input_devices = host
        .input_devices()
        .expect("failed to enumerate audio input devices");
    let devices: Vec<cpal::Device> = input_devices.collect();

    if let Ok(index) = requested_device.parse::<usize>() {
        if let Some(device) = devices.get(index) {
            log::info!("using configured audio device index {}", index);
            return device.clone();
        }
        panic!("configured audio_device index {} is out of range", index);
    }

    let requested_device_lower = requested_device.to_lowercase();
    for device in devices {
        let Ok(device_name) = device.name() else {
            continue;
        };
        if device_name == requested_device || device_name.to_lowercase() == requested_device_lower {
            log::info!("using configured audio device '{}'", device_name);
            return device;
        }
    }

    panic!(
        "configured audio_device '{}' was not found",
        requested_device
    );
}

fn select_input_config(device: &cpal::Device, preferred_sample_rate: u32) -> SupportedStreamConfig {
    let default_config = device
        .default_input_config()
        .expect("no default input config");

    let Ok(supported_configs) = device.supported_input_configs() else {
        log::warn!("failed to enumerate supported input configs; using device default");
        return default_config;
    };

    for supported_config in supported_configs {
        if let Some(config) =
            supported_config.try_with_sample_rate(cpal::SampleRate(preferred_sample_rate))
        {
            return config;
        }
    }

    log::warn!(
        "preferred sample rate {}Hz is unsupported; falling back to device default {}Hz",
        preferred_sample_rate,
        default_config.sample_rate().0
    );
    default_config
}

fn build_stream_for_format<T>(
    device: &cpal::Device,
    config: &SupportedStreamConfig,
    state: Arc<AppState>,
    controller: TranscriptionController,
    gain: f32,
) -> Stream
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

    device
        .build_input_stream(
            &stream_config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                let is_recording = meter_state.is_recording();
                if !is_recording {
                    if was_recording {
                        smoothed_level = 0.0;
                        smoothed_peak = 0.0;
                        was_recording = false;
                    }
                    meter_state.clear_mic_meter();
                    return;
                }

                if !was_recording {
                    smoothed_level = 0.0;
                    smoothed_peak = 0.0;
                    was_recording = true;
                }

                let encoded_chunk = encode_pcm_mono(data, channels, gain);
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
        .expect("failed to build audio input stream")
}

fn encode_pcm_mono<T>(data: &[T], channels: usize, gain: f32) -> EncodedAudioChunk
where
    T: Sample,
    f32: FromSample<T>,
{
    if channels == 0 {
        return EncodedAudioChunk {
            clipped_sample_count: 0,
            mic_level: 0.0,
            mic_peak: 0.0,
            pcm_bytes: Vec::new(),
        };
    }

    let frame_count = data.len() / channels;
    let mut pcm_bytes = Vec::with_capacity(frame_count * 2);
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
        pcm_bytes.extend_from_slice(&linear16_sample.to_le_bytes());
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
        pcm_bytes,
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
    use super::{encode_pcm_mono, normalize_meter_amplitude, smooth_meter_value};

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
        let encoded_chunk = encode_pcm_mono(&[0.2f32, 0.5, -0.7, 0.1], 1, 2.0);

        assert_eq!(encoded_chunk.clipped_sample_count, 2);
    }
}
