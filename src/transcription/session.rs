use bytes::Bytes;
use deepgram::common::options::{Encoding, Endpointing, Options};
use deepgram::common::stream_response::{Channel, StreamResponse};
use deepgram::listen::websocket::{TranscriptionStream, WebsocketBuilder};
use deepgram::Deepgram;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self as tokio_mpsc, Sender as TokioSender};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::config::DeepgramConfig;
use crate::state::AppState;
use super::text_builder::{build_overlay_text, join_transcript_parts};

pub const AUDIO_QUEUE_CAPACITY: usize = 512;
/// How long a chunk waits for room in a full audio queue before it is
/// dropped. Audio held while a correction was applied arrives all at once
/// when dictation resumes, faster than the session sends it on; waiting keeps
/// it, and the audio thread never waits because it hands audio over through
/// an unbounded channel.
const AUDIO_QUEUE_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushAudioResult {
    Ok,
    Full,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionKind {
    Dictation,
    Correction,
}

pub struct ActiveSession {
    audio_tx: TokioSender<Result<Bytes, std::io::Error>>,
    kind: SessionKind,
    task: tokio::task::JoinHandle<Result<String, String>>,
}

impl ActiveSession {
    pub fn kind(&self) -> SessionKind {
        self.kind
    }

    /// Queues `pcm_data` for the session, waiting up to `AUDIO_QUEUE_WAIT`
    /// for room when the queue is full.
    pub fn push_audio(&self, runtime: &Runtime, pcm_data: Bytes) -> PushAudioResult {
        self.push_audio_within(runtime, pcm_data, AUDIO_QUEUE_WAIT)
    }

    fn push_audio_within(&self, runtime: &Runtime, pcm_data: Bytes, wait: std::time::Duration) -> PushAudioResult {
        let pcm_data = match self.audio_tx.try_send(Ok(pcm_data)) {
            Ok(()) => return PushAudioResult::Ok,
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => return PushAudioResult::Closed,
            Err(tokio_mpsc::error::TrySendError::Full(pcm_data)) => pcm_data,
        };
        // Built inside the runtime: the timeout needs its timer.
        match runtime.block_on(async { tokio::time::timeout(wait, self.audio_tx.send(pcm_data)).await }) {
            Ok(Ok(())) => PushAudioResult::Ok,
            Ok(Err(_)) => PushAudioResult::Closed,
            Err(_) => PushAudioResult::Full,
        }
    }

    pub fn finish(self, runtime: &Runtime, _state: Arc<AppState>) -> Result<String, String> {
        drop(self.audio_tx);
        runtime
            .block_on(self.task)
            .map_err(|error| format!("transcription task join error: {}", error))?
    }
}

pub fn start_session(
    runtime: &Runtime,
    state: Arc<AppState>,
    config: &DeepgramConfig,
    sample_rate: u32,
    session_kind: SessionKind,
    recording_prefix: String,
) -> Result<ActiveSession, String> {
    if let Ok(simulated_error) = std::env::var("SIMPLE_PTT_SIMULATE_ERROR") {
        return Err(format!("simulated error: {}", simulated_error));
    }

    let (audio_tx, audio_rx) = tokio_mpsc::channel(AUDIO_QUEUE_CAPACITY);
    let deepgram_config = config.clone();

    let transcription_stream = runtime.block_on(async move {
        let client = Deepgram::new(deepgram_config.api_key.as_deref().unwrap_or(""))
            .map_err(format_deepgram_error)?;
        let transcription = client.transcription();

        configure_stream_request(
            transcription.stream_request_with_options(stream_options(&deepgram_config)),
            &deepgram_config,
            sample_rate,
        )
        .stream(ReceiverStream::new(audio_rx))
        .await
        .map_err(format_deepgram_error)
    })?;

    log::info!(
        "Deepgram session started (request_id={}, sample_rate={}Hz, model={}, language={}, kind={:?})",
        transcription_stream.request_id(),
        sample_rate,
        config.model,
        config.language,
        session_kind
    );

    let task = runtime.spawn(async move {
        let mut stream = transcription_stream;
        run_transcription_stream(&mut stream, state, session_kind, recording_prefix).await
    });

    Ok(ActiveSession {
        audio_tx,
        kind: session_kind,
        task,
    })
}

fn stream_options(config: &DeepgramConfig) -> Options {
    let mut options_builder = Options::builder()
        .punctuate(true)
        .smart_format(true)
        .dictation(true)
        .query_params([
            ("model".to_owned(), config.model.clone()),
            ("language".to_owned(), config.language.clone()),
        ]);

    let keyterm_refs: Vec<&str> = config.keyterms.iter().map(String::as_str).collect();
    if !keyterm_refs.is_empty() {
        options_builder = options_builder.keyterms(keyterm_refs);
    }

    options_builder.build()
}

fn configure_stream_request<'a>(
    builder: WebsocketBuilder<'a>,
    config: &DeepgramConfig,
    sample_rate: u32,
) -> WebsocketBuilder<'a> {
    builder
        .encoding(Encoding::Linear16)
        .sample_rate(sample_rate)
        .channels(1)
        .endpointing(Endpointing::CustomDurationMs(u32::from(
            config.endpointing_ms,
        )))
        .utterance_end_ms(config.utterance_end_ms)
        .interim_results(true)
        .vad_events(true)
        .keep_alive()
}

