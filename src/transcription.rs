use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use bytes::Bytes;
use deepgram::common::options::{Encoding, Endpointing, Options};
use deepgram::common::stream_response::{Channel, StreamResponse};
use deepgram::listen::websocket::TranscriptionStream;
use deepgram::{Deepgram, DeepgramError};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;
use rdev::{simulate, EventType, Key, SimulateError};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::state::{AppState, STATE_ERROR, STATE_IDLE, STATE_PROCESSING};

const AUDIO_QUEUE_CAPACITY: usize = 32;
const COMMAND_QUEUE_CAPACITY: usize = 256;
const KEY_EVENT_DELAY_MS: u64 = 20;
const PASTEBOARD_SETTLE_DELAY_MS: u64 = 50;

#[derive(Clone, Debug)]
pub struct DeepgramConfig {
    pub api_key: String,
    pub model: String,
    pub language: String,
    pub endpointing_ms: u16,
    pub utterance_end_ms: u16,
}

#[derive(Clone)]
pub struct TranscriptionController {
    command_tx: SyncSender<Command>,
    sample_rate: Arc<AtomicU32>,
}

enum Command {
    StartSession,
    AudioChunk(Vec<u8>),
    StopSession,
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

    pub fn stop_session(&self) -> Result<(), String> {
        self.command_tx
            .send(Command::StopSession)
            .map_err(|_| "transcription worker is not running".to_owned())
    }

    pub fn send_audio(&self, pcm_bytes: Vec<u8>) {
        if pcm_bytes.is_empty() {
            return;
        }

        match self.command_tx.try_send(Command::AudioChunk(pcm_bytes)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                log::debug!("dropping audio chunk because the transcription command queue is full");
            }
            Err(TrySendError::Disconnected(_)) => {
                log::error!("dropping audio chunk because the transcription worker is unavailable");
            }
        }
    }
}

pub fn spawn_transcription_thread(
    state: Arc<AppState>,
    config: DeepgramConfig,
) -> TranscriptionController {
    let (command_tx, command_rx) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
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

            while let Ok(command) = command_rx.recv() {
                match command {
                    Command::StartSession => {
                        if active_session.is_some() {
                            log::warn!("ignoring start request because a Deepgram session is already active");
                            continue;
                        }

                        let current_sample_rate = worker_sample_rate.load(Ordering::Relaxed);
                        state.clear_overlay_text();
                        match start_session(&runtime, state.clone(), &config, current_sample_rate) {
                            Ok(session) => {
                                active_session = Some(session);
                            }
                            Err(error) => {
                                log::error!("failed to start Deepgram session: {}", error);
                                state.set_state(STATE_ERROR);
                            }
                        }
                    }
                    Command::AudioChunk(audio_chunk) => {
                        let Some(session) = active_session.as_mut() else {
                            continue;
                        };

                        match session.audio_tx.try_send(Ok(Bytes::from(audio_chunk))) {
                            Ok(()) => {}
                            Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                                log::debug!("dropping audio chunk because the Deepgram session queue is full");
                            }
                            Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                                log::error!("Deepgram session queue closed unexpectedly");
                                active_session = None;
                                state.set_state(STATE_ERROR);
                            }
                        }
                    }
                    Command::StopSession => {
                        state.set_state(STATE_PROCESSING);

                        let Some(session) = active_session.take() else {
                            log::warn!("received stop request without an active Deepgram session");
                            state.set_state(STATE_IDLE);
                            continue;
                        };

                        match finish_session(&runtime, session) {
                            Ok(transcript) => {
                                if transcript.is_empty() {
                                    log::info!("Deepgram session completed without a final transcript");
                                    state.clear_overlay_text();
                                    state.set_state(STATE_IDLE);
                                    continue;
                                }

                                log::info!("final transcript: {}", transcript);
                                match write_clipboard_and_paste(&transcript) {
                                    Ok(()) => {
                                        state.clear_overlay_text();
                                        state.set_state(STATE_IDLE);
                                    }
                                    Err(error) => {
                                        log::error!("failed to copy/paste transcript: {}", error);
                                        state.clear_overlay_text();
                                        state.set_state(STATE_ERROR);
                                    }
                                }
                            }
                            Err(error) => {
                                log::error!("Deepgram session failed: {}", error);
                                state.clear_overlay_text();
                                state.set_state(STATE_ERROR);
                            }
                        }
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

fn start_session(
    runtime: &Runtime,
    state: Arc<AppState>,
    config: &DeepgramConfig,
    sample_rate: u32,
) -> Result<ActiveSession, String> {
    let (audio_tx, audio_rx) = tokio_mpsc::channel(AUDIO_QUEUE_CAPACITY);
    let deepgram_config = config.clone();

    let mut transcription_stream = runtime.block_on(async move {
        let client = Deepgram::new(&deepgram_config.api_key).map_err(format_deepgram_error)?;
        let options = Options::builder()
            .punctuate(true)
            .smart_format(true)
            .dictation(true)
            .query_params([
                ("model".to_owned(), deepgram_config.model.clone()),
                ("language".to_owned(), deepgram_config.language.clone()),
            ])
            .build();

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

    let task = runtime
        .spawn(async move { run_transcription_stream(&mut transcription_stream, state).await });

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

async fn run_transcription_stream(
    stream: &mut TranscriptionStream,
    state: Arc<AppState>,
) -> Result<String, String> {
    let mut interim_transcript = String::new();
    let mut transcript_parts: Vec<String> = Vec::new();
    let mut last_final_transcript = String::new();

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
                        state.set_overlay_text(build_overlay_text(&transcript_parts, None));
                    }
                    continue;
                }

                log::debug!("Deepgram interim: {}", transcript);
                interim_transcript = transcript;
                state.set_overlay_text(build_overlay_text(
                    &transcript_parts,
                    Some(interim_transcript.as_str()),
                ));
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
                return Err(format_deepgram_error(error));
            }
        }
    }

    let final_transcript = join_transcript_parts(&transcript_parts);
    state.set_overlay_text(final_transcript.clone());
    Ok(final_transcript)
}

