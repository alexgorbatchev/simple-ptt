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
    AppState, STATE_BUFFER_READY, STATE_IDLE, STATE_RECORDING,
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
                        if matches!(finish_session(old_session, &state), Ok(None)) {
                            buffered_text.clear();
                            recording_prefix.clear();
                            resume_after_correction = false;
                            continue;
                        }
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
                                log::warn!("Deepgram session queue closed");
                                active_session = None;
                                state.report_error("Deepgram session queue closed unexpectedly");
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

                        state.set_overlay_correction_active(false);
                        state.clear_overlay_correction_text();

                        if correction_request.trim().is_empty() {
                            log::info!("ignoring empty correction request");
                            state.set_overlay_text(buffered_text.clone());
                            state.set_overlay_text_opacity(1.0);
                            state.set_state(STATE_BUFFER_READY);
                            continue;
                        }

                        if state.consume_abort_request() {
                            log::info!("discarding correction transformation because abort was requested");
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

                        // Dictation that resumes after the correction keeps
                        // capturing while it is applied; its audio waits in
                        // this worker's queue for the resumed session.
                        let _resuming =
                            ResumingDictation::new(&state, should_resume_after_correction);
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
                                        Err(error) => {
                                            log::error!(
                                                "failed to resume session after correction: {}",
                                                error
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
                                    match runtime.block_on(transform_text(
                                        state.clone(),
                                        &transformation_config,
                                        &buffered_text,
                                        TransformationPreviewMode::PreserveOverlay,
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

/// Settle cancellation before any caller can paste, transform, or resume.
/// `None` means the recording was discarded, not an empty transcript.
fn finish_session(session: ActiveSession, state: &AppState) -> Result<Option<String>, String> {
    settle_session_finish(session.finish(state), state)
}

fn settle_session_finish(
    result: Result<String, SessionFinishError>,
    state: &AppState,
) -> Result<Option<String>, String> {
    // Also catch an abort that raced a normal completion or timeout.
    if state.consume_abort_request() || matches!(result, Err(SessionFinishError::Aborted)) {
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
        Err(SessionFinishError::Failed(error)) => Err(error),
        Err(SessionFinishError::Aborted) => unreachable!("aborted sessions are settled above"),
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
    fn aborted_session_finish_clears_capture_and_abort_state() {
        use crate::state::{AppState, STATE_IDLE, STATE_PROCESSING};
        let state = AppState::new();
        state.set_overlay_text("discard this");
        state.set_overlay_correction_text("discard correction");
        state.set_overlay_correction_active(true);
        state.set_dictation_resuming(true);
        state.set_state(STATE_PROCESSING);
        state.dismiss_overlay();
        state.request_abort();

        let result = super::settle_session_finish(Err(super::SessionFinishError::Aborted), &state);

        assert_eq!(result, Ok(None));
        assert_eq!(state.get_state(), STATE_IDLE);
        assert!(!state.is_abort_requested());
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
            Err(super::SessionFinishError::Failed("shutdown timed out".to_owned())), &state,
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
        worker(&state).start_session().unwrap();

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
}
