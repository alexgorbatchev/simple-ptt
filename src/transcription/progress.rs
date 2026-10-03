//! Deadlines and progress for one Deepgram connection. Silence does not
//! start a live-response deadline; finishing has its own absolute deadline.

use std::{
    fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};

use crate::state::AppState;

#[derive(Debug, PartialEq, Eq)]
pub enum SessionError {
    Cancelled,
    Failed(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Transcription cancelled"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

#[derive(Clone, Copy)]
pub struct SessionLimits {
    pub notice: Duration,
    pub connect: Duration,
    pub finish: Duration,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            notice: Duration::from_secs(5),
            connect: Duration::from_secs(15),
            finish: Duration::from_secs(15),
        }
    }
}

#[derive(Default)]
struct Progress {
    audio_seconds: f64,
    audible_seconds: f64,
    processed_seconds: f64,
    pending_since: Option<Instant>,
    finishing_since: Option<Instant>,
}

pub struct SessionProgress {
    progress: Mutex<Progress>,
    changed: Notify,
    sample_rate: u32,
    pub limits: SessionLimits,
}

impl SessionProgress {
    pub fn new(sample_rate: u32, limits: SessionLimits) -> Arc<Self> {
        Arc::new(Self {
            progress: Mutex::new(Progress::default()),
            changed: Notify::new(),
            sample_rate,
            limits,
        })
    }

    pub fn audio_queued(&self, pcm: &[u8]) {
        let samples = pcm.len() / 2;
        if samples == 0 {
            return;
        }
        let power = pcm
            .chunks_exact(2)
            .map(|bytes| {
                let sample = i16::from_le_bytes([bytes[0], bytes[1]]) as f64 / 32768.0;
                sample * sample
            })
            .sum::<f64>()
            / samples as f64;
        // Use the microphone meter's existing floor, rather than treating
        // every silence packet as speech awaiting a transcript.
        let audible = power > 0.0
            && crate::audio::normalize_meter_amplitude((10.0 * power.log10()) as f32) > 0.0;
        let mut progress = self.progress.lock().unwrap();
        progress.audio_seconds += samples as f64 / self.sample_rate as f64;
        if audible {
            progress.audible_seconds = progress.audio_seconds;
            if progress.audible_seconds > progress.processed_seconds
                && progress.pending_since.is_none()
            {
                progress.pending_since = Some(Instant::now());
                self.changed.notify_waiters();
            }
        }
    }

    pub fn transcript_received(&self, audio_end: f64) {
        let mut progress = self.progress.lock().unwrap();
        progress.processed_seconds = progress.processed_seconds.max(audio_end);
        progress.pending_since = if progress.audible_seconds > progress.processed_seconds {
            Some(Instant::now())
        } else {
            None
        };
        self.changed.notify_waiters();
    }

    pub fn finish(&self) {
        self.progress.lock().unwrap().finishing_since = Some(Instant::now());
        self.changed.notify_waiters();
    }

    pub fn is_finishing(&self) -> bool {
        self.progress.lock().unwrap().finishing_since.is_some()
    }

    /// Waits for a message, cancellation or the session's deadline. A progress
    /// notification reschedules the deadlines without losing a stream item.
    pub async fn next<S>(
        &self,
        stream: &mut S,
        state: &AppState,
    ) -> Result<Option<S::Item>, SessionError>
    where
        S: tokio_stream::Stream + Unpin,
    {
        use tokio_stream::StreamExt;
        loop {
            let changed = self.changed.notified();
            let (notice_since, timeout_at, phase) = {
                let progress = self.progress.lock().unwrap();
                match progress.finishing_since {
                    Some(since) => (
                        Some(
                            progress
                                .pending_since
                                .map_or(since, |pending| pending.min(since)),
                        ),
                        Some(since + self.limits.finish),
                        "finishing transcription",
                    ),
                    None => (
                        progress.pending_since,
                        None,
                        "waiting for live transcription",
                    ),
                }
            };
            let waiting = notice_since.is_some_and(|since| since.elapsed() >= self.limits.notice);
            state.set_deepgram_waiting(waiting);
            tokio::select! {
                biased;
                _ = state.wait_for_abort() => return Err(SessionError::Cancelled),
                _ = deadline(timeout_at) => return Err(SessionError::Failed(format!("Deepgram timed out while {phase}. Please try recording again."))),
                item = stream.next() => return Ok(item),
                _ = changed => {},
                _ = deadline(notice_since.map(|since| since + self.limits.notice)), if !waiting => {},
            }
        }
    }
}

