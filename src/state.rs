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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DeepgramConnectionStatus {
    #[default]
    Unknown,
    Disconnected,
    Connected,
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

/// The last Deepgram connection result and the API key it was measured with.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeepgramConnection {
    status: DeepgramConnectionStatus,
    api_key: Option<DeepgramApiKeyFingerprint>,
}

impl DeepgramConnection {
    /// The status as it applies to `current_api_key`, the key the app would
    /// use now. A result measured with any other key (a key that has since
    /// been replaced, or one only checked in Settings and never saved) says
    /// nothing about it, so it reads as Unknown.
    pub fn status_for(
        &self,
        current_api_key: Option<DeepgramApiKeyFingerprint>,
    ) -> DeepgramConnectionStatus {
        match current_api_key {
            Some(current_api_key) if self.api_key == Some(current_api_key) => self.status,
            _ => DeepgramConnectionStatus::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MicMeterSnapshot {
    pub clip_event_counter: u32,
    pub level: u8,
    pub peak: u8,
    pub mic_active: bool,
}

#[derive(Debug)]
pub struct AppState {
    abort_requested: AtomicBool,
    clip_event_counter: AtomicU32,
    deepgram_connection: Mutex<DeepgramConnection>,
    mic_meter_level: AtomicU8,
    mic_meter_peak: AtomicU8,
    overlay_dismissed: AtomicBool,
    overlay_correction_active: AtomicBool,
    overlay_correction_text: Mutex<OverlayText>,
    overlay_window_visible: AtomicBool,
    settings_window_visible: AtomicBool,
    overlay_footer_text: Mutex<Arc<str>>,
    overlay_text: Mutex<OverlayText>,
    overlay_error_text: Mutex<Arc<str>>,
    overlay_text_opacity: AtomicU8,
    preview_mic_gain: AtomicU32,
    state: AtomicU8,
    mic_active: AtomicBool,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            abort_requested: AtomicBool::new(false),
            clip_event_counter: AtomicU32::new(0),
            deepgram_connection: Mutex::new(DeepgramConnection::default()),
            mic_meter_level: AtomicU8::new(0),
            mic_meter_peak: AtomicU8::new(0),
            overlay_dismissed: AtomicBool::new(false),
            overlay_correction_active: AtomicBool::new(false),
            overlay_correction_text: Mutex::new(OverlayText::default()),
            overlay_window_visible: AtomicBool::new(false),
            settings_window_visible: AtomicBool::new(false),
            overlay_footer_text: Mutex::new(Arc::from("")),
            overlay_text: Mutex::new(OverlayText::default()),
            overlay_error_text: Mutex::new(Arc::from("")),
            overlay_text_opacity: AtomicU8::new(u8::MAX),
            preview_mic_gain: AtomicU32::new(f32::to_bits(f32::NAN)),
            state: AtomicU8::new(STATE_IDLE),
            mic_active: AtomicBool::new(false),
        })
    }

    pub fn is_recording(&self) -> bool {
        self.get_state() == STATE_RECORDING
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

    pub fn set_deepgram_connection_status(
        &self,
        status: DeepgramConnectionStatus,
        api_key: DeepgramApiKeyFingerprint,
    ) {
        if let Ok(mut connection) = self.deepgram_connection.lock() {
            *connection = DeepgramConnection {
                status,
                api_key: Some(api_key),
            };
        }
    }

    pub fn deepgram_connection(&self) -> DeepgramConnection {
        self.deepgram_connection
            .lock()
            .map(|connection| *connection)
            .unwrap_or_default()
    }

    pub fn set_state(&self, state: u8) {
        self.state.store(state, Ordering::Relaxed);
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

    pub fn set_overlay_footer_text(&self, overlay_footer_text: impl Into<String>) {
        if let Ok(mut current_overlay_footer_text) = self.overlay_footer_text.lock() {
            *current_overlay_footer_text = Arc::from(overlay_footer_text.into());
        }
    }

    pub fn overlay_footer_text(&self) -> Arc<str> {
        self.overlay_footer_text
            .lock()
            .map(|overlay_footer_text| overlay_footer_text.clone())
            .unwrap_or_else(|_| Arc::from(""))
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

fn normalized_meter_value(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * u8::MAX as f32).round() as u8
}

#[cfg(test)]
mod tests {
    use super::{
        AppState, DeepgramApiKeyFingerprint, DeepgramConnectionStatus, STATE_IDLE, STATE_RECORDING,
    };

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

    fn key(api_key: &str) -> Option<DeepgramApiKeyFingerprint> {
        Some(DeepgramApiKeyFingerprint::of(api_key))
    }

    #[test]
    fn deepgram_connection_status_round_trips_for_the_measured_key() {
        let state = AppState::new();

        assert_eq!(
            state.deepgram_connection().status_for(key("key-a")),
            DeepgramConnectionStatus::Unknown
        );

        for status in [
            DeepgramConnectionStatus::Connected,
            DeepgramConnectionStatus::Disconnected,
        ] {
            state.set_deepgram_connection_status(status, DeepgramApiKeyFingerprint::of("key-a"));
            assert_eq!(state.deepgram_connection().status_for(key("key-a")), status);
        }
    }

    /// A Settings check of key A, then saving key A: the check's result is the
    /// status of the saved key. Saving settings never writes the status; the
    /// overlay applies it to the saved key through `status_for`, so this is
    /// where the outcome is decided.
    #[test]
    fn check_result_survives_saving_the_checked_key() {
        let state = AppState::new();
        state.set_deepgram_connection_status(
            DeepgramConnectionStatus::Connected,
            DeepgramApiKeyFingerprint::of("key-a"),
        );

        assert_eq!(
            state.deepgram_connection().status_for(key(" key-a ")),
            DeepgramConnectionStatus::Connected
        );
    }

    /// A status measured with key A (by a recording, a check, or a billing
    /// refresh that finishes after the save) says nothing once key B is saved.
    #[test]
    fn status_measured_with_a_replaced_key_reads_as_unknown() {
        let state = AppState::new();
        for status in [
            DeepgramConnectionStatus::Connected,
            DeepgramConnectionStatus::Disconnected,
        ] {
            state.set_deepgram_connection_status(status, DeepgramApiKeyFingerprint::of("key-a"));

            assert_eq!(
                state.deepgram_connection().status_for(key("key-b")),
                DeepgramConnectionStatus::Unknown
            );
        }
    }

    /// A Settings check of unsaved key B, then Cancel: key A stays in use, and
    /// the footer must not show key B's result as key A's.
    #[test]
    fn check_of_an_unsaved_key_does_not_speak_for_the_saved_key() {
        let state = AppState::new();
        state.set_deepgram_connection_status(
            DeepgramConnectionStatus::Disconnected,
            DeepgramApiKeyFingerprint::of("key-a"),
        );
        state.set_deepgram_connection_status(
            DeepgramConnectionStatus::Connected,
            DeepgramApiKeyFingerprint::of("unsaved-key-b"),
        );

        assert_eq!(
            state.deepgram_connection().status_for(key("key-a")),
            DeepgramConnectionStatus::Unknown
        );
    }

    #[test]
    fn deepgram_connection_status_is_unknown_without_a_current_key() {
        let state = AppState::new();
        state.set_deepgram_connection_status(
            DeepgramConnectionStatus::Connected,
            DeepgramApiKeyFingerprint::of("key-a"),
        );

        assert_eq!(
            state.deepgram_connection().status_for(None),
            DeepgramConnectionStatus::Unknown
        );
    }
}
