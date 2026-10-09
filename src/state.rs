use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::text_edit::{edited_offset, merge_text};

pub const STATE_IDLE: u8 = 0;
pub const STATE_RECORDING: u8 = 1;
pub const STATE_PROCESSING: u8 = 2;
pub const STATE_BUFFER_READY: u8 = 3;
pub const STATE_TRANSFORMING: u8 = 4;
pub const STATE_ERROR: u8 = 5;

/// Text shown in an overlay text view, with the byte offset where its
/// provisional tail (a Deepgram interim transcript that may still change)
/// begins. `provisional_start` is `None` when all of `text` is final.
#[derive(Clone, Debug)]
pub struct OverlayText {
    pub text: Arc<str>,
    pub provisional_start: Option<usize>,
}

impl OverlayText {
    fn new(text: String, provisional_start: Option<usize>) -> Self {
        let provisional_start = provisional_start
            .filter(|&start| start < text.len() && text.is_char_boundary(start));
        Self {
            text: Arc::from(text),
            provisional_start,
        }
    }
}

impl Default for OverlayText {
    fn default() -> Self {
        Self::new(String::new(), None)
    }
}

fn merge_overlay_text(current: &mut OverlayText, before: &str, incoming: OverlayText) {
    let text = merge_text(before, &current.text, &incoming.text);
    let provisional_start = incoming
        .provisional_start
        .map(|start| edited_offset(&incoming.text, &text, start));
    *current = OverlayText::new(text, provisional_start);
}

/// Identifies a Deepgram API key without keeping the key: the first 8 bytes of
/// the SHA-256 of the trimmed key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeepgramApiKeyFingerprint([u8; 8]);

impl DeepgramApiKeyFingerprint {
    pub fn of(api_key: &str) -> Self {
        let digest = Sha256::digest(api_key.trim().as_bytes());
        let mut prefix = [0; 8];
        prefix.copy_from_slice(&digest[..8]);
        Self(prefix)
    }
}

/// How many bands the pill meter's spectrum has: one pill each.
pub const SPECTRUM_BANDS: usize = 40;
/// Spectrum frames the audio callback may queue before the overlay takes them
/// in: several seconds at the usual callback rate, while the overlay takes
/// them in every 75 ms. Frames beyond it are dropped.
const QUEUED_SPECTRUM_FRAMES: usize = 256;

/// The spectrum of the captured audio as one block of it arrived, `at` on the
/// media clock (`CACurrentMediaTime`): each band's level in dB (0 is about a
/// full-scale sine) and its centre frequency, low pitches first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumFrame {
    pub at: f64,
    pub levels_db: [f32; SPECTRUM_BANDS],
    pub centres_hz: [f32; SPECTRUM_BANDS],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MicMeterSnapshot {
    pub clip_event_counter: u32,
    /// The smoothed RMS level mapped onto the meter's fixed range, where
    /// everything below it reads 0.
    pub level: u8,
    pub peak: u8,
    pub mic_active: bool,
}

