use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

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

/// How long speech analysis results are kept: longer than the meter shows.
const SPEECH_WINDOW_KEEP_SECONDS: f64 = 20.0;
/// How long the loudness of captured audio is kept: longer than a pill waits
/// to be painted.
const LEVEL_BLOCK_KEEP_SECONDS: f64 = 2.0;

/// One speech analysis result: whether the captured audio from `start` to
/// `end`, on the media clock (`CACurrentMediaTime`), was speech.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeechWindow {
    pub start: f64,
    pub end: f64,
    pub speech: bool,
}

/// The loudness of one block of captured audio, from `start` to `end` on the
/// media clock: its mean square (its RMS squared, 1 at full scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelBlock {
    pub start: f64,
    pub end: f64,
    pub mean_square: f32,
}

/// What the pills are drawn from: the recent speech analysis results
/// (`None` when speech analysis does not run) and loudness, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioTimeline {
    pub speech: Option<Vec<SpeechWindow>>,
    pub levels: Vec<LevelBlock>,
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
    clip_event_counter: AtomicU32,
    mic_meter_level: AtomicU8,
    mic_meter_peak: AtomicU8,
    overlay_dismissed: AtomicBool,
    overlay_correction_active: AtomicBool,
    overlay_correction_text: Mutex<OverlayText>,
    overlay_window_visible: AtomicBool,
    settings_window_visible: AtomicBool,
    overlay_text: Mutex<OverlayText>,
    overlay_error_text: Mutex<Arc<str>>,
    overlay_text_opacity: AtomicU8,
    preview_mic_gain: AtomicU32,
    state: AtomicU8,
    mic_active: AtomicBool,
    /// Whether the audio stream found a microphone to use the last time it
    /// looked.
    microphone_available: AtomicBool,
    /// Whether speech analysis runs on the captured audio.
    speech_analysis_available: AtomicBool,
    /// Its recent results, oldest first.
    speech_windows: Mutex<VecDeque<SpeechWindow>>,
    /// The loudness of the recent captured audio, oldest first.
    level_blocks: Mutex<VecDeque<LevelBlock>>,
    /// Dictation resumes once the background work running now (a
    /// transformation or a correction) finishes, so audio capture and the
    /// meter carry on through it.
    dictation_resuming: AtomicBool,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            abort_requested: AtomicBool::new(false),
            clip_event_counter: AtomicU32::new(0),
            mic_meter_level: AtomicU8::new(0),
            mic_meter_peak: AtomicU8::new(0),
            overlay_dismissed: AtomicBool::new(false),
            overlay_correction_active: AtomicBool::new(false),
            overlay_correction_text: Mutex::new(OverlayText::default()),
            overlay_window_visible: AtomicBool::new(false),
            settings_window_visible: AtomicBool::new(false),
            overlay_text: Mutex::new(OverlayText::default()),
            overlay_error_text: Mutex::new(Arc::from("")),
            overlay_text_opacity: AtomicU8::new(u8::MAX),
            preview_mic_gain: AtomicU32::new(f32::to_bits(f32::NAN)),
            state: AtomicU8::new(STATE_IDLE),
            mic_active: AtomicBool::new(false),
            dictation_resuming: AtomicBool::new(false),
            microphone_available: AtomicBool::new(true),
            speech_analysis_available: AtomicBool::new(false),
            speech_windows: Mutex::new(VecDeque::new()),
            level_blocks: Mutex::new(VecDeque::new()),
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

    pub fn set_speech_analysis_available(&self, available: bool) {
        self.speech_analysis_available.store(available, Ordering::Relaxed);
    }

    /// The speech analysis results kept (`None` when speech analysis does
    /// not run) and the loudness kept, oldest first.
    pub fn audio_timeline(&self) -> AudioTimeline {
        AudioTimeline {
            speech: self.speech_analysis_available.load(Ordering::Relaxed).then(|| self.speech_windows()),
            levels: self
                .level_blocks
                .lock()
                .map(|blocks| blocks.iter().copied().collect())
                .unwrap_or_default(),
        }
    }

    /// Keeps `block`, dropping loudness older than
    /// `LEVEL_BLOCK_KEEP_SECONDS` before it.
    pub fn record_level_block(&self, block: LevelBlock) {
        if let Ok(mut blocks) = self.level_blocks.lock() {
            blocks.push_back(block);
            while blocks.front().is_some_and(|oldest| oldest.end < block.end - LEVEL_BLOCK_KEEP_SECONDS) {
                blocks.pop_front();
            }
        }
    }

    pub fn speech_windows(&self) -> Vec<SpeechWindow> {
        self.speech_windows
            .lock()
            .map(|windows| windows.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Keeps `window`, dropping results older than
    /// `SPEECH_WINDOW_KEEP_SECONDS` before it.
    pub fn record_speech_window(&self, window: SpeechWindow) {
        if let Ok(mut windows) = self.speech_windows.lock() {
            windows.push_back(window);
            while windows
                .front()
                .is_some_and(|oldest| oldest.end < window.end - SPEECH_WINDOW_KEEP_SECONDS)
            {
                windows.pop_front();
            }
        }
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

    pub fn get_state(&self) -> u8 {
        self.state.load(Ordering::Relaxed)
    }

    pub fn request_abort(&self) {
        self.abort_requested.store(true, Ordering::Relaxed);
    }

    pub fn clear_abort_request(&self) {
        self.abort_requested.store(false, Ordering::Relaxed);
    }

    pub fn is_abort_requested(&self) -> bool {
        self.abort_requested.load(Ordering::Relaxed)
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
    fn speech_results_are_kept_for_twenty_seconds_and_only_while_analysis_runs() {
        use super::SpeechWindow;

        let state = AppState::new();
        let window = |end: f64| SpeechWindow { start: end - 0.5, end, speech: true };
        state.record_speech_window(window(10.0));
        state.record_speech_window(window(29.0));
        assert_eq!(state.audio_timeline().speech, None);
        state.set_speech_analysis_available(true);
        assert_eq!(state.audio_timeline().speech, Some(vec![window(10.0), window(29.0)]));
        // A result 20.5 s after the first drops it.
        state.record_speech_window(window(30.5));
        assert_eq!(state.speech_windows(), vec![window(29.0), window(30.5)]);
    }

    #[test]
    fn loudness_is_kept_for_two_seconds_whether_or_not_analysis_runs() {
        use super::LevelBlock;

        let state = AppState::new();
        let block = |end: f64| LevelBlock { start: end - 0.01, end, mean_square: 0.25 };
        state.record_level_block(block(10.0));
        state.record_level_block(block(11.5));
        assert_eq!(state.audio_timeline().levels, vec![block(10.0), block(11.5)]);
        // A block 2.5 s after the first drops it.
        state.record_level_block(block(12.5));
        assert_eq!(state.audio_timeline().levels, vec![block(11.5), block(12.5)]);
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