async fn deadline(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

pub struct WaitingIndicator(pub Arc<AppState>);

impl Drop for WaitingIndicator {
    fn drop(&mut self) {
        self.0.set_deepgram_waiting(false);
    }
}

pub async fn connect<T>(
    state: Arc<AppState>,
    limits: SessionLimits,
    connection: impl Future<Output = Result<T, String>>,
) -> Result<T, SessionError> {
    let _indicator = WaitingIndicator(state.clone());
    tokio::pin!(connection);
    let started = Instant::now();
    let mut waiting = false;
    loop {
        tokio::select! {
            biased;
            _ = state.wait_for_abort() => return Err(SessionError::Cancelled),
            _ = tokio::time::sleep_until(started + limits.connect) => return Err(SessionError::Failed("Deepgram connection timed out. Please try recording again.".to_owned())),
            result = &mut connection => return result.map_err(SessionError::Failed),
            _ = tokio::time::sleep_until(started + limits.notice), if !waiting => {
                waiting = true;
                state.set_deepgram_waiting(true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::wrappers::ReceiverStream;

    fn limits() -> SessionLimits {
        SessionLimits {
            notice: Duration::from_millis(10),
            connect: Duration::from_millis(80),
            finish: Duration::from_millis(80),
        }
    }

    fn voice() -> Vec<u8> {
        (0..320).flat_map(|_| 1000_i16.to_le_bytes()).collect()
    }

    #[tokio::test]
    async fn quiet_audio_does_not_show_a_wait_notice() {
        let state = AppState::new();
        let progress = SessionProgress::new(16000, limits());
        progress.audio_queued(&[0; 640]);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let mut stream = ReceiverStream::new(rx);
        let check = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert!(!state.is_deepgram_waiting());
            tx.send(1).await.unwrap();
        };
        let (result, _) = tokio::join!(progress.next(&mut stream, &state), check);
        assert_eq!(result.unwrap(), Some(1));
    }

    #[tokio::test]
    async fn live_wait_notice_clears_when_transcripts_resume() {
        let state = AppState::new();
        let progress = SessionProgress::new(16000, limits());
        progress.audio_queued(&voice());
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let mut stream = ReceiverStream::new(rx);
        let reply = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert!(state.is_deepgram_waiting());
            tx.send(1).await.unwrap();
        };
        let (result, _) = tokio::join!(progress.next(&mut stream, &state), reply);
        assert_eq!(result.unwrap(), Some(1));
        progress.transcript_received(0.02);
        tx.send(2).await.unwrap();
        assert_eq!(progress.next(&mut stream, &state).await.unwrap(), Some(2));
        assert!(!state.is_deepgram_waiting());
    }

    #[tokio::test]
    async fn replies_do_not_extend_the_finishing_deadline() {
        let state = AppState::new();
        let progress = SessionProgress::new(16000, limits());
        progress.finish();
        let (_tx, rx) = tokio::sync::mpsc::channel::<()>(1);
        let mut stream = ReceiverStream::new(rx);
        let reply = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert!(state.is_deepgram_waiting());
            progress.transcript_received(1.0);
        };
        let (result, _) = tokio::join!(progress.next(&mut stream, &state), reply);
        assert!(
            matches!(result, Err(SessionError::Failed(message)) if message.contains("finishing"))
        );
    }

    #[tokio::test]
    async fn a_quiet_period_after_a_transcript_does_not_start_another_wait() {
        let state = AppState::new();
        let progress = SessionProgress::new(16000, limits());
        progress.audio_queued(&voice());
        progress.transcript_received(0.02);
        progress.audio_queued(&[0; 640]);
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let mut stream = ReceiverStream::new(rx);
        let reply = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert!(!state.is_deepgram_waiting());
            tx.send(1).await.unwrap();
        };
        let (result, _) = tokio::join!(progress.next(&mut stream, &state), reply);
        assert_eq!(result.unwrap(), Some(1));
    }

    #[tokio::test]
    async fn a_live_wait_notice_keeps_recording_until_the_user_finishes() {
        let state = AppState::new();
        state.set_state(crate::state::STATE_RECORDING);
        let progress = SessionProgress::new(16000, limits());
        progress.audio_queued(&voice());
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let mut stream = ReceiverStream::new(rx);
        let reply = async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert!(state.is_deepgram_waiting());
            tx.send(1).await.unwrap();
        };
        let (result, _) = tokio::join!(progress.next(&mut stream, &state), reply);
        assert_eq!(result.unwrap(), Some(1));
        assert!(state.is_capturing_audio());
    }

    #[tokio::test]
    async fn connection_wait_is_visible_and_bounded_and_clears_on_timeout() {
        let state = AppState::new();
        let check = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert!(state.is_deepgram_waiting());
        };
        let (result, _) = tokio::join!(
            connect::<()>(state.clone(), limits(), std::future::pending()),
            check
        );
        assert!(
            matches!(result, Err(SessionError::Failed(message)) if message.contains("connection timed out"))
        );
        assert!(!state.is_deepgram_waiting());
    }

    #[tokio::test]
    async fn abort_wakes_both_live_and_connect_waits() {
        let state = AppState::new();
        let progress = SessionProgress::new(16000, limits());
        let (_tx, rx) = tokio::sync::mpsc::channel::<()>(1);
        let mut stream = ReceiverStream::new(rx);
        let abort = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            state.request_abort();
        };
        let (live, connection, _) = tokio::join!(
            progress.next(&mut stream, &state),
            connect::<()>(state.clone(), limits(), std::future::pending()),
            abort
        );
        assert_eq!(live, Err(SessionError::Cancelled));
        assert_eq!(connection, Err(SessionError::Cancelled));
        assert!(state.consume_abort_request());
        assert!(!state.is_abort_requested());
        assert_eq!(
            connect(state.clone(), limits(), async { Ok("next session") })
                .await
                .unwrap(),
            "next session"
        );
    }
}