#[derive(Debug)]
pub struct AppState {
    abort_requested: AtomicBool,
    abort_notification: tokio::sync::Notify,
    deepgram_waiting: AtomicBool,
    clip_event_counter: AtomicU32,
    mic_meter_level: AtomicU8,
    mic_meter_peak: AtomicU8,
    overlay_dismissed: AtomicBool,
    /// Nonempty narration is held while the record hotkey finishes its paste.
    overlay_finishing: AtomicBool,
    overlay_correction_active: AtomicBool,
    overlay_correction_text: Mutex<OverlayText>,
    overlay_window_visible: AtomicBool,
    settings_window_visible: AtomicBool,
    settings_requested: AtomicBool,
    overlay_text: Mutex<OverlayText>,
    overlay_error_text: Mutex<Arc<str>>,
    overlay_text_opacity: AtomicU8,
    preview_mic_gain: AtomicU32,
    state: AtomicU8,
    mic_active: AtomicBool,
    /// Whether the audio stream found a microphone to use the last time it
    /// looked.
    microphone_available: AtomicBool,
    /// Where the audio callback queues the spectrum of each block it
    /// captures: a bounded channel, so the real-time thread never waits.
    spectrum_sender: SyncSender<SpectrumFrame>,
    spectrum_receiver: Mutex<Receiver<SpectrumFrame>>,
    /// Dictation resumes once the background work running now (a
    /// transformation or a correction) finishes, so audio capture and the
    /// meter carry on through it.
    dictation_resuming: AtomicBool,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        let (spectrum_sender, spectrum_receiver) = sync_channel(QUEUED_SPECTRUM_FRAMES);
        Arc::new(Self {
            abort_requested: AtomicBool::new(false),
            abort_notification: tokio::sync::Notify::new(),
            deepgram_waiting: AtomicBool::new(false),
            clip_event_counter: AtomicU32::new(0),
            mic_meter_level: AtomicU8::new(0),
            mic_meter_peak: AtomicU8::new(0),
            overlay_dismissed: AtomicBool::new(false),
            overlay_finishing: AtomicBool::new(false),
            overlay_correction_active: AtomicBool::new(false),
            overlay_correction_text: Mutex::new(OverlayText::default()),
            overlay_window_visible: AtomicBool::new(false),
            settings_window_visible: AtomicBool::new(false),
            settings_requested: AtomicBool::new(false),
            overlay_text: Mutex::new(OverlayText::default()),
            overlay_error_text: Mutex::new(Arc::from("")),
            overlay_text_opacity: AtomicU8::new(u8::MAX),
            preview_mic_gain: AtomicU32::new(f32::to_bits(f32::NAN)),
            state: AtomicU8::new(STATE_IDLE),
            mic_active: AtomicBool::new(false),
            dictation_resuming: AtomicBool::new(false),
            microphone_available: AtomicBool::new(true),
            spectrum_sender,
            spectrum_receiver: Mutex::new(spectrum_receiver),
        })
    }

    pub fn is_recording(&self) -> bool {
        self.get_state() == STATE_RECORDING
    }

    /// Whether the microphone's audio goes to dictation: while recording, and
    /// while a dictation that resumes afterwards is transformed or corrected.
    pub fn is_capturing_audio(&self) -> bool {
        self.is_recording() || self.is_dictation_resuming()
    }

    /// Where to queue the spectrum of each captured block. `try_send` on it
    /// never waits, so the audio callback can use it: when the queue is full,
    /// the frame is dropped.
    pub fn spectrum_sender(&self) -> SyncSender<SpectrumFrame> {
        self.spectrum_sender.clone()
    }

    /// The spectrum frames queued since the last call, oldest first.
    pub fn take_spectrum_frames(&self) -> Vec<SpectrumFrame> {
        self.spectrum_receiver
            .lock()
            .map(|receiver| receiver.try_iter().collect())
            .unwrap_or_default()
    }

    pub fn set_microphone_available(&self, available: bool) {
        self.microphone_available.store(available, Ordering::Relaxed);
    }

    pub fn is_microphone_available(&self) -> bool {
        self.microphone_available.load(Ordering::Relaxed)
    }

    pub fn set_dictation_resuming(&self, resuming: bool) {
        self.dictation_resuming.store(resuming, Ordering::Relaxed);
    }

    pub fn is_dictation_resuming(&self) -> bool {
        self.dictation_resuming.load(Ordering::Relaxed)
    }

    pub fn set_preview_mic_gain(&self, gain: Option<f32>) {
        let bits = gain.map(f32::to_bits).unwrap_or(f32::to_bits(f32::NAN));
        self.preview_mic_gain.store(bits, Ordering::Relaxed);
    }

    pub fn preview_mic_gain(&self) -> Option<f32> {
        let val = f32::from_bits(self.preview_mic_gain.load(Ordering::Relaxed));
        if val.is_nan() {
            None
        } else {
            Some(val)
        }
    }

    pub fn set_state(&self, state: u8) {
        if !matches!(state, STATE_PROCESSING | STATE_TRANSFORMING | STATE_IDLE) {
            self.overlay_finishing.store(false, Ordering::Relaxed);
        }
        self.state.store(state, Ordering::Relaxed);
        // Transforming or correcting a dictation that resumes after it never
        // stops the microphone, so its meter and activity carry on; any other
        // state (the resume failed) stops them as usual.
        if self.is_dictation_resuming()
            && matches!(state, STATE_RECORDING | STATE_PROCESSING | STATE_TRANSFORMING)
        {
            return;
        }
        if state == STATE_RECORDING {
            self.set_mic_active(false);
        }
        if state != STATE_RECORDING && !self.is_settings_window_visible() {
            self.clear_mic_meter();
            self.set_mic_active(false);
        }
    }

    pub fn begin_finishing_dictation(&self) {
        let has_narration = !self.overlay_text().trim().is_empty();
        self.overlay_finishing.store(has_narration, Ordering::Relaxed);
        if !has_narration {
            // Nothing is displayed to hold while final audio is transcribed.
            // Dismiss now; the worker still finishes the session and pastes
            // any final transcript, without reopening this presentation.
            self.dismiss_overlay();
        }
        self.set_state(STATE_PROCESSING);
    }

    pub fn is_finishing_dictation(&self) -> bool {
        self.overlay_finishing.load(Ordering::Relaxed)
    }

    pub fn get_state(&self) -> u8 {
        self.state.load(Ordering::Relaxed)
    }

    pub fn request_abort(&self) {
        self.abort_requested.store(true, Ordering::Release);
        self.abort_notification.notify_waiters();
    }

    pub fn clear_abort_request(&self) {
        self.abort_requested.store(false, Ordering::Relaxed);
    }

    pub fn is_abort_requested(&self) -> bool {
        self.abort_requested.load(Ordering::Acquire)
    }

    /// Waits without polling, including a request made before this wait.
    /// Register before checking the latch so a concurrent request is not lost.
    pub async fn wait_for_abort(&self) {
        loop {
            let notified = self.abort_notification.notified();
            if self.is_abort_requested() {
                return;
            }
            notified.await;
        }
    }

    pub fn set_deepgram_waiting(&self, waiting: bool) {
        self.deepgram_waiting.store(waiting, Ordering::Relaxed);
    }

    pub fn is_deepgram_waiting(&self) -> bool {
        self.deepgram_waiting.load(Ordering::Relaxed)
    }

    pub fn consume_abort_request(&self) -> bool {
        self.abort_requested.swap(false, Ordering::Relaxed)
    }

    pub fn dismiss_overlay(&self) {
        self.overlay_dismissed.store(true, Ordering::Relaxed);
    }

    pub fn restore_overlay(&self) {
        self.overlay_dismissed.store(false, Ordering::Relaxed);
    }

    pub fn is_overlay_dismissed(&self) -> bool {
        self.overlay_dismissed.load(Ordering::Relaxed)
    }

    pub fn set_overlay_correction_active(&self, active: bool) {
        self.overlay_correction_active
            .store(active, Ordering::Relaxed);
    }

    pub fn is_overlay_correction_active(&self) -> bool {
        self.overlay_correction_active.load(Ordering::Relaxed)
    }

    pub fn set_overlay_window_visible(&self, visible: bool) {
        if !visible {
            self.overlay_finishing.store(false, Ordering::Relaxed);
        }
        self.overlay_window_visible
            .store(visible, Ordering::Relaxed);
    }

    pub fn is_overlay_window_visible(&self) -> bool {
        self.overlay_window_visible.load(Ordering::Relaxed)
    }

    pub fn set_settings_window_visible(&self, visible: bool) {
        self.settings_window_visible
            .store(visible, Ordering::Relaxed);
    }

    pub fn is_settings_window_visible(&self) -> bool {
        self.settings_window_visible.load(Ordering::Relaxed)
    }

    /// Hand a hotkey request to the main-thread UI poll.
    pub fn request_settings(&self) {
        self.settings_requested.store(true, Ordering::Release);
    }

    pub fn take_settings_request(&self) -> bool {
        self.settings_requested.swap(false, Ordering::AcqRel)
    }

    pub fn set_overlay_text(&self, overlay_text: impl Into<String>) {
        self.set_live_overlay_text(overlay_text, None);
    }

    /// Replaces the overlay text; `provisional_start` marks where the interim
    /// transcript begins, and is dropped unless it is a character boundary
    /// inside the text.
    pub fn set_live_overlay_text(
        &self,
        overlay_text: impl Into<String>,
        provisional_start: Option<usize>,
    ) {
        if let Ok(mut current_overlay_text) = self.overlay_text.lock() {
            *current_overlay_text = OverlayText::new(overlay_text.into(), provisional_start);
        }
    }

    pub fn clear_overlay_text(&self) {
        self.set_overlay_text(String::new());
    }

    /// Merge a speech revision with keyboard edits under the text's lock.
    /// `before` is the preceding speech result, before any keyboard edits.
    pub fn merge_live_overlay_text(
        &self,
        before: &str,
        text: String,
        provisional_start: Option<usize>,
    ) {
        if let Ok(mut current) = self.overlay_text.lock() {
            merge_overlay_text(&mut current, before, OverlayText::new(text, provisional_start));
        }
    }

    /// The editor's baseline is its last rendered text, which may precede
    /// the latest speech result. Keep speech that arrived between UI ticks.
    pub fn apply_overlay_edit(&self, rendered: &str, edited: &str) {
        if let Ok(mut current) = self.overlay_text.lock() {
            let text = merge_text(rendered, edited, &current.text);
            let provisional_start = current
                .provisional_start
                .map(|start| edited_offset(&current.text, &text, start));
            *current = OverlayText::new(text, provisional_start);
        }
    }

    pub fn set_overlay_error_text(&self, overlay_error_text: impl Into<String>) {
        if let Ok(mut current_overlay_error_text) = self.overlay_error_text.lock() {
            *current_overlay_error_text = Arc::from(overlay_error_text.into());
        }
    }

    pub fn clear_overlay_error_text(&self) {
        self.set_overlay_error_text(String::new());
    }

    pub fn report_error(&self, message: impl Into<String>) {
        self.restore_overlay();
        self.set_overlay_error_text(message);
        self.set_state(STATE_ERROR);
    }

    pub fn set_overlay_correction_text(&self, overlay_correction_text: impl Into<String>) {
        self.set_live_overlay_correction_text(overlay_correction_text, None);
    }

    /// The correction text counterpart of `set_live_overlay_text`.
    pub fn set_live_overlay_correction_text(
        &self,
        overlay_correction_text: impl Into<String>,
        provisional_start: Option<usize>,
    ) {
        if let Ok(mut current_overlay_correction_text) = self.overlay_correction_text.lock() {
            *current_overlay_correction_text =
                OverlayText::new(overlay_correction_text.into(), provisional_start);
        }
    }

    pub fn clear_overlay_correction_text(&self) {
        self.set_overlay_correction_text(String::new());
    }

    pub fn merge_live_overlay_correction_text(
        &self,
        before: &str,
        text: String,
        provisional_start: Option<usize>,
    ) {
        if let Ok(mut current) = self.overlay_correction_text.lock() {
            merge_overlay_text(&mut current, before, OverlayText::new(text, provisional_start));
        }
    }

    pub fn set_overlay_text_opacity(&self, overlay_text_opacity: f64) {
        self.overlay_text_opacity.store(
            normalized_meter_value(overlay_text_opacity as f32),
            Ordering::Relaxed,
        );
    }

    pub fn overlay_text_opacity(&self) -> f64 {
        self.overlay_text_opacity.load(Ordering::Relaxed) as f64 / u8::MAX as f64
    }

    pub fn overlay_text(&self) -> Arc<str> {
        self.overlay_text_snapshot().text
    }

    /// The overlay text together with its provisional start, read under one
    /// lock so the offset always belongs to the text.
    pub fn overlay_text_snapshot(&self) -> OverlayText {
        self.overlay_text
            .lock()
            .map(|overlay_text| overlay_text.clone())
            .unwrap_or_default()
    }

    pub fn overlay_error_text(&self) -> Arc<str> {
        self.overlay_error_text
            .lock()
            .map(|overlay_error_text| overlay_error_text.clone())
            .unwrap_or_else(|_| Arc::from(""))
    }

    pub fn overlay_correction_text(&self) -> Arc<str> {
        self.overlay_correction_text_snapshot().text
    }

    /// The correction text counterpart of `overlay_text_snapshot`.
    pub fn overlay_correction_text_snapshot(&self) -> OverlayText {
        self.overlay_correction_text
            .lock()
            .map(|overlay_correction_text| overlay_correction_text.clone())
            .unwrap_or_default()
    }

    pub fn is_mic_active(&self) -> bool {
        self.mic_active.load(Ordering::Relaxed)
    }

    pub fn set_mic_active(&self, active: bool) {
        self.mic_active.store(active, Ordering::Relaxed);
    }

    pub fn set_mic_meter(&self, level: f32, peak: f32, clip_detected: bool) {
        if clip_detected {
            self.clip_event_counter.fetch_add(1, Ordering::Relaxed);
        }

        self.mic_meter_level
            .store(normalized_meter_value(level), Ordering::Relaxed);
        self.mic_meter_peak
            .store(normalized_meter_value(peak), Ordering::Relaxed);
    }

    pub fn clear_mic_meter(&self) {
        self.mic_meter_level.store(0, Ordering::Relaxed);
        self.mic_meter_peak.store(0, Ordering::Relaxed);
    }

    pub fn mic_meter_snapshot(&self) -> MicMeterSnapshot {
        MicMeterSnapshot {
            clip_event_counter: self.clip_event_counter.load(Ordering::Relaxed),
            level: self.mic_meter_level.load(Ordering::Relaxed),
            peak: self.mic_meter_peak.load(Ordering::Relaxed),
            mic_active: self.is_mic_active(),
        }
    }
}

