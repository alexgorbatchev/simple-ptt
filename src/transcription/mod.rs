pub mod clipboard;
pub mod session;
mod progress;
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
    AppState, STATE_BUFFER_READY, STATE_IDLE, STATE_RECORDING,
    STATE_TRANSFORMING,
};
use crate::transformation::{
    transform_text, TransformationPreviewMode, TransformationRuntimeConfig,
};

const DEFAULT_SAMPLE_RATE: u32 = 16000;

enum Command {
    SetSampleRate(u32),
    StartSession { recording_prefix: String },
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

#[cfg(test)]
impl TranscriptionController {
    /// A controller whose commands are queued and never handled, for tests
    /// that look at the state a hotkey leaves before the worker acts.
    pub fn without_worker() -> Self {
        let (command_tx, command_rx) = channel();
        // Keeps the queue open for the test's lifetime.
        Box::leak(Box::new(command_rx));
        Self { command_tx }
    }
}

impl TranscriptionController {
    pub fn set_sample_rate(&self, sample_rate: u32) {
        let _ = self.command_tx.send(Command::SetSampleRate(sample_rate));
    }

    pub fn start_session(&self, state: &AppState) -> Result<(), String> {
        // Capture the editable annotation before the hotkey changes the state
        // to recording; the worker may receive this command afterwards.
        let recording_prefix = if state.get_state() == STATE_BUFFER_READY {
            state.overlay_text().to_string()
        } else {
            String::new()
        };
        state.set_state(STATE_RECORDING);
        self.command_tx
            .send(Command::StartSession { recording_prefix })
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
                Command::StartSession { recording_prefix: requested_prefix } => {
                    if let Some(old_session) = active_session.take() {
                        log::info!("cleaning up previous active session before starting new session");
                        if matches!(finish_session(old_session, &state), Ok(None)) {
                            buffered_text.clear();
                            recording_prefix.clear();
                            resume_after_correction = false;
                            continue;
                        }
                    }

                    recording_prefix = requested_prefix;

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
                    // Skipping the start uses the abort up; left set, it would
                    // skip every later start too.
                    if state.consume_abort_request() {
                        log::info!(
                            "skipping dictation start because abort was requested before startup completed"
                        );
                        recording_prefix.clear();
                        state.clear_overlay_text();
                        state.set_overlay_text_opacity(1.0);
                        state.set_state(STATE_IDLE);
                        continue;
                    }

                    // The hotkey owns this presentation's visibility. A
                    // queued start must not undo a later empty F5 dismissal.
                    state.set_overlay_text(recording_prefix.clone());
                    state.set_overlay_text_opacity(1.0);

                    match start_session(
                        state.clone(),
                        &deepgram_config,
                        current_sample_rate,
                        SessionKind::Dictation,
                        recording_prefix.clone(),
                    ) {
                        Ok(session) => {
                            active_session = Some(session);
                        }
                        Err(SessionError::Cancelled) => {
                            complete_session_cancellation(&state, &mut buffered_text, SessionKind::Dictation);
                        }
                        Err(error) => {
                            log::error!("failed to start Deepgram session: {}", error);
                            recording_prefix.clear();
                            state.clear_overlay_text();
                            state.set_overlay_text_opacity(1.0);
                            state.report_error(error.to_string());
                        }
                    }
                }
                Command::StartCorrectionSession => {
                    let was_recording_dictation = active_session
                        .as_ref()
                        .map(|session| session.kind() == SessionKind::Dictation)
                        .unwrap_or(false);

                    if let Some(session) = active_session.take() {
                        match finish_session(session, &state) {
                            Ok(None) => {
                                buffered_text.clear();
                                recording_prefix.clear();
                                resume_after_correction = false;
                                continue;
                            }
                            Ok(Some(text)) => {
                                if !text.trim().is_empty() {
                                    buffered_text = text;
                                }
                            }
                            Err(error) => {
                                log::error!(
                                    "failed to checkpoint dictation before correction: {}",
                                    error
                                );
                                state.report_error(error.to_string());
                                continue;
                            }
                        }
                    }

                    if buffered_text.trim().is_empty() {
                        buffered_text = state.overlay_text().to_string();
                    }

                    if buffered_text.trim().is_empty() {
                        log::warn!(
                            "ignoring correction request because no working text is available"
                        );
                        state.clear_overlay_correction_text();
                        state.set_overlay_correction_active(false);
                        state.set_state(STATE_IDLE);
                        continue;
                    }

                    resume_after_correction = was_recording_dictation;

                    let current_sample_rate = thread_worker_sample_rate.load(Ordering::Relaxed);
                    // As for the dictation start: skipping uses the abort up.
                    if state.consume_abort_request() {
                        log::info!(
                            "skipping correction start because abort was requested before startup completed"
                        );
                        state.clear_overlay_correction_text();
                        state.set_overlay_correction_active(false);
                        state.set_overlay_text_opacity(1.0);
                        state.set_state(STATE_BUFFER_READY);
                        continue;
                    }

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
                        state.clone(),
                        &deepgram_config,
                        current_sample_rate,
                        SessionKind::Correction,
                        String::new(),
                    ) {
                        Ok(session) => {
                            active_session = Some(session);
                        }
                        Err(SessionError::Cancelled) => {
                            complete_session_cancellation(&state, &mut buffered_text, SessionKind::Correction);
                        }
                        Err(error) => {
                            log::error!("failed to start correction Deepgram session: {}", error);
                            state.set_overlay_correction_active(false);
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
                                let session = active_session.take().expect("the closed queue belongs to the active session");
                                match finish_session(session, &state) {
                                    Ok(None) => {
                                        buffered_text.clear();
                                        recording_prefix.clear();
                                        resume_after_correction = false;
                                    }
                                    Err(error) => state.report_error(error),
                                    Ok(Some(_)) => state.report_error("Deepgram session queue closed unexpectedly"),
                                }
                            }
                        }
                    }
                }
                Command::StopSessionAndPaste => {
                    if let Some(session) = active_session.take() {
                        match finish_session(session, &state) {
                            Ok(None) => {
                                buffered_text.clear();
                                recording_prefix.clear();
                                resume_after_correction = false;
                                continue;
                            }
                            Ok(Some(text)) => {
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
                            match finish_session(session, &state) {
                                Ok(None) => {
                                    buffered_text.clear();
                                    recording_prefix.clear();
                                    resume_after_correction = false;
                                    continue;
                                }
                                Ok(Some(text)) => {
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
                            state.clone(),
                            &deepgram_config,
                            current_sample_rate,
                            SessionKind::Dictation,
                            recording_prefix.clone(),
                        ) {
                            Ok(session) => {
                                active_session = Some(session);
                            }
                            Err(SessionError::Cancelled) => {
                                complete_session_cancellation(&state, &mut buffered_text, SessionKind::Dictation);
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

                    // Also clear the hotkey's pending resume intent if the
                    // correction connection failed and there is no session.
                    let _resuming =
                        ResumingDictation::new(&state, should_resume_after_correction);
                    if let Some(session) = active_session.take() {
                        let correction_request = match finish_session(session, &state) {
                            Ok(None) => {
                                buffered_text.clear();
                                recording_prefix.clear();
                                resume_after_correction = false;
                                continue;
                            }
                            Ok(Some(text)) => text,
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

                        let current_config = config_store.current();
                        match apply_correction_request(
                            &runtime,
                            &state,
                            &current_config,
                            &mut buffered_text,
                            &correction_request,
                            should_resume_after_correction,
                        ) {
                            Ok(Some(prefix)) => {
                                recording_prefix = prefix;
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
                                    state.clone(),
                                    &deepgram_config,
                                    current_sample_rate,
                                    SessionKind::Dictation,
                                    recording_prefix.clone(),
                                ) {
                                    Ok(session) => {
                                        state.set_state(STATE_RECORDING);
                                        active_session = Some(session);
                                    }
                                    Err(SessionError::Cancelled) => {
                                        complete_session_cancellation(&state, &mut buffered_text, SessionKind::Dictation);
                                    }
                                    Err(error) => {
                                        log::error!(
                                            "failed to resume session after correction: {}",
                                            error
                                        );
                                        state.report_error(error.to_string());
                                    }
                                }
                            }
                            Ok(None) => {}
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
                        match finish_session(session, &state) {
                            Ok(None) => {
                                buffered_text.clear();
                                recording_prefix.clear();
                                resume_after_correction = false;
                                continue;
                            }
                            Ok(Some(text)) => {
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
                                    match runtime.block_on(finish_transformation(&state, transform_text(
                                        state.clone(),
                                        &transformation_config,
                                        &buffered_text,
                                        TransformationPreviewMode::PreserveOverlay,
                                    ))) {
                                        Ok(Some(transformed_text)) => {
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
                                        Ok(None) => {
                                            buffered_text.clear();
                                            recording_prefix.clear();
                                            continue;
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
                    // The transform hotkey marked dictation as resuming;
                    // capture carries on until this ends, however it ends.
                    let _resuming = ResumingDictation::new(&state, true);
                    if let Some(session) = active_session.take() {
                        match finish_session(session, &state) {
                            Ok(None) => {
                                buffered_text.clear();
                                recording_prefix.clear();
                                resume_after_correction = false;
                                continue;
                            }
                            Ok(Some(text)) => {
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
                                    match runtime.block_on(finish_transformation(&state, transform_text(
                                        state.clone(),
                                        &transformation_config,
                                        &buffered_text,
                                        TransformationPreviewMode::ReplaceOverlay,
                                    ))) {
                                        Ok(Some(transformed_text)) => {
                                            log::info!(
                                                "transformation completed: chars={}",
                                                transformed_text.chars().count()
                                            );
                                            buffered_text = transformed_text;
                                        }
                                        Ok(None) => {
                                            buffered_text.clear();
                                            recording_prefix.clear();
                                            continue;
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
                                    state.clone(),
                                    &deepgram_config,
                                    current_sample_rate,
                                    SessionKind::Dictation,
                                    recording_prefix.clone(),
                                ) {
                                    Ok(session) => {
                                        state.set_state(STATE_RECORDING);
                                        active_session = Some(session);
                                    }
                                    Err(SessionError::Cancelled) => {
                                        complete_session_cancellation(&state, &mut buffered_text, SessionKind::Dictation);
                                    }
                                    Err(error) => {
                                        log::error!("failed to resume session: {}", error);
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

                    match runtime.block_on(finish_transformation(&state, transform_text(
                        state.clone(),
                        &transformation_config,
                        &buffered_text,
                        TransformationPreviewMode::ReplaceOverlay,
                    ))) {
                        Ok(Some(transformed_text)) => {
                            log::info!(
                                "transformation completed: chars={}",
                                transformed_text.chars().count()
                            );
                            buffered_text = transformed_text;
                            state.set_overlay_text(buffered_text.clone());
                            state.set_state(STATE_BUFFER_READY);
                        }
                        Ok(None) => {
                            buffered_text.clear();
                            recording_prefix.clear();
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

                    if buffered_text.trim().is_empty() && !state.is_abort_requested() {
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

/// Apply spoken changes, if any, then prepare the original dictation to
/// resume. `None` leaves the annotation buffered without starting a session.
fn apply_correction_request(
    runtime: &Runtime,
    state: &Arc<AppState>,
    config: &Config,
    buffered_text: &mut String,
    correction_request: &str,
    resume_dictation: bool,
) -> Result<Option<String>, String> {
    state.set_overlay_correction_active(false);
    state.clear_overlay_correction_text();
    if state.consume_abort_request() {
        state.set_overlay_text(buffered_text.clone());
        state.set_overlay_text_opacity(1.0);
        state.set_state(STATE_BUFFER_READY);
        return Ok(None);
    }

    // Only the correction field received speech while the key was held.
    // The annotation may have received keyboard edits since its checkpoint.
    *buffered_text = state.overlay_text().to_string();
    if correction_request.trim().is_empty() {
        log::info!("ignoring empty correction request");
    } else {
        let transformation_config = resolve_transformation_config(config)?;
        let correction_config = transformation_correction_runtime_config(&transformation_config);
        let prompt_input = build_correction_transform_input(buffered_text, correction_request);
        state.set_state(STATE_TRANSFORMING);
        let transformed = runtime.block_on(finish_transformation(state, transform_text(
            state.clone(),
            &correction_config,
            &prompt_input,
            TransformationPreviewMode::ReplaceOverlay,
        )))?;
        let Some(transformed) = transformed else {
            buffered_text.clear();
            return Ok(None);
        };
        *buffered_text = transformed;
    }

    state.set_overlay_text(buffered_text.clone());
    state.set_overlay_text_opacity(1.0);
    if resume_dictation {
        let mut prefix = buffered_text.clone();
        if !prefix.is_empty() && !prefix.ends_with(|c: char| c.is_whitespace()) {
            prefix.push(' ');
        }
        state.set_overlay_text(prefix.clone());
        Ok(Some(prefix))
    } else {
        state.set_state(STATE_BUFFER_READY);
        Ok(None)
    }
}

/// Cancel an LLM request even while its connection or stream is quiet, and
/// settle it through the same discard path as cancelled transcription.
async fn finish_transformation(
    state: &AppState,
    transformation: impl std::future::Future<Output = Result<String, String>>,
) -> Result<Option<String>, String> {
    let result = tokio::select! {
        biased;
        _ = state.wait_for_abort() => Err(SessionError::Cancelled),
        result = transformation => result.map_err(SessionError::Failed),
    };
    settle_session_finish(result, state)
}

/// Settle cancellation before any caller can paste, transform, or resume.
/// `None` means the recording was discarded, not an empty transcript.
fn finish_session(session: ActiveSession, state: &AppState) -> Result<Option<String>, String> {
    settle_session_finish(session.finish(state), state)
}

fn settle_session_finish(
    result: Result<String, SessionError>,
    state: &AppState,
) -> Result<Option<String>, String> {
    // Also catch an abort that raced a normal completion or timeout.
    if state.consume_abort_request() || matches!(result, Err(SessionError::Cancelled)) {
        state.set_deepgram_waiting(false);
        state.set_dictation_resuming(false);
        state.set_overlay_correction_active(false);
        state.clear_overlay_correction_text();
        state.clear_overlay_text();
        state.set_overlay_text_opacity(1.0);
        state.set_state(STATE_IDLE);
        log::info!("discarded aborted transcription session");
        return Ok(None);
    }
    match result {
        Ok(text) => Ok(Some(text)),
        Err(SessionError::Failed(error)) => Err(error),
        Err(SessionError::Cancelled) => unreachable!("aborted sessions are settled above"),
    }
}

fn complete_session_cancellation(state: &AppState, buffered_text: &mut String, kind: SessionKind) {
    state.consume_abort_request();
    state.set_dictation_resuming(false);
    state.set_deepgram_waiting(false);
    state.clear_overlay_error_text();
    state.clear_overlay_correction_text();
    state.set_overlay_correction_active(false);
    state.set_overlay_text_opacity(1.0);
    match kind {
        SessionKind::Dictation => {
            buffered_text.clear();
            state.clear_overlay_text();
            state.set_state(STATE_IDLE);
        }
        SessionKind::Correction => {
            state.set_overlay_text(buffered_text.clone());
            state.set_state(STATE_BUFFER_READY);
        }
    }
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

/// Marks dictation as resuming (`AppState::set_dictation_resuming`) while it
/// lives, and clears the mark however the work it covers ends.
struct ResumingDictation<'a> {
    state: &'a AppState,
}

impl<'a> ResumingDictation<'a> {
    fn new(state: &'a AppState, resuming: bool) -> Self {
        state.set_dictation_resuming(resuming);
        Self { state }
    }
}

impl Drop for ResumingDictation<'_> {
    fn drop(&mut self) {
        self.state.set_dictation_resuming(false);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn settings_cancellation_interrupts_a_quiet_transformation() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = crate::state::AppState::new();
        state.set_state(crate::state::STATE_TRANSFORMING);
        state.set_dictation_resuming(true);
        state.set_overlay_text("A partial rewrite");
        state.set_overlay_correction_active(true);
        state.set_overlay_correction_text("A correction");
        state.dismiss_overlay();
        let request = async {
            state.request_abort();
            std::future::pending::<Result<String, String>>().await
        };
        let result = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(1),
                super::finish_transformation(&state, request)).await
        }).expect("cancellation must not wait for another LLM response");
        assert_eq!(result.unwrap(), None);
        assert_eq!(state.get_state(), crate::state::STATE_IDLE);
        assert!(state.is_overlay_dismissed());
        assert!(state.overlay_text().is_empty());
        assert!(state.overlay_correction_text().is_empty());
        assert!(!state.is_overlay_correction_active());
        assert!(!state.is_capturing_audio());
        assert!(!state.is_abort_requested());
    }

    #[test]
    fn settings_cancellation_wins_a_transformation_result_or_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for result in [Ok("Discard this rewrite".to_owned()), Err("A late error".to_owned())] {
            let state = crate::state::AppState::new();
            state.set_state(crate::state::STATE_TRANSFORMING);
            state.dismiss_overlay();
            assert_eq!(runtime.block_on(super::finish_transformation(&state, async {
                state.request_abort();
                result
            })).unwrap(), None);
            assert!(state.is_overlay_dismissed());
            assert!(state.overlay_error_text().is_empty());
            assert_eq!(state.get_state(), crate::state::STATE_IDLE);
            // The abort is consumed, so subsequent work can finish normally.
            assert_eq!(runtime.block_on(super::finish_transformation(&state,
                async { Ok("The next rewrite".to_owned()) })).unwrap(),
                Some("The next rewrite".to_owned()));
        }
    }

    #[test]
    fn resuming_dictation_captures_the_editable_annotation_before_the_state_changes() {
        let state = crate::state::AppState::new();
        state.set_overlay_text("The annotation, including keyboard edits.");
        state.set_state(crate::state::STATE_BUFFER_READY);
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let controller = super::TranscriptionController { command_tx };

        controller.start_session(&state).unwrap();
        state.set_state(crate::state::STATE_RECORDING);
        state.set_overlay_text("A later UI update");

        let super::Command::StartSession { recording_prefix } = command_rx.recv().unwrap() else {
            panic!("expected a dictation start");
        };
        assert_eq!(recording_prefix, "The annotation, including keyboard edits.");
    }

    #[test]
    fn an_empty_correction_resumes_dictation_without_requiring_an_llm() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for request in ["", " \n\t "] {
            let state = crate::state::AppState::new();
            state.set_state(crate::state::STATE_PROCESSING);
            state.set_overlay_correction_active(true);
            state.set_overlay_correction_text("provisional correction");
            state.set_overlay_text_opacity(0.25);
            let mut annotation = "Keep the entire narration.".to_owned();
            state.set_overlay_text(annotation.clone());
            let _resuming = super::ResumingDictation::new(&state, true);

            let prefix = super::apply_correction_request(
                &runtime, &state, &crate::config::Config::default(),
                &mut annotation, request, true,
            ).unwrap();

            assert_eq!(prefix, Some("Keep the entire narration. ".to_owned()));
            assert_eq!(&*state.overlay_text(), "Keep the entire narration. ");
            assert!(state.is_capturing_audio());
            assert!(!state.is_overlay_correction_active());
            assert!(state.overlay_correction_text().is_empty());
            assert_eq!(state.overlay_text_opacity(), 1.0);
            assert!(state.overlay_error_text().is_empty());
        }
    }

    #[test]
    fn an_empty_correction_of_a_buffer_preserves_it_without_starting_dictation() {
        let state = crate::state::AppState::new();
        state.set_state(crate::state::STATE_PROCESSING);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut annotation = "A buffered annotation.".to_owned();
        state.set_overlay_text(annotation.clone());

        assert_eq!(super::apply_correction_request(
            &runtime, &state, &crate::config::Config::default(),
            &mut annotation, "", false,
        ).unwrap(), None);

        assert_eq!(state.get_state(), crate::state::STATE_BUFFER_READY);
        assert_eq!(&*state.overlay_text(), "A buffered annotation.");
        assert!(!state.is_capturing_audio());
    }

    #[test]
    fn an_empty_correction_preserves_edits_made_while_the_key_was_held() {
        let state = crate::state::AppState::new();
        state.set_state(crate::state::STATE_PROCESSING);
        state.set_overlay_text("The edited narration.");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut annotation = "The earlier narration.".to_owned();

        let prefix = super::apply_correction_request(
            &runtime, &state, &crate::config::Config::default(),
            &mut annotation, "", true,
        ).unwrap();

        assert_eq!(prefix, Some("The edited narration. ".to_owned()));
        assert_eq!(annotation, "The edited narration.");
        assert_eq!(&*state.overlay_text(), "The edited narration. ");
    }

    #[test]
    fn cancelling_a_dictation_connection_returns_to_idle_without_reopening_the_overlay() {
        use crate::state::{AppState, STATE_IDLE, STATE_PROCESSING};
        let state = AppState::new();
        state.set_state(STATE_PROCESSING);
        state.set_dictation_resuming(true);
        state.set_deepgram_waiting(true);
        state.request_abort();
        state.dismiss_overlay();
        let mut buffered_text = "must not be pasted".to_owned();
        super::complete_session_cancellation(&state, &mut buffered_text, super::SessionKind::Dictation);
        assert_eq!(state.get_state(), STATE_IDLE);
        assert!(state.is_overlay_dismissed());
        assert!(!state.is_abort_requested());
        assert!(!state.is_capturing_audio());
        assert!(!state.is_deepgram_waiting());
        assert!(buffered_text.is_empty());
    }

    #[test]
    fn cancelling_a_correction_connection_preserves_the_annotation_without_pasting_it() {
        use crate::state::{AppState, STATE_BUFFER_READY};
        let state = AppState::new();
        state.request_abort();
        state.dismiss_overlay();
        state.set_overlay_correction_active(true);
        state.set_overlay_correction_text("unfinished correction");
        let mut buffered_text = "the annotation".to_owned();
        super::complete_session_cancellation(&state, &mut buffered_text, super::SessionKind::Correction);
        assert_eq!(state.get_state(), STATE_BUFFER_READY);
        assert_eq!(&*state.overlay_text(), "the annotation");
        assert!(!state.is_overlay_correction_active());
        assert!(!state.is_abort_requested());
        assert!(state.is_overlay_dismissed());
    }

    #[test]
    fn aborted_session_finish_clears_capture_and_abort_state() {
        use crate::state::{AppState, STATE_IDLE, STATE_PROCESSING};
        let state = AppState::new();
        state.set_overlay_text("discard this");
        state.set_overlay_correction_text("discard correction");
        state.set_overlay_correction_active(true);
        state.set_dictation_resuming(true);
        state.set_deepgram_waiting(true);
        state.set_state(STATE_PROCESSING);
        state.dismiss_overlay();
        state.request_abort();

        let result = super::settle_session_finish(Err(super::SessionError::Cancelled), &state);

        assert_eq!(result, Ok(None));
        assert_eq!(state.get_state(), STATE_IDLE);
        assert!(!state.is_abort_requested());
        assert!(!state.is_deepgram_waiting());
        assert!(!state.is_capturing_audio());
        assert!(!state.is_overlay_correction_active());
        assert!(state.overlay_text().is_empty());
        assert!(state.overlay_correction_text().is_empty());
        assert!(state.is_overlay_dismissed());
    }

    #[test]
    fn an_abort_racing_empty_session_completion_is_consumed() {
        let state = crate::state::AppState::new();
        state.set_state(crate::state::STATE_PROCESSING);
        state.request_abort();

        assert_eq!(super::settle_session_finish(Ok(String::new()), &state), Ok(None));
        assert!(!state.is_abort_requested());
        assert_eq!(state.get_state(), crate::state::STATE_IDLE);
    }

    #[test]
    fn session_finish_errors_remain_errors_instead_of_pasting_partial_text() {
        let state = crate::state::AppState::new();
        state.set_overlay_text("incomplete words");
        let result = super::settle_session_finish(
            Err(super::SessionError::Failed("shutdown timed out".to_owned())), &state,
        );
        assert_eq!(result, Err("shutdown timed out".to_owned()));
    }

    #[test]
    fn dictation_counts_as_resuming_only_while_the_guard_lives() {
        let state = crate::state::AppState::new();
        {
            let _resuming = super::ResumingDictation::new(&state, true);
            assert!(state.is_dictation_resuming());
        }
        assert!(!state.is_dictation_resuming());

        let _not_resuming = super::ResumingDictation::new(&state, false);
        assert!(!state.is_dictation_resuming());
    }

    /// A worker for `state`, with a Deepgram API key set so a dictation start
    /// gets as far as its abort check. No test here lets it connect.
    fn worker(state: &std::sync::Arc<crate::state::AppState>) -> super::TranscriptionController {
        let mut config = crate::config::Config::default();
        config.deepgram.api_key = Some("test-key".to_owned());
        let config_store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from("/tmp/config.toml"),
        );
        super::spawn_transcription_thread(state.clone(), config_store)
    }

    /// Waits up to 2 s for `state` to reach `expected`.
    fn wait_for_state(state: &crate::state::AppState, expected: u8) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.get_state() != expected && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(state.get_state(), expected);
    }

    #[test]
    fn a_dictation_start_skipped_for_an_abort_uses_the_abort_up() {
        use crate::state::{AppState, STATE_IDLE, STATE_RECORDING};

        // The record key was pressed, then Escape before the session started.
        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        state.request_abort();
        worker(&state).start_session(&state).unwrap();

        wait_for_state(&state, STATE_IDLE);
        // Left set, it would skip every later start as well.
        assert!(!state.is_abort_requested());
    }

    #[test]
    fn a_correction_start_skipped_for_an_abort_uses_the_abort_up() {
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_RECORDING};

        let state = AppState::new();
        state.set_overlay_text("the annotation");
        state.set_state(STATE_RECORDING);
        state.request_abort();
        worker(&state).start_correction_session().unwrap();

        wait_for_state(&state, STATE_BUFFER_READY);
        assert!(!state.is_abort_requested());
    }

    #[test]
    fn a_queued_correction_finish_clears_capture_when_the_connection_failed() {
        let state = crate::state::AppState::new();
        state.report_error("correction connection failed");
        state.set_overlay_text("Keep the annotation");
        state.set_dictation_resuming(true);
        worker(&state).stop_correction_session_and_apply().unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.is_capturing_audio() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!state.is_capturing_audio());
        assert_eq!(state.get_state(), crate::state::STATE_ERROR);
        assert_eq!(&*state.overlay_text(), "Keep the annotation");
    }
}
