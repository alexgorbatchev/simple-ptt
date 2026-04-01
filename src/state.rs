use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

pub const STATE_IDLE: u8 = 0;
pub const STATE_RECORDING: u8 = 1;
pub const STATE_PROCESSING: u8 = 2;
pub const STATE_ERROR: u8 = 3;

pub struct AppState {
    overlay_footer_text: Mutex<String>,
    overlay_text: Mutex<String>,
    state: AtomicU8,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            overlay_footer_text: Mutex::new(String::new()),
            overlay_text: Mutex::new(String::new()),
            state: AtomicU8::new(STATE_IDLE),
        })
    }

    pub fn is_recording(&self) -> bool {
        self.get_state() == STATE_RECORDING
    }

    pub fn set_state(&self, state: u8) {
        self.state.store(state, Ordering::Relaxed);
    }

    pub fn get_state(&self) -> u8 {
        self.state.load(Ordering::Relaxed)
    }

    pub fn set_overlay_text(&self, overlay_text: impl Into<String>) {
        if let Ok(mut current_overlay_text) = self.overlay_text.lock() {
            *current_overlay_text = overlay_text.into();
        }
    }

    pub fn clear_overlay_text(&self) {
        self.set_overlay_text(String::new());
    }

    pub fn overlay_text(&self) -> String {
        self.overlay_text
            .lock()
            .map(|overlay_text| overlay_text.clone())
            .unwrap_or_default()
    }

    pub fn set_overlay_footer_text(&self, overlay_footer_text: impl Into<String>) {
        if let Ok(mut current_overlay_footer_text) = self.overlay_footer_text.lock() {
            *current_overlay_footer_text = overlay_footer_text.into();
        }
    }

    pub fn overlay_footer_text(&self) -> String {
        self.overlay_footer_text
            .lock()
            .map(|overlay_footer_text| overlay_footer_text.clone())
            .unwrap_or_default()
    }
}