/// A meter level from 0 to 1 as a byte.
pub(crate) fn normalized_meter_value(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * u8::MAX as f32).round() as u8
}

#[cfg(test)]
mod tests {
    use super::{
        AppState, normalized_meter_value, STATE_BUFFER_READY, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING,
        STATE_TRANSFORMING,
    };

    #[test]
    fn finishing_survives_a_fast_result_until_the_overlay_is_hidden() {
        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        state.set_overlay_text("Keep narration visible through the fade.");
        state.set_overlay_window_visible(true);
        state.begin_finishing_dictation();
        state.set_state(STATE_IDLE);
        assert!(state.is_finishing_dictation());
        state.set_overlay_window_visible(false);
        assert!(!state.is_finishing_dictation());
    }

    #[test]
    fn audio_capture_carries_on_while_dictation_resumes_after_a_correction() {
        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        assert!(state.is_capturing_audio());

        state.set_dictation_resuming(true);
        state.set_state(STATE_TRANSFORMING);
        assert!(!state.is_recording());
        assert!(state.is_capturing_audio());

        state.set_dictation_resuming(false);
        assert!(!state.is_capturing_audio());
    }

    #[test]
    fn queued_spectrum_frames_are_taken_once_in_order() {
        use super::{SpectrumFrame, SPECTRUM_BANDS};

        let state = AppState::new();
        let frame = |at: f64| SpectrumFrame { at, levels_db: [-60.0; SPECTRUM_BANDS], centres_hz: [1_000.0; SPECTRUM_BANDS] };
        let sender = state.spectrum_sender();
        sender.try_send(frame(10.0)).unwrap();
        sender.try_send(frame(10.03)).unwrap();
        assert_eq!(state.take_spectrum_frames(), vec![frame(10.0), frame(10.03)]);
        assert_eq!(state.take_spectrum_frames(), Vec::new());
    }

