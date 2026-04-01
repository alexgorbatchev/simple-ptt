use rdev::{grab, Event, EventType, Key};
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::state::{AppState, STATE_ERROR, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING};
use crate::transcription::TranscriptionController;

pub fn spawn_hotkey_thread(
    state: Arc<AppState>,
    controller: TranscriptionController,
    hotkey_name: &str,
    hold_ms: u64,
) {
    let hotkey = parse_key(hotkey_name);
    if hotkey.is_none() {
        log::warn!("hotkey '{}' not recognized", hotkey_name);
    }

    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || {
            let hold_threshold = Duration::from_millis(hold_ms);
            log::info!("hotkey thread started (hold threshold: {}ms)", hold_ms);

            let press_time: Cell<Option<Instant>> = Cell::new(None);
            let was_idle_on_press: Cell<bool> = Cell::new(false);

            if let Err(error) = grab(move |event: Event| -> Option<Event> {
                match event.event_type {
                    EventType::KeyPress(key) if hotkey == Some(key) => {
                        if press_time.get().is_some() {
                            return None;
                        }

                        let current_state = state.get_state();
                        if current_state == STATE_PROCESSING {
                            log::info!(
                                "ignoring hotkey while a transcript is still being finalized"
                            );
                            return None;
                        }

                        press_time.set(Some(Instant::now()));
                        let should_start_recording =
                            matches!(current_state, STATE_IDLE | STATE_ERROR);
                        was_idle_on_press.set(should_start_recording);

                        if should_start_recording {
                            match controller.start_session() {
                                Ok(()) => {
                                    state.set_state(STATE_RECORDING);
                                    log::info!("recording started");
                                }
                                Err(start_error) => {
                                    log::error!("failed to start recording: {}", start_error);
                                    state.set_state(STATE_ERROR);
                                }
                            }
                        }

                        None
                    }
                    EventType::KeyRelease(key) if hotkey == Some(key) => {
                        if let Some(pressed_at) = press_time.get() {
                            press_time.set(None);

                            if pressed_at.elapsed() >= hold_threshold {
                                stop_recording(&state, &controller, "hold release");
                            } else if was_idle_on_press.get() {
                                log::info!("recording (tap to stop)");
                            } else {
                                stop_recording(&state, &controller, "tap");
                            }
                        }

                        None
                    }
                    _ => Some(event),
                }
            }) {
                log::error!("rdev grab failed: {:?}", error);
            }
        })
        .expect("failed to spawn hotkey thread");
}

fn stop_recording(state: &AppState, controller: &TranscriptionController, reason: &str) {
    if !state.is_recording() {
        return;
    }

    match controller.stop_session() {
        Ok(()) => {
            state.set_state(STATE_PROCESSING);
            log::info!("recording stopped ({})", reason);
        }
        Err(stop_error) => {
            log::error!("failed to stop recording: {}", stop_error);
            state.set_state(STATE_ERROR);
        }
    }
}

fn parse_key(name: &str) -> Option<Key> {
    match name {
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        "Escape" | "Esc" => Some(Key::Escape),
        "Space" => Some(Key::Space),
        "Tab" => Some(Key::Tab),
        "CapsLock" => Some(Key::CapsLock),
        "LeftShift" | "LShift" => Some(Key::ShiftLeft),
        "RightShift" | "RShift" => Some(Key::ShiftRight),
        "LeftControl" | "LCtrl" => Some(Key::ControlLeft),
        "RightControl" | "RCtrl" => Some(Key::ControlRight),
        "LeftAlt" | "LAlt" | "LeftOption" => Some(Key::Alt),
        "RightAlt" | "RAlt" | "RightOption" => Some(Key::AltGr),
        "LeftMeta" | "LeftCommand" | "LCmd" => Some(Key::MetaLeft),
        "RightMeta" | "RightCommand" | "RCmd" => Some(Key::MetaRight),
        "Return" | "Enter" => Some(Key::Return),
        "Backspace" | "Delete" => Some(Key::Backspace),
        "ForwardDelete" => Some(Key::Delete),
        "Home" => Some(Key::Home),
        "End" => Some(Key::End),
        "PageUp" => Some(Key::PageUp),
        "PageDown" => Some(Key::PageDown),
        "UpArrow" | "Up" => Some(Key::UpArrow),
        "DownArrow" | "Down" => Some(Key::DownArrow),
        "LeftArrow" | "Left" => Some(Key::LeftArrow),
        "RightArrow" | "Right" => Some(Key::RightArrow),
        _ => {
            log::warn!("unknown key name: '{}'", name);
            None
        }
    }
}
