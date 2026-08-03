use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::settings::LiveConfigStore;
use crate::state::AppState;
use crate::transcription::TranscriptionController;
use super::devices::{encode_pcm_mono, normalize_meter_amplitude, resolve_input_device, smooth_meter_value};

pub fn build_input_stream(
    state: Arc<AppState>,
    controller: TranscriptionController,
    config_store: LiveConfigStore,
    mic_config: &crate::config::MicConfig,
) -> Result<
    (
        Stream,
        u32,
        Option<String>,
        Arc<AtomicBool>,
        Arc<AtomicU64>,
    ),
    String,
> {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, mic_config.audio_device.as_deref())?;

    let config = select_input_config(&device, mic_config.sample_rate)?;
    let actual_rate = config.sample_rate().0;
    let stream_config = config.config();
    let channels = usize::from(stream_config.channels);

    let actual_audio_device_name = device.name().ok();
    log::info!(
        "using audio input device: {:?}",
        actual_audio_device_name.as_deref().unwrap_or("<unknown>")
    );

    let healthy = Arc::new(AtomicBool::new(true));
    let last_callback_millis = Arc::new(AtomicU64::new(0));

    let stream = match config.sample_format() {
        SampleFormat::F32 => build_stream_for_format::<f32>(
            &device,
            &config,
            state,
            controller,
            config_store,
            healthy.clone(),
            last_callback_millis.clone(),
        )?,
        SampleFormat::I16 => build_stream_for_format::<i16>(
            &device,
            &config,
            state,
            controller,
            config_store,
            healthy.clone(),
            last_callback_millis.clone(),
        )?,
        SampleFormat::U16 => build_stream_for_format::<u16>(
            &device,
            &config,
            state,
            controller,
            config_store,
            healthy.clone(),
            last_callback_millis.clone(),
        )?,
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

    Ok((
        stream,
        actual_rate,
        actual_audio_device_name,
        healthy,
        last_callback_millis,
    ))
}

pub fn select_input_config(
    device: &cpal::Device,
    preferred_sample_rate: u32,
) -> Result<SupportedStreamConfig, String> {
    let supported_configs = device
        .supported_input_configs()
        .map_err(|error| format!("failed to query supported input configs: {}", error))?;

    let preferred_rate = cpal::SampleRate(preferred_sample_rate);
    for config in supported_configs {
        if config.min_sample_rate() <= preferred_rate && preferred_rate <= config.max_sample_rate() {
            return Ok(config.with_sample_rate(preferred_rate));
        }
    }

    let default_config = device
        .default_input_config()
        .map_err(|error| format!("failed to query default input config: {}", error))?;

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
    healthy: Arc<AtomicBool>,
    last_callback_millis: Arc<AtomicU64>,
) -> Result<Stream, String>
where
    T: Sample + SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let stream_config = config.config();
    let channels = usize::from(stream_config.channels);
    let meter_state = Arc::clone(&state);
    let error_state = Arc::clone(&state);
    let stream_healthy = Arc::clone(&healthy);
    let callback_last_millis = Arc::clone(&last_callback_millis);
    let mut smoothed_level = 0.0f32;
    let mut smoothed_peak = 0.0f32;
    let mut was_recording = false;
    let mut pcm_buffer = bytes::BytesMut::with_capacity(65536);

    static PROCESS_START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

    device
        .build_input_stream(
            &stream_config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                callback_last_millis.store(
                    PROCESS_START.elapsed().as_millis() as u64,
                    Ordering::Relaxed,
                );
                let is_recording = meter_state.is_recording();
                let is_preview = meter_state.is_settings_window_visible();
                if !is_recording && !is_preview {
                    if was_recording {
                        smoothed_level = 0.0;
                        smoothed_peak = 0.0;
                        was_recording = false;
                        pcm_buffer.clear();
                    }
                    meter_state.clear_mic_meter();
                    meter_state.set_mic_active(false);
                    return;
                }

                let current_mic_config = config_store.current().mic;
                let gain_linear = if let Some(preview_gain) = meter_state.preview_mic_gain() {
                    10.0f32.powf(preview_gain / 20.0)
                } else {
                    10.0f32.powf(current_mic_config.gain / 20.0)
                };

                let (pcm_chunk, clipped_count, level_db, peak_db) =
                    encode_pcm_mono(data, channels, gain_linear);

                let target_level = normalize_meter_amplitude(level_db);
                let target_peak = normalize_meter_amplitude(peak_db);
                smoothed_level = smooth_meter_value(smoothed_level, target_level);
                smoothed_peak = smooth_meter_value(smoothed_peak, target_peak);

                meter_state.set_mic_meter(
                    smoothed_level,
                    smoothed_peak,
                    clipped_count > 0,
                );
                meter_state.set_mic_active(true);

                if is_recording {
                    was_recording = true;
                    pcm_buffer.extend_from_slice(&pcm_chunk);
                    while pcm_buffer.len() >= 640 {
                        let chunk = pcm_buffer.split_to(640).freeze();
                        controller.send_audio(chunk);
                    }
                }
            },
            move |error| {
                log::error!("audio stream error: {}", error);
                stream_healthy.store(false, Ordering::SeqCst);
                error_state.report_error(error.to_string());
            },
            None,
        )
        .map_err(|error| format!("failed to build audio stream: {}", error))
}

pub fn build_validation_stream<T>(
    device: &cpal::Device,
    stream_config: &cpal::StreamConfig,
) -> Result<(), String>
where
    T: Sample + SizedSample + Send + 'static,
{
    let stream = device
        .build_input_stream(
            stream_config,
            move |_: &[T], _: &cpal::InputCallbackInfo| {},
            move |error| log::error!("validation audio stream error: {}", error),
            None,
        )
        .map_err(|error| format!("failed to build validation audio stream: {}", error))?;

    stream
        .play()
        .map_err(|error| format!("failed to start validation audio stream: {}", error))?;

    Ok(())
}
