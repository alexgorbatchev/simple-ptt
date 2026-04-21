use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

pub const STATE_IDLE: u8 = 0;
pub const STATE_RECORDING: u8 = 1;
pub const STATE_PROCESSING: u8 = 2;
pub const STATE_BUFFER_READY: u8 = 3;
pub const STATE_TRANSFORMING: u8 = 4;
pub const STATE_ERROR: u8 = 5;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum DeepgramConnectionStatus {
    #[default]
    Unknown = 0,
    Disconnected = 1,
    Connected = 2,
}

impl DeepgramConnectionStatus {
    fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::Disconnected,
            2 => Self::Connected,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MicMeterSnapshot {
    pub clip_event_counter: u32,
    pub level: u8,
    pub peak: u8,
}

#[derive(Debug)]
pub struct AppState {
    abort_requested: AtomicBool,
    clip_event_counter: AtomicU32,
    deepgram_connection_status: AtomicU8,
    mic_meter_level: AtomicU8,
    mic_meter_peak: AtomicU8,
    overlay_dismissed: AtomicBool,
    overlay_correction_active: AtomicBool,
    overlay_correction_text: Mutex<Arc<str>>,
    overlay_window_visible: AtomicBool,
    settings_window_visible: AtomicBool,
    overlay_footer_text: Mutex<Arc<str>>,
    overlay_text: Mutex<Arc<str>>,
    overlay_text_opacity: AtomicU8,
    preview_mic_gain: AtomicU32,
    state: AtomicU8,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            abort_requested: AtomicBool::new(false),
            clip_event_counter: AtomicU32::new(0),
            deepgram_connection_status: AtomicU8::new(DeepgramConnectionStatus::Unknown as u8),
            mic_meter_level: AtomicU8::new(0),
            mic_meter_peak: AtomicU8::new(0),
            overlay_dismissed: AtomicBool::new(false),
            overlay_correction_active: AtomicBool::new(false),
            overlay_correction_text: Mutex::new(Arc::from("")),
            overlay_window_visible: AtomicBool::new(false),
            settings_window_visible: AtomicBool::new(false),
            overlay_footer_text: Mutex::new(Arc::from("")),
            overlay_text: Mutex::new(Arc::from("")),
            overlay_text_opacity: AtomicU8::new(u8::MAX),
            preview_mic_gain: AtomicU32::new(f32::to_bits(f32::NAN)),
            state: AtomicU8::new(STATE_IDLE),
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

    pub fn set_deepgram_connection_status(&self, status: DeepgramConnectionStatus) {
        self.deepgram_connection_status
            .store(status as u8, Ordering::Relaxed);
    }

    pub fn deepgram_connection_status(&self) -> DeepgramConnectionStatus {
        DeepgramConnectionStatus::from_raw(self.deepgram_connection_status.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, state: u8) {
        self.state.store(state, Ordering::Relaxed);
        if state != STATE_RECORDING && !self.is_settings_window_visible() {
            self.clear_mic_meter();
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
        self.overlay_correction_active.store(active, Ordering::Relaxed);
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
        if let Ok(mut current_overlay_text) = self.overlay_text.lock() {
            *current_overlay_text = Arc::from(overlay_text.into());
        }
    }

    pub fn clear_overlay_text(&self) {
        self.set_overlay_text(String::new());
    }

    pub fn set_overlay_correction_text(&self, overlay_correction_text: impl Into<String>) {
        if let Ok(mut current_overlay_correction_text) = self.overlay_correction_text.lock() {
            *current_overlay_correction_text = Arc::from(overlay_correction_text.into());
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
        self.overlay_text
            .lock()
            .map(|overlay_text| overlay_text.clone())
            .unwrap_or_else(|_| Arc::from(""))
    }

    pub fn overlay_correction_text(&self) -> Arc<str> {
        self.overlay_correction_text
            .lock()
            .map(|overlay_correction_text| overlay_correction_text.clone())
            .unwrap_or_else(|_| Arc::from(""))
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
        }
    }
}

fn normalized_meter_value(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * u8::MAX as f32).round() as u8
}

#[cfg(test)]
mod tests {
    use super::{AppState, DeepgramConnectionStatus, STATE_IDLE, STATE_RECORDING};

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
    fn deepgram_connection_status_round_trips() {
        let state = AppState::new();

        assert_eq!(
            state.deepgram_connection_status(),
            DeepgramConnectionStatus::Unknown
        );

        state.set_deepgram_connection_status(DeepgramConnectionStatus::Connected);
        assert_eq!(
            state.deepgram_connection_status(),
            DeepgramConnectionStatus::Connected
        );

        state.set_deepgram_connection_status(DeepgramConnectionStatus::Disconnected);
        assert_eq!(
            state.deepgram_connection_status(),
            DeepgramConnectionStatus::Disconnected
        );
    }
}
