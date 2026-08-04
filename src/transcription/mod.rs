pub mod clipboard;
pub mod session;
pub mod text_builder;

pub use clipboard::*;
pub use session::*;
pub use text_builder::*;

use bytes::Bytes;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread;
use tokio::runtime::Runtime;

use crate::config::{Config, DeepgramConfig};
use crate::settings::LiveConfigStore;
use crate::state::{
    AppState, DeepgramConnectionStatus, STATE_BUFFER_READY, STATE_IDLE, STATE_RECORDING,
    STATE_TRANSFORMING,
};
use crate::transformation::{
    transform_text, TransformationPreviewMode, TransformationRuntimeConfig,
};

const DEFAULT_SAMPLE_RATE: u32 = 16000;

enum Command {
    SetSampleRate(u32),
    StartSession,
    StartCorrectionSession,
    PushAudio(Bytes),
    StopSessionAndPaste,
    StopCorrectionSessionAndApply,
    StopSessionAndTransformAndPaste,
    StopSessionAndTransformAndResume,
    TransformBuffer,
    PasteBuffer,
    QueueClipboardInsertion,
}

#[derive(Clone)]
pub struct TranscriptionController {
    command_tx: Sender<Command>,
}

impl TranscriptionController {
    pub fn set_sample_rate(&self, sample_rate: u32) {
        let _ = self.command_tx.send(Command::SetSampleRate(sample_rate));
    }

    pub fn start_session(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StartSession)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn start_correction_session(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StartCorrectionSession)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn send_audio(&self, pcm_data: Bytes) {
        let _ = self.command_tx.send(Command::PushAudio(pcm_data));
    }

    pub fn stop_session_and_paste(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StopSessionAndPaste)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn stop_correction_session_and_apply(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StopCorrectionSessionAndApply)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn stop_session_and_transform_and_paste(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StopSessionAndTransformAndPaste)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn stop_session_and_transform_and_resume(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StopSessionAndTransformAndResume)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn transform_buffer(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::TransformBuffer)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn paste_buffer(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::PasteBuffer)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn insert_clipboard_text(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::QueueClipboardInsertion)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn discard_buffer(&self) -> Result<(), String> {
        self.paste_buffer()
    }
}

