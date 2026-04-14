use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use deepgram::common::options::{Encoding, Endpointing, Options};
use deepgram::common::stream_response::{Channel, StreamResponse};
use deepgram::listen::websocket::TranscriptionStream;
use deepgram::{Deepgram, DeepgramError};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSPasteboard, NSPasteboardTypeString, NSWorkspace};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};
use objc2_foundation::NSString;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::settings::LiveConfigStore;
use crate::state::{
    AppState, DeepgramConnectionStatus, STATE_BUFFER_READY, STATE_ERROR, STATE_IDLE,
    STATE_PROCESSING, STATE_RECORDING, STATE_TRANSFORMING,
};
use crate::transformation::{transform_text, TransformationRuntimeConfig};

const AUDIO_QUEUE_CAPACITY: usize = 32;
const COMMAND_FLAGGED_V_KEYCODE: u16 = 9;
const KEY_EVENT_DELAY_MS: u64 = 20;
const PASTE_HANDOFF_POLL_INTERVAL_MS: u64 = 10;
const PASTE_HANDOFF_TIMEOUT_MS: u64 = 750;
const PASTEBOARD_SETTLE_DELAY_MS: u64 = 80;

#[derive(Clone, Copy, Debug)]
enum PasteStep {
    WaitForOverlayHide,
    WaitForAppDeactivation,
    WriteClipboard,
    WaitForPasteboardSettle,
    SendPasteShortcut,
}

#[derive(Debug, Default)]
struct PasteDiagnostics {
    app_active: bool,
    frontmost_application_name: Option<String>,
    frontmost_bundle_identifier: Option<String>,
    overlay_window_visible: bool,
}

#[derive(Clone, Debug)]
pub struct DeepgramConfig {
    pub api_key: String,
    pub model: String,
    pub language: String,
    pub endpointing_ms: u16,
    pub utterance_end_ms: u16,
    pub keyterms: Vec<String>,
}

#[derive(Clone)]
pub struct TranscriptionController {
    command_tx: Sender<Command>,
    sample_rate: Arc<AtomicU32>,
}

enum Command {
    StartSession,
    AudioChunk(bytes::Bytes),
    InsertClipboardText,
    StopSessionAndPaste,
    StopSessionAndTransformAndResume,
    StopSessionAndTransformAndPaste,
    PasteBuffer,
    TransformBuffer,
    DiscardBuffer,
}

struct ActiveSession {
    audio_tx: tokio_mpsc::Sender<Result<Bytes, io::Error>>,
    task: tokio::task::JoinHandle<Result<String, String>>,
}

impl TranscriptionController {
    pub fn set_sample_rate(&self, sample_rate: u32) {
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
    }

    pub fn start_session(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StartSession)
            .map_err(|_| "transcription worker is not running".to_owned())
    }

    pub fn insert_clipboard_text(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::InsertClipboardText)
            .map_err(|_| "transcription worker is not running".to_owned())
    }

    pub fn stop_session_and_paste(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StopSessionAndPaste)
            .map_err(|_| "transcription worker is not running".to_owned())
    }

    pub fn stop_session_and_transform_and_resume(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StopSessionAndTransformAndResume)
            .map_err(|_| "transcription worker is not running".to_owned())
    }

    pub fn stop_session_and_transform_and_paste(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StopSessionAndTransformAndPaste)
            .map_err(|_| "transcription worker is not running".to_owned())
    }

    pub fn paste_buffer(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::PasteBuffer)
            .map_err(|_| "transcription worker is not running".to_owned())
    }

    pub fn transform_buffer(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::TransformBuffer)
            .map_err(|_| "transcription worker is not running".to_owned())
    }

    pub fn discard_buffer(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::DiscardBuffer)
            .map_err(|_| "transcription worker is not running".to_owned())
    }

    pub fn send_audio(&self, pcm_bytes: bytes::Bytes) {
        if pcm_bytes.is_empty() {
            return;
        }

        if let Err(_) = self.command_tx.send(Command::AudioChunk(pcm_bytes)) {
            log::error!("dropping audio chunk because the transcription worker is unavailable");
        }
    }
}

