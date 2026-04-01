use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig};
use std::sync::Arc;

use crate::state::AppState;
use crate::transcription::TranscriptionController;

pub fn print_input_devices() -> Result<(), String> {
    let host = cpal::default_host();
    let default_device_name = host.default_input_device().and_then(|device| device.name().ok());
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

    device
        .build_input_stream(
            &stream_config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                if !state.is_recording() {
                    return;
                }

                let pcm_bytes = encode_pcm_mono(data, channels, gain);
                controller.send_audio(pcm_bytes);
            },
            |error| {
                log::error!("audio stream error: {}", error);
            },
            None,
        )
        .expect("failed to build audio input stream")
}

fn encode_pcm_mono<T>(data: &[T], channels: usize, gain: f32) -> Vec<u8>
where
    T: Sample,
    f32: FromSample<T>,
{
    if channels == 0 {
        return Vec::new();
    }

    let frame_count = data.len() / channels;
    let mut pcm_bytes = Vec::with_capacity(frame_count * 2);

    for frame in data.chunks(channels) {
        let mono_sample = frame
            .iter()
            .fold(0.0f32, |sum, sample| sum + f32::from_sample(*sample))
            / channels as f32;
        let amplified_sample = (mono_sample * gain).clamp(-1.0, 1.0);
        let linear16_sample = (amplified_sample * i16::MAX as f32) as i16;
        pcm_bytes.extend_from_slice(&linear16_sample.to_le_bytes());
    }

    pcm_bytes
}
