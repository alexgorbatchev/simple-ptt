use bytes::Bytes;
use deepgram::common::options::{Encoding, Endpointing, Options};
use deepgram::common::stream_response::{Channel, StreamResponse};
use deepgram::listen::websocket::WebsocketBuilder;
use deepgram::Deepgram;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self as tokio_mpsc, Sender as TokioSender};
use tokio_stream::wrappers::ReceiverStream;

pub use super::progress::SessionError;
use super::progress::{self, SessionLimits, SessionProgress, WaitingIndicator};
use super::text_builder::{build_overlay_text, join_transcript_parts};
use crate::config::DeepgramConfig;
use crate::state::AppState;

pub const AUDIO_QUEUE_CAPACITY: usize = 512;
/// How long a chunk waits for room in a full audio queue before it is
/// dropped and the session counts as stalled (see `push_audio_within`).
/// Audio held while a correction or transformation was applied arrives all at
/// once when dictation resumes, faster than the session sends it on; waiting
/// keeps it, and the audio thread never waits because it hands audio over
/// through an unbounded channel.
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
    audio_tx: Option<TokioSender<Result<Bytes, std::io::Error>>>,
    kind: SessionKind,
    task: tokio::task::JoinHandle<Result<String, SessionError>>,
    progress: Arc<SessionProgress>,
    // Keep all transport work within the session's lifetime, including tasks
    // spawned independently of the transcription reader.
    runtime: Runtime,
    /// The queue stayed full through a whole wait: chunks are dropped
    /// without waiting until it has room again.
    stalled: std::cell::Cell<bool>,
}

impl ActiveSession {
    pub fn kind(&self) -> SessionKind {
        self.kind
    }

    /// Queues `pcm_data` for the session, waiting up to `AUDIO_QUEUE_WAIT`
    /// for room when the queue is full.
    pub fn push_audio(&self, pcm_data: Bytes) -> PushAudioResult {
        self.push_audio_within(pcm_data, AUDIO_QUEUE_WAIT)
    }

    /// Queues `pcm_data`, waiting up to `wait` for room in a full queue. Once
    /// a wait runs out the session is stalled, and chunks are dropped at once
    /// until the queue takes one again, so a stall holds the worker (and the
    /// commands queued behind the audio) up for one wait, not one per chunk.
    fn push_audio_within(&self, pcm_data: Bytes, wait: std::time::Duration) -> PushAudioResult {
        let audio_tx = self
            .audio_tx
            .as_ref()
            .expect("a recording session owns its audio sender");
        let queued_pcm = pcm_data.clone();
        let pcm_data = match audio_tx.try_send(Ok(pcm_data)) {
            Ok(()) => {
                self.stalled.set(false);
                self.progress.audio_queued(&queued_pcm);
                return PushAudioResult::Ok;
            }
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => return PushAudioResult::Closed,
            Err(tokio_mpsc::error::TrySendError::Full(_)) if self.stalled.get() => {
                return PushAudioResult::Full
            }
            Err(tokio_mpsc::error::TrySendError::Full(pcm_data)) => pcm_data,
        };
        // Built inside the runtime: the timeout needs its timer.
        match self
            .runtime
            .block_on(async { tokio::time::timeout(wait, audio_tx.send(pcm_data)).await })
        {
            Ok(Ok(())) => {
                self.progress.audio_queued(&queued_pcm);
                PushAudioResult::Ok
            }
            Ok(Err(_)) => PushAudioResult::Closed,
            Err(_) => {
                self.stalled.set(true);
                PushAudioResult::Full
            }
        }
    }

    pub fn finish(self, state: &AppState) -> Result<String, SessionError> {
        let wait = self.progress.limits.finish;
        self.finish_within(state, wait)
    }

    fn finish_within(
        mut self,
        state: &AppState,
        wait: std::time::Duration,
    ) -> Result<String, SessionError> {
        self.progress.finish();
        drop(self.audio_tx.take());
        self.runtime.block_on(async {
            let result = tokio::select! {
                biased;
                _ = state.wait_for_abort() => Err(SessionError::Cancelled),
                _ = tokio::time::sleep(wait) => Err(SessionError::Failed("Deepgram timed out while finishing transcription. Please try recording again.".to_owned())),
                result = &mut self.task => return result.map_err(|error| SessionError::Failed(format!("transcription task join error: {error}")))?,
            };
            // Await after abort: dropping a JoinHandle alone detaches its task.
            self.task.abort();
            let _ = (&mut self.task).await;
            state.set_deepgram_waiting(false);
            result
        })
    }
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn session_runtime() -> Result<Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("deepgram-session")
        .enable_all()
        .build()
        .map_err(|error| format!("failed to create Deepgram session runtime: {error}"))
}