pub fn spawn_transcription_thread(
    state: Arc<AppState>,
    config_store: LiveConfigStore,
) -> TranscriptionController {
    let (command_tx, command_rx) = mpsc::channel();
    let sample_rate = Arc::new(AtomicU32::new(16000));
    let worker_sample_rate = Arc::clone(&sample_rate);

    thread::Builder::new()
        .name("deepgram".into())
        .spawn(move || {
            let runtime = Builder::new_multi_thread()
                .enable_all()
                .thread_name("deepgram-rt")
                .build()
                .expect("failed to build tokio runtime for Deepgram");

            let mut active_session: Option<ActiveSession> = None;
            let mut buffered_text = String::new();
            let mut recording_prefix = String::new();

            while let Ok(command) = command_rx.recv() {
                match command {
                    Command::StartSession => {
                        if active_session.is_some() {
                            log::warn!("ignoring start request because a Deepgram session is already active");
                            continue;
                        }

                        if !buffered_text.is_empty() {
                            recording_prefix = buffered_text.clone();
                            buffered_text.clear();
                        } else {
                            recording_prefix.clear();
                        }

                        let current_sample_rate = worker_sample_rate.load(Ordering::Relaxed);
                        if state.is_abort_requested() {
                            log::info!(
                                "skipping Deepgram start because abort was requested before startup completed"
                            );
                            state.clear_overlay_text();
                            state.set_overlay_text_opacity(1.0);
                            state.set_state(STATE_IDLE);
                            continue;
                        }

                        state.clear_abort_request();
                        state.restore_overlay();
                        if recording_prefix.is_empty() {
                            state.clear_overlay_text();
                        } else {
                            state.set_overlay_text(recording_prefix.clone());
                        }
                        state.set_overlay_text_opacity(1.0);
                        let current_config = config_store.current();
                        let deepgram_config = match resolved_deepgram_config(&current_config) {
                            Ok(deepgram_config) => deepgram_config,
                            Err(error) => {
                                if state.is_abort_requested() {
                                    log::info!(
                                        "ignoring Deepgram config resolution failure after abort request: {}",
                                        error
                                    );
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                } else {
                                    log::error!("failed to resolve Deepgram config: {}", error);
                                    state.set_state(STATE_ERROR);
                                }
                                continue;
                            }
                        };
                        match start_session(
                            &runtime,
                            state.clone(),
                            &deepgram_config,
                            current_sample_rate,
                            recording_prefix.clone(),
                        ) {
                            Ok(session) => {
                                state.set_deepgram_connection_status(
                                    DeepgramConnectionStatus::Connected,
                                );
                                active_session = Some(session);
                            }
                            Err(error) => {
                                if state.is_abort_requested() {
                                    log::info!(
                                        "ignoring Deepgram start failure after abort request: {}",
                                        error
                                    );
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                } else {
                                    log::error!("failed to start Deepgram session: {}", error);
                                    state.set_deepgram_connection_status(
                                        DeepgramConnectionStatus::Disconnected,
                                    );
                                    state.set_state(STATE_ERROR);
                                }
                            }
                        }
                    }
                    Command::AudioChunk(audio_chunk) => {
                        let Some(session) = active_session.as_mut() else {
                            continue;
                        };

                        match session.audio_tx.try_send(Ok(audio_chunk)) {
                            Ok(()) => {}
                            Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                                log::debug!("dropping audio chunk because the Deepgram session queue is full");
                            }
                            Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                                log::error!("Deepgram session queue closed unexpectedly");
                                active_session = None;
                                recording_prefix.clear();
                                state.set_deepgram_connection_status(
                                    DeepgramConnectionStatus::Disconnected,
                                );
                                state.set_state(STATE_ERROR);
                            }
                        }
                    }
                    Command::InsertClipboardText => {
                        let Some(session) = active_session.take() else {
                            log::warn!(
                                "received clipboard-insert request without an active Deepgram session"
                            );
                            continue;
                        };

                        match finish_session(&runtime, session) {
                            Ok(transcript) => {
                                // Since transcript ALREADY contains recording_prefix,
                                // we just take transcript.
                                recording_prefix = transcript;

                                match read_clipboard_text() {
                                    Ok(clipboard_text) => {
                                        if clipboard_text.trim().is_empty() {
                                            log::warn!(
                                                "ignoring clipboard insert request because the clipboard text is empty"
                                            );
                                        } else {
                                            append_text_segment(
                                                &mut recording_prefix,
                                                clipboard_text.as_str(),
                                            );
                                            log::info!(
                                                "inserted {} clipboard characters into the active transcript",
                                                clipboard_text.trim().chars().count()
                                            );
                                        }
                                    }
                                    Err(error) => {
                                        log::warn!(
                                            "skipping clipboard insertion because clipboard text could not be read: {}",
                                            error
                                        );
                                    }
                                }

                                if recording_prefix.is_empty() {
                                    state.clear_overlay_text();
                                } else {
                                    state.set_overlay_text(recording_prefix.clone());
                                }
                                state.set_overlay_text_opacity(1.0);

                                let current_sample_rate = worker_sample_rate.load(Ordering::Relaxed);
                                let current_config = config_store.current();
                                let deepgram_config = match resolved_deepgram_config(&current_config)
                                {
                                    Ok(deepgram_config) => deepgram_config,
                                    Err(error) => {
                                        log::error!(
                                            "failed to resolve Deepgram config for clipboard insertion: {}",
                                            error
                                        );
                                        buffered_text = recording_prefix.clone();
                                        recording_prefix.clear();
                                        if buffered_text.is_empty() {
                                            state.set_state(STATE_ERROR);
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
                                            state.set_state(STATE_ERROR);
                                        } else {
                                            state.set_overlay_text(buffered_text.clone());
                                            state.set_state(STATE_BUFFER_READY);
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                log::error!(
                                    "failed to checkpoint the active transcript for clipboard insertion: {}",
                                    error
                                );
                                recording_prefix.clear();
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                                state.set_state(STATE_ERROR);
                            }
                        }
                    }
                    Command::StopSessionAndPaste => {
                        let abort_requested = state.is_abort_requested();
                        if !abort_requested {
                            state.set_state(STATE_PROCESSING);
                        }

                        let Some(session) = active_session.take() else {
                            log::warn!("received paste-stop request without an active Deepgram session");
                            if abort_requested {
                                state.clear_abort_request();
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                            }
                            state.set_state(STATE_IDLE);
                            continue;
                        };

                        match finish_session(&runtime, session) {
                            Ok(transcript) => {
                                if state.consume_abort_request() {
                                    log::info!("discarding transcript because the session was aborted");
                                    recording_prefix.clear();
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                recording_prefix.clear();

                                if transcript.trim().is_empty() {
                                    log::info!("Deepgram session completed without a final transcript");
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                buffered_text = transcript;
                                paste_buffered_text(&state, &mut buffered_text);
                            }
                            Err(error) => {
                                if state.consume_abort_request() {
                                    log::info!(
                                        "ignoring Deepgram session error after abort request: {}",
                                        error
                                    );
                                    recording_prefix.clear();
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                recording_prefix.clear();
                                log::error!("Deepgram session failed: {}", error);
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                                state.set_state(STATE_ERROR);
                            }
                        }
                    }
                    Command::StopSessionAndTransformAndResume => {
                        let abort_requested = state.is_abort_requested();
                        if !abort_requested {
                            state.set_state(STATE_PROCESSING);
                        }
                        let is_resume = matches!(command, Command::StopSessionAndTransformAndResume);

                        let Some(session) = active_session.take() else {
                            log::warn!(
                                "received transform-stop request without an active Deepgram session"
                            );
                            if abort_requested {
                                state.clear_abort_request();
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                            }
                            state.set_state(STATE_IDLE);
                            continue;
                        };

                        match finish_session(&runtime, session) {
                            Ok(transcript) => {
                                if state.consume_abort_request() {
                                    log::info!("discarding transcript because the session was aborted");
                                    recording_prefix.clear();
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                recording_prefix.clear();

                                if transcript.trim().is_empty() {
                                    log::info!("Deepgram session completed without a final transcript");
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                buffered_text = transcript;
                                let transformation_config =
                                    config_store.current().resolve_transformation_config().ok();
                                transform_buffered_text(
                                    &runtime,
                                    state.clone(),
                                    &transformation_config,
                                    &mut buffered_text,
                                    false,
                                );

                                if is_resume {
                                    let current_sample_rate = worker_sample_rate.load(Ordering::Relaxed);
                                    let current_config = config_store.current();

                                    if !buffered_text.is_empty() {
                                        recording_prefix = buffered_text.clone();
                                        buffered_text.clear();
                                    } else {
                                        recording_prefix.clear();
                                    }

                                    if let Ok(deepgram_config) = resolved_deepgram_config(&current_config) {
                                        match start_session(
                                            &runtime,
                                            state.clone(),
                                            &deepgram_config,
                                            current_sample_rate,
                                            recording_prefix.clone(),
                                        ) {
                                            Ok(session) => {
                                                active_session = Some(session);
                                                state.set_deepgram_connection_status(
                                                    DeepgramConnectionStatus::Connected,
                                                );
                                                state.set_state(STATE_RECORDING);
                                            }
                                            Err(error) => {
                                                log::error!("failed to auto-resume Deepgram session: {}", error);
                                                state.set_deepgram_connection_status(
                                                    DeepgramConnectionStatus::Disconnected,
                                                );
                                                state.set_state(STATE_ERROR);
                                            }
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                if state.consume_abort_request() {
                                    log::info!(
                                        "ignoring Deepgram session error after abort request: {}",
                                        error
                                    );
                                    recording_prefix.clear();
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                recording_prefix.clear();
                                log::error!("Deepgram session failed: {}", error);
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                                state.set_state(STATE_ERROR);
                            }
                        }
                    }
                    Command::StopSessionAndTransformAndPaste => {
                        let abort_requested = state.is_abort_requested();
                        if !abort_requested {
                            state.set_state(STATE_PROCESSING);
                        }

                        let Some(session) = active_session.take() else {
                            log::warn!(
                                "received transform-and-paste stop request without an active Deepgram session"
                            );
                            if abort_requested {
                                state.clear_abort_request();
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                            }
                            state.set_state(STATE_IDLE);
                            continue;
                        };

                        match finish_session(&runtime, session) {
                            Ok(transcript) => {
                                if state.consume_abort_request() {
                                    log::info!("discarding transcript because the session was aborted");
                                    recording_prefix.clear();
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                recording_prefix.clear();

                                if transcript.trim().is_empty() {
                                    log::info!("Deepgram session completed without a final transcript");
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                buffered_text = transcript;
                                let transformation_config =
                                    config_store.current().resolve_transformation_config().ok();
                                transform_buffered_text(
                                    &runtime,
                                    state.clone(),
                                    &transformation_config,
                                    &mut buffered_text,
                                    true,
                                );
                            }
                            Err(error) => {
                                if state.consume_abort_request() {
                                    log::info!(
                                        "ignoring Deepgram session error after abort request: {}",
                                        error
                                    );
                                    recording_prefix.clear();
                                    state.clear_overlay_text();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                recording_prefix.clear();
                                log::error!("Deepgram session failed: {}", error);
                                state.clear_overlay_text();
                                state.set_overlay_text_opacity(1.0);
                                state.set_state(STATE_ERROR);
                            }
                        }
                    }
                    Command::PasteBuffer => {
                        let current_ui_text = state.overlay_text().to_string();
                        if !current_ui_text.is_empty() && current_ui_text != buffered_text {
                            buffered_text = current_ui_text;
                        }
                        paste_buffered_text(&state, &mut buffered_text);
                    }
                    Command::TransformBuffer => {
                        let current_ui_text = state.overlay_text().to_string();
                        if !current_ui_text.is_empty() && current_ui_text != buffered_text {
                            buffered_text = current_ui_text;
                        }
                        let transformation_config =
                            config_store.current().resolve_transformation_config().ok();
                        transform_buffered_text(
                            &runtime,
                            state.clone(),
                            &transformation_config,
                            &mut buffered_text,
                            false,
                        );
                    }
                    Command::DiscardBuffer => {
                        buffered_text.clear();
                        recording_prefix.clear();
                        state.clear_abort_request();
                        state.clear_overlay_text();
                        state.set_overlay_text_opacity(1.0);
                        state.set_state(STATE_IDLE);
                    }
                }
            }
        })
        .expect("failed to spawn Deepgram worker thread");

    TranscriptionController {
        command_tx,
        sample_rate,
    }
}

fn resolved_deepgram_config(config: &crate::config::Config) -> Result<DeepgramConfig, String> {
    Ok(DeepgramConfig {
        api_key: config.resolve_deepgram_api_key()?,
        model: config.deepgram.model.clone(),
        language: config.deepgram.language.clone(),
        endpointing_ms: config.deepgram.endpointing_ms,
        utterance_end_ms: config.deepgram.utterance_end_ms,
        keyterms: config.deepgram.keyterms.clone(),
    })
}

fn start_session(
    runtime: &Runtime,
    state: Arc<AppState>,
    config: &DeepgramConfig,
    sample_rate: u32,
    recording_prefix: String,
) -> Result<ActiveSession, String> {
    let (audio_tx, audio_rx) = tokio_mpsc::channel(AUDIO_QUEUE_CAPACITY);
    let deepgram_config = config.clone();

    let mut transcription_stream = runtime.block_on(async move {
        let client = Deepgram::new(&deepgram_config.api_key).map_err(format_deepgram_error)?;
        let mut options_builder = Options::builder()
            .punctuate(true)
            .smart_format(true)
            .dictation(true)
            .query_params([
                ("model".to_owned(), deepgram_config.model.clone()),
                ("language".to_owned(), deepgram_config.language.clone()),
            ]);
            
        let keyterm_refs: Vec<&str> = deepgram_config.keyterms.iter().map(String::as_str).collect();
        if !keyterm_refs.is_empty() {
            options_builder = options_builder.keyterms(keyterm_refs);
        }
            
        let options = options_builder.build();

        client
            .transcription()
            .stream_request_with_options(options)
            .encoding(Encoding::Linear16)
            .sample_rate(sample_rate)
            .channels(1)
            .endpointing(Endpointing::CustomDurationMs(u32::from(
                deepgram_config.endpointing_ms,
            )))
            .utterance_end_ms(deepgram_config.utterance_end_ms)
            .interim_results(true)
            .vad_events(true)
            .keep_alive()
            .stream(ReceiverStream::new(audio_rx))
            .await
            .map_err(format_deepgram_error)
    })?;

    log::info!(
        "Deepgram session started (request_id={}, sample_rate={}Hz, model={}, language={})",
        transcription_stream.request_id(),
        sample_rate,
        config.model,
        config.language
    );

    let task = runtime.spawn(async move {
        run_transcription_stream(&mut transcription_stream, state, recording_prefix).await
    });

    Ok(ActiveSession { audio_tx, task })
}

fn finish_session(runtime: &Runtime, session: ActiveSession) -> Result<String, String> {
    drop(session.audio_tx);

    runtime.block_on(async move {
        match session.task.await {
            Ok(result) => result,
            Err(error) => Err(format!("transcription task join error: {}", error)),
        }
    })
}

fn paste_buffered_text(state: &AppState, buffered_text: &mut String) {
    if buffered_text.trim().is_empty() {
        log::warn!("ignoring paste request because no buffered text is available");
        state.clear_overlay_text();
        state.set_overlay_text_opacity(1.0);
        state.set_state(STATE_IDLE);
        return;
    }

    state.clear_overlay_text();
    state.set_overlay_text_opacity(1.0);
    state.set_state(STATE_IDLE);

    match run_paste_sequence(state, buffered_text.as_str()) {
        Ok(()) => {
            buffered_text.clear();
            state.clear_abort_request();
            log::info!("buffer pasted successfully");
        }
        Err(error) => {
            if state.consume_abort_request() {
                log::info!("discarding buffered text because abort was requested during paste");
                buffered_text.clear();
                return;
            }

            log::error!("failed to copy/paste buffered text: {}", error);
            state.set_overlay_text(buffered_text.clone());
            state.set_overlay_text_opacity(1.0);
            state.set_state(STATE_BUFFER_READY);
        }
    }
}

fn run_paste_sequence(state: &AppState, text: &str) -> Result<(), String> {
    log::info!(
        "paste handoff start: chars={}, diagnostics={}",
        text.chars().count(),
        describe_paste_diagnostics(&query_paste_diagnostics(state))
    );

    for step in [
        PasteStep::WaitForOverlayHide,
        PasteStep::WaitForAppDeactivation,
        PasteStep::WriteClipboard,
        PasteStep::WaitForPasteboardSettle,
        PasteStep::SendPasteShortcut,
    ] {
        match step {
            PasteStep::WaitForOverlayHide => {
                wait_for_paste_condition(state, step, |diagnostics| {
                    !diagnostics.overlay_window_visible
                })?;
            }
            PasteStep::WaitForAppDeactivation => {
                wait_for_paste_condition(state, step, |diagnostics| !diagnostics.app_active)?;
            }
            PasteStep::WriteClipboard => {
                write_clipboard_text(text)?;
                log::info!("paste step=write-clipboard chars={}", text.chars().count());
            }
            PasteStep::WaitForPasteboardSettle => {
                thread::sleep(Duration::from_millis(PASTEBOARD_SETTLE_DELAY_MS));
            }
            PasteStep::SendPasteShortcut => {
                let diagnostics = query_paste_diagnostics(state);
                log::info!(
                    "paste step=send-shortcut diagnostics={}",
                    describe_paste_diagnostics(&diagnostics)
                );
                send_paste_shortcut()?;
            }
        }
    }

    Ok(())
}

fn wait_for_paste_condition(
    state: &AppState,
    step: PasteStep,
    condition: impl Fn(&PasteDiagnostics) -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(PASTE_HANDOFF_TIMEOUT_MS);

    loop {
        let diagnostics = query_paste_diagnostics(state);
        if condition(&diagnostics) {
            log::info!(
                "paste step={:?} ready diagnostics={}",
                step,
                describe_paste_diagnostics(&diagnostics)
            );
            return Ok(());
        }

        if Instant::now() >= deadline {
            let details = describe_paste_diagnostics(&diagnostics);
            log::warn!("paste step={:?} timed out diagnostics={}", step, details);
            return Err(format!(
                "timed out while preparing paste ({:?}); {}",
                step, details
            ));
        }

        thread::sleep(Duration::from_millis(PASTE_HANDOFF_POLL_INTERVAL_MS));
    }
}

fn describe_paste_diagnostics(diagnostics: &PasteDiagnostics) -> String {
    let frontmost_name = diagnostics
        .frontmost_application_name
        .as_deref()
        .unwrap_or("unknown");
    let frontmost_bundle = diagnostics
        .frontmost_bundle_identifier
        .as_deref()
        .unwrap_or("unknown");

    format!(
        "overlay_visible={}, app_active={}, frontmost_name={}, frontmost_bundle={}",
        diagnostics.overlay_window_visible,
        diagnostics.app_active,
        frontmost_name,
        frontmost_bundle
    )
}

struct PasteDiagnosticsRequest {
    app_active: bool,
    frontmost_application_name: Option<String>,
    frontmost_bundle_identifier: Option<String>,
}

extern "C" fn perform_paste_diagnostics_query(context: *mut c_void) {
    let request = unsafe { &mut *(context as *mut PasteDiagnosticsRequest) };
    let mtm = MainThreadMarker::new().expect("paste diagnostics must run on main thread");
    let app = NSApplication::sharedApplication(mtm);
    request.app_active = app.isActive();

    let workspace = NSWorkspace::sharedWorkspace();
    if let Some(frontmost_application) = workspace.frontmostApplication() {
        request.frontmost_application_name = frontmost_application
            .localizedName()
            .map(|name| name.to_string());
        request.frontmost_bundle_identifier = frontmost_application
            .bundleIdentifier()
            .map(|bundle_identifier| bundle_identifier.to_string());
    }
}

fn query_paste_diagnostics(state: &AppState) -> PasteDiagnostics {
    let mut request = Box::new(PasteDiagnosticsRequest {
        app_active: false,
        frontmost_application_name: None,
        frontmost_bundle_identifier: None,
    });

    unsafe {
        dispatch_sync_f(
            &_dispatch_main_q,
            (&mut *request) as *mut PasteDiagnosticsRequest as *mut c_void,
            perform_paste_diagnostics_query,
        );
    }

    PasteDiagnostics {
        app_active: request.app_active,
        frontmost_application_name: request.frontmost_application_name.take(),
        frontmost_bundle_identifier: request.frontmost_bundle_identifier.take(),
        overlay_window_visible: state.is_overlay_window_visible(),
    }
}

fn transform_buffered_text(
    runtime: &Runtime,
    state: Arc<AppState>,
    transformation_config: &Option<TransformationRuntimeConfig>,
    buffered_text: &mut String,
    paste_after_transform: bool,
) {
    if buffered_text.trim().is_empty() {
        log::warn!("ignoring transformation request because no buffered text is available");
        state.clear_overlay_text();
        state.set_overlay_text_opacity(1.0);
        state.set_state(STATE_IDLE);
        return;
    }

    let Some(transform_config) = transformation_config.clone() else {
        log::warn!("ignoring transformation request because transformation config is incomplete");
        state.set_overlay_text(buffered_text.clone());
        state.set_overlay_text_opacity(1.0);
        state.set_state(STATE_BUFFER_READY);
        return;
    };

    let original_buffer = buffered_text.clone();
    state.clear_abort_request();
    state.restore_overlay();
    state.set_overlay_text(original_buffer.clone());
    state.set_overlay_text_opacity(0.02);
    state.set_state(STATE_TRANSFORMING);

    match runtime.block_on(transform_text(
        state.clone(),
        &transform_config,
        original_buffer.as_str(),
    )) {
        Ok(transformed_text) => {
            if state.consume_abort_request() {
                log::info!("discarding transformation result because abort was requested");
                buffered_text.clear();
                state.clear_overlay_text();
                state.set_overlay_text_opacity(1.0);
                state.set_state(STATE_IDLE);
                return;
            }

            *buffered_text = transformed_text;
            if !buffered_text.is_empty() && !buffered_text.ends_with(|c: char| c.is_whitespace()) {
                buffered_text.push(' ');
            }
            if paste_after_transform {
                log::info!("buffer transformed successfully; pasting result");
                paste_buffered_text(&state, buffered_text);
            } else {
                state.set_overlay_text(buffered_text.clone());
                state.set_overlay_text_opacity(1.0);
                state.set_state(STATE_BUFFER_READY);
                log::info!("buffer transformed successfully");
            }
        }
        Err(error) => {
            if state.consume_abort_request() {
                log::info!("discarding buffered text because transformation was aborted");
                buffered_text.clear();
                state.clear_overlay_text();
                state.set_overlay_text_opacity(1.0);
                state.set_state(STATE_IDLE);
                return;
            }

            log::error!("transformation failed: {}", error);
            state.set_overlay_text(original_buffer.clone());
            state.set_overlay_text_opacity(1.0);
            state.set_state(STATE_BUFFER_READY);
        }
    }
}

async fn run_transcription_stream(
    stream: &mut TranscriptionStream,
    state: Arc<AppState>,
    mut recording_prefix: String,
) -> Result<String, String> {
    let mut interim_transcript = String::new();
    let mut transcript_parts: Vec<String> = Vec::new();
    let mut last_final_transcript = String::new();
    let mut last_pushed_text = state.overlay_text().to_string();

    while let Some(message) = stream.next().await {
        match message {
            Ok(StreamResponse::TranscriptResponse {
                is_final,
                channel,
                from_finalize,
                ..
            }) => {
                let transcript = extract_transcript(&channel);
                if transcript.is_empty() {
                    continue;
                }

                let current_ui_text = state.overlay_text().to_string();
                if current_ui_text != last_pushed_text {
                    let mut new_prefix = current_ui_text.clone();
                    if !new_prefix.is_empty() && !new_prefix.ends_with(|c: char| c.is_whitespace())
                    {
                        new_prefix.push(' ');
                    }
                    recording_prefix = new_prefix;
                    transcript_parts.clear();
                    interim_transcript.clear();
                    last_final_transcript.clear();
                }

                if is_final {
                    if transcript != last_final_transcript {
                        log::info!(
                            "Deepgram final{}: {}",
                            if from_finalize {
                                " (from finalize)"
                            } else {
                                ""
                            },
                            transcript
                        );
                        last_final_transcript = transcript.clone();
                        interim_transcript.clear();
                        transcript_parts.push(transcript);
                        if !state.is_abort_requested() {
                            let new_text = build_overlay_text(
                                recording_prefix.as_str(),
                                &transcript_parts,
                                None,
                            );
                            last_pushed_text = new_text.clone();
                            state.set_overlay_text(new_text);
                        }
                    }
                    continue;
                }

                log::debug!("Deepgram interim: {}", transcript);
                interim_transcript = transcript;
                if !state.is_abort_requested() {
                    let new_text = build_overlay_text(
                        recording_prefix.as_str(),
                        &transcript_parts,
                        Some(interim_transcript.as_str()),
                    );
                    last_pushed_text = new_text.clone();
                    state.set_overlay_text(new_text);
                }
            }
            Ok(StreamResponse::TerminalResponse { duration, .. }) => {
                log::info!("Deepgram stream closed after {:.2}s", duration);
            }
            Ok(StreamResponse::SpeechStartedResponse { .. }) => {
                log::debug!("Deepgram detected speech start");
            }
            Ok(StreamResponse::UtteranceEndResponse { .. }) => {
                log::debug!("Deepgram detected utterance end");
            }
            Ok(other_message) => {
                log::debug!("ignoring unhandled Deepgram message: {:?}", other_message);
            }
            Err(error) => {
                state.set_deepgram_connection_status(DeepgramConnectionStatus::Disconnected);
                return Err(format_deepgram_error(error));
            }
        }
    }

    let final_ui_text = state.overlay_text().to_string();
    let final_transcript = if final_ui_text != last_pushed_text {
        final_ui_text
    } else {
        join_transcript_parts(recording_prefix.as_str(), &transcript_parts)
    };

    if !state.is_abort_requested() {
        state.set_overlay_text(final_transcript.clone());
    }
    Ok(final_transcript)
}

fn extract_transcript(channel: &Channel) -> String {
    channel
        .alternatives
        .first()
        .map(|alternative| alternative.transcript.trim().to_owned())
        .unwrap_or_default()
}

fn append_text_segment(output: &mut String, segment: &str) {
    let trimmed_segment = segment.trim();
    if trimmed_segment.is_empty() {
        return;
    }

    if !output.is_empty() {
        output.push(' ');
    }
    output.push_str(trimmed_segment);
}

fn build_overlay_text(
    recording_prefix: &str,
    transcript_parts: &[String],
    interim_transcript: Option<&str>,
) -> String {
    let mut overlay_text = String::new();
    append_text_segment(&mut overlay_text, recording_prefix);

    for transcript in transcript_parts {
        append_text_segment(&mut overlay_text, transcript.as_str());
    }

    if let Some(interim_transcript) = interim_transcript {
        append_text_segment(&mut overlay_text, interim_transcript);
    }

    if !overlay_text.is_empty() && !overlay_text.ends_with(|c: char| c.is_whitespace()) {
        overlay_text.push(' ');
    }

    overlay_text
}

fn join_transcript_parts(recording_prefix: &str, transcript_parts: &[String]) -> String {
    build_overlay_text(recording_prefix, transcript_parts, None)
}

fn send_paste_shortcut() -> Result<(), String> {
    let key_down = create_keyboard_event(COMMAND_FLAGGED_V_KEYCODE, true)?;
    CGEvent::set_flags(Some(&key_down), CGEventFlags::MaskCommand);
    CGEvent::post(
        CGEventTapLocation::AnnotatedSessionEventTap,
        Some(&key_down),
    );
    thread::sleep(Duration::from_millis(KEY_EVENT_DELAY_MS));

    let key_up = create_keyboard_event(COMMAND_FLAGGED_V_KEYCODE, false)?;
    CGEvent::set_flags(Some(&key_up), CGEventFlags::MaskCommand);
    CGEvent::post(CGEventTapLocation::AnnotatedSessionEventTap, Some(&key_up));
    thread::sleep(Duration::from_millis(KEY_EVENT_DELAY_MS));

    Ok(())
}

fn create_keyboard_event(virtual_key: u16, key_down: bool) -> Result<CFRetained<CGEvent>, String> {
    extern "C-unwind" {
        fn CGEventCreateKeyboardEvent(
            source: *const c_void,
            virtual_key: u16,
            key_down: bool,
        ) -> Option<NonNull<CGEvent>>;
    }

    let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null(), virtual_key, key_down) }
        .ok_or_else(|| {
            format!(
                "failed to create synthetic keyboard event for keycode {} ({})",
                virtual_key,
                if key_down { "down" } else { "up" }
            )
        })?;

    Ok(unsafe { CFRetained::from_raw(event) })
}

struct ClipboardReadRequest {
    text: Option<String>,
}

struct ClipboardWriteRequest {
    text: String,
    success: bool,
}

extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_sync_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

extern "C" fn perform_clipboard_read(context: *mut c_void) {
    let request = unsafe { &mut *(context as *mut ClipboardReadRequest) };
    let pasteboard = NSPasteboard::generalPasteboard();
    request.text = pasteboard
        .stringForType(unsafe { NSPasteboardTypeString })
        .map(|text| text.to_string());
}

fn read_clipboard_text() -> Result<String, String> {
    let mut request = Box::new(ClipboardReadRequest { text: None });

    unsafe {
        dispatch_sync_f(
            &_dispatch_main_q,
            (&mut *request) as *mut ClipboardReadRequest as *mut c_void,
            perform_clipboard_read,
        );
    }

    request
        .text
        .ok_or_else(|| "clipboard does not currently contain plain text".to_owned())
}

extern "C" fn perform_clipboard_write(context: *mut c_void) {
    let request = unsafe { &mut *(context as *mut ClipboardWriteRequest) };
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();

    let ns_text = NSString::from_str(&request.text);
    request.success = pasteboard.setString_forType(&ns_text, unsafe { NSPasteboardTypeString });
}

fn write_clipboard_text(text: &str) -> Result<(), String> {
    let mut request = Box::new(ClipboardWriteRequest {
        text: text.to_owned(),
        success: false,
    });

    unsafe {
        dispatch_sync_f(
            &_dispatch_main_q,
            (&mut *request) as *mut ClipboardWriteRequest as *mut c_void,
            perform_clipboard_write,
        );
    }

    if request.success {
        Ok(())
    } else {
        Err("failed to update the macOS general pasteboard".to_owned())
    }
}

fn format_deepgram_error(error: DeepgramError) -> String {
    format!("{}", error)
}

#[cfg(test)]
mod tests {
    use super::{append_text_segment, build_overlay_text};

    #[test]
    fn append_text_segment_joins_non_empty_segments_with_single_spaces() {
        let mut output = String::new();
        append_text_segment(&mut output, "  hello  ");
        append_text_segment(&mut output, "\nworld\n");
        append_text_segment(&mut output, "   ");

        assert_eq!(output, "hello world");
    }

    #[test]
    fn build_overlay_text_preserves_recording_prefix_before_live_interim_text() {
        let overlay_text = build_overlay_text(
            "hello copied-url",
            &["spoken".to_owned()],
            Some("right now"),
        );

        assert_eq!(overlay_text, "hello copied-url spoken right now ");
    }
}