pub async fn run_transcription_stream(
    stream: &mut TranscriptionStream,
    state: Arc<AppState>,
    session_kind: SessionKind,
    mut recording_prefix: String,
) -> Result<String, String> {
    log::debug!("running transcription stream for {:?}", session_kind);
    let mut interim_transcript = String::new();
    let mut transcript_parts: Vec<String> = Vec::new();
    let mut last_final_transcript = String::new();
    let mut last_pushed_text = session_overlay_text(&state, session_kind);

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

                let current_ui_text = session_overlay_text(&state, session_kind);
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
                            let live_text = build_overlay_text(
                                recording_prefix.as_str(),
                                &transcript_parts,
                                None,
                            );
                            last_pushed_text = live_text.text.clone();
                            set_session_overlay_text(
                                &state,
                                session_kind,
                                live_text.text,
                                live_text.provisional_start,
                            );
                        }
                    }
                    continue;
                }

                log::debug!("Deepgram interim: {}", transcript);
                interim_transcript = transcript;
                if !state.is_abort_requested() {
                    let live_text = build_overlay_text(
                        recording_prefix.as_str(),
                        &transcript_parts,
                        Some(interim_transcript.as_str()),
                    );
                    last_pushed_text = live_text.text.clone();
                    set_session_overlay_text(
                        &state,
                        session_kind,
                        live_text.text,
                        live_text.provisional_start,
                    );
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
                return Err(format_deepgram_error(error));
            }
        }
    }

    let final_ui_text = session_overlay_text(&state, session_kind);
    let final_transcript = if final_ui_text != last_pushed_text {
        final_ui_text
    } else {
        join_transcript_parts(recording_prefix.as_str(), &transcript_parts)
    };

    if !state.is_abort_requested() {
        set_session_overlay_text(&state, session_kind, final_transcript.clone(), None);
    }
    Ok(final_transcript)
}

pub fn session_overlay_text(state: &AppState, session_kind: SessionKind) -> String {
    match session_kind {
        SessionKind::Dictation => state.overlay_text().to_string(),
        SessionKind::Correction => state.overlay_correction_text().to_string(),
    }
}

pub fn set_session_overlay_text(
    state: &AppState,
    session_kind: SessionKind,
    text: impl Into<String>,
    provisional_start: Option<usize>,
) {
    match session_kind {
        SessionKind::Dictation => state.set_live_overlay_text(text, provisional_start),
        SessionKind::Correction => {
            state.set_live_overlay_correction_text(text, provisional_start)
        }
    }
}