pub fn spawn_transcription_thread(
    state: Arc<AppState>,
    config_store: LiveConfigStore,
) -> TranscriptionController {
    let (command_tx, command_rx) = channel();
    let worker_sample_rate = Arc::new(AtomicU32::new(DEFAULT_SAMPLE_RATE));
    let thread_worker_sample_rate = Arc::clone(&worker_sample_rate);

    thread::spawn(move || {
        let runtime = match Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                log::error!("failed to create Tokio runtime for transcription: {}", error);
                state.report_error(error.to_string());
                return;
            }
        };

        let mut active_session: Option<ActiveSession> = None;
        let mut recording_prefix = String::new();
        let mut buffered_text = String::new();
        let mut resume_after_correction = false;

        while let Ok(command) = command_rx.recv() {
            match command {
                Command::SetSampleRate(sample_rate) => {
                    thread_worker_sample_rate.store(sample_rate, Ordering::Relaxed);
                }
                Command::StartSession => {
                    if let Some(old_session) = active_session.take() {
                        log::info!("cleaning up previous active session before starting new session");
                        let _ = old_session.finish(&runtime, state.clone());
                    }

                    if state.get_state() == STATE_BUFFER_READY {
                        recording_prefix = buffered_text.clone();
                    } else {
                        recording_prefix.clear();
                    }

                    let current_config = config_store.current();
                    let deepgram_config = match resolved_deepgram_config(&current_config) {
                        Ok(deepgram_config) => deepgram_config,
                        Err(error) => {
                            if error.contains("Deepgram API key is not configured") {
                                log::info!(
                                    "ignoring dictation recording start because Deepgram is not configured"
                                );
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                                state.set_state(STATE_IDLE);
                            } else {
                                log::error!("failed to resolve Deepgram config: {}", error);
                                state.report_error(error.to_string());
                            }
                            continue;
                        }
                    };

                    let current_sample_rate = thread_worker_sample_rate.load(Ordering::Relaxed);
                    if state.is_abort_requested() {
                        log::info!(
                            "skipping dictation start because abort was requested before startup completed"
                        );
                        recording_prefix.clear();
                        state.clear_overlay_text();
                        state.set_overlay_text_opacity(1.0);
                        state.set_state(STATE_IDLE);
                        continue;
                    }

                    state.clear_abort_request();
                    state.restore_overlay();
                    state.set_overlay_text(recording_prefix.clone());
                    state.set_overlay_text_opacity(1.0);

                    match start_session(
                        &runtime,
                        state.clone(),
                        &deepgram_config,
                        current_sample_rate,
                        SessionKind::Dictation,
                        recording_prefix.clone(),
                    ) {
                        Ok(session) => {
                            state.set_deepgram_connection_status(
                                DeepgramConnectionStatus::Connected,
                            );
                            active_session = Some(session);
                        }
                        Err(error) => {
                            log::error!("failed to start Deepgram session: {}", error);
                            recording_prefix.clear();
                            state.clear_overlay_text();
                            state.set_overlay_text_opacity(1.0);
                            state.set_deepgram_connection_status(
                                DeepgramConnectionStatus::Disconnected,
                            );
                            state.report_error(error.to_string());
                        }
                    }
                }
                Command::StartCorrectionSession => {
                    if active_session.is_some() {
                        log::info!("ignoring correction start while session is active");
                        continue;
                    }

                    buffered_text = state.overlay_text().to_string();
                    if buffered_text.trim().is_empty() {
                        log::warn!(
                            "ignoring correction request because no working text is available"
                        );
                        state.clear_overlay_correction_text();
                        state.set_overlay_correction_active(false);
                        state.set_state(STATE_IDLE);
                        continue;
                    }

                    let current_sample_rate = thread_worker_sample_rate.load(Ordering::Relaxed);
                    if state.is_abort_requested() {
                        log::info!(
                            "skipping correction start because abort was requested before startup completed"
                        );
                        state.clear_overlay_correction_text();
                        state.set_overlay_correction_active(false);
                        state.set_overlay_text_opacity(1.0);
                        state.set_state(STATE_BUFFER_READY);
                        continue;
                    }

                    state.clear_abort_request();
                    state.restore_overlay();
                    state.set_overlay_correction_active(true);
                    state.clear_overlay_correction_text();
                    state.set_overlay_text(buffered_text.clone());
                    state.set_overlay_text_opacity(1.0);
                    let current_config = config_store.current();
                    let deepgram_config = match resolved_deepgram_config(&current_config) {
                        Ok(deepgram_config) => deepgram_config,
                        Err(error) => {
                            log::error!(
                                "failed to resolve Deepgram config for correction: {}",
                                error
                            );
                            state.set_overlay_correction_active(false);
                            state.report_error(error.to_string());
                            continue;
                        }
                    };

                    match start_session(
                        &runtime,
                        state.clone(),
                        &deepgram_config,
                        current_sample_rate,
                        SessionKind::Correction,
                        String::new(),
                    ) {
                        Ok(session) => {
                            state.set_deepgram_connection_status(
                                DeepgramConnectionStatus::Connected,
                            );
                            active_session = Some(session);
                        }
                        Err(error) => {
                            log::error!("failed to start correction Deepgram session: {}", error);
                            state.set_overlay_correction_active(false);
                            state.set_deepgram_connection_status(
                                DeepgramConnectionStatus::Disconnected,
                            );
                            state.report_error(error.to_string());
                        }
                    }
                }
                Command::PushAudio(pcm_data) => {
                    if let Some(session) = &active_session {
                        match session.push_audio(pcm_data) {
                            PushAudioResult::Ok => {}
                            PushAudioResult::Full => {
                                log::warn!("audio queue full; dropping audio chunk");
                            }
                            PushAudioResult::Closed => {
                                log::warn!("Deepgram session queue closed");
                                active_session = None;
                                state.set_deepgram_connection_status(
                                    DeepgramConnectionStatus::Disconnected,
                                );
                                state.report_error("Deepgram session queue closed unexpectedly");
                            }
                        }
                    }
                }
                Command::StopSessionAndPaste => {
                    if let Some(session) = active_session.take() {
                        match session.finish(&runtime, state.clone()) {
                            Ok(text) => {
                                buffered_text = text;
                                flush_buffered_text_or_paste(&state, &mut buffered_text, true);
                            }
                            Err(error) => {
                                log::error!("Deepgram session failed: {}", error);
                                recording_prefix.clear();
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                                state.report_error(error.to_string());
                            }
                        }
                    }
                }
                Command::QueueClipboardInsertion => {
                    let text_to_insert = match read_clipboard_text() {
                        Ok(text) => text,
                        Err(error) => {
                            log::warn!("ignoring clipboard insertion because clipboard read failed: {}", error);
                            continue;
                        }
                    };

                    if text_to_insert.trim().is_empty() {
                        log::info!("ignoring clipboard insertion because clipboard is empty");
                        continue;
                    }

                    let was_recording = state.is_recording();
                    if was_recording {
                        if let Some(session) = active_session.take() {
                            match session.finish(&runtime, state.clone()) {
                                Ok(text) => {
                                    recording_prefix = text;
                                }
                                Err(error) => {
                                    log::error!(
                                        "failed to checkpoint active session before clipboard insertion: {}",
                                        error
                                    );
                                    recording_prefix.clear();
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.report_error(error.to_string());
                                    continue;
                                }
                            }
                        }
                    }

                    append_text_segment(&mut recording_prefix, text_to_insert.as_str());
                    if !recording_prefix.is_empty() && !recording_prefix.ends_with(|c: char| c.is_whitespace()) {
                        recording_prefix.push(' ');
                    }

                    state.set_overlay_text(recording_prefix.clone());

                    if was_recording {
                        let current_sample_rate = thread_worker_sample_rate.load(Ordering::Relaxed);
                        let current_config = config_store.current();
                        let deepgram_config = match resolved_deepgram_config(&current_config) {
                            Ok(deepgram_config) => deepgram_config,
                            Err(error) => {
                                log::error!(
                                    "failed to resolve Deepgram config for clipboard insertion: {}",
                                    error
                                );
                                buffered_text = recording_prefix.clone();
                                recording_prefix.clear();
                                if buffered_text.is_empty() {
                                    state.report_error(error.to_string());
                                } else {
                                    state.set_overlay_text(buffered_text.clone());
                                    state.set_state(STATE_BUFFER_READY);
                                }
                                continue;
                            }
                        };

                        match start_session(
                            &runtime,
                            state.clone(),
                            &deepgram_config,
                            current_sample_rate,
                            SessionKind::Dictation,
                            recording_prefix.clone(),
                        ) {
                            Ok(session) => {
                                active_session = Some(session);
                            }
                            Err(error) => {
                                log::error!(
                                    "failed to resume Deepgram session after clipboard insertion: {}",
                                    error
                                );
                                buffered_text = recording_prefix.clone();
                                recording_prefix.clear();
                                if buffered_text.is_empty() {
                                    state.report_error(error.to_string());
                                } else {
                                    state.set_overlay_text(buffered_text.clone());
                                    state.set_state(STATE_BUFFER_READY);
                                }
                            }
                        }
                    }
                }
                Command::StopCorrectionSessionAndApply => {
                    let should_resume_after_correction = resume_after_correction;
                    resume_after_correction = false;

                    if let Some(session) = active_session.take() {
                        let correction_request = match session.finish(&runtime, state.clone()) {
                            Ok(text) => text,
                            Err(error) => {
                                log::error!("correction Deepgram session failed: {}", error);
                                state.set_overlay_correction_active(false);
                                state.clear_overlay_correction_text();
                                state.set_overlay_text(buffered_text.clone());
                                state.set_overlay_text_opacity(1.0);
                                state.report_error(error.to_string());
                                continue;
                            }
                        };

                        state.set_overlay_correction_active(false);
                        state.clear_overlay_correction_text();

                        if correction_request.trim().is_empty() {
                            log::info!("ignoring empty correction request");
                            state.set_overlay_text(buffered_text.clone());
                            state.set_overlay_text_opacity(1.0);
                            state.set_state(STATE_BUFFER_READY);
                            continue;
                        }

                        let current_config = config_store.current();
                        let transformation_config =
                            match resolve_transformation_config(&current_config) {
                                Ok(config) => config,
                                Err(error) => {
                                    log::error!(
                                        "failed to resolve transformation config for correction: {}",
                                        error
                                    );
                                    state.set_overlay_text(buffered_text.clone());
                                    state.set_overlay_text_opacity(1.0);
                                    state.report_error(error.to_string());
                                    continue;
                                }
                            };

                        state.set_state(STATE_TRANSFORMING);
                        let correction_config =
                            transformation_correction_runtime_config(&transformation_config);
                        let prompt_input = build_correction_transform_input(
                            buffered_text.as_str(),
                            correction_request.as_str(),
                        );

                        match runtime.block_on(transform_text(
                            state.clone(),
                            &correction_config,
                            &prompt_input,
                            TransformationPreviewMode::ReplaceOverlay,
                        )) {
                            Ok(transformed_text) => {
                                log::info!(
                                    "correction transformation completed: chars={}",
                                    transformed_text.chars().count()
                                );
                                buffered_text = transformed_text;
                                state.set_overlay_text(buffered_text.clone());
                                state.set_overlay_text_opacity(1.0);

                                if should_resume_after_correction {
                                    recording_prefix = buffered_text.clone();
                                    if !recording_prefix.is_empty()
                                        && !recording_prefix.ends_with(|c: char| c.is_whitespace())
                                    {
                                        recording_prefix.push(' ');
                                    }
                                    state.set_overlay_text(recording_prefix.clone());

                                    let current_sample_rate =
                                        thread_worker_sample_rate.load(Ordering::Relaxed);
                                    let deepgram_config =
                                        match resolved_deepgram_config(&current_config) {
                                            Ok(config) => config,
                                            Err(error) => {
                                                log::error!(
                                                    "failed to resolve Deepgram config for resume after correction: {}",
                                                    error
                                                );
                                                state.set_state(STATE_BUFFER_READY);
                                                state.report_error(error.to_string());
                                                continue;
                                            }
                                        };

                                    match start_session(
                                        &runtime,
                                        state.clone(),
                                        &deepgram_config,
                                        current_sample_rate,
                                        SessionKind::Dictation,
                                        recording_prefix.clone(),
                                    ) {
                                        Ok(session) => {
                                            state.set_deepgram_connection_status(
                                                DeepgramConnectionStatus::Connected,
                                            );
                                            state.set_state(STATE_RECORDING);
                                            active_session = Some(session);
                                        }
                                        Err(error) => {
                                            log::error!(
                                                "failed to resume session after correction: {}",
                                                error
                                            );
                                            state.set_deepgram_connection_status(
                                                DeepgramConnectionStatus::Disconnected,
                                            );
                                            state.report_error(error.to_string());
                                        }
                                    }
                                } else {
                                    state.set_state(STATE_BUFFER_READY);
                                }
                            }
                            Err(error) => {
                                log::error!("correction transformation failed: {}", error);
                                state.set_overlay_text(buffered_text.clone());
                                state.set_overlay_text_opacity(1.0);
                                state.report_error(error.to_string());
                            }
                        }
                    }
                }
                Command::StopSessionAndTransformAndPaste => {
                    if let Some(session) = active_session.take() {
                        match session.finish(&runtime, state.clone()) {
                            Ok(text) => {
                                buffered_text = text;
                                if buffered_text.trim().is_empty() {
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                if state.consume_abort_request() {
                                    log::info!("discarding transformation because abort was requested");
                                    buffered_text.clear();
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                let current_config = config_store.current();
                                let transformation_config =
                                    match resolve_transformation_config(&current_config) {
                                        Ok(config) => Some(config),
                                        Err(error) => {
                                            log::info!("transformation provider not active ({}); pasting raw transcript", error);
                                            None
                                        }
                                    };

                                if let Some(transformation_config) = transformation_config {
                                    state.set_state(STATE_TRANSFORMING);
                                    match runtime.block_on(transform_text(
                                        state.clone(),
                                        &transformation_config,
                                        &buffered_text,
                                        TransformationPreviewMode::ReplaceOverlay,
                                    )) {
                                        Ok(transformed_text) => {
                                            log::info!(
                                                "transformation completed: chars={}",
                                                transformed_text.chars().count()
                                            );
                                            buffered_text = transformed_text;
                                            flush_buffered_text_or_paste(
                                                &state,
                                                &mut buffered_text,
                                                true,
                                            );
                                        }
                                        Err(error) => {
                                            log::error!("transformation failed: {}", error);
                                            state.set_overlay_text(buffered_text.clone());
                                            state.set_overlay_text_opacity(1.0);
                                            state.report_error(error.to_string());
                                        }
                                    }
                                } else {
                                    state.set_overlay_text_opacity(1.0);
                                    flush_buffered_text_or_paste(
                                        &state,
                                        &mut buffered_text,
                                        true,
                                    );
                                }
                            }
                            Err(error) => {
                                log::error!("Deepgram session failed: {}", error);
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                                state.report_error(error.to_string());
                            }
                        }
                    }
                }
                Command::StopSessionAndTransformAndResume => {
                    if let Some(session) = active_session.take() {
                        match session.finish(&runtime, state.clone()) {
                            Ok(text) => {
                                buffered_text = text;
                                if buffered_text.trim().is_empty() {
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                if state.consume_abort_request() {
                                    log::info!("discarding transformation because abort was requested");
                                    buffered_text.clear();
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                let current_config = config_store.current();
                                let transformation_config =
                                    match resolve_transformation_config(&current_config) {
                                        Ok(config) => Some(config),
                                        Err(error) => {
                                            log::info!("transformation provider not active ({}); resuming with raw transcript", error);
                                            None
                                        }
                                    };

                                if let Some(transformation_config) = transformation_config {
                                    state.set_state(STATE_TRANSFORMING);
                                    match runtime.block_on(transform_text(
                                        state.clone(),
                                        &transformation_config,
                                        &buffered_text,
                                        TransformationPreviewMode::ReplaceOverlay,
                                    )) {
                                        Ok(transformed_text) => {
                                            log::info!(
                                                "transformation completed: chars={}",
                                                transformed_text.chars().count()
                                            );
                                            buffered_text = transformed_text;
                                        }
                                        Err(error) => {
                                            log::error!("transformation failed: {}", error);
                                            state.set_overlay_text(buffered_text.clone());
                                            state.set_overlay_text_opacity(1.0);
                                            state.report_error(error.to_string());
                                            continue;
                                        }
                                    }
                                }

                                recording_prefix = buffered_text.clone();
                                if !recording_prefix.is_empty()
                                    && !recording_prefix
                                        .ends_with(|c: char| c.is_whitespace())
                                {
                                    recording_prefix.push(' ');
                                }

                                state.set_overlay_text(recording_prefix.clone());

                                let current_sample_rate =
                                    thread_worker_sample_rate.load(Ordering::Relaxed);
                                let deepgram_config =
                                    match resolved_deepgram_config(&current_config) {
                                        Ok(config) => config,
                                        Err(error) => {
                                            log::error!(
                                                "failed to resolve Deepgram config for resume: {}",
                                                error
                                            );
                                            state.set_state(STATE_BUFFER_READY);
                                            state.report_error(error.to_string());
                                            continue;
                                        }
                                    };

                                match start_session(
                                    &runtime,
                                    state.clone(),
                                    &deepgram_config,
                                    current_sample_rate,
                                    SessionKind::Dictation,
                                    recording_prefix.clone(),
                                ) {
                                    Ok(session) => {
                                        state.set_deepgram_connection_status(
                                            DeepgramConnectionStatus::Connected,
                                        );
                                        state.set_state(STATE_RECORDING);
                                        active_session = Some(session);
                                    }
                                    Err(error) => {
                                        log::error!("failed to resume session: {}", error);
                                        state.set_deepgram_connection_status(
                                            DeepgramConnectionStatus::Disconnected,
                                        );
                                        state.report_error(error.to_string());
                                    }
                                }
                            }
                            Err(error) => {
                                log::error!("Deepgram session failed: {}", error);
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                                state.report_error(error.to_string());
                            }
                        }
                    }
                }
                Command::TransformBuffer => {
                    if buffered_text.trim().is_empty() {
                        buffered_text = state.overlay_text().to_string();
                    }

                    if buffered_text.trim().is_empty() {
                        log::info!("ignoring transform_buffer command because buffer is empty");
                        continue;
                    }

                    let current_config = config_store.current();
                    let transformation_config = match resolve_transformation_config(&current_config)
                    {
                        Ok(config) => config,
                        Err(error) => {
                            log::error!("failed to resolve transformation config: {}", error);
                            state.clear_overlay_text();
                            state.set_overlay_text_opacity(1.0);
                            state.report_error(error.to_string());
                            continue;
                        }
                    };

                    match runtime.block_on(transform_text(
                        state.clone(),
                        &transformation_config,
                        &buffered_text,
                        TransformationPreviewMode::ReplaceOverlay,
                    )) {
                        Ok(transformed_text) => {
                            log::info!(
                                "transformation completed: chars={}",
                                transformed_text.chars().count()
                            );
                            buffered_text = transformed_text;
                            state.set_overlay_text(buffered_text.clone());
                            state.set_state(STATE_BUFFER_READY);
                        }
                        Err(error) => {
                            log::error!("transformation failed: {}", error);
                            state.clear_overlay_text();
                            state.set_overlay_text_opacity(1.0);
                            state.report_error(error.to_string());
                        }
                    }
                }
                Command::PasteBuffer => {
                    if buffered_text.trim().is_empty() {
                        buffered_text = state.overlay_text().to_string();
                    }

                    if buffered_text.trim().is_empty() {
                        log::info!("ignoring paste_buffer command because buffer is empty");
                        state.clear_overlay_text();
                        state.set_overlay_text_opacity(1.0);
                        state.set_state(STATE_IDLE);
                        continue;
                    }

                    flush_buffered_text_or_paste(&state, &mut buffered_text, true);
                }
            }
        }
    });

    TranscriptionController { command_tx }
}

fn resolved_deepgram_config(config: &Config) -> Result<DeepgramConfig, String> {
    let runtime_config = crate::config::materialize_runtime_config(config);
    if runtime_config
        .deepgram
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .is_some()
    {
        Ok(runtime_config.deepgram)
    } else {
        Err("Deepgram API key is not configured".to_owned())
    }
}

fn resolve_transformation_config(
    config: &Config,
) -> Result<TransformationRuntimeConfig, String> {
    config.resolve_transformation_config()
}
