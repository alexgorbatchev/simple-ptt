use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    SupportedStreamConfigRange,
};
use objc2_quartz_core::CACurrentMediaTime;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::settings::LiveConfigStore;
use crate::state::AppState;
use crate::transcription::TranscriptionController;

use super::devices::{
    device_name, encode_pcm_mono, normalize_meter_amplitude, resolve_input_device,
    smooth_meter_value,
};
use super::speech::SpeechAnalysis;

pub struct InputStreamHandle {
    pub stream: Stream,
    pub sample_rate: u32,
    pub device: cpal::Device,
    pub healthy: Arc<AtomicBool>,
    pub last_callback_millis: Arc<AtomicU64>,
}

pub fn build_input_stream(
    state: Arc<AppState>,
    controller: TranscriptionController,
    config_store: LiveConfigStore,
    mic_config: &crate::config::MicConfig,
    prefer_built_in: bool,
) -> Result<InputStreamHandle, String> {
    let host = cpal::default_host();
    let device = resolve_input_device(&host, mic_config.audio_device.as_deref(), prefer_built_in)?;

    let config = select_input_config(&device, mic_config.sample_rate)?;
    let actual_rate = config.sample_rate();
    let stream_config = config.config();
    let channels = usize::from(stream_config.channels);

    let actual_audio_device_name = device_name(&device);
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

    Ok(InputStreamHandle {
        stream,
        sample_rate: actual_rate,
        device,
        healthy,
        last_callback_millis,
    })
}

pub fn select_input_config(
    device: &cpal::Device,
    preferred_sample_rate: u32,
) -> Result<SupportedStreamConfig, String> {
    let supported_configs = device
        .supported_input_configs()
        .map_err(|error| format!("failed to query supported input configs: {}", error))?;

    if let Some(config) = config_with_preferred_rate(supported_configs, preferred_sample_rate) {
        return Ok(config);
    }

    let default_config = device
        .default_input_config()
        .map_err(|error| format!("failed to query default input config: {}", error))?;

    log::warn!(
        "preferred sample rate {}Hz is unsupported; falling back to device default {}Hz",
        preferred_sample_rate,
        default_config.sample_rate()
    );
    Ok(default_config)
}

/// First supported input config whose sample-rate range contains `preferred_sample_rate`,
/// in the order the device reports them.
pub fn config_with_preferred_rate(
    supported_configs: impl IntoIterator<Item = SupportedStreamConfigRange>,
    preferred_sample_rate: u32,
) -> Option<SupportedStreamConfig> {
    supported_configs
        .into_iter()
        .find_map(|config| config.try_with_sample_rate(preferred_sample_rate))
}

/// Where one callback's audio goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AudioRoute {
    /// Nowhere: the meter rests.
    Idle,
    /// Only the meter, for the Settings window's live meter.
    Meter,
    /// The meter and dictation.
    MeterAndDictation,
}

/// Where audio goes now: to dictation while it is captured (recording, or a
/// dictation that resumes after the transformation or correction running
/// now), to the meter alone while Settings is open.
fn audio_route(state: &AppState) -> AudioRoute {
    if state.is_capturing_audio() {
        AudioRoute::MeterAndDictation
    } else if state.is_settings_window_visible() {
        AudioRoute::Meter
    } else {
        AudioRoute::Idle
    }
}