pub fn extract_transcript(channel: &Channel) -> String {
    channel
        .alternatives
        .first()
        .map(|alternative| alternative.transcript.trim().to_owned())
        .unwrap_or_default()
}

pub fn format_deepgram_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_queue_capacity_provides_ample_buffer_depth() {
        assert!(AUDIO_QUEUE_CAPACITY >= 256);
    }

    /// A session whose audio queue holds one chunk, and its receiving end.
    fn one_chunk_session(runtime: &Runtime) -> (ActiveSession, tokio_mpsc::Receiver<Result<Bytes, std::io::Error>>) {
        let (audio_tx, audio_rx) = tokio_mpsc::channel(1);
        let task = runtime.spawn(async { Ok(String::new()) });
        (ActiveSession { audio_tx, kind: SessionKind::Dictation, task }, audio_rx)
    }

    #[test]
    fn a_full_audio_queue_waits_for_room_instead_of_dropping() {
        let runtime = Runtime::new().unwrap();
        let (session, mut audio_rx) = one_chunk_session(&runtime);
        assert_eq!(session.push_audio(&runtime, Bytes::from_static(b"first")), PushAudioResult::Ok);
        // The queue is full until the receiver takes a chunk 50 ms later.
        let received = runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let first = audio_rx.recv().await;
            let second = audio_rx.recv().await;
            (first, second)
        });

        assert_eq!(session.push_audio(&runtime, Bytes::from_static(b"second")), PushAudioResult::Ok);
        drop(session);
        let (first, second) = runtime.block_on(received).unwrap();
        assert_eq!(first.unwrap().unwrap(), Bytes::from_static(b"first"));
        assert_eq!(second.unwrap().unwrap(), Bytes::from_static(b"second"));
    }

    #[test]
    fn a_queue_that_stays_full_drops_the_chunk_after_waiting() {
        let runtime = Runtime::new().unwrap();
        let (session, _audio_rx) = one_chunk_session(&runtime);
        assert_eq!(session.push_audio(&runtime, Bytes::from_static(b"first")), PushAudioResult::Ok);
        let started = std::time::Instant::now();
        assert_eq!(
            session.push_audio_within(&runtime, Bytes::from_static(b"second"), std::time::Duration::from_millis(30)),
            PushAudioResult::Full
        );
        assert!(started.elapsed() >= std::time::Duration::from_millis(30));
    }

    #[test]
    fn a_closed_audio_queue_reports_closed() {
        let runtime = Runtime::new().unwrap();
        let (session, audio_rx) = one_chunk_session(&runtime);
        drop(audio_rx);
        assert_eq!(session.push_audio(&runtime, Bytes::from_static(b"chunk")), PushAudioResult::Closed);
    }

    #[test]
    fn stream_request_sends_configured_live_transcription_options() {
        let config = DeepgramConfig {
            keyterms: vec!["macOS".to_owned(), "GitHub".to_owned()],
            api_key: Some("test-key".to_owned()),
            language: "en-US".to_owned(),
            model: "nova-3".to_owned(),
            endpointing_ms: 300,
            utterance_end_ms: 1000,
        };
        let client = Deepgram::new("test-key").unwrap();
        let transcription = client.transcription();

        let query = configure_stream_request(
            transcription.stream_request_with_options(stream_options(&config)),
            &config,
            16000,
        )
        .urlencoded()
        .unwrap();

        let mut pairs: Vec<&str> = query.split('&').collect();
        pairs.sort_unstable();
        let mut expected = vec![
            "punctuate=true",
            "smart_format=true",
            "dictation=true",
            "model=nova-3",
            "language=en-US",
            "keyterm=macOS",
            "keyterm=GitHub",
            "encoding=linear16",
            "sample_rate=16000",
            "channels=1",
            "endpointing=300",
            "utterance_end_ms=1000",
            "interim_results=true",
            "vad_events=true",
        ];
        expected.sort_unstable();
        assert_eq!(pairs, expected, "unexpected query: {}", query);
    }
}