fn extract_transcript(channel: &Channel) -> String {
    channel
        .alternatives
        .first()
        .map(|alternative| alternative.transcript.trim().to_owned())
        .unwrap_or_default()
}

fn build_overlay_text(transcript_parts: &[String], interim_transcript: Option<&str>) -> String {
    let mut segments: Vec<&str> = transcript_parts
        .iter()
        .map(|transcript| transcript.trim())
        .filter(|transcript| !transcript.is_empty())
        .collect();

    if let Some(interim_transcript) = interim_transcript {
        let trimmed_interim_transcript = interim_transcript.trim();
        if !trimmed_interim_transcript.is_empty() {
            segments.push(trimmed_interim_transcript);
        }
    }

    segments.join(" ")
}

fn join_transcript_parts(transcript_parts: &[String]) -> String {
    build_overlay_text(transcript_parts, None)
}

fn write_clipboard_and_paste(transcript: &str) -> Result<(), String> {
    write_clipboard_text(transcript)?;
    thread::sleep(Duration::from_millis(PASTEBOARD_SETTLE_DELAY_MS));
    send_paste_shortcut()
}

fn send_paste_shortcut() -> Result<(), String> {
    send_key_event(EventType::KeyPress(Key::MetaLeft))?;
    send_key_event(EventType::KeyPress(Key::KeyV))?;
    send_key_event(EventType::KeyRelease(Key::KeyV))?;
    send_key_event(EventType::KeyRelease(Key::MetaLeft))?;
    Ok(())
}

fn send_key_event(event: EventType) -> Result<(), String> {
    simulate(&event)
        .map_err(|SimulateError| format!("failed to synthesize input event: {:?}", event))?;
    thread::sleep(Duration::from_millis(KEY_EVENT_DELAY_MS));
    Ok(())
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

extern "C" fn perform_clipboard_write(context: *mut c_void) {
    let request = unsafe { &mut *(context as *mut ClipboardWriteRequest) };
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();

    let ns_text = NSString::from_str(&request.text);
    request.success = pasteboard.setString_forType(&ns_text, unsafe { NSPasteboardTypeString });
}

fn write_clipboard_text(transcript: &str) -> Result<(), String> {
    let mut request = Box::new(ClipboardWriteRequest {
        text: transcript.to_owned(),
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
