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
    AppState, CurrentRecording, RecordingGeneration, STATE_BUFFER_READY, STATE_IDLE,
    STATE_RECORDING, STATE_TRANSFORMING,
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
        /// The recording continues the previous one's text
        /// (`RecordingStart::continues`).
        continues: bool,
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

/// Why the record hotkey did not start a recording.
#[derive(Debug, Eq, PartialEq)]
pub enum StartSessionError {
    /// The previous recording's text is being pasted. A failed paste returns
    /// it to the overlay, so a recording waits for the paste's outcome.
    PasteRunning,
    WorkerStopped,
}

impl std::fmt::Display for StartSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PasteRunning => "the previous recording is still being pasted",
            Self::WorkerStopped => "transcription worker thread is not running",
        })
    }
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

    pub fn start_session(&self, state: &AppState) -> Result<(), StartSessionError> {
        let start = state
            .begin_recording()
            .ok_or(StartSessionError::PasteRunning)?;
        self.command_tx
            .send(Command::StartSession {
                recording_prefix: start.prefix,
                generation: start.generation,
                continues: start.continues,
            })
            .map_err(|_| StartSessionError::WorkerStopped)
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
        // Text of a recording that a newer one replaced before the worker
        // could show it; the newer recording's start continues from it.
        let mut handed_over = String::new();

        while let Ok(command) = command_rx.recv() {
            match command {
                Command::SetSampleRate(sample_rate) => {
                    thread_worker_sample_rate.store(sample_rate, Ordering::Relaxed);
                }
                Command::StartSession {
                    recording_prefix: requested_prefix,
                    generation: requested_generation,
                    continues,
                } => {
                    if let Some(old_session) = active_session.take() {
                        log::info!(
                            "cleaning up previous active session before starting new session"
                        );
                        // An error interrupted the earlier recording with its
                        // session still running. A cancellation settles only
                        // that recording; this start checks its own below.
                        match finish_session(old_session, &state, generation) {
                            Ok(None) => {
                                buffered_text.clear();
                                recording_prefix.clear();
                                resume_after_correction = false;
                            }
                            // Its final transcript, with the words that
                            // arrived since the error, is what this start
                            // continues.
                            Ok(Some(text)) => hand_over(&mut handed_over, &text),
                            Err(error) => {
                                log::warn!("the interrupted session failed to finish: {}", error)
                            }
                        }
                    }

                    generation = requested_generation;
                    recording_prefix =
                        starting_prefix(&mut handed_over, requested_prefix, continues);
                    transformation_history.begin_dictation(&recording_prefix);

                    // Skipping the start uses the abort up; left set, it would
                    // skip every later start too. Checked before anything can
                    // fail, so a cancelled start reports no error.
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

                    let current_config = config_store.current();
                    let deepgram_config = match resolved_deepgram_config(&current_config) {
                        Ok(deepgram_config) => deepgram_config,
                        Err(error) => {
                            log::error!("failed to resolve Deepgram config: {}", error);
                            publish_or_hand_over(
                                &state,
                                generation,
                                &mut handed_over,
                                &recording_prefix,
                                |_| state.report_error_with_text(recording_prefix.clone(), error),
                            );
                            continue;
                        }
                    };
                    let current_sample_rate = thread_worker_sample_rate.load(Ordering::Relaxed);

                    // The hotkey owns this presentation's visibility. A
                    // queued start must not undo a later empty F5 dismissal.
                    if publish_or_hand_over(
                        &state,
                        generation,
                        &mut handed_over,
                        &recording_prefix,
                        |_| {
                            state.set_overlay_text(recording_prefix.clone());
                            state.set_overlay_text_opacity(1.0);
                        },
                    ) != Publication::Published
                    {
                        recording_prefix.clear();
                        continue;
                    }

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
                            publish_or_hand_over(
                                &state,
                                generation,
                                &mut handed_over,
                                &recording_prefix,
                                |_| {
                                    state.report_error_with_text(
                                        recording_prefix.clone(),
                                        error.to_string(),
                                    )
                                },
                            );
                            recording_prefix.clear();
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
                            state.report_error_with_text(buffered_text.clone(), error.to_string());
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
                            state.report_error_with_text(buffered_text.clone(), error.to_string());
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
                                paste_buffered_text(
                                    &state,
                                    generation,
                                    &mut buffered_text,
                                    &mut handed_over,
                                );
                            }
                            Err(error) => {
                                recording_prefix.clear();
                                publish_session_failure(&state, generation, &error);
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
                                    // The overlay keeps what was dictated.
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
                                let publication = publish_or_hand_over(
                                    &state,
                                    generation,
                                    &mut handed_over,
                                    &buffered_text,
                                    |_| {
                                        state.set_overlay_correction_active(false);
                                        state.clear_overlay_correction_text();
                                        state.report_error_with_text(
                                            buffered_text.clone(),
                                            error.to_string(),
                                        );
                                    },
                                );
                                if publication != Publication::Published {
                                    buffered_text.clear();
                                }
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
                            Ok(CorrectionOutcome::Resume(prefix)) => {
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
                                        if publish_or_hand_over(
                                            &state,
                                            generation,
                                            &mut handed_over,
                                            &recording_prefix,
                                            |_| {
                                                state.report_error_with_text(
                                                    recording_prefix.clone(),
                                                    error,
                                                )
                                            },
                                        ) != Publication::Published
                                        {
                                            buffered_text.clear();
                                            recording_prefix.clear();
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
                                        active_session = publish_resumed_dictation(
                                            &state,
                                            generation,
                                            session,
                                            &mut recording_prefix,
                                            &mut handed_over,
                                        );
                                        if active_session.is_none() {
                                            buffered_text.clear();
                                        }
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
                                        if publish_or_hand_over(
                                            &state,
                                            generation,
                                            &mut handed_over,
                                            &recording_prefix,
                                            |_| {
                                                state.report_error_with_text(
                                                    recording_prefix.clone(),
                                                    error.to_string(),
                                                )
                                            },
                                        ) != Publication::Published
                                        {
                                            buffered_text.clear();
                                            recording_prefix.clear();
                                        }
                                    }
                                }
                            }
                            Ok(CorrectionOutcome::Settled) => {}
                            Ok(CorrectionOutcome::Superseded) => {
                                hand_over(&mut handed_over, &buffered_text);
                                buffered_text.clear();
                            }
                            Err(error) => {
                                log::error!("correction transformation failed: {}", error);
                                publish_error_with_text(
                                    &state,
                                    generation,
                                    &mut handed_over,
                                    &mut buffered_text,
                                    &error,
                                );
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
                                    publish_outcome(&state, generation, |_| {
                                        state.clear_overlay_text();
                                        state.set_overlay_text_opacity(1.0);
                                        state.set_state(STATE_IDLE);
                                    });
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
                                            paste_buffered_text(
                                                &state,
                                                generation,
                                                &mut buffered_text,
                                                &mut handed_over,
                                            );
                                        }
                                        Ok(None) => {
                                            buffered_text.clear();
                                            recording_prefix.clear();
                                            continue;
                                        }
                                        Err(error) => {
                                            log::error!("transformation failed: {}", error);
                                            publish_error_with_text(
                                                &state,
                                                generation,
                                                &mut handed_over,
                                                &mut buffered_text,
                                                &error,
                                            );
                                        }
                                    }
                                } else {
                                    paste_buffered_text(
                                        &state,
                                        generation,
                                        &mut buffered_text,
                                        &mut handed_over,
                                    );
                                }
                            }
                            Err(error) => {
                                recording_prefix.clear();
                                publish_session_failure(&state, generation, &error);
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
                                    publish_outcome(&state, generation, |_| {
                                        state.clear_overlay_text();
                                        state.set_overlay_text_opacity(1.0);
                                        state.set_state(STATE_IDLE);
                                    });
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
                                            publish_error_with_text(
                                                &state,
                                                generation,
                                                &mut handed_over,
                                                &mut buffered_text,
                                                &error,
                                            );
                                            continue;
                                        }
                                    }
                                }

                                recording_prefix = resume_prefix(&buffered_text);
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
                                            if publish_or_hand_over(
                                                &state,
                                                generation,
                                                &mut handed_over,
                                                &recording_prefix,
                                                |_| {
                                                    state.report_error_with_text(
                                                        recording_prefix.clone(),
                                                        error,
                                                    )
                                                },
                                            ) != Publication::Published
                                            {
                                                buffered_text.clear();
                                                recording_prefix.clear();
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
                                        active_session = publish_resumed_dictation(
                                            &state,
                                            generation,
                                            session,
                                            &mut recording_prefix,
                                            &mut handed_over,
                                        );
                                        if active_session.is_none() {
                                            buffered_text.clear();
                                        }
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
                                        if publish_or_hand_over(
                                            &state,
                                            generation,
                                            &mut handed_over,
                                            &recording_prefix,
                                            |_| {
                                                state.report_error_with_text(
                                                    recording_prefix.clone(),
                                                    error.to_string(),
                                                )
                                            },
                                        ) != Publication::Published
                                        {
                                            buffered_text.clear();
                                            recording_prefix.clear();
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                recording_prefix.clear();
                                publish_session_failure(&state, generation, &error);
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
                            publish_error_with_text(
                                &state,
                                generation,
                                &mut handed_over,
                                &mut buffered_text,
                                &error,
                            );
                            continue;
                        }
                    };

                    let result = runtime.block_on(transformation_history.finish_manual(
                        &state,
                        generation,
                        transform_text(
                            state.clone(),
                            &transformation_config,
                            &buffered_text,
                            TransformationPreviewMode::ReplaceOverlay,
                        ),
                    ));
                    if publish_buffer_transformation(
                        &state,
                        generation,
                        result,
                        &mut buffered_text,
                        &mut handed_over,
                    ) == Publication::Cancelled
                    {
                        recording_prefix.clear();
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

/// What applying a correction leaves the worker to do.
#[derive(Debug, Eq, PartialEq)]
enum CorrectionOutcome {
    /// Resume dictation from this prefix.
    Resume(String),
    /// Nothing: the annotation is a ready buffer, or it was discarded.
    Settled,
    /// A newer recording replaced this one before the corrected annotation,
    /// still in the worker's buffer, could be shown.
    Superseded,
}

/// Apply spoken changes, if any, then prepare the original dictation to
/// resume, or leave the annotation buffered without starting a session.
fn apply_correction_request(
    runtime: &Runtime,
    state: &Arc<AppState>,
    generation: RecordingGeneration,
    config: &Config,
    buffered_text: &mut String,
    correction_request: &str,
    resume_dictation: bool,
) -> Result<CorrectionOutcome, String> {
    state.set_overlay_correction_active(false);
    state.clear_overlay_correction_text();
    if state.consume_abort_request(generation) {
        if let Some(_recording) = state.current_recording(generation) {
            state.set_overlay_text(buffered_text.clone());
            state.set_overlay_text_opacity(1.0);
            state.set_state(STATE_BUFFER_READY);
        }
        return Ok(CorrectionOutcome::Settled);
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
            return Ok(CorrectionOutcome::Settled);
        };
        *buffered_text = transformed;
    }

    if resume_dictation {
        let prefix = resume_prefix(buffered_text);
        state.set_overlay_text(prefix.clone());
        state.set_overlay_text_opacity(1.0);
        return Ok(CorrectionOutcome::Resume(prefix));
    }
    // An abort requested since the transformation settled cancels it here.
    match publish_outcome(state, generation, |_| {
        state.set_overlay_text(buffered_text.clone());
        state.set_overlay_text_opacity(1.0);
        state.set_state(STATE_BUFFER_READY);
    }) {
        Publication::Published => Ok(CorrectionOutcome::Settled),
        Publication::Cancelled => {
            buffered_text.clear();
            Ok(CorrectionOutcome::Settled)
        }
        Publication::Superseded => Ok(CorrectionOutcome::Superseded),
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
        if let Some(recording) = state.current_recording(generation) {
            discard_presentation(state, &recording);
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

/// Clears what a discarded recording showed and returns to idle, while
/// holding that recording.
fn discard_presentation(state: &AppState, _recording: &CurrentRecording<'_>) {
    state.set_deepgram_waiting(false);
    state.set_dictation_resuming(false);
    state.set_overlay_correction_active(false);
    state.clear_overlay_correction_text();
    state.clear_overlay_text();
    state.set_overlay_text_opacity(1.0);
    state.set_state(STATE_IDLE);
}

/// Reports a Deepgram session that failed while finishing. The overlay keeps
/// everything dictated so far, keyboard edits and the recording's prefix
/// included, for the next recording to continue.
fn publish_session_failure(state: &AppState, generation: RecordingGeneration, error: &str) {
    log::error!("Deepgram session failed: {}", error);
    publish_outcome(state, generation, |_| {
        state.set_overlay_text_opacity(1.0);
        state.report_error(error);
    });
}

/// What became of the outcome of a recording's work.
#[derive(Debug, Eq, PartialEq)]
enum Publication {
    /// The outcome is in the state and overlay.
    Published,
    /// An abort cancelled the work first, and the recording is discarded.
    Cancelled,
    /// A newer recording owns the state and overlay; nothing was written.
    Superseded,
}

/// Publishes the outcome of the recording `generation`'s work with
/// `publish`, unless an abort cancelled the work first. It holds the
/// recording to check the abort and write the outcome, as the cancel
/// shortcuts hold it to read the state and request the abort
/// (`AppState::cancel_work_in_progress`). An abort requested before this
/// cancels the work here; one requested after it finds the published state,
/// which the hotkey cancels with a command. Either way none stays pending.
fn publish_outcome(
    state: &AppState,
    generation: RecordingGeneration,
    publish: impl FnOnce(&mut CurrentRecording<'_>),
) -> Publication {
    let recording = state.current_recording(generation);
    if state.consume_abort_request(generation) {
        if let Some(recording) = &recording {
            discard_presentation(state, recording);
        }
        log::info!("discarded the outcome of cancelled work");
        return Publication::Cancelled;
    }
    match recording {
        Some(mut recording) => {
            publish(&mut recording);
            Publication::Published
        }
        None => Publication::Superseded,
    }
}

/// Publishes as `publish_outcome` does. When a newer recording replaced this
/// one, `text`, which the outcome would have shown, goes to that recording.
fn publish_or_hand_over(
    state: &AppState,
    generation: RecordingGeneration,
    handed_over: &mut String,
    text: &str,
    publish: impl FnOnce(&mut CurrentRecording<'_>),
) -> Publication {
    let publication = publish_outcome(state, generation, publish);
    if publication == Publication::Superseded {
        hand_over(handed_over, text);
    }
    publication
}

/// Keeps `text` of a recording that a newer one replaced before the worker
/// could show it, for the newer recording's start to continue in place of
/// the overlay text it continued (`starting_prefix`).
fn hand_over(handed_over: &mut String, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    log::warn!("a newer recording started before this one's text was shown; it continues the text");
    append_text_segment(handed_over, text);
}

/// Shows `error` and keeps the text that failed for the next recording to
/// continue. Clears the worker's `text` when it is not kept.
fn publish_error_with_text(
    state: &AppState,
    generation: RecordingGeneration,
    handed_over: &mut String,
    text: &mut String,
    error: &str,
) -> Publication {
    let publication = publish_or_hand_over(state, generation, handed_over, text, |_| {
        state.report_error_with_text(text.as_str(), error)
    });
    if publication != Publication::Published {
        text.clear();
    }
    publication
}

/// Publishes the dictation that resumes after a transformation or
/// correction, once its session has connected. An abort requested while it
/// connected cancels it instead, so no hidden recording keeps capturing.
/// Returns the session to keep.
fn publish_resumed_dictation<S>(
    state: &AppState,
    generation: RecordingGeneration,
    session: S,
    recording_prefix: &mut String,
    handed_over: &mut String,
) -> Option<S> {
    let publication =
        publish_or_hand_over(state, generation, handed_over, recording_prefix, |_| {
            state.set_state(STATE_RECORDING)
        });
    if publication == Publication::Published {
        return Some(session);
    }
    // Dropping the session aborts its task and releases its socket.
    recording_prefix.clear();
    None
}

/// Shows a ready buffer's transformation, or why it failed, unless an abort
/// requested since the transformation settled cancels it.
fn publish_buffer_transformation(
    state: &AppState,
    generation: RecordingGeneration,
    result: Result<Option<String>, String>,
    buffered_text: &mut String,
    handed_over: &mut String,
) -> Publication {
    match result {
        Ok(Some(transformed_text)) => {
            log::info!(
                "transformation completed: chars={}",
                transformed_text.chars().count()
            );
            *buffered_text = transformed_text;
            let publication =
                publish_or_hand_over(state, generation, handed_over, buffered_text, |_| {
                    state.set_overlay_text(buffered_text.clone());
                    state.set_state(STATE_BUFFER_READY);
                });
            if publication != Publication::Published {
                buffered_text.clear();
            }
            publication
        }
        Ok(None) => {
            buffered_text.clear();
            Publication::Cancelled
        }
        Err(error) => {
            log::error!("transformation failed: {}", error);
            publish_error_with_text(state, generation, handed_over, buffered_text, &error)
        }
    }
}

/// The text a dictation resumes from: `text` and a separator for the speech
/// that follows it.
fn resume_prefix(text: &str) -> String {
    let mut prefix = text.to_owned();
    if !prefix.is_empty() && !prefix.ends_with(char::is_whitespace) {
        prefix.push(' ');
    }
    prefix
}

/// The prefix a recording starts from. Text handed over from the recording
/// it replaced is that recording's final version of the text the start
/// continued from the overlay, so it takes the overlay text's place. A start
/// that continues nothing follows an Escape that discarded the replaced
/// recording's text, so the handed-over text is dropped with it.
fn starting_prefix(handed_over: &mut String, requested_prefix: String, continues: bool) -> String {
    let handed_over = std::mem::take(handed_over);
    if handed_over.is_empty() || !continues {
        return requested_prefix;
    }
    resume_prefix(&handed_over)
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
        state.set_state(crate::state::STATE_BUFFER_READY);
        let (controller, _queue) = queued_controller();
        let keyboard = Keyboard::with_transformation();
        keyboard.tap(&state, &controller, crate::key::Key::F6);
        assert!(keyboard.escape(&state, &controller));

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

            assert_eq!(
                prefix,
                super::CorrectionOutcome::Resume("Keep the entire narration. ".to_owned())
            );
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
            super::CorrectionOutcome::Settled
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

        assert_eq!(
            prefix,
            super::CorrectionOutcome::Resume("The edited narration. ".to_owned())
        );
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
        run_worker_with_config(state, config_with_deepgram_key(), commands);
    }

    fn run_worker_with_config(
        state: &std::sync::Arc<crate::state::AppState>,
        config: crate::config::Config,
        commands: Vec<super::Command>,
    ) {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        for command in commands {
            command_tx.send(command).unwrap();
        }
        drop(command_tx);
        super::spawn_worker(state.clone(), config_store(config), command_rx)
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
            ..
        }) = commands.pop()
        else {
            panic!("expected a dictation start last");
        };
        (commands, recording_prefix, generation)
    }

    /// Presses keys through the hotkey thread's own handlers, with the
    /// default bindings: F5 records, F6 transforms, and Escape cancels. A tap
    /// never lasts long enough to count as holding F5.
    struct Keyboard {
        store: crate::settings::LiveConfigStore,
        tracking: crate::hotkey::HotkeyTracking,
    }

    impl Keyboard {
        fn new() -> Self {
            Self::with_config(crate::config::Config::default())
        }

        /// With a transformation provider, so F6 transforms and F5 transforms
        /// before pasting.
        fn with_transformation() -> Self {
            let mut config = crate::config::Config::default();
            config.transformation.provider = Some("openai".to_owned());
            config.transformation.api_key = Some("test-key".to_owned());
            Self::with_config(config)
        }

        fn with_config(mut config: crate::config::Config) -> Self {
            config.mic.hold_ms = u64::MAX;
            Self {
                store: config_store(config),
                tracking: crate::hotkey::HotkeyTracking::default(),
            }
        }

        /// Presses and releases `key`, returning whether the press was
        /// handled rather than left to the frontmost app.
        fn tap(
            &self,
            state: &crate::state::AppState,
            controller: &super::TranscriptionController,
            key: crate::key::Key,
        ) -> bool {
            let modifiers = crate::hotkey_binding::HotkeyModifiers::default();
            let handled = crate::hotkey::handle_key_press(
                key,
                modifiers,
                &self.store,
                state,
                controller,
                &self.tracking,
            );
            crate::hotkey::handle_key_release(
                key,
                modifiers,
                &self.store,
                state,
                controller,
                &self.tracking,
            );
            handled
        }

        fn escape(
            &self,
            state: &crate::state::AppState,
            controller: &super::TranscriptionController,
        ) -> bool {
            self.tap(state, controller, crate::key::Key::Escape)
        }

        /// Taps the record key to start a dictation, then types words into
        /// its overlay.
        fn start_dictation_with_typed_words(
            &self,
            state: &crate::state::AppState,
            controller: &super::TranscriptionController,
        ) {
            self.tap(state, controller, crate::key::Key::F5);
            assert_eq!(state.get_state(), crate::state::STATE_RECORDING);
            state.set_overlay_text("Words typed into the new recording.");
        }
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
        let keyboard = Keyboard::new();
        keyboard.tap(&state, &controller, crate::key::Key::F5);
        assert!(keyboard.escape(&state, &controller));
        keyboard.start_dictation_with_typed_words(&state, &controller);
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
        let keyboard = Keyboard::new();
        // Escape on the ready buffer leaves its state to the worker.
        assert!(keyboard.escape(&state, &controller));
        assert_eq!(state.get_state(), STATE_BUFFER_READY);
        keyboard.start_dictation_with_typed_words(&state, &controller);

        let (earlier, prefix, generation) = split_before_last_start(&queue);
        assert_eq!(earlier.len(), 1, "the discard");
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

        // Escape during background work that then ends without settling the
        // abort, leaving the text it published.
        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        state.set_state(STATE_TRANSFORMING);
        assert!(keyboard.escape(&state, &controller));
        assert!(state.is_abort_requested());
        state.set_overlay_text("The transformed text.");
        state.set_state(STATE_BUFFER_READY);
        keyboard.start_dictation_with_typed_words(&state, &controller);

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
            let (controller, _queue) = queued_controller();
            let keyboard = Keyboard::new();
            assert!(keyboard.escape(&state, &controller));
            keyboard.start_dictation_with_typed_words(&state, &controller);

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
            let (controller, _queue) = queued_controller();
            let keyboard = Keyboard::new();
            keyboard.tap(&state, &controller, crate::key::Key::F5);
            let cancelled = state.recording_generation();
            assert!(keyboard.escape(&state, &controller));
            keyboard.start_dictation_with_typed_words(&state, &controller);

            assert_eq!(
                super::settle_session_finish(result, &state, cancelled),
                Ok(None)
            );
            assert_new_recording_intact(&state);
        }
    }

    /// The dictation start the record key queued last, if any.
    fn last_start(
        queue: &std::sync::mpsc::Receiver<super::Command>,
    ) -> Option<(String, crate::state::RecordingGeneration)> {
        queue.try_iter().fold(None, |last, command| match command {
            super::Command::StartSession {
                recording_prefix,
                generation,
                ..
            } => Some((recording_prefix, generation)),
            _ => last,
        })
    }

    #[test]
    fn escape_while_a_resumed_dictation_connects_leaves_no_hidden_recording() {
        use crate::key::Key;
        use crate::state::{AppState, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING};

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::with_transformation();
        keyboard.tap(&state, &controller, Key::F5);
        state.set_overlay_text("Words before the transformation.");
        keyboard.tap(&state, &controller, Key::F6);
        assert_eq!(state.get_state(), STATE_PROCESSING);
        assert!(state.is_capturing_audio());
        let generation = state.recording_generation();
        let (_, started) = last_start(&queue).unwrap();
        assert_eq!(started, generation);

        // The worker transformed the text and reconnected; Escape lands
        // before it publishes the resumed dictation.
        assert!(keyboard.escape(&state, &controller));
        let mut recording_prefix = "The transformed words. ".to_owned();
        let mut handed_over = String::new();
        let session = super::publish_resumed_dictation(
            &state,
            generation,
            "the connected session",
            &mut recording_prefix,
            &mut handed_over,
        );

        assert_eq!(session, None, "the connected session is dropped");
        assert_eq!(state.get_state(), STATE_IDLE);
        assert!(!state.is_capturing_audio());
        assert!(!state.is_abort_requested());
        assert!(recording_prefix.is_empty());
        assert!(handed_over.is_empty());

        // The record key starts a new recording instead of stopping a
        // hidden one.
        keyboard.tap(&state, &controller, Key::F5);
        assert_eq!(state.get_state(), STATE_RECORDING);
        let (prefix, started) = last_start(&queue).expect("a new dictation start");
        assert!(started > generation);
        assert!(prefix.is_empty());
    }

    #[test]
    fn a_record_press_during_a_failed_paste_waits_for_the_text_to_return_to_the_overlay() {
        use crate::key::Key;
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_PROCESSING, STATE_RECORDING};
        use std::cell::Cell;
        use std::rc::Rc;

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Rc::new(Keyboard::new());
        keyboard.tap(&state, &controller, Key::F5);
        state.set_overlay_text("Dictated words to paste.");
        // The second tap stops the recording, to paste it.
        keyboard.tap(&state, &controller, Key::F5);
        assert_eq!(state.get_state(), STATE_PROCESSING);
        let generation = state.recording_generation();
        assert!(last_start(&queue).is_some());

        let pressed_during_paste = Rc::new(Cell::new(None));
        {
            let state = state.clone();
            let controller = controller.clone();
            let keyboard = keyboard.clone();
            let pressed = pressed_during_paste.clone();
            super::clipboard::during_next_paste(move || {
                let record_handled = keyboard.tap(&state, &controller, Key::F5);
                let escape_handled = keyboard.escape(&state, &controller);
                pressed.set(Some((
                    record_handled,
                    escape_handled,
                    state.get_state(),
                    state.recording_generation(),
                    state.is_overlay_dismissed(),
                    state.is_abort_requested(),
                )));
            });
        }
        let mut buffered_text = "Dictated words to paste. ".to_owned();
        let mut handed_over = String::new();
        super::clipboard::paste_buffered_text(
            &state,
            generation,
            &mut buffered_text,
            &mut handed_over,
        );

        assert_eq!(state.paste_attempts(), 1);
        // While the paste ran, the record key started nothing and Escape
        // went to the app being pasted into.
        assert_eq!(
            pressed_during_paste.get(),
            Some((true, false, STATE_PROCESSING, generation, true, false))
        );
        assert!(last_start(&queue).is_none());
        // The failed paste returned the text to the overlay.
        assert_eq!(state.get_state(), STATE_BUFFER_READY);
        assert_eq!(&*state.overlay_text(), "Dictated words to paste. ");
        assert!(!state.is_overlay_dismissed());
        assert!(!state.is_abort_requested());
        assert!(handed_over.is_empty());

        // The next recording continues it.
        keyboard.tap(&state, &controller, Key::F5);
        assert_eq!(state.get_state(), STATE_RECORDING);
        let (prefix, started) = last_start(&queue).expect("a dictation start");
        assert!(started > generation);
        assert_eq!(prefix, "Dictated words to paste. ");
    }

    #[test]
    fn text_a_newer_recording_replaced_before_its_paste_continues_in_that_recording() {
        use crate::state::{AppState, STATE_ERROR, STATE_RECORDING};

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        keyboard.tap(&state, &controller, crate::key::Key::F5);
        let pasted = state.recording_generation();
        // The microphone failed while the worker finished the transcript,
        // and the record key then started a new recording.
        state.report_error("Audio capture stopped.");
        assert_eq!(state.get_state(), STATE_ERROR);
        keyboard.tap(&state, &controller, crate::key::Key::F5);
        let (requested_prefix, newer) = last_start(&queue).unwrap();
        assert!(newer > pasted);

        let mut buffered_text = "Words of the replaced recording.".to_owned();
        let mut handed_over = String::new();
        super::clipboard::paste_buffered_text(&state, pasted, &mut buffered_text, &mut handed_over);

        assert_eq!(state.paste_attempts(), 0);
        assert_eq!(state.get_state(), STATE_RECORDING);
        assert!(buffered_text.is_empty());
        assert_eq!(
            super::starting_prefix(&mut handed_over, requested_prefix, true),
            "Words of the replaced recording. "
        );
        assert!(handed_over.is_empty());
    }

    #[test]
    fn a_start_cancelled_before_its_config_failed_leaves_the_next_recording_alone() {
        if run_without_deepgram_key(
            "a_start_cancelled_before_its_config_failed_leaves_the_next_recording_alone",
        ) {
            return;
        }
        let state = crate::state::AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        keyboard.tap(&state, &controller, crate::key::Key::F5);
        assert!(keyboard.escape(&state, &controller));
        keyboard.start_dictation_with_typed_words(&state, &controller);

        // The cancelled start, whose config has no Deepgram key, and its
        // abort.
        let (earlier, _, _) = split_before_last_start(&queue);
        assert_eq!(earlier.len(), 2);
        run_worker_with_config(&state, crate::config::Config::default(), earlier);

        assert_new_recording_intact(&state);
        assert!(state.overlay_error_text().is_empty());
    }

    /// Taps the record key to stop the dictation in progress and pastes it,
    /// which fails in tests, so its text returns to the overlay.
    fn stop_and_fail_to_paste(
        state: &std::sync::Arc<crate::state::AppState>,
        controller: &super::TranscriptionController,
        keyboard: &Keyboard,
        text: &str,
    ) {
        state.set_overlay_text(text);
        keyboard.tap(state, controller, crate::key::Key::F5);
        let mut buffered_text = text.to_owned();
        super::clipboard::paste_buffered_text(
            state,
            state.recording_generation(),
            &mut buffered_text,
            &mut String::new(),
        );
        assert_eq!(state.get_state(), crate::state::STATE_BUFFER_READY);
    }

    #[test]
    fn a_failed_connection_keeps_the_text_a_recording_continued() {
        if run_in_child(
            "a_failed_connection_keeps_the_text_a_recording_continued",
            |child| {
                child.env("SIMPLE_PTT_SIMULATE_ERROR", "connection failed");
            },
        ) {
            return;
        }
        use crate::key::Key;
        use crate::state::{AppState, STATE_ERROR, STATE_IDLE};

        for discard in [false, true] {
            let state = AppState::new();
            let (controller, queue) = queued_controller();
            let keyboard = Keyboard::new();
            keyboard.tap(&state, &controller, Key::F5);
            stop_and_fail_to_paste(&state, &controller, &keyboard, "Words a paste returned. ");
            // The next recording continues the returned text, then fails to
            // connect.
            keyboard.tap(&state, &controller, Key::F5);
            run_worker(&state, queue.try_iter().collect());

            assert_eq!(state.get_state(), STATE_ERROR);
            assert!(state.overlay_error_text().contains("connection failed"));
            assert_eq!(&*state.overlay_text(), "Words a paste returned. ");
            if discard {
                assert!(keyboard.escape(&state, &controller));
                assert_eq!(state.get_state(), STATE_IDLE);
            }
            keyboard.tap(&state, &controller, Key::F5);
            let (prefix, _) = last_start(&queue).expect("a dictation start");
            let expected = if discard {
                ""
            } else {
                "Words a paste returned. "
            };
            assert_eq!(prefix, expected, "discard={discard}");
        }
    }

    #[test]
    fn a_failed_connection_keeps_text_handed_over_from_a_replaced_recording() {
        if run_in_child(
            "a_failed_connection_keeps_text_handed_over_from_a_replaced_recording",
            |child| {
                child.env("SIMPLE_PTT_SIMULATE_ERROR", "connection failed");
            },
        ) {
            return;
        }
        use crate::key::Key;
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_ERROR};

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        state.set_overlay_text("Text of the replaced recording.");
        state.set_state(STATE_BUFFER_READY);
        keyboard.tap(&state, &controller, Key::F5);
        // The microphone failed before the worker started that recording,
        // and the record key then started another.
        state.report_error("Audio capture stopped.");
        keyboard.tap(&state, &controller, Key::F5);
        run_worker(&state, queue.try_iter().collect());

        assert_eq!(state.get_state(), STATE_ERROR);
        assert!(state.overlay_error_text().contains("connection failed"));
        assert_eq!(&*state.overlay_text(), "Text of the replaced recording. ");
        keyboard.tap(&state, &controller, Key::F5);
        let (prefix, _) = last_start(&queue).expect("a dictation start");
        assert_eq!(prefix, "Text of the replaced recording. ");
    }

    #[test]
    fn a_configuration_error_keeps_the_text_a_recording_continued() {
        if run_without_deepgram_key("a_configuration_error_keeps_the_text_a_recording_continued") {
            return;
        }
        use crate::key::Key;
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_ERROR};

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        state.set_overlay_text("A draft to keep.");
        state.set_state(STATE_BUFFER_READY);
        keyboard.tap(&state, &controller, Key::F5);
        run_worker_with_config(
            &state,
            crate::config::Config::default(),
            queue.try_iter().collect(),
        );

        assert_eq!(state.get_state(), STATE_ERROR);
        assert!(state.overlay_error_text().contains("Settings > Deepgram"));
        assert_eq!(&*state.overlay_text(), "A draft to keep.");
        keyboard.tap(&state, &controller, Key::F5);
        let (prefix, _) = last_start(&queue).expect("a dictation start");
        assert_eq!(prefix, "A draft to keep.");
    }

    #[test]
    fn a_paste_that_panics_returns_the_text_like_a_failed_one() {
        use crate::key::Key;
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_RECORDING};

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        keyboard.tap(&state, &controller, Key::F5);
        state.set_overlay_text("Words to paste.");
        keyboard.tap(&state, &controller, Key::F5);
        let generation = state.recording_generation();
        super::clipboard::during_next_paste(|| panic!("the paste panicked"));
        let mut buffered_text = "Words to paste.".to_owned();
        let pasted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::clipboard::paste_buffered_text(
                &state,
                generation,
                &mut buffered_text,
                &mut String::new(),
            )
        }));

        assert!(pasted.is_err());
        assert!(!state.is_pasting());
        assert_eq!(state.get_state(), STATE_BUFFER_READY);
        assert_eq!(&*state.overlay_text(), "Words to paste.");
        assert!(!state.is_overlay_dismissed());
        keyboard.tap(&state, &controller, Key::F5);
        assert_eq!(state.get_state(), STATE_RECORDING);
        let (prefix, started) = last_start(&queue).expect("a dictation start");
        assert!(started > generation);
        assert_eq!(prefix, "Words to paste.");
    }

    #[test]
    fn a_record_press_refused_during_a_paste_changes_nothing() {
        use crate::key::Key;
        use crate::state::AppState;
        use std::cell::Cell;
        use std::rc::Rc;

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Rc::new(Keyboard::new());
        keyboard.tap(&state, &controller, Key::F5);
        state.set_overlay_text("Words to paste.");
        keyboard.tap(&state, &controller, Key::F5);
        let generation = state.recording_generation();
        assert!(last_start(&queue).is_some());

        let unchanged = Rc::new(Cell::new(None));
        {
            let state = state.clone();
            let controller = controller.clone();
            let keyboard = keyboard.clone();
            let unchanged = unchanged.clone();
            super::clipboard::during_next_paste(move || {
                // An audio error lands while the paste waits.
                state.report_error("Audio capture stopped.");
                state.dismiss_overlay();
                let observe = || {
                    (
                        state.get_state(),
                        state.recording_generation(),
                        state.overlay_error_text().to_string(),
                        state.is_overlay_dismissed(),
                        state.overlay_text().to_string(),
                    )
                };
                let before = observe();
                assert!(keyboard.tap(&state, &controller, Key::F5));
                unchanged.set(Some(before == observe()));
            });
        }
        super::clipboard::paste_buffered_text(
            &state,
            generation,
            &mut "Words to paste.".to_owned(),
            &mut String::new(),
        );

        assert_eq!(unchanged.get(), Some(true));
        assert!(last_start(&queue).is_none());
    }

    /// An audio controller with no input stream, as after the microphone
    /// stopped delivering audio.
    fn stalled_audio(
        state: &std::sync::Arc<crate::state::AppState>,
    ) -> crate::audio::AudioController {
        crate::audio::AudioController::inactive(
            state.clone(),
            super::TranscriptionController::without_worker(),
            config_store(crate::config::Config::default()),
        )
    }

    #[test]
    fn a_stalled_microphone_keeps_the_dictation_for_the_record_key_to_continue() {
        use crate::key::Key;
        use crate::state::{AppState, STATE_ERROR, STATE_RECORDING};

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        keyboard.tap(&state, &controller, Key::F5);
        state.set_overlay_text("Words dictated before the microphone stopped. ");

        stalled_audio(&state).sync_stream_state();
        assert_eq!(state.get_state(), STATE_ERROR);
        assert!(state.overlay_error_text().contains("keep dictating"));

        keyboard.tap(&state, &controller, Key::F5);
        assert_eq!(state.get_state(), STATE_RECORDING);
        let (prefix, _) = last_start(&queue).expect("a dictation start");
        assert_eq!(prefix, "Words dictated before the microphone stopped. ");
    }

    #[test]
    fn text_an_error_kept_survives_a_microphone_error_on_the_next_press() {
        use crate::key::Key;
        use crate::state::{AppState, STATE_ERROR, STATE_RECORDING};

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        keyboard.tap(&state, &controller, Key::F5);
        let mut text = "Words a failed transformation kept.".to_owned();
        super::publish_error_with_text(
            &state,
            state.recording_generation(),
            &mut String::new(),
            &mut text,
            "transformation failed",
        );
        assert_eq!(state.get_state(), STATE_ERROR);

        // The microphone is gone when the record key is pressed, then back.
        state.set_microphone_available(false);
        keyboard.tap(&state, &controller, Key::F5);
        assert_eq!(
            &*state.overlay_error_text(),
            crate::audio::NO_MICROPHONE_MESSAGE
        );
        state.set_microphone_available(true);
        keyboard.tap(&state, &controller, Key::F5);

        assert_eq!(state.get_state(), STATE_RECORDING);
        let (prefix, _) = last_start(&queue).expect("a dictation start");
        assert_eq!(prefix, "Words a failed transformation kept.");
    }

    #[test]
    fn a_session_that_fails_while_finishing_keeps_everything_dictated() {
        use crate::key::Key;
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_ERROR, STATE_PROCESSING};

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        state.set_overlay_text("A ready buffer.");
        state.set_state(STATE_BUFFER_READY);
        keyboard.tap(&state, &controller, Key::F5);
        // Speech and a keyboard edit, then the record key stops to paste.
        state.set_overlay_text("A ready buffer, edited. Then new speech. ");
        keyboard.tap(&state, &controller, Key::F5);
        assert_eq!(state.get_state(), STATE_PROCESSING);

        super::publish_session_failure(
            &state,
            state.recording_generation(),
            "Deepgram timed out while finishing transcription. Please try recording again.",
        );

        assert_eq!(state.get_state(), STATE_ERROR);
        assert_eq!(
            &*state.overlay_text(),
            "A ready buffer, edited. Then new speech. "
        );
        keyboard.tap(&state, &controller, Key::F5);
        let (prefix, _) = last_start(&queue).expect("a dictation start");
        assert_eq!(prefix, "A ready buffer, edited. Then new speech. ");
    }

    #[test]
    fn a_socket_failure_mid_dictation_keeps_the_transcript_for_the_record_key() {
        use crate::key::Key;
        use crate::state::{AppState, STATE_ERROR, STATE_RECORDING};

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        keyboard.tap(&state, &controller, Key::F5);

        // One final result, then the socket closes while still recording.
        let mut stream = tokio_stream::iter(vec![Ok(super::session::tests::transcript_response(
            "Words before the socket closed.",
            true,
            0.0,
            1.0,
        ))]);
        let progress =
            super::progress::SessionProgress::new(16000, super::progress::SessionLimits::default());
        let result = runtime.block_on(super::session::transcribe_and_report(
            &mut stream,
            state.clone(),
            super::SessionKind::Dictation,
            String::new(),
            progress,
        ));

        assert!(result.is_err());
        assert_eq!(state.get_state(), STATE_ERROR);
        assert!(state.overlay_error_text().contains("disconnected"));
        keyboard.tap(&state, &controller, Key::F5);
        assert_eq!(state.get_state(), STATE_RECORDING);
        let (prefix, _) = last_start(&queue).expect("a dictation start");
        assert_eq!(prefix, "Words before the socket closed. ");
    }

    #[test]
    fn handed_over_text_replaces_the_continued_overlay_text_or_goes_with_its_discard() {
        let mut handed_over = "The replaced recording's final text.".to_owned();
        assert_eq!(
            super::starting_prefix(
                &mut handed_over,
                "The replaced recording's".to_owned(),
                true
            ),
            "The replaced recording's final text. "
        );
        assert!(handed_over.is_empty());

        let mut handed_over = "Text the user discarded with Escape.".to_owned();
        assert_eq!(
            super::starting_prefix(&mut handed_over, String::new(), false),
            ""
        );
        assert!(handed_over.is_empty());
    }

    #[test]
    fn escape_on_an_error_stops_the_work_it_interrupted_and_the_next_start_runs() {
        if run_in_child(
            "escape_on_an_error_stops_the_work_it_interrupted_and_the_next_start_runs",
            |child| {
                child.env("SIMPLE_PTT_SIMULATE_ERROR", "connection reached");
            },
        ) {
            return;
        }
        use crate::key::Key;
        use crate::state::{AppState, STATE_ERROR, STATE_IDLE, STATE_PROCESSING};

        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::new();
        keyboard.tap(&state, &controller, Key::F5);
        state.set_overlay_text("Words the user throws away.");
        keyboard.tap(&state, &controller, Key::F5);
        assert_eq!(state.get_state(), STATE_PROCESSING);
        let interrupted = state.recording_generation();
        // An audio error lands while the worker finishes the transcript.
        state.report_error("Audio stream error.");
        assert!(keyboard.escape(&state, &controller));
        assert_eq!(state.get_state(), STATE_IDLE);

        // The finish then succeeds, and the worker goes on to paste it.
        let mut buffered_text = "Words the user throws away. ".to_owned();
        super::clipboard::paste_buffered_text(
            &state,
            interrupted,
            &mut buffered_text,
            &mut String::new(),
        );

        assert_eq!(
            state.paste_attempts(),
            0,
            "Escape must never be followed by a paste"
        );
        assert_eq!(state.get_state(), STATE_IDLE);
        assert!(state.overlay_text().is_empty());
        assert!(buffered_text.is_empty());
        assert!(!state.is_abort_requested());

        // The next start runs, here to a simulated connection failure, and an
        // abort that no work used up does not skip the start after it.
        for _ in 0..2 {
            let _ = queue.try_iter().count();
            keyboard.tap(&state, &controller, Key::F5);
            let start = queue.try_iter().last().expect("a dictation start");
            let super::Command::StartSession {
                ref recording_prefix,
                continues,
                ..
            } = start
            else {
                panic!("expected a dictation start");
            };
            assert!(recording_prefix.is_empty());
            assert!(!continues);
            run_worker(&state, vec![start]);

            assert_eq!(state.get_state(), STATE_ERROR);
            assert!(state
                .overlay_error_text()
                .contains("simulated error: connection reached"));
            assert!(!state.is_abort_requested());
            // Escape on this error leaves an abort that nothing uses.
            assert!(keyboard.escape(&state, &controller));
            assert!(state.is_abort_requested());
        }
    }

    #[test]
    fn escape_after_a_buffer_transformation_settled_cancels_it_and_leaves_nothing_pending() {
        use crate::key::Key;
        use crate::state::{AppState, STATE_BUFFER_READY, STATE_IDLE, STATE_TRANSFORMING};

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = AppState::new();
        let (controller, queue) = queued_controller();
        let keyboard = Keyboard::with_transformation();
        state.set_overlay_text("The buffer to transform.");
        state.set_state(STATE_BUFFER_READY);
        keyboard.tap(&state, &controller, Key::F6);
        assert_eq!(state.get_state(), STATE_TRANSFORMING);
        let generation = state.recording_generation();

        let result = runtime.block_on(super::finish_transformation(&state, generation, async {
            Ok("The transformed buffer.".to_owned())
        }));
        assert_eq!(result, Ok(Some("The transformed buffer.".to_owned())));
        // Escape lands after the transformation settled, before its result
        // is published.
        assert!(keyboard.escape(&state, &controller));
        let mut buffered_text = "The buffer to transform.".to_owned();
        let mut handed_over = String::new();
        assert_eq!(
            super::publish_buffer_transformation(
                &state,
                generation,
                result,
                &mut buffered_text,
                &mut handed_over,
            ),
            super::Publication::Cancelled
        );

        assert_eq!(state.get_state(), STATE_IDLE);
        assert!(state.overlay_text().is_empty());
        assert!(!state.is_abort_requested());
        assert!(buffered_text.is_empty());
        assert!(handed_over.is_empty());
        // The cancelled result does not come back as the next prefix.
        keyboard.tap(&state, &controller, Key::F5);
        let (prefix, started) = last_start(&queue).expect("a dictation start");
        assert!(started > generation);
        assert!(prefix.is_empty());
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
        let keyboard = Keyboard::new();
        keyboard.tap(&state, &controller, crate::key::Key::F5);
        assert!(keyboard.escape(&state, &controller));
        // Only the start, without the abort command Escape queued after it.
        let start = queue.try_iter().next().unwrap();
        assert!(matches!(start, super::Command::StartSession { .. }));
        run_worker(&state, vec![start]);

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