    #[test]
    fn a_microphone_counts_as_available_until_the_audio_stream_finds_none() {
        let state = AppState::new();
        assert!(state.is_microphone_available());
        state.set_microphone_available(false);
        assert!(!state.is_microphone_available());
    }

    #[test]
    fn the_meter_carries_on_while_a_transformation_is_prepared_mid_dictation() {
        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        state.set_mic_meter(0.4, 0.5, false);
        state.set_mic_active(true);

        state.set_dictation_resuming(true);
        state.set_state(STATE_PROCESSING);

        let meter = state.mic_meter_snapshot();
        assert_eq!(meter.level, normalized_meter_value(0.4));
        assert!(meter.mic_active);
        assert!(state.is_capturing_audio());
    }

    #[test]
    fn a_resume_that_fails_stops_the_meter() {
        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        state.set_mic_meter(0.4, 0.5, false);
        state.set_mic_active(true);
        state.set_dictation_resuming(true);
        state.set_state(STATE_TRANSFORMING);

        // The resumed session could not start: the buffer is ready instead.
        state.set_state(STATE_BUFFER_READY);

        let meter = state.mic_meter_snapshot();
        assert_eq!(meter.level, 0);
        assert!(!meter.mic_active);
    }

    #[test]
    fn a_transformation_without_resuming_dictation_stops_capture() {
        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        state.set_mic_meter(0.4, 0.5, false);

        state.set_state(STATE_TRANSFORMING);

        assert!(!state.is_capturing_audio());
        assert_eq!(state.mic_meter_snapshot().level, 0);
    }

