use bytes::Bytes;
use deepgram::common::options::{Encoding, Endpointing, Options};
use deepgram::common::stream_response::{Channel, StreamResponse};
use deepgram::listen::websocket::TranscriptionStream;
use deepgram::Deepgram;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self as tokio_mpsc, Sender as TokioSender};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::config::DeepgramConfig;
use crate::state::{AppState, DeepgramConnectionStatus};
use super::text_builder::{build_overlay_text, join_transcript_parts};

pub const AUDIO_QUEUE_CAPACITY: usize = 512;

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
    stream: TranscriptionStream,
    kind: SessionKind,
    recording_prefix: String,
}

impl ActiveSession {
    #[allow(dead_code)]
    pub fn kind(&self) -> SessionKind {
        self.kind
    }

    pub fn push_audio(&self, pcm_data: Bytes) -> PushAudioResult {
        match self.audio_tx.try_send(Ok(pcm_data)) {
            Ok(()) => PushAudioResult::Ok,
            Err(tokio_mpsc::error::TrySendError::Full(_)) => PushAudioResult::Full,
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => PushAudioResult::Closed,
        }
    }

    pub fn finish(self, runtime: &Runtime, state: Arc<AppState>) -> Result<String, String> {
        drop(self.audio_tx);
        let mut stream = self.stream;
        let recording_prefix = self.recording_prefix;
        let kind = self.kind;

        runtime.block_on(run_transcription_stream(
            &mut stream,
            state,
            kind,
            recording_prefix,
        ))
    }
}

pub fn start_session(
    runtime: &Runtime,
    _state: Arc<AppState>,
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
        let mut options_builder = Options::builder()
            .punctuate(true)
            .smart_format(true)
            .dictation(true)
            .query_params([
                ("model".to_owned(), deepgram_config.model.clone()),
                ("language".to_owned(), deepgram_config.language.clone()),
            ]);

        let keyterm_refs: Vec<&str> = deepgram_config
            .keyterms
            .iter()
            .map(String::as_str)
            .collect();
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
        "Deepgram session started (request_id={}, sample_rate={}Hz, model={}, language={}, kind={:?})",
        transcription_stream.request_id(),
        sample_rate,
        config.model,
        config.language,
        session_kind
    );

    Ok(ActiveSession {
        audio_tx,
        stream: transcription_stream,
        kind: session_kind,
        recording_prefix,
    })
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
                            let new_text = build_overlay_text(
                                recording_prefix.as_str(),
                                &transcript_parts,
                                None,
                            );
                            last_pushed_text = new_text.clone();
                            set_session_overlay_text(&state, session_kind, new_text);
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
                    set_session_overlay_text(&state, session_kind, new_text);
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

    let final_ui_text = session_overlay_text(&state, session_kind);
    let final_transcript = if final_ui_text != last_pushed_text {
        final_ui_text
    } else {
        join_transcript_parts(recording_prefix.as_str(), &transcript_parts)
    };

    if !state.is_abort_requested() {
        set_session_overlay_text(&state, session_kind, final_transcript.clone());
    }
    Ok(final_transcript)
}

pub fn session_overlay_text(state: &AppState, session_kind: SessionKind) -> String {
    match session_kind {
        SessionKind::Dictation => state.overlay_text().to_string(),
        SessionKind::Correction => state.overlay_correction_text().to_string(),
    }
}

pub fn set_session_overlay_text(state: &AppState, session_kind: SessionKind, text: impl Into<String>) {
    match session_kind {
        SessionKind::Dictation => state.set_overlay_text(text),
        SessionKind::Correction => state.set_overlay_correction_text(text),
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
}
