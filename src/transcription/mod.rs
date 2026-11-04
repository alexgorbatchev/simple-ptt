pub mod clipboard;
mod progress;
pub mod session;
pub mod text_builder;
mod transformation_history;

pub use clipboard::*;
pub use session::*;
pub use text_builder::*;

use bytes::Bytes;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use tokio::runtime::Runtime;

use crate::config::{Config, DeepgramConfig};
use crate::settings::LiveConfigStore;
use crate::state::{
    AppState, RecordingGeneration, STATE_BUFFER_READY, STATE_IDLE, STATE_RECORDING,
    STATE_TRANSFORMING,
};
use crate::transformation::{
    transform_text, TransformationPreviewMode, TransformationRuntimeConfig,
};
use transformation_history::TransformationHistory;

const DEFAULT_SAMPLE_RATE: u32 = 16000;

enum Command {
    SetSampleRate(u32),
    /// Starts the recording `generation`. Every command queued after it
    /// belongs to that recording, until the next start.
    StartSession {
        recording_prefix: String,
        generation: RecordingGeneration,
    },
    StartCorrectionSession,
    PushAudio(Bytes),
    StopSessionAndPaste,
    StopCorrectionSessionAndApply,
    StopSessionAndTransformAndPaste,
    StopSessionAndTransformAndResume,
    TransformBuffer,
    /// Throws the ready buffer away; its handler never pastes.
    DiscardBuffer,
    /// Ends the dictation session and throws its transcript away; its
    /// handler never pastes.
    AbortSession,
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
        let start = state.begin_recording();
        self.command_tx
            .send(Command::StartSession {
                recording_prefix: start.prefix,
                generation: start.generation,
            })
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

    pub fn insert_clipboard_text(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::QueueClipboardInsertion)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn discard_buffer(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::DiscardBuffer)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }

    pub fn abort_session(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::AbortSession)
            .map_err(|_| "transcription worker thread is not running".to_owned())
    }
}

pub fn spawn_transcription_thread(
    state: Arc<AppState>,
    config_store: LiveConfigStore,
) -> TranscriptionController {
    let (command_tx, command_rx) = channel();
    spawn_worker(state, config_store, command_rx);
    TranscriptionController { command_tx }
}