    #[test]
    fn the_meter_carries_on_through_a_correction_applied_mid_dictation() {
        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        state.set_mic_meter(0.4, 0.5, false);
        state.set_mic_active(true);

        state.set_dictation_resuming(true);
        state.set_state(STATE_TRANSFORMING);
        let meter = state.mic_meter_snapshot();
        assert_eq!(meter.level, normalized_meter_value(0.4));
        assert!(meter.mic_active);

        // Dictation resumes: the microphone never stopped delivering.
        state.set_state(STATE_RECORDING);
        assert!(state.mic_meter_snapshot().mic_active);
    }

    #[test]
    fn non_recording_states_clear_the_mic_meter() {
        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        state.set_mic_meter(0.4, 0.7, true);

        state.set_state(STATE_IDLE);

        let mic_meter = state.mic_meter_snapshot();
        assert_eq!(mic_meter.level, 0);
        assert_eq!(mic_meter.peak, 0);
        assert_eq!(mic_meter.clip_event_counter, 1);
    }

    #[test]
    fn clip_events_increment_the_counter() {
        let state = AppState::new();

        state.set_mic_meter(0.1, 0.2, false);
        assert_eq!(state.mic_meter_snapshot().clip_event_counter, 0);

        state.set_mic_meter(0.3, 0.4, true);
        state.set_mic_meter(0.3, 0.4, true);

        assert_eq!(state.mic_meter_snapshot().clip_event_counter, 2);
    }

