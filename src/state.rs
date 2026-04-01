use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

pub const STATE_IDLE: u8 = 0;
pub const STATE_RECORDING: u8 = 1;
pub const STATE_PROCESSING: u8 = 2;
pub const STATE_ERROR: u8 = 3;

pub struct AppState {
    state: AtomicU8,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
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
}
