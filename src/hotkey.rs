use rdev::{grab, Event, EventType, Key};
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::billing::BillingController;
use crate::state::{
    AppState, STATE_BUFFER_READY, STATE_ERROR, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING,
    STATE_TRANSFORMING,
};
use crate::transcription::TranscriptionController;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordHotkeyAction {
    StartRecording,
    StopAndPaste,
    PasteBuffer,
}

pub fn spawn_hotkey_thread(
    state: Arc<AppState>,
    billing_controller: BillingController,
    controller: TranscriptionController,
    record_hotkey_name: &str,
    transform_hotkey_name: Option<&str>,
    hold_ms: u64,
) {
    let record_hotkey_name = record_hotkey_name.to_owned();
    let transform_hotkey_name = transform_hotkey_name.map(str::to_owned);

    let record_hotkey = parse_key(&record_hotkey_name);
    if record_hotkey.is_none() {
        log::warn!("record hotkey '{}' not recognized", record_hotkey_name);
    }

    let transform_hotkey = transform_hotkey_name.as_deref().and_then(|hotkey_name| {
        let parsed_hotkey = parse_key(hotkey_name);
        if parsed_hotkey.is_none() {
            log::warn!("transformation hotkey '{}' not recognized", hotkey_name);
        }
        parsed_hotkey
    });

    let transformation_hotkey_enabled =
        transform_hotkey.is_some() && transform_hotkey != record_hotkey;
    if !transformation_hotkey_enabled && transform_hotkey == record_hotkey {
        log::warn!(
            "transformation hotkey '{}' conflicts with record hotkey '{}'; transformation hotkey disabled",
            transform_hotkey_name.as_deref().unwrap_or("disabled"),
            record_hotkey_name
        );
    }

    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || {
            let hold_threshold = Duration::from_millis(hold_ms);
            log::info!(
                "hotkey thread started (record={}, transform={}, hold threshold: {}ms)",
                record_hotkey_name,
                if transformation_hotkey_enabled {
                    transform_hotkey_name.clone().unwrap_or_else(|| "disabled".to_owned())
                } else {
                    "disabled".to_owned()
                },
                hold_ms
            );

            let press_time: Cell<Option<Instant>> = Cell::new(None);
            let record_hotkey_action: Cell<Option<RecordHotkeyAction>> = Cell::new(None);
            let transform_hotkey_is_down: Cell<bool> = Cell::new(false);

            if let Err(error) = grab(move |event: Event| -> Option<Event> {
                match event.event_type {
                    EventType::KeyPress(Key::Escape) if record_hotkey != Some(Key::Escape) => {
                        let current_state = state.get_state();
                        if !matches!(
                            current_state,
                            STATE_RECORDING
                                | STATE_PROCESSING
                                | STATE_BUFFER_READY
                                | STATE_TRANSFORMING
                        ) {
                            return Some(event);
                        }

                        state.request_abort();
                        state.set_overlay_text("Canceling…");
                        state.set_overlay_text_opacity(1.0);
                        press_time.set(None);
                        record_hotkey_action.set(None);
                        transform_hotkey_is_down.set(false);

                        match current_state {
                            STATE_RECORDING => {
                                stop_recording_and_paste(&state, &controller, "escape abort");
                            }
                            STATE_BUFFER_READY => match controller.discard_buffer() {
                                Ok(()) => log::info!("buffer discarded"),
                                Err(error) => log::error!("failed to discard buffer: {}", error),
                            },
                            STATE_PROCESSING | STATE_TRANSFORMING => {
                                log::info!("abort requested while background work is in progress");
                            }
                            _ => {}
                        }

                        None
                    }
                    EventType::KeyPress(key) if record_hotkey == Some(key) => {
                        if press_time.get().is_some() {
                            return None;
                        }

                        let current_state = state.get_state();
                        if matches!(current_state, STATE_PROCESSING | STATE_TRANSFORMING) {
                            log::info!(
                                "ignoring record hotkey while background work is still running"
                            );
                            return None;
                        }

                        let action = match current_state {
                            STATE_IDLE | STATE_ERROR => match controller.start_session() {
                                Ok(()) => {
                                    billing_controller.refresh_month_to_date_spend();
                                    state.set_overlay_text_opacity(1.0);
                                    state.set_state(STATE_RECORDING);
                                    log::info!("recording started");
                                    Some(RecordHotkeyAction::StartRecording)
                                }
                                Err(start_error) => {
                                    log::error!("failed to start recording: {}", start_error);
                                    state.set_state(STATE_ERROR);
                                    None
                                }
                            },
                            STATE_RECORDING => Some(RecordHotkeyAction::StopAndPaste),
                            STATE_BUFFER_READY => Some(RecordHotkeyAction::PasteBuffer),
                            _ => None,
                        };

                        if action.is_some() {
                            press_time.set(Some(Instant::now()));
                            record_hotkey_action.set(action);
                            return None;
                        }

                        Some(event)
                    }
                    EventType::KeyRelease(key) if record_hotkey == Some(key) => {
                        let Some(pressed_at) = press_time.get() else {
                            return None;
                        };

                        let Some(action) = record_hotkey_action.get() else {
                            press_time.set(None);
                            return None;
                        };

                        press_time.set(None);
                        record_hotkey_action.set(None);

                        match action {
                            RecordHotkeyAction::StartRecording => {
                                if pressed_at.elapsed() >= hold_threshold {
                                    stop_recording_and_paste(&state, &controller, "hold release");
                                } else {
                                    log::info!("recording (tap to stop)");
                                }
                            }
                            RecordHotkeyAction::StopAndPaste => {
                                stop_recording_and_paste(&state, &controller, "tap");
                            }
                            RecordHotkeyAction::PasteBuffer => match controller.paste_buffer() {
                                Ok(()) => {
                                    state.set_state(STATE_PROCESSING);
                                    log::info!("pasting buffered text");
                                }
                                Err(error) => {
                                    log::error!("failed to paste buffered text: {}", error);
                                    state.set_state(STATE_ERROR);
                                }
                            },
                        }

                        None
                    }
                    EventType::KeyPress(key)
                        if transformation_hotkey_enabled && transform_hotkey == Some(key) =>
                    {
                        if transform_hotkey_is_down.replace(true) {
                            return None;
                        }

                        let current_state = state.get_state();
                        if !matches!(current_state, STATE_RECORDING | STATE_BUFFER_READY) {
                            log::info!(
                                "ignoring transformation hotkey because no transformable transcript is available"
                            );
                        }

                        None
                    }
                    EventType::KeyRelease(key)
                        if transformation_hotkey_enabled && transform_hotkey == Some(key) =>
                    {
                        let was_down = transform_hotkey_is_down.replace(false);
                        if !was_down {
                            return None;
                        }

                        match state.get_state() {
                            STATE_RECORDING => match controller.stop_session_and_transform() {
                                Ok(()) => {
                                    state.set_overlay_text_opacity(0.02);
                                    state.set_state(STATE_PROCESSING);
                                    log::info!("stopping recording and transforming buffered text");
                                }
                                Err(error) => {
                                    log::error!(
                                        "failed to stop recording for transformation: {}",
                                        error
                                    );
                                    state.set_state(STATE_ERROR);
                                }
                            },
                            STATE_BUFFER_READY => match controller.transform_buffer() {
                                Ok(()) => {
                                    state.set_state(STATE_TRANSFORMING);
                                    log::info!("transforming buffered text");
                                }
                                Err(error) => {
                                    log::error!("failed to start transformation: {}", error);
                                }
                            },
                            _ => {}
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

fn stop_recording_and_paste(state: &AppState, controller: &TranscriptionController, reason: &str) {
    if !state.is_recording() {
        return;
    }

    match controller.stop_session_and_paste() {
        Ok(()) => {
            state.set_overlay_text_opacity(1.0);
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