    #[test]
    fn overlay_dismissal_can_be_toggled() {
        let state = AppState::new();

        assert!(!state.is_overlay_dismissed());

        state.dismiss_overlay();
        assert!(state.is_overlay_dismissed());

        state.restore_overlay();
        assert!(!state.is_overlay_dismissed());
    }

    #[test]
    fn overlay_window_visibility_can_be_toggled() {
        let state = AppState::new();

        assert!(!state.is_overlay_window_visible());

        state.set_overlay_window_visible(true);
        assert!(state.is_overlay_window_visible());

        state.set_overlay_window_visible(false);
        assert!(!state.is_overlay_window_visible());
    }

    #[test]
    fn preview_mic_gain_can_be_set_and_cleared() {
        let state = AppState::new();

        assert_eq!(state.preview_mic_gain(), None);

        state.set_preview_mic_gain(Some(1.2));
        assert_eq!(state.preview_mic_gain(), Some(1.2));

        state.set_preview_mic_gain(None);
        assert_eq!(state.preview_mic_gain(), None);
    }

    #[test]
    fn report_error_restores_overlay_and_sets_error_state() {
        let state = AppState::new();

        state.dismiss_overlay();
        assert!(state.is_overlay_dismissed());

        state.report_error("mic unplugged");

        assert!(!state.is_overlay_dismissed());
        assert_eq!(state.get_state(), super::STATE_ERROR);
        assert_eq!(&*state.overlay_error_text(), "mic unplugged");
    }