/// Amplitude multiplier for `mic.gain`, which is in decibels.
fn db_to_linear_gain(gain_db: f32) -> f32 {
    10.0f32.powf(gain_db / 20.0)
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
    let speech = SpeechAnalysis::start(stream_config.sample_rate, Arc::clone(&state));
    let meter_state = Arc::clone(&state);
    let error_state = Arc::clone(&state);
    let stream_healthy = Arc::clone(&healthy);
    let callback_last_millis = Arc::clone(&last_callback_millis);
    let mut smoothed_level = 0.0f32;
    let mut smoothed_peak = 0.0f32;
    let mut was_capturing = false;
    let mut pcm_buffer = bytes::BytesMut::with_capacity(65536);

    static PROCESS_START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

    device
        .build_input_stream(
            stream_config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                callback_last_millis.store(
                    PROCESS_START.elapsed().as_millis() as u64,
                    Ordering::Relaxed,
                );
                let route = audio_route(&meter_state);
                if route == AudioRoute::Idle {
                    if was_capturing {
                        smoothed_level = 0.0;
                        smoothed_peak = 0.0;
                        was_capturing = false;
                        pcm_buffer.clear();
                    }
                    meter_state.clear_mic_meter();
                    meter_state.set_mic_active(false);
                    return;
                }

                let current_mic_config = config_store.current().mic;
                let gain_linear = db_to_linear_gain(
                    meter_state
                        .preview_mic_gain()
                        .unwrap_or(current_mic_config.gain),
                );

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

                if route == AudioRoute::MeterAndDictation {
                    was_capturing = true;
                    speech.analyze(
                        pcm_chunk
                            .chunks_exact(2)
                            .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.0)
                            .collect(),
                        CACurrentMediaTime(),
                    );
                    pcm_buffer.extend_from_slice(&pcm_chunk);
                    while pcm_buffer.len() >= 640 {
                        let chunk = pcm_buffer.split_to(640).freeze();
                        controller.send_audio(chunk);
                    }
                }
            },
            move |error| match stream_error_response(&error) {
                StreamErrorResponse::Ignore => {
                    // Xruns are delivered on the real-time audio thread; keep this cheap.
                    log::debug!("audio stream glitch: {}", error);
                }
                StreamErrorResponse::Rebuild => {
                    log::warn!("audio stream invalidated, rebuilding: {}", error);
                    stream_healthy.store(false, Ordering::SeqCst);
                }
                StreamErrorResponse::DeviceLost => {
                    log::warn!("audio input device disappeared, switching: {}", error);
                    stream_healthy.store(false, Ordering::SeqCst);
                    if error_state.is_capturing_audio() {
                        error_state.report_error(super::MICROPHONE_LOST_MESSAGE);
                    }
                }
                StreamErrorResponse::RebuildAndReport => {
                    log::error!("audio stream error: {}", error);
                    stream_healthy.store(false, Ordering::SeqCst);
                    error_state.report_error(error.to_string());
                }
            },
            None,
        )
        .map_err(|error| format!("failed to build audio stream: {}", error))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamErrorResponse {
    /// The stream keeps delivering audio; log only.
    Ignore,
    /// The stream stopped for a recoverable reason; mark it unhealthy so it is rebuilt.
    Rebuild,
    /// The stream's device disappeared; rebuild it on another microphone,
    /// telling the user only if it cut audio being captured short.
    DeviceLost,
    /// The stream failed; rebuild it and surface the error to the user.
    RebuildAndReport,
}

/// How the input stream reacts to an error from cpal's error callback.
/// - `Xrun`: CoreAudio processor overload; the stream keeps running.
/// - `DeviceChanged`: cpal rerouted the stream itself and it remains active.
/// - `RealtimeDenied`: real-time thread promotion was refused; the stream keeps running.
/// - `StreamInvalidated`: CoreAudio pauses the stream on any device sample-rate change
///   (including our own validation stream or another app); a rebuild recovers it.
pub fn stream_error_response(error: &cpal::Error) -> StreamErrorResponse {
    match error.kind() {
        cpal::ErrorKind::Xrun
        | cpal::ErrorKind::DeviceChanged
        | cpal::ErrorKind::RealtimeDenied => StreamErrorResponse::Ignore,
        cpal::ErrorKind::StreamInvalidated => StreamErrorResponse::Rebuild,
        cpal::ErrorKind::DeviceNotAvailable => StreamErrorResponse::DeviceLost,
        _ => StreamErrorResponse::RebuildAndReport,
    }
}

pub fn build_validation_stream<T>(
    device: &cpal::Device,
    stream_config: cpal::StreamConfig,
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

#[cfg(test)]
mod tests {
    use super::{audio_route, db_to_linear_gain, AudioRoute};
    use crate::state::{AppState, STATE_RECORDING, STATE_TRANSFORMING};

    #[test]
    fn audio_goes_to_dictation_while_captured_and_only_to_the_meter_for_settings() {
        let state = AppState::new();
        assert_eq!(audio_route(&state), AudioRoute::Idle);

        state.set_settings_window_visible(true);
        assert_eq!(audio_route(&state), AudioRoute::Meter);
        state.set_settings_window_visible(false);

        state.set_state(STATE_RECORDING);
        assert_eq!(audio_route(&state), AudioRoute::MeterAndDictation);

        // A correction or transformation applied mid-dictation.
        state.set_dictation_resuming(true);
        state.set_state(STATE_TRANSFORMING);
        assert_eq!(audio_route(&state), AudioRoute::MeterAndDictation);

        state.set_dictation_resuming(false);
        assert_eq!(audio_route(&state), AudioRoute::Idle);
    }

    #[test]
    fn db_to_linear_gain_converts_decibels_to_an_amplitude_multiplier() {
        assert_eq!(db_to_linear_gain(0.0), 1.0);
        assert!((db_to_linear_gain(20.0) - 10.0).abs() < 1e-5);
        assert!((db_to_linear_gain(6.0) - 1.995_262).abs() < 1e-5);
    }
}