/// Runs the worker until every sender of `command_rx` is gone.
fn spawn_worker(
    state: Arc<AppState>,
    config_store: LiveConfigStore,
    command_rx: Receiver<Command>,
) -> thread::JoinHandle<()> {
    let worker_sample_rate = Arc::new(AtomicU32::new(DEFAULT_SAMPLE_RATE));
    let thread_worker_sample_rate = Arc::clone(&worker_sample_rate);

    thread::spawn(move || {
        let runtime = match Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                log::error!(
                    "failed to create Tokio runtime for transcription: {}",
                    error
                );
                state.report_error(error.to_string());
                return;
            }
        };

        let mut active_session: Option<ActiveSession> = None;
        let mut recording_prefix = String::new();
        let mut buffered_text = String::new();
        let mut resume_after_correction = false;
        let mut transformation_history = TransformationHistory::default();
        // The recording the commands being handled belong to.
        let mut generation = RecordingGeneration::FIRST;

        while let Ok(command) = command_rx.recv() {
            match command {
                Command::SetSampleRate(sample_rate) => {
                    thread_worker_sample_rate.store(sample_rate, Ordering::Relaxed);
                }
                Command::StartSession {
                    recording_prefix: requested_prefix,
                    generation: requested_generation,
                } => {
                    if let Some(old_session) = active_session.take() {
                        log::info!(
                            "cleaning up previous active session before starting new session"
                        );
                        // A cancellation of the earlier recording settles
                        // only that one; this start checks its own below.
                        if matches!(finish_session(old_session, &state, generation), Ok(None)) {
                            buffered_text.clear();
                            recording_prefix.clear();
                            resume_after_correction = false;
                        }
                    }

                    generation = requested_generation;
                    recording_prefix = requested_prefix;
                    transformation_history.begin_dictation(&recording_prefix);

                    let current_config = config_store.current();
                    let deepgram_config = match resolved_deepgram_config(&current_config) {
                        Ok(deepgram_config) => deepgram_config,
                        Err(error) => {
                            log::error!("failed to resolve Deepgram config: {}", error);
                            state.report_error(error);
                            continue;
                        }
                    };

                    let current_sample_rate = thread_worker_sample_rate.load(Ordering::Relaxed);
                    // Skipping the start uses the abort up; left set, it would
                    // skip every later start too.
                    if state.consume_abort_request(generation) {
                        log::info!(
                            "skipping dictation start because abort was requested before startup completed"
                        );
                        recording_prefix.clear();
                        if let Some(_recording) = state.current_recording(generation) {
                            state.clear_overlay_text();
                            state.set_overlay_text_opacity(1.0);
                            state.set_state(STATE_IDLE);
                        }
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
                            complete_session_cancellation(
                                &state,
                                generation,
                                &mut buffered_text,
                                SessionKind::Dictation,
                            );
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
                        match finish_session(session, &state, generation) {
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
                    if state.consume_abort_request(generation) {
                        log::info!(
                            "skipping correction start because abort was requested before startup completed"
                        );
                        if let Some(_recording) = state.current_recording(generation) {
                            state.clear_overlay_correction_text();
                            state.set_overlay_correction_active(false);
                            state.set_overlay_text_opacity(1.0);
                            state.set_state(STATE_BUFFER_READY);
                        }
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
                            complete_session_cancellation(
                                &state,
                                generation,
                                &mut buffered_text,
                                SessionKind::Correction,
                            );
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
                                let session = active_session
                                    .take()
                                    .expect("the closed queue belongs to the active session");
                                match finish_session(session, &state, generation) {
                                    Ok(None) => {
                                        buffered_text.clear();
                                        recording_prefix.clear();
                                        resume_after_correction = false;
                                    }
                                    Err(error) => state.report_error(error),
                                    Ok(Some(_)) => state
                                        .report_error("Deepgram session queue closed unexpectedly"),
                                }
                            }
                        }
                    }
                }
                Command::StopSessionAndPaste => {
                    if let Some(session) = active_session.take() {
                        match finish_session(session, &state, generation) {
                            Ok(None) => {
                                buffered_text.clear();
                                recording_prefix.clear();
                                resume_after_correction = false;
                                continue;
                            }
                            Ok(Some(text)) => {
                                buffered_text = text;
                                flush_buffered_text_or_paste(
                                    &state,
                                    generation,
                                    &mut buffered_text,
                                    true,
                                );
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
                            log::warn!(
                                "ignoring clipboard insertion because clipboard read failed: {}",
                                error
                            );
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
                            match finish_session(session, &state, generation) {
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
                    if !recording_prefix.is_empty()
                        && !recording_prefix.ends_with(|c: char| c.is_whitespace())
                    {
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
                                complete_session_cancellation(
                                    &state,
                                    generation,
                                    &mut buffered_text,
                                    SessionKind::Dictation,
                                );
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
                    let _resuming = ResumingDictation::new(&state, should_resume_after_correction);
                    if let Some(session) = active_session.take() {
                        let correction_request = match finish_session(session, &state, generation) {
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
                            generation,
                            &current_config,
                            &mut buffered_text,
                            &correction_request,
                            should_resume_after_correction,
                        ) {
                            Ok(Some(prefix)) => {
                                recording_prefix = prefix;
                                let current_sample_rate =
                                    thread_worker_sample_rate.load(Ordering::Relaxed);
                                let deepgram_config = match resolved_deepgram_config(
                                    &current_config,
                                ) {
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
                                        complete_session_cancellation(
                                            &state,
                                            generation,
                                            &mut buffered_text,
                                            SessionKind::Dictation,
                                        );
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
                        match finish_session(session, &state, generation) {
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

                                if state.consume_abort_request(generation) {
                                    log::info!(
                                        "discarding transformation because abort was requested"
                                    );
                                    buffered_text.clear();
                                    if let Some(_recording) = state.current_recording(generation) {
                                        state.clear_overlay_text();
                                        state.set_overlay_text_opacity(1.0);
                                        state.set_state(STATE_IDLE);
                                    }
                                    continue;
                                }

                                let current_config = config_store.current();
                                let transformation_config = match resolve_transformation_config(
                                    &current_config,
                                ) {
                                    Ok(config) => Some(config),
                                    Err(error) => {
                                        log::info!("transformation provider not active ({}); pasting raw transcript", error);
                                        None
                                    }
                                };

                                if let Some(transformation_config) = transformation_config {
                                    match runtime.block_on(transformation_history.finish_automatic(
                                        &state,
                                        generation,
                                        &buffered_text,
                                        async {
                                            state.set_state(STATE_TRANSFORMING);
                                            transform_text(
                                                state.clone(),
                                                &transformation_config,
                                                &buffered_text,
                                                TransformationPreviewMode::PreserveOverlay,
                                            )
                                            .await
                                        },
                                    )) {
                                        Ok(Some(transformed_text)) => {
                                            log::info!(
                                                "transcript ready for paste: chars={}",
                                                transformed_text.chars().count()
                                            );
                                            buffered_text = transformed_text;
                                            flush_buffered_text_or_paste(
                                                &state,
                                                generation,
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
                                        generation,
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
                        match finish_session(session, &state, generation) {
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

                                if state.consume_abort_request(generation) {
                                    log::info!(
                                        "discarding transformation because abort was requested"
                                    );
                                    buffered_text.clear();
                                    if let Some(_recording) = state.current_recording(generation) {
                                        state.clear_overlay_text();
                                        state.set_overlay_text_opacity(1.0);
                                        state.set_state(STATE_IDLE);
                                    }
                                    continue;
                                }

                                let current_config = config_store.current();
                                let transformation_config = match resolve_transformation_config(
                                    &current_config,
                                ) {
                                    Ok(config) => Some(config),
                                    Err(error) => {
                                        log::info!("transformation provider not active ({}); resuming with raw transcript", error);
                                        None
                                    }
                                };

                                if let Some(transformation_config) = transformation_config {
                                    state.set_state(STATE_TRANSFORMING);
                                    match runtime.block_on(transformation_history.finish_manual(
                                        &state,
                                        generation,
                                        transform_text(
                                            state.clone(),
                                            &transformation_config,
                                            &buffered_text,
                                            TransformationPreviewMode::ReplaceOverlay,
                                        ),
                                    )) {
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
                                        complete_session_cancellation(
                                            &state,
                                            generation,
                                            &mut buffered_text,
                                            SessionKind::Dictation,
                                        );
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
                    if !prepare_buffer_transform(&state, generation, &mut buffered_text) {
                        continue;
                    }

                    let current_config = config_store.current();
                    let transformation_config = match resolve_transformation_config(&current_config)
                    {
                        Ok(config) => config,
                        Err(error) => {
                            log::error!("failed to resolve transformation config: {}", error);
                            state.set_overlay_text(buffered_text.clone());
                            state.set_overlay_text_opacity(1.0);
                            state.report_error(error.to_string());
                            continue;
                        }
                    };

                    match runtime.block_on(transformation_history.finish_manual(
                        &state,
                        generation,
                        transform_text(
                            state.clone(),
                            &transformation_config,
                            &buffered_text,
                            TransformationPreviewMode::ReplaceOverlay,
                        ),
                    )) {
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
                            state.set_overlay_text(buffered_text.clone());
                            state.set_overlay_text_opacity(1.0);
                            state.report_error(error.to_string());
                        }
                    }
                }
                Command::DiscardBuffer => {
                    recording_prefix.clear();
                    resume_after_correction = false;
                    complete_session_cancellation(
                        &state,
                        generation,
                        &mut buffered_text,
                        SessionKind::Dictation,
                    );
                    log::info!("discarded the ready buffer");
                }
                Command::AbortSession => {
                    // Dropping the session aborts its task and releases its
                    // socket; its transcript is never read.
                    drop(active_session.take());
                    recording_prefix.clear();
                    resume_after_correction = false;
                    complete_session_cancellation(
                        &state,
                        generation,
                        &mut buffered_text,
                        SessionKind::Dictation,
                    );
                    log::info!("discarded the aborted dictation session");
                }
            }
        }
    })
}

/// Refresh from the editable annotation before deciding whether to transform.
fn prepare_buffer_transform(
    state: &AppState,
    generation: RecordingGeneration,
    buffered_text: &mut String,
) -> bool {
    if state.is_abort_requested() {
        complete_session_cancellation(state, generation, buffered_text, SessionKind::Dictation);
        return false;
    }

    // Ready-buffer keyboard edits live in AppState, independently of the
    // worker's last transcript or transformation result. Empty edits count too.
    *buffered_text = state.overlay_text().to_string();
    if buffered_text.trim().is_empty() {
        log::info!("ignoring transform_buffer command because buffer is empty");
        state.clear_overlay_text();
        state.set_overlay_text_opacity(1.0);
        state.set_state(STATE_IDLE);
        return false;
    }
    true
}

/// Apply spoken changes, if any, then prepare the original dictation to
/// resume. `None` leaves the annotation buffered without starting a session.
fn apply_correction_request(
    runtime: &Runtime,
    state: &Arc<AppState>,
    generation: RecordingGeneration,
    config: &Config,
    buffered_text: &mut String,
    correction_request: &str,
    resume_dictation: bool,
) -> Result<Option<String>, String> {
    state.set_overlay_correction_active(false);
    state.clear_overlay_correction_text();
    if state.consume_abort_request(generation) {
        if let Some(_recording) = state.current_recording(generation) {
            state.set_overlay_text(buffered_text.clone());
            state.set_overlay_text_opacity(1.0);
            state.set_state(STATE_BUFFER_READY);
        }
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
        let transformed = runtime.block_on(finish_transformation(
            state,
            generation,
            transform_text(
                state.clone(),
                &correction_config,
                &prompt_input,
                TransformationPreviewMode::ReplaceOverlay,
            ),
        ))?;
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
    generation: RecordingGeneration,
    transformation: impl std::future::Future<Output = Result<String, String>>,
) -> Result<Option<String>, String> {
    let result = tokio::select! {
        biased;
        _ = state.wait_for_abort() => Err(SessionError::Cancelled),
        result = transformation => result.map_err(SessionError::Failed),
    };
    settle_session_finish(result, state, generation)
}

/// Settle cancellation before any caller can paste, transform, or resume.
/// `None` means the recording was discarded, not an empty transcript.
fn finish_session(
    session: ActiveSession,
    state: &AppState,
    generation: RecordingGeneration,
) -> Result<Option<String>, String> {
    settle_session_finish(session.finish(state), state, generation)
}

/// `generation` is the recording the finished work belongs to. A recording
/// started since then keeps its state and overlay when this one is discarded.
fn settle_session_finish(
    result: Result<String, SessionError>,
    state: &AppState,
    generation: RecordingGeneration,
) -> Result<Option<String>, String> {
    // Also catch an abort that raced a normal completion or timeout.
    if state.consume_abort_request(generation) || matches!(result, Err(SessionError::Cancelled)) {
        if let Some(_recording) = state.current_recording(generation) {
            state.set_deepgram_waiting(false);
            state.set_dictation_resuming(false);
            state.set_overlay_correction_active(false);
            state.clear_overlay_correction_text();
            state.clear_overlay_text();
            state.set_overlay_text_opacity(1.0);
            state.set_state(STATE_IDLE);
        }
        log::info!("discarded aborted transcription session");
        return Ok(None);
    }
    match result {
        Ok(text) => Ok(Some(text)),
        Err(SessionError::Failed(error)) => Err(error),
        Err(SessionError::Cancelled) => unreachable!("aborted sessions are settled above"),
    }
}

/// Settles the cancelled work of the recording `generation`. Only the worker's
/// own text is cleared when a recording started since then owns the state and
/// overlay.
fn complete_session_cancellation(
    state: &AppState,
    generation: RecordingGeneration,
    buffered_text: &mut String,
    kind: SessionKind,
) {
    state.consume_abort_request(generation);
    if kind == SessionKind::Dictation {
        buffered_text.clear();
    }
    let Some(_recording) = state.current_recording(generation) else {
        log::info!("a newer recording started; leaving its state and overlay");
        return;
    };
    state.set_dictation_resuming(false);
    state.set_deepgram_waiting(false);
    state.clear_overlay_error_text();
    state.clear_overlay_correction_text();
    state.set_overlay_correction_active(false);
    state.set_overlay_text_opacity(1.0);
    match kind {
        SessionKind::Dictation => {
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
    let api_key = config
        .resolve_deepgram_api_key()
        .map_err(|error| format!("{error} You can also add it in Settings > Deepgram."))?;
    let mut deepgram = config.deepgram.clone();
    deepgram.api_key = Some(api_key);
    Ok(deepgram)
}

fn resolve_transformation_config(config: &Config) -> Result<TransformationRuntimeConfig, String> {
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
    fn ready_buffer_transform_worker_stops_after_the_user_deletes_a_cached_annotation() {
        use crate::state::{AppState, STATE_ERROR, STATE_IDLE, STATE_TRANSFORMING};

        let state = AppState::new();
        let config = crate::config::Config::default();
        let store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            ".tmp/ready-buffer-edits/config.toml".into(),
        );
        let controller = super::spawn_transcription_thread(state.clone(), store);
        state.set_overlay_text("This text enters the worker cache.");
        state.set_state(STATE_TRANSFORMING);
        controller.transform_buffer().unwrap();
        // An unconfigured provider fails before making a network request,
        // leaving a nonempty worker cache for the second command.
        wait_for_state(&state, STATE_ERROR);
        assert_eq!(
            &*state.overlay_text(),
            "This text enters the worker cache.",
            "a failed transform must leave the editable input available for retry"
        );

        state.set_overlay_text("The annotation before the keyboard deletion.");
        let rendered = state.overlay_text();
        state.apply_overlay_edit(&rendered, "");
        state.set_state(STATE_TRANSFORMING);
        controller.transform_buffer().unwrap();
        wait_for_state(&state, STATE_IDLE);
        assert!(state.overlay_text().is_empty());
    }

    #[test]
    fn ready_buffer_transform_uses_keyboard_edits_instead_of_cached_text() {
        let state = crate::state::AppState::new();
        for edited in [
            "Meet Friday instead.",
            "Edited after the previous transform.",
            "Café à Montréal.",
        ] {
            let mut buffered_text = "Meet Thursday.".to_owned();
            state.set_state(crate::state::STATE_BUFFER_READY);
            state.set_overlay_text(buffered_text.clone());
            state.apply_overlay_edit(&buffered_text, edited);
            state.set_state(crate::state::STATE_TRANSFORMING);

            assert!(super::prepare_buffer_transform(
                &state,
                state.recording_generation(),
                &mut buffered_text
            ));
            assert_eq!(
                buffered_text, edited,
                "F6 must transform the editable annotation"
            );
        }
    }

    #[test]
    fn ready_buffer_transform_does_not_restore_text_deleted_in_the_overlay() {
        let state = crate::state::AppState::new();
        for edited in ["", " \n\t "] {
            let mut buffered_text = "Delete this entire annotation.".to_owned();
            state.set_overlay_text(buffered_text.clone());
            state.apply_overlay_edit(&buffered_text, edited);
            state.set_state(crate::state::STATE_TRANSFORMING);

            assert!(!super::prepare_buffer_transform(
                &state,
                state.recording_generation(),
                &mut buffered_text
            ));
            assert!(buffered_text.trim().is_empty());
            assert_eq!(state.get_state(), crate::state::STATE_IDLE);
            assert!(state.overlay_text().is_empty());
        }
    }

    #[test]
    fn ready_buffer_transform_settles_cancellation_before_reading_a_deleted_annotation() {
        let state = crate::state::AppState::new();
        let mut buffered_text = "The worker still holds the discarded annotation.".to_owned();
        state.set_overlay_text(buffered_text.clone());
        state.request_abort();
        state.dismiss_overlay();
        state.clear_overlay_text();

        assert!(!super::prepare_buffer_transform(
            &state,
            state.recording_generation(),
            &mut buffered_text
        ));
        assert!(buffered_text.is_empty());
        assert!(!state.is_abort_requested());
        assert!(state.is_overlay_dismissed());
        assert_eq!(state.get_state(), crate::state::STATE_IDLE);
    }

    #[test]
    fn manual_transform_is_not_repeated_when_pasting_unchanged_text() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = crate::state::AppState::new();
        let requests = std::cell::Cell::new(0);
        let mut history = super::TransformationHistory::default();

        let transformed = runtime
            .block_on(
                history.finish_manual(&state, state.recording_generation(), async {
                    requests.set(requests.get() + 1);
                    Ok("The cleaned transcript.".to_owned())
                }),
            )
            .unwrap()
            .unwrap();
        // The resumed session adds its normal separator, with no new speech.
        let final_transcript = super::join_transcript_parts(&transformed, &[]);
        state.set_state(crate::state::STATE_PROCESSING);
        let pasted = runtime
            .block_on(history.finish_automatic(
                &state,
                state.recording_generation(),
                &final_transcript,
                async {
                    state.set_state(crate::state::STATE_TRANSFORMING);
                    requests.set(requests.get() + 1);
                    Ok(final_transcript.clone())
                },
            ))
            .unwrap()
            .unwrap();

        assert_eq!(pasted.trim(), transformed);
        assert_eq!(
            requests.get(),
            1,
            "F5 must reuse the unchanged manual result"
        );
        assert_eq!(state.get_state(), crate::state::STATE_PROCESSING);
    }

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
        let result = runtime
            .block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    super::finish_transformation(&state, state.recording_generation(), request),
                )
                .await
            })
            .expect("cancellation must not wait for another LLM response");
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
        for result in [
            Ok("Discard this rewrite".to_owned()),
            Err("A late error".to_owned()),
        ] {
            let state = crate::state::AppState::new();
            state.set_state(crate::state::STATE_TRANSFORMING);
            state.dismiss_overlay();
            assert_eq!(
                runtime
                    .block_on(super::finish_transformation(
                        &state,
                        state.recording_generation(),
                        async {
                            state.request_abort();
                            result
                        }
                    ))
                    .unwrap(),
                None
            );
            assert!(state.is_overlay_dismissed());
            assert!(state.overlay_error_text().is_empty());
            assert_eq!(state.get_state(), crate::state::STATE_IDLE);
            // The abort is consumed, so subsequent work can finish normally.
            assert_eq!(
                runtime
                    .block_on(super::finish_transformation(
                        &state,
                        state.recording_generation(),
                        async { Ok("The next rewrite".to_owned()) }
                    ))
                    .unwrap(),
                Some("The next rewrite".to_owned())
            );
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

        let super::Command::StartSession {
            recording_prefix, ..
        } = command_rx.recv().unwrap()
        else {
            panic!("expected a dictation start");
        };
        assert_eq!(
            recording_prefix,
            "The annotation, including keyboard edits."
        );
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
                &runtime,
                &state,
                state.recording_generation(),
                &crate::config::Config::default(),
                &mut annotation,
                request,
                true,
            )
            .unwrap();

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

        assert_eq!(
            super::apply_correction_request(
                &runtime,
                &state,
                state.recording_generation(),
                &crate::config::Config::default(),
                &mut annotation,
                "",
                false,
            )
            .unwrap(),
            None
        );

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
            &runtime,
            &state,
            state.recording_generation(),
            &crate::config::Config::default(),
            &mut annotation,
            "",
            true,
        )
        .unwrap();

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
        super::complete_session_cancellation(
            &state,
            state.recording_generation(),
            &mut buffered_text,
            super::SessionKind::Dictation,
        );
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
        super::complete_session_cancellation(
            &state,
            state.recording_generation(),
            &mut buffered_text,
            super::SessionKind::Correction,
        );
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

        let result = super::settle_session_finish(
            Err(super::SessionError::Cancelled),
            &state,
            state.recording_generation(),
        );

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

        assert_eq!(
            super::settle_session_finish(Ok(String::new()), &state, state.recording_generation()),
            Ok(None)
        );
        assert!(!state.is_abort_requested());
        assert_eq!(state.get_state(), crate::state::STATE_IDLE);
    }

    #[test]
    fn session_finish_errors_remain_errors_instead_of_pasting_partial_text() {
        let state = crate::state::AppState::new();
        state.set_overlay_text("incomplete words");
        let result = super::settle_session_finish(
            Err(super::SessionError::Failed("shutdown timed out".to_owned())),
            &state,
            state.recording_generation(),
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
        super::spawn_transcription_thread(state.clone(), config_store(config_with_deepgram_key()))
    }

    /// Builds a worker from an explicit config; tests do not connect to Deepgram.
    fn worker_with_config(
        state: &std::sync::Arc<crate::state::AppState>,
        config: crate::config::Config,
    ) -> super::TranscriptionController {
        super::spawn_transcription_thread(state.clone(), config_store(config))
    }

    fn config_with_deepgram_key() -> crate::config::Config {
        let mut config = crate::config::Config::default();
        config.deepgram.api_key = Some("test-key".to_owned());
        config
    }

    fn config_store(config: crate::config::Config) -> crate::settings::LiveConfigStore {
        crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from("/tmp/config.toml"),
        )
    }

    /// A controller whose commands wait in the returned queue, so a test
    /// decides what the hotkey does before the worker handles any of them.
    fn queued_controller() -> (
        super::TranscriptionController,
        std::sync::mpsc::Receiver<super::Command>,
    ) {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        (super::TranscriptionController { command_tx }, command_rx)
    }

    /// Hands `commands` to a worker, in order, and returns once it has
    /// handled every one of them.
    fn run_worker(state: &std::sync::Arc<crate::state::AppState>, commands: Vec<super::Command>) {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        for command in commands {
            command_tx.send(command).unwrap();
        }
        drop(command_tx);
        super::spawn_worker(
            state.clone(),
            config_store(config_with_deepgram_key()),
            command_rx,
        )
        .join()
        .unwrap();
    }

    /// The queued commands, split before the last dictation start: the
    /// commands queued earlier, and the start's prefix and generation.
    fn split_before_last_start(
        queue: &std::sync::mpsc::Receiver<super::Command>,
    ) -> (
        Vec<super::Command>,
        String,
        crate::state::RecordingGeneration,
    ) {
        let mut commands: Vec<_> = queue.try_iter().collect();
        let Some(super::Command::StartSession {
            recording_prefix,
            generation,
        }) = commands.pop()
        else {
            panic!("expected a dictation start last");
        };
        (commands, recording_prefix, generation)
    }

    /// What the hotkey does for Escape during a dictation.
    fn escape_dictation(
        state: &crate::state::AppState,
        controller: &super::TranscriptionController,
    ) {
        state.request_abort();
        state.dismiss_overlay();
        state.clear_overlay_text();
        controller.abort_session().unwrap();
        state.set_state(crate::state::STATE_IDLE);
    }

    /// What the record hotkey does to start a dictation, with words then
    /// typed into its overlay.
    fn start_dictation_with_typed_words(
        state: &crate::state::AppState,
        controller: &super::TranscriptionController,
    ) {
        state.restore_overlay();
        state.clear_overlay_error_text();
        if state.get_state() != crate::state::STATE_BUFFER_READY {
            state.clear_overlay_text();
        }
        controller.start_session(state).unwrap();
        state.set_overlay_text("Words typed into the new recording.");
    }

    fn assert_new_recording_intact(state: &crate::state::AppState) {
        assert_eq!(state.get_state(), crate::state::STATE_RECORDING);
        assert_eq!(
            &*state.overlay_text(),
            "Words typed into the new recording."
        );
        assert!(!state.is_overlay_dismissed());
        assert!(state.is_capturing_audio());
        // Nothing is left to skip the new recording's start.
        assert!(!state.is_abort_requested());
    }

    #[test]
    fn an_aborted_dictation_settled_after_the_next_start_leaves_that_recording_alone() {
        let state = crate::state::AppState::new();
        let (controller, queue) = queued_controller();
        controller.start_session(&state).unwrap();
        escape_dictation(&state, &controller);
        start_dictation_with_typed_words(&state, &controller);
        let new_recording = state.recording_generation();

        // The first start, skipped for its abort, and the abort itself.
        let (earlier, prefix, generation) = split_before_last_start(&queue);
        assert_eq!(earlier.len(), 2);
        run_worker(&state, earlier);

        assert_new_recording_intact(&state);
        assert_eq!(generation, new_recording);
        assert!(prefix.is_empty());
    }

    #[test]
    fn a_discarded_buffer_settled_after_the_next_start_leaves_that_recording_alone() {
        use crate::state::{AppState, STATE_BUFFER_READY};

        let state = AppState::new();
        state.set_overlay_text("A buffer to discard.");
        state.set_state(STATE_BUFFER_READY);
        let (controller, queue) = queued_controller();
        // Escape on the ready buffer leaves its state to the worker.
        state.request_abort();
        state.dismiss_overlay();
        state.clear_overlay_text();
        controller.discard_buffer().unwrap();
        assert_eq!(state.get_state(), STATE_BUFFER_READY);
        start_dictation_with_typed_words(&state, &controller);

        let (earlier, prefix, generation) = split_before_last_start(&queue);
        run_worker(&state, earlier);

        assert_new_recording_intact(&state);
        assert_eq!(generation, state.recording_generation());
        assert!(prefix.is_empty(), "the discarded buffer must not resume");
    }

    #[test]
    fn an_abort_nothing_settled_does_not_skip_the_next_recording_start() {
        if run_in_child(
            "an_abort_nothing_settled_does_not_skip_the_next_recording_start",
            |child| {
                child.env("SIMPLE_PTT_SIMULATE_ERROR", "connection reached");
            },
        ) {
            return;
        }
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_ERROR, STATE_TRANSFORMING};

        // Escape during a transformation lands after the worker's last abort
        // check, so the worker publishes the result and leaves the abort set.
        let state = AppState::new();
        state.set_state(STATE_TRANSFORMING);
        state.request_abort();
        state.dismiss_overlay();
        state.clear_overlay_text();
        state.set_overlay_text("The transformed text.");
        state.set_state(STATE_BUFFER_READY);
        let (controller, queue) = queued_controller();
        start_dictation_with_typed_words(&state, &controller);

        let commands: Vec<_> = queue.try_iter().collect();
        assert_eq!(commands.len(), 1);
        run_worker(&state, commands);

        // The start went on to connect, here to a simulated failure, rather
        // than being skipped for the earlier recording's abort.
        assert_eq!(state.get_state(), STATE_ERROR);
        let error = state.overlay_error_text();
        assert!(
            error.contains("simulated error: connection reached"),
            "{error}"
        );
        assert!(!state.is_abort_requested());
    }

    #[test]
    fn a_cancellation_settled_after_a_newer_recording_started_clears_only_the_worker_text() {
        use crate::state::{AppState, STATE_BUFFER_READY};

        for kind in [
            super::SessionKind::Dictation,
            super::SessionKind::Correction,
        ] {
            let state = AppState::new();
            state.set_overlay_text("The cancelled recording's annotation.");
            state.set_state(STATE_BUFFER_READY);
            let cancelled = state.recording_generation();
            state.request_abort();
            state.dismiss_overlay();
            state.clear_overlay_text();
            let (controller, _queue) = queued_controller();
            start_dictation_with_typed_words(&state, &controller);

            let mut buffered_text = "The cancelled recording's annotation.".to_owned();
            super::complete_session_cancellation(&state, cancelled, &mut buffered_text, kind);

            assert_new_recording_intact(&state);
            assert_eq!(
                buffered_text.is_empty(),
                kind == super::SessionKind::Dictation
            );
        }
    }

    #[test]
    fn a_session_finish_discarded_after_a_newer_recording_started_leaves_that_recording_alone() {
        use crate::state::AppState;

        for result in [
            Err(super::SessionError::Cancelled),
            Ok("Late words".to_owned()),
        ] {
            let state = AppState::new();
            let cancelled = state.recording_generation();
            state.request_abort();
            let (controller, _queue) = queued_controller();
            start_dictation_with_typed_words(&state, &controller);

            assert_eq!(
                super::settle_session_finish(result, &state, cancelled),
                Ok(None)
            );
            assert_new_recording_intact(&state);
        }
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
    fn discarding_the_buffer_needs_no_abort_and_leaves_nothing_to_paste() {
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_IDLE};

        let state = AppState::new();
        state.set_state(STATE_BUFFER_READY);
        state.set_overlay_text("Throw this away");

        worker(&state).discard_buffer().unwrap();

        wait_for_state(&state, STATE_IDLE);
        assert!(state.overlay_text().is_empty());
        assert!(!state.is_abort_requested());
        assert_eq!(state.paste_attempts(), 0);
    }

    #[test]
    fn aborting_a_session_needs_no_abort_flag_and_consumes_a_raced_one() {
        use crate::state::{AppState, STATE_IDLE, STATE_RECORDING};

        for abort_requested in [false, true] {
            let state = AppState::new();
            state.set_state(STATE_RECORDING);
            state.set_overlay_text("Dictated words to drop");
            if abort_requested {
                state.request_abort();
            }

            worker(&state).abort_session().unwrap();

            wait_for_state(&state, STATE_IDLE);
            assert!(state.overlay_text().is_empty(), "{abort_requested}");
            assert!(!state.is_abort_requested(), "{abort_requested}");
            assert_eq!(state.paste_attempts(), 0, "{abort_requested}");
        }
    }

    #[test]
    fn a_dictation_start_skipped_for_an_abort_uses_the_abort_up() {
        use crate::state::{AppState, STATE_IDLE};

        // The record key was pressed, then Escape before the session started.
        let state = AppState::new();
        let (controller, queue) = queued_controller();
        controller.start_session(&state).unwrap();
        state.request_abort();
        run_worker(&state, queue.try_iter().collect());

        assert_eq!(state.get_state(), STATE_IDLE);
        assert!(state.overlay_text().is_empty());
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

    #[test]
    fn dictation_without_deepgram_key_reports_guidance_and_keeps_buffer() {
        if run_without_deepgram_key(
            "dictation_without_deepgram_key_reports_guidance_and_keeps_buffer",
        ) {
            return;
        }
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_ERROR};

        let state = AppState::new();
        state.set_overlay_text("draft to keep");
        state.set_state(STATE_BUFFER_READY);
        worker_with_config(&state, crate::config::Config::default())
            .start_session(&state)
            .unwrap();

        wait_for_state(&state, STATE_ERROR);
        assert_eq!(&*state.overlay_text(), "draft to keep");
        let error = state.overlay_error_text();
        assert!(error.contains("Settings > Deepgram"), "{error}");
        assert!(error.contains("deepgram.api_key"), "{error}");
        assert!(error.contains("DEEPGRAM_API_KEY"), "{error}");
    }

    #[test]
    fn correction_without_deepgram_key_reports_guidance_and_keeps_buffer() {
        if run_without_deepgram_key(
            "correction_without_deepgram_key_reports_guidance_and_keeps_buffer",
        ) {
            return;
        }
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_ERROR};

        let state = AppState::new();
        state.set_overlay_text("draft to keep");
        state.set_state(STATE_BUFFER_READY);
        worker_with_config(&state, crate::config::Config::default())
            .start_correction_session()
            .unwrap();

        wait_for_state(&state, STATE_ERROR);
        assert_eq!(&*state.overlay_text(), "draft to keep");
        let error = state.overlay_error_text();
        assert!(error.contains("Settings > Deepgram"), "{error}");
        assert!(error.contains("deepgram.api_key"), "{error}");
        assert!(error.contains("DEEPGRAM_API_KEY"), "{error}");
    }

    #[test]
    fn idle_dictation_without_deepgram_key_reports_guidance() {
        if run_without_deepgram_key("idle_dictation_without_deepgram_key_reports_guidance") {
            return;
        }
        let state = crate::state::AppState::new();
        worker_with_config(&state, crate::config::Config::default())
            .start_session(&state)
            .unwrap();
        wait_for_state(&state, crate::state::STATE_ERROR);
        assert!(state.overlay_text().is_empty());
        let error = state.overlay_error_text();
        assert!(error.contains("Settings > Deepgram"), "{error}");
        assert!(error.contains("deepgram.api_key"), "{error}");
    }

    #[test]
    fn resolved_deepgram_config_uses_the_resolved_key() {
        let mut config = crate::config::Config::default();
        config.deepgram.api_key = Some("  configured-key  ".to_owned());
        let resolved = super::resolved_deepgram_config(&config).unwrap();
        assert_eq!(resolved.api_key.as_deref(), Some("configured-key"));
        assert_eq!(resolved.model, config.deepgram.model);
    }

    // Run only the requested test in a child with no real credentials. Other
    // configuration tests may mutate the parent process's environment.
    fn run_without_deepgram_key(test_name: &str) -> bool {
        run_in_child(test_name, |child| {
            child.env_remove("DEEPGRAM_API_KEY");
        })
    }

    /// Runs only `test_name` in a child process whose environment `configure`
    /// changes, and returns whether this is the parent, which is then done.
    fn run_in_child(test_name: &str, configure: impl FnOnce(&mut std::process::Command)) -> bool {
        if std::env::var("SIMPLE_PTT_CHILD_TEST").as_deref() == Ok(test_name) {
            return false;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                &format!("transcription::tests::{test_name}"),
                "--nocapture",
            ])
            .env("SIMPLE_PTT_CHILD_TEST", test_name);
        configure(&mut child);
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }
}