    #[test]
    fn live_overlay_text_keeps_its_provisional_start_with_the_text() {
        let state = AppState::new();

        state.set_live_overlay_text("final words still talking ", Some(12));

        let snapshot = state.overlay_text_snapshot();
        assert_eq!(&*snapshot.text, "final words still talking ");
        assert_eq!(snapshot.provisional_start, Some(12));
        assert!(std::sync::Arc::ptr_eq(&snapshot.text, &state.overlay_text()));
    }

    #[test]
    fn keyboard_edits_keep_speech_received_since_the_last_render() {
        let state = AppState::new();
        state.set_live_overlay_text("hello still talking today ", Some("hello ".len()));
        state.apply_overlay_edit("hello still talking ", "Hé🙂 still talking ");
        let snapshot = state.overlay_text_snapshot();
        assert_eq!(&*snapshot.text, "Hé🙂 still talking today ");
        assert_eq!(snapshot.provisional_start, Some("Hé🙂 ".len()));
        state.merge_live_overlay_text(
            "hello still talking today ",
            "hello still talking today and tomorrow ".to_owned(),
            Some("hello ".len()),
        );
        let snapshot = state.overlay_text_snapshot();
        assert_eq!(&*snapshot.text, "Hé🙂 still talking today and tomorrow ");
        assert_eq!(snapshot.provisional_start, Some("Hé🙂 ".len()));
    }

    #[test]
    fn keyboard_edits_survive_finalization_and_a_later_edit() {
        let state = AppState::new();
        state.set_live_overlay_text("meet Thursday ", Some(0));
        state.apply_overlay_edit("meet Thursday ", "meet Friday ");
        state.merge_live_overlay_text(
            "meet Thursday ",
            "Meet Thursday afternoon. ".to_owned(),
            None,
        );
        assert_eq!(&*state.overlay_text(), "Meet Friday afternoon. ");
        assert_eq!(state.overlay_text_snapshot().provisional_start, None);
        state.apply_overlay_edit("Meet Friday afternoon. ", "Meet Friday morning. ");
        state.merge_live_overlay_text(
            "Meet Thursday afternoon. ",
            "Meet Thursday afternoon. See you then. ".to_owned(),
            None,
        );
        assert_eq!(
            &*state.overlay_text(),
            "Meet Friday morning. See you then. "
        );
    }

    #[test]
    fn replacing_overlay_text_clears_its_provisional_start() {
        let state = AppState::new();
        state.set_live_overlay_text("still talking ", Some(0));

        state.set_overlay_text("edited by hand");

        assert_eq!(state.overlay_text_snapshot().provisional_start, None);
    }

    #[test]
    fn provisional_start_outside_the_text_or_inside_a_character_is_dropped() {
        let state = AppState::new();

        state.set_live_overlay_text("héllo", Some(2));
        assert_eq!(state.overlay_text_snapshot().provisional_start, None);

        state.set_live_overlay_text("hello", Some(5));
        assert_eq!(state.overlay_text_snapshot().provisional_start, None);

        state.set_live_overlay_text("hello", Some(9));
        assert_eq!(state.overlay_text_snapshot().provisional_start, None);
    }

    #[test]
    fn live_correction_text_keeps_its_provisional_start_until_replaced() {
        let state = AppState::new();

        state.set_live_overlay_correction_text("make it shorter ", Some(8));
        let snapshot = state.overlay_correction_text_snapshot();
        assert_eq!(&*snapshot.text, "make it shorter ");
        assert_eq!(snapshot.provisional_start, Some(8));
        assert!(std::sync::Arc::ptr_eq(
            &snapshot.text,
            &state.overlay_correction_text()
        ));

        state.clear_overlay_correction_text();
        assert_eq!(state.overlay_correction_text_snapshot().provisional_start, None);
    }
}