pub fn start_session(
    state: Arc<AppState>,
    config: &DeepgramConfig,
    sample_rate: u32,
    session_kind: SessionKind,
    recording_prefix: String,
) -> Result<ActiveSession, SessionError> {
    if let Ok(simulated_error) = std::env::var("SIMPLE_PTT_SIMULATE_ERROR") {
        return Err(SessionError::Failed(format!(
            "simulated error: {}",
            simulated_error
        )));
    }

    let (audio_tx, audio_rx) = tokio_mpsc::channel(AUDIO_QUEUE_CAPACITY);
    let deepgram_config = config.clone();
    let runtime = session_runtime().map_err(SessionError::Failed)?;
    let limits = SessionLimits::default();
    let progress = SessionProgress::new(sample_rate, limits);

    let transcription_stream =
        runtime.block_on(progress::connect(state.clone(), limits, async move {
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
        }))?;

    log::info!(
        "Deepgram session started (request_id={}, sample_rate={}Hz, model={}, language={}, kind={:?})",
        transcription_stream.request_id(),
        sample_rate,
        config.model,
        config.language,
        session_kind
    );

    let task_progress = progress.clone();
    let task = runtime.spawn(async move {
        let mut stream = transcription_stream;
        let result = run_transcription_stream(
            &mut stream,
            state.clone(),
            session_kind,
            recording_prefix,
            task_progress,
        )
        .await;
        if let Err(SessionError::Failed(error)) = &result {
            if !state.is_abort_requested() {
                state.report_error(error.clone());
            }
        }
        result
    });

    Ok(ActiveSession {
        audio_tx: Some(audio_tx),
        kind: session_kind,
        task,
        progress,
        runtime,
        stalled: std::cell::Cell::new(false),
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

pub async fn run_transcription_stream<S>(
    stream: &mut S,
    state: Arc<AppState>,
    session_kind: SessionKind,
    mut recording_prefix: String,
    progress: Arc<SessionProgress>,
) -> Result<String, SessionError>
where
    S: tokio_stream::Stream<Item = Result<StreamResponse, deepgram::DeepgramError>> + Unpin,
{
    let _indicator = WaitingIndicator(state.clone());
    log::debug!("running transcription stream for {:?}", session_kind);
    let mut interim_transcript = String::new();
    let mut transcript_parts: Vec<String> = Vec::new();
    let mut last_final_transcript = String::new();
    let mut last_pushed_text = session_overlay_text(&state, session_kind);

    loop {
        let message = progress.next(stream, &state).await?
            .ok_or_else(|| SessionError::Failed("Deepgram disconnected before completing transcription. Please try recording again.".to_owned()))?;
        match message {
            Ok(StreamResponse::TranscriptResponse {
                is_final,
                channel,
                from_finalize,
                start,
                duration,
                ..
            }) => {
                progress.transcript_received(start + duration);
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
                if !progress.is_finishing() {
                    return Err(SessionError::Failed(
                        "Deepgram closed the recording unexpectedly. Please try recording again."
                            .to_owned(),
                    ));
                }
                break;
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
                return Err(SessionError::Failed(format_deepgram_error(error)));
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
        SessionKind::Correction => state.set_live_overlay_correction_text(text, provisional_start),
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
    fn one_chunk_session() -> (
        ActiveSession,
        tokio_mpsc::Receiver<Result<Bytes, std::io::Error>>,
    ) {
        let runtime = session_runtime().unwrap();
        let (audio_tx, audio_rx) = tokio_mpsc::channel(1);
        let task = runtime.spawn(async { Ok(String::new()) });
        (
            ActiveSession {
                audio_tx: Some(audio_tx),
                kind: SessionKind::Dictation,
                task,
                progress: SessionProgress::new(16000, SessionLimits::default()),
                runtime,
                stalled: std::cell::Cell::new(false),
            },
            audio_rx,
        )
    }

    #[test]
    fn finishing_a_session_observes_an_abort_while_waiting() {
        let (mut session, _audio_rx) = one_chunk_session();
        session.task = session.runtime.spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok("must not be pasted".to_owned())
        });
        let state = AppState::new();
        let abort_state = state.clone();
        session.runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            abort_state.request_abort();
        });
        let started = std::time::Instant::now();
        assert_eq!(session.finish(&state), Err(SessionError::Cancelled));
        assert!(started.elapsed() < std::time::Duration::from_millis(150));
        assert!(
            state.is_abort_requested(),
            "the worker must consume the abort"
        );
    }

    #[test]
    fn finishing_a_stalled_session_times_out_and_drops_all_session_tasks() {
        let (mut session, audio_rx) = one_chunk_session();
        session.task = session.runtime.spawn(async move {
            let _audio_rx = audio_rx;
            std::future::pending().await
        });
        let (transport_tx, transport_rx) = std::sync::mpsc::channel::<()>();
        session.runtime.spawn(async move {
            let _transport_tx = transport_tx;
            std::future::pending::<()>().await;
        });
        let task_abort = session.task.abort_handle();
        let state = AppState::new();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let finishing = std::thread::spawn(move || {
            let result = session.finish_within(&state, std::time::Duration::from_millis(30));
            result_tx.send(result).unwrap();
        });
        let result = result_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("stalled session shutdown never returned");
        finishing.join().unwrap();
        assert!(
            matches!(result, Err(SessionError::Failed(message)) if message.contains("timed out"))
        );
        assert!(task_abort.is_finished());
        assert_eq!(
            transport_rx.recv_timeout(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn finishing_a_session_drains_audio_and_preserves_the_final_transcript() {
        let (mut session, mut audio_rx) = one_chunk_session();
        session.task = session.runtime.spawn(async move {
            assert_eq!(
                audio_rx.recv().await.unwrap().unwrap(),
                Bytes::from_static(b"last audio")
            );
            assert!(audio_rx.recv().await.is_none());
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            Ok("final words".to_owned())
        });
        assert_eq!(
            session.push_audio(Bytes::from_static(b"last audio")),
            PushAudioResult::Ok
        );
        assert_eq!(
            session.finish(&AppState::new()),
            Ok("final words".to_owned())
        );
    }

    #[test]
    fn aborting_a_session_drops_detached_transport_tasks() {
        let (mut session, audio_rx) = one_chunk_session();
        session.task = session.runtime.spawn(async move {
            let _audio_rx = audio_rx;
            std::future::pending().await
        });
        let (transport_tx, transport_rx) = std::sync::mpsc::channel::<()>();
        session.runtime.spawn(async move {
            let _transport_tx = transport_tx;
            std::future::pending::<()>().await;
        });
        let state = AppState::new();
        state.request_abort();
        assert_eq!(session.finish(&state), Err(SessionError::Cancelled));
        assert_eq!(
            transport_rx.recv_timeout(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn terminal_metadata_finishes_without_waiting_for_transport_eof() {
        use tokio_stream::StreamExt;
        let runtime = session_runtime().unwrap();
        for kind in [SessionKind::Dictation, SessionKind::Correction] {
            let state = AppState::new();
            let final_message = serde_json::from_value::<StreamResponse>(serde_json::json!({
                "type": "Results", "start": 0.0, "duration": 1.0,
                "is_final": true, "speech_final": true, "from_finalize": true,
                "channel_index": [0, 1],
                "channel": {"alternatives": [{"transcript": "final words", "confidence": 1.0, "words": []}]},
                "metadata": {"request_id": "test-request", "model_uuid": "test-model", "model_info": {"name": "test", "version": "test", "arch": "test"}}
            })).unwrap();
            let terminal = StreamResponse::TerminalResponse {
                request_id: "test-request".to_owned(),
                created: String::new(),
                duration: 1.0,
                channels: 1,
            };
            let mut stream = tokio_stream::iter([Ok(final_message), Ok(terminal)])
                .chain(tokio_stream::pending());
            let progress = SessionProgress::new(16000, SessionLimits::default());
            progress.finish();
            let result = runtime.block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    run_transcription_stream(
                        &mut stream,
                        state.clone(),
                        kind,
                        "prefix ".to_owned(),
                        progress,
                    ),
                )
                .await
            });
            assert_eq!(result.unwrap().unwrap(), "prefix final words ");
            assert_eq!(session_overlay_text(&state, kind), "prefix final words ");
        }
    }

    #[test]
    fn escape_interrupts_finishing_without_waiting_for_another_response() {
        let runtime = session_runtime().unwrap();
        let (audio_tx, _audio_rx) = tokio_mpsc::channel(1);
        let task = runtime.spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok("must not be pasted".to_owned())
        });
        let session = ActiveSession {
            audio_tx: Some(audio_tx),
            kind: SessionKind::Dictation,
            task,
            progress: SessionProgress::new(16000, SessionLimits::default()),
            runtime,
            stalled: std::cell::Cell::new(false),
        };
        let state = AppState::new();
        let abort_state = state.clone();
        session.runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            abort_state.request_abort();
        });

        let started = std::time::Instant::now();
        assert!(
            session.finish(&state).is_err(),
            "Escape must cancel the finishing session"
        );
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert!(
            state.is_abort_requested(),
            "the worker must consume the cancellation before starting another recording"
        );
    }

    #[tokio::test]
    async fn terminal_metadata_completes_finishing_without_waiting_for_socket_eof() {
        use deepgram::common::stream_response::{Alternatives, Metadata, ModelInfo};
        let state = AppState::new();
        let progress = SessionProgress::new(16000, SessionLimits::default());
        progress.finish();
        let (tx, rx) = tokio_mpsc::channel(2);
        tx.send(Ok(StreamResponse::TranscriptResponse {
            type_field: "Results".to_owned(),
            start: 0.0,
            duration: 1.0,
            is_final: true,
            speech_final: true,
            from_finalize: true,
            channel: Channel {
                alternatives: vec![Alternatives {
                    transcript: "hello world".to_owned(),
                    words: vec![],
                    confidence: 1.0,
                    languages: vec![],
                }],
            },
            metadata: Metadata {
                request_id: "test".to_owned(),
                model_info: ModelInfo {
                    name: "test-model".to_owned(),
                    version: "test".to_owned(),
                    arch: "test".to_owned(),
                },
                model_uuid: "test".to_owned(),
            },
            channel_index: vec![0, 1],
        }))
        .await
        .unwrap();
        tx.send(Ok(StreamResponse::TerminalResponse {
            request_id: "test".to_owned(),
            created: "test".to_owned(),
            duration: 1.0,
            channels: 1,
        }))
        .await
        .unwrap();
        // Keep the sender alive: EOF has not arrived and may never arrive.
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            run_transcription_stream(
                &mut ReceiverStream::new(rx),
                state.clone(),
                SessionKind::Dictation,
                String::new(),
                progress,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, "hello world ");
        assert_eq!(&*state.overlay_text(), "hello world ");
        assert!(!state.is_deepgram_waiting());
    }

    #[tokio::test]
    async fn a_disconnect_without_terminal_metadata_is_an_error() {
        let state = AppState::new();
        state.set_overlay_text("partial transcript");
        let progress = SessionProgress::new(16000, SessionLimits::default());
        progress.finish();
        let mut stream = tokio_stream::empty::<Result<StreamResponse, deepgram::DeepgramError>>();
        assert!(
            matches!(run_transcription_stream(&mut stream, state.clone(), SessionKind::Dictation, String::new(), progress).await, Err(SessionError::Failed(message)) if message.contains("disconnected"))
        );
        assert_eq!(&*state.overlay_text(), "partial transcript");
    }

    struct TaskResource(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for TaskResource {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }

    fn pending_session() -> (ActiveSession, tokio::sync::oneshot::Receiver<()>) {
        let runtime = session_runtime().unwrap();
        let (audio_tx, _audio_rx) = tokio_mpsc::channel(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let task = runtime.spawn(async move {
            let _resource = TaskResource(Some(dropped_tx));
            started_tx.send(()).unwrap();
            std::future::pending().await
        });
        runtime.block_on(started_rx).unwrap();
        let progress = SessionProgress::new(
            16000,
            SessionLimits {
                finish: std::time::Duration::from_millis(30),
                ..SessionLimits::default()
            },
        );
        (
            ActiveSession {
                audio_tx: Some(audio_tx),
                kind: SessionKind::Dictation,
                task,
                progress,
                runtime,
                stalled: std::cell::Cell::new(false),
            },
            dropped_rx,
        )
    }

    #[test]
    fn a_finishing_deadline_cancels_and_joins_the_transcription_task() {
        let (session, mut dropped) = pending_session();
        assert!(
            matches!(session.finish(&AppState::new()), Err(SessionError::Failed(message)) if message.contains("timed out"))
        );
        assert!(
            dropped.try_recv().is_ok(),
            "the task's resources must be released before finish returns"
        );
    }

    #[test]
    fn dropping_a_session_cancels_its_quiet_transcription_task() {
        let runtime = Runtime::new().unwrap();
        let (session, dropped) = pending_session();
        drop(session);
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(1), dropped)
                .await
                .unwrap()
                .unwrap();
        });
    }

    #[test]
    fn a_full_audio_queue_waits_for_room_instead_of_dropping() {
        let runtime = Runtime::new().unwrap();
        let (session, mut audio_rx) = one_chunk_session();
        assert_eq!(
            session.push_audio(Bytes::from_static(b"first")),
            PushAudioResult::Ok
        );
        // The queue is full until the receiver takes a chunk 50 ms later.
        let received = runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let first = audio_rx.recv().await;
            let second = audio_rx.recv().await;
            (first, second)
        });

        assert_eq!(
            session.push_audio(Bytes::from_static(b"second")),
            PushAudioResult::Ok
        );
        drop(session);
        let (first, second) = runtime.block_on(received).unwrap();
        assert_eq!(first.unwrap().unwrap(), Bytes::from_static(b"first"));
        assert_eq!(second.unwrap().unwrap(), Bytes::from_static(b"second"));
    }

    #[test]
    fn a_queue_that_stays_full_drops_the_chunk_after_waiting() {
        let (session, _audio_rx) = one_chunk_session();
        assert_eq!(
            session.push_audio(Bytes::from_static(b"first")),
            PushAudioResult::Ok
        );
        let started = std::time::Instant::now();
        assert_eq!(
            session.push_audio_within(
                Bytes::from_static(b"second"),
                std::time::Duration::from_millis(30)
            ),
            PushAudioResult::Full
        );
        assert!(started.elapsed() >= std::time::Duration::from_millis(30));
    }

    #[test]
    fn a_stalled_queue_drops_at_once_until_it_has_room_again() {
        let (session, mut audio_rx) = one_chunk_session();
        let wait = std::time::Duration::from_millis(40);
        assert_eq!(
            session.push_audio_within(Bytes::from_static(b"1"), wait),
            PushAudioResult::Ok
        );

        // The first chunk to meet the full queue waits, then is dropped.
        let started = std::time::Instant::now();
        assert_eq!(
            session.push_audio_within(Bytes::from_static(b"2"), wait),
            PushAudioResult::Full
        );
        assert!(started.elapsed() >= wait);

        // The session is stalled: the next chunks are dropped without waiting,
        // so commands behind them are not held up a wait each.
        let started = std::time::Instant::now();
        assert_eq!(
            session.push_audio_within(Bytes::from_static(b"3"), wait),
            PushAudioResult::Full
        );
        assert!(started.elapsed() < wait / 2, "{:?}", started.elapsed());

        // Once the queue has room, chunks go in and a full queue is waited on
        // again.
        assert!(audio_rx.try_recv().is_ok());
        assert_eq!(
            session.push_audio_within(Bytes::from_static(b"4"), wait),
            PushAudioResult::Ok
        );
        let started = std::time::Instant::now();
        assert_eq!(
            session.push_audio_within(Bytes::from_static(b"5"), wait),
            PushAudioResult::Full
        );
        assert!(started.elapsed() >= wait);
    }

    #[test]
    fn a_closed_audio_queue_reports_closed() {
        let (session, audio_rx) = one_chunk_session();
        drop(audio_rx);
        assert_eq!(
            session.push_audio(Bytes::from_static(b"chunk")),
            PushAudioResult::Closed
        );
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
