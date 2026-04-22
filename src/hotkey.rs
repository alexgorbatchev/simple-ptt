#[path = "hotkey_macos.rs"]
mod platform;

use rdev::Key;
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use self::platform::run_hotkey_event_loop;
use crate::billing::BillingController;
use crate::hotkey_binding::{
    is_modifier_key, parse_hotkey_binding, parse_key, HotkeyBinding, HotkeyModifiers,
};
use crate::hotkey_capture::HotkeyCaptureController;
use crate::settings::LiveConfigStore;
use crate::state::{
    AppState, STATE_BUFFER_READY, STATE_ERROR, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING,
    STATE_TRANSFORMING,
};
use crate::transcription::TranscriptionController;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordHotkeyAction {
    StartRecording,
    StopAndPaste,
    StopAndTransformAndPaste,
    PasteBuffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HotkeyEvent {
    KeyPress(Key),
    KeyRelease(Key),
}

struct CurrentHotkeyConfig {
    auto_transform_enabled: bool,
    correction_key: Option<Key>,
    hold_ms: u64,
    record_hotkey: Option<HotkeyBinding>,
    transform_hotkey: Option<HotkeyBinding>,
    transformation_hotkey_enabled: bool,
}

pub fn spawn_hotkey_thread(
    state: Arc<AppState>,
    billing_controller: BillingController,
    controller: TranscriptionController,
    config_store: LiveConfigStore,
    hotkey_capture_controller: HotkeyCaptureController,
) {
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || {
            log::info!("hotkey thread started");

            let active_modifiers: Cell<HotkeyModifiers> = Cell::new(HotkeyModifiers::default());
            let press_time: Cell<Option<Instant>> = Cell::new(None);
            let record_hotkey_action: Cell<Option<RecordHotkeyAction>> = Cell::new(None);
            let correction_key_is_down: Cell<bool> = Cell::new(false);
            let transform_hotkey_is_down: Cell<bool> = Cell::new(false);
            let clipboard_insert_is_down: Cell<bool> = Cell::new(false);

            if let Err(error) = run_hotkey_event_loop(move |event| {
                let current_modifiers = active_modifiers.get();
                let settings_window_visible = hotkey_capture_controller.settings_window_visible();
                let handled = match event {
                    HotkeyEvent::KeyPress(key) => {
                        if hotkey_capture_controller.handle_key_press(key, current_modifiers) {
                            true
                        } else if settings_window_visible {
                            false
                        } else {
                            handle_key_press(
                                key,
                                current_modifiers,
                                &config_store,
                                &state,
                                &billing_controller,
                                &controller,
                                &press_time,
                                &record_hotkey_action,
                                &correction_key_is_down,
                                &transform_hotkey_is_down,
                                &clipboard_insert_is_down,
                            )
                        }
                    }
                    HotkeyEvent::KeyRelease(key) => {
                        if hotkey_capture_controller.handle_key_release(key) {
                            true
                        } else if settings_window_visible {
                            false
                        } else {
                            handle_key_release(
                                key,
                                &config_store,
                                &state,
                                &controller,
                                &press_time,
                                &record_hotkey_action,
                                &correction_key_is_down,
                                &transform_hotkey_is_down,
                                &clipboard_insert_is_down,
                            )
                        }
                    }
                };

                match event {
                    HotkeyEvent::KeyPress(key) if is_modifier_key(key) => {
                        active_modifiers.set(current_modifiers.with_key_pressed(key));
                    }
                    HotkeyEvent::KeyRelease(key) if is_modifier_key(key) => {
                        active_modifiers.set(current_modifiers.with_key_released(key));
                    }
                    _ => {}
                }

                handled
            }) {
                log::error!("global hotkey tap failed: {}", error);
            }
        })
        .expect("failed to spawn hotkey thread");
}

fn handle_key_press(
    key: Key,
    current_modifiers: HotkeyModifiers,
    config_store: &LiveConfigStore,
    state: &AppState,
    billing_controller: &BillingController,
    controller: &TranscriptionController,
    press_time: &Cell<Option<Instant>>,
    record_hotkey_action: &Cell<Option<RecordHotkeyAction>>,
    correction_key_is_down: &Cell<bool>,
    transform_hotkey_is_down: &Cell<bool>,
    clipboard_insert_is_down: &Cell<bool>,
) -> bool {
    let hotkey_config = current_hotkey_config(config_store);

    if key == Key::Escape
        && !hotkey_config
            .record_hotkey
            .map(|binding| binding.matches_press(Key::Escape, current_modifiers))
            .unwrap_or(false)
    {
        let current_state = state.get_state();
        if !matches!(
            current_state,
            STATE_RECORDING | STATE_PROCESSING | STATE_BUFFER_READY | STATE_TRANSFORMING
        ) {
            return false;
        }

        state.request_abort();
        state.dismiss_overlay();
        state.clear_overlay_text();
        state.set_overlay_text_opacity(1.0);
        press_time.set(None);
        record_hotkey_action.set(None);
        correction_key_is_down.set(false);
        transform_hotkey_is_down.set(false);
        clipboard_insert_is_down.set(false);

        match current_state {
            STATE_RECORDING => {
                if state.is_overlay_correction_active() {
                    match controller.stop_correction_session_and_apply() {
                        Ok(()) => log::info!("aborting correction recording"),
                        Err(error) => {
                            log::error!("failed to abort correction recording: {}", error);
                            state.set_state(STATE_ERROR);
                        }
                    }
                } else {
                    abort_recording(
                        state,
                        controller,
                        hotkey_config.auto_transform_enabled,
                        "escape abort",
                    );
                }
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

        return true;
    }

    if hotkey_config.correction_key == Some(key) {
        if correction_key_is_down.replace(true) {
            return true;
        }

        let current_state = state.get_state();
        if matches!(current_state, STATE_PROCESSING | STATE_TRANSFORMING) {
            correction_key_is_down.set(false);
            log::info!("ignoring correction while background work is still running");
            return false;
        }

        if is_modifier_key(key) && current_modifiers.any() {
            correction_key_is_down.set(false);
            return false;
        }

        let has_annotation_text = !state.overlay_text().trim().is_empty();

        match current_state {
            STATE_RECORDING if state.is_overlay_correction_active() => {
                return true;
            }
            STATE_RECORDING | STATE_BUFFER_READY if !has_annotation_text => {
                correction_key_is_down.set(false);
                log::info!("ignoring correction because no narrated annotation is available");
                return false;
            }
            STATE_RECORDING | STATE_BUFFER_READY => match controller.start_correction_session() {
                Ok(()) => {
                    billing_controller.refresh_month_to_date_spend();
                    state.restore_overlay();
                    state.set_overlay_correction_active(true);
                    state.clear_overlay_correction_text();
                    state.set_overlay_text_opacity(1.0);
                    state.set_state(STATE_RECORDING);
                    log::info!("correction recording started");
                }
                Err(start_error) => {
                    correction_key_is_down.set(false);
                    log::error!("failed to start correction: {}", start_error);
                    state.set_state(STATE_ERROR);
                }
            },
            _ => {
                correction_key_is_down.set(false);
                log::info!("ignoring correction because no buffered annotation is available");
                return false;
            }
        }

        return true;
    }

    if hotkey_config
        .record_hotkey
        .map(|binding| binding.matches_press(key, current_modifiers))
        .unwrap_or(false)
    {
        if press_time.get().is_some() {
            return true;
        }

        let current_state = state.get_state();
        if matches!(current_state, STATE_PROCESSING | STATE_TRANSFORMING) {
            log::info!("ignoring record hotkey while background work is still running");
            return true;
        }

        let action = match current_state {
            STATE_IDLE | STATE_ERROR => match controller.start_session() {
                Ok(()) => {
                    billing_controller.refresh_month_to_date_spend();
                    state.restore_overlay();
                    state.clear_overlay_text();
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
            STATE_RECORDING => Some(if hotkey_config.auto_transform_enabled {
                RecordHotkeyAction::StopAndTransformAndPaste
            } else {
                RecordHotkeyAction::StopAndPaste
            }),
            STATE_BUFFER_READY => Some(RecordHotkeyAction::PasteBuffer),
            _ => None,
        };

        if action.is_some() {
            press_time.set(Some(Instant::now()));
            record_hotkey_action.set(action);
            return true;
        }

        return false;
    }

    if hotkey_config.transformation_hotkey_enabled
        && hotkey_config
            .transform_hotkey
            .map(|binding| binding.matches_press(key, current_modifiers))
            .unwrap_or(false)
    {
        if transform_hotkey_is_down.replace(true) {
            return true;
        }

        let current_state = state.get_state();
        if !matches!(current_state, STATE_RECORDING | STATE_BUFFER_READY) {
            log::info!(
                "ignoring transformation hotkey because no transformable transcript is available"
            );
        }

        return true;
    }

    if is_clipboard_insert_shortcut(key, current_modifiers) {
        if !state.is_recording() {
            return false;
        }

        if state.is_overlay_correction_active() {
            return true;
        }

        if clipboard_insert_is_down.replace(true) {
            return true;
        }

        match controller.insert_clipboard_text() {
            Ok(()) => {
                log::info!("checkpointing the active transcript and inserting clipboard text");
            }
            Err(error) => {
                clipboard_insert_is_down.set(false);
                log::error!("failed to queue clipboard insertion: {}", error);
                state.set_state(STATE_ERROR);
            }
        }

        return true;
    }

    false
}

fn handle_key_release(
    key: Key,
    config_store: &LiveConfigStore,
    state: &AppState,
    controller: &TranscriptionController,
    press_time: &Cell<Option<Instant>>,
    record_hotkey_action: &Cell<Option<RecordHotkeyAction>>,
    correction_key_is_down: &Cell<bool>,
    transform_hotkey_is_down: &Cell<bool>,
    clipboard_insert_is_down: &Cell<bool>,
) -> bool {
    let hotkey_config = current_hotkey_config(config_store);

    if hotkey_config.correction_key == Some(key) {
        let was_down = correction_key_is_down.replace(false);
        if !was_down {
            return false;
        }

        if state.is_overlay_correction_active() {
            match controller.stop_correction_session_and_apply() {
                Ok(()) => {
                    state.set_overlay_correction_active(false);
                    state.set_state(STATE_PROCESSING);
                    log::info!("stopping correction and applying it");
                }
                Err(error) => {
                    log::error!("failed to stop correction: {}", error);
                    state.set_state(STATE_ERROR);
                }
            }
        }

        return true;
    }

    if hotkey_config
        .record_hotkey
        .map(|binding| binding.matches_release(key))
        .unwrap_or(false)
    {
        let Some(pressed_at) = press_time.get() else {
            return true;
        };

        let Some(action) = record_hotkey_action.get() else {
            press_time.set(None);
            return true;
        };

        press_time.set(None);
        record_hotkey_action.set(None);

        match action {
            RecordHotkeyAction::StartRecording => {
                if pressed_at.elapsed() >= Duration::from_millis(hotkey_config.hold_ms) {
                    if hotkey_config.auto_transform_enabled {
                        stop_recording_and_transform_and_paste(state, controller, "hold release");
                    } else {
                        stop_recording_and_paste(state, controller, "hold release");
                    }
                } else {
                    log::info!("recording (tap to stop)");
                }
            }
            RecordHotkeyAction::StopAndPaste => {
                stop_recording_and_paste(state, controller, "tap");
            }
            RecordHotkeyAction::StopAndTransformAndPaste => {
                stop_recording_and_transform_and_paste(state, controller, "tap");
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

        return true;
    }

    if key == Key::KeyV && clipboard_insert_is_down.replace(false) {
        return true;
    }

    if hotkey_config.transformation_hotkey_enabled
        && hotkey_config
            .transform_hotkey
            .map(|binding| binding.matches_release(key))
            .unwrap_or(false)
    {
        let was_down = transform_hotkey_is_down.replace(false);
        if !was_down {
            return true;
        }

        match state.get_state() {
            STATE_RECORDING if state.is_overlay_correction_active() => {
                log::info!("ignoring transform hotkey while correction is recording");
            }
            STATE_RECORDING => match controller.stop_session_and_transform_and_resume() {
                Ok(()) => {
                    state.set_overlay_text_opacity(0.02);
                    state.set_state(STATE_PROCESSING);
                    log::info!("stopping recording, transforming buffered text, and resuming");
                }
                Err(error) => {
                    log::error!("failed to stop recording for transformation: {}", error);
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

        return true;
    }

    false
}

fn current_hotkey_config(config_store: &LiveConfigStore) -> CurrentHotkeyConfig {
    let config = config_store.current();
    let correction_key = parse_correction_key(config.ui.correction_key.as_str());
    let record_hotkey = parse_hotkey_binding(&config.ui.hotkey).ok();
    let transform_hotkey = config
        .resolve_transformation_config()
        .ok()
        .and_then(|_| parse_hotkey_binding(&config.transformation.hotkey).ok());
    let transformation_hotkey_enabled =
        transform_hotkey.is_some() && transform_hotkey != record_hotkey;

    CurrentHotkeyConfig {
        auto_transform_enabled: config.transformation.auto
            && config.resolve_transformation_config().is_ok(),
        hold_ms: config.mic.hold_ms,
        correction_key,
        record_hotkey,
        transform_hotkey,
        transformation_hotkey_enabled,
    }
}

fn is_clipboard_insert_shortcut(key: Key, current_modifiers: HotkeyModifiers) -> bool {
    key == Key::KeyV
        && current_modifiers.meta
        && !current_modifiers.shift
        && !current_modifiers.control
        && !current_modifiers.alt
}

fn parse_correction_key(raw: &str) -> Option<Key> {
    parse_key(raw.trim())
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

fn stop_recording_and_transform_and_paste(
    state: &AppState,
    controller: &TranscriptionController,
    reason: &str,
) {
    if !state.is_recording() {
        return;
    }

    match controller.stop_session_and_transform_and_paste() {
        Ok(()) => {
            state.set_overlay_text_opacity(0.02);
            state.set_state(STATE_PROCESSING);
            log::info!("recording stopped ({})", reason);
        }
        Err(stop_error) => {
            log::error!("failed to stop recording: {}", stop_error);
            state.set_state(STATE_ERROR);
        }
    }
}

fn abort_recording(
    state: &AppState,
    controller: &TranscriptionController,
    auto_transform_enabled: bool,
    reason: &str,
) {
    if !state.is_recording() {
        return;
    }

    let stop_result = if auto_transform_enabled {
        controller.stop_session_and_transform_and_paste()
    } else {
        controller.stop_session_and_paste()
    };

    match stop_result {
        Ok(()) => {
            state.set_state(STATE_IDLE);
            log::info!("recording aborted ({})", reason);
        }
        Err(stop_error) => {
            log::error!("failed to abort recording: {}", stop_error);
            state.set_state(STATE_ERROR);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_clipboard_insert_shortcut, parse_correction_key};
    use crate::hotkey_binding::HotkeyModifiers;
    use rdev::Key;

    #[test]
    fn clipboard_insert_shortcut_requires_exact_command_v() {
        assert!(is_clipboard_insert_shortcut(
            Key::KeyV,
            HotkeyModifiers {
                meta: true,
                ..HotkeyModifiers::default()
            }
        ));
        assert!(!is_clipboard_insert_shortcut(
            Key::KeyV,
            HotkeyModifiers {
                meta: true,
                shift: true,
                ..HotkeyModifiers::default()
            }
        ));
        assert!(!is_clipboard_insert_shortcut(
            Key::KeyV,
            HotkeyModifiers::default()
        ));
        assert!(!is_clipboard_insert_shortcut(
            Key::KeyC,
            HotkeyModifiers {
                meta: true,
                ..HotkeyModifiers::default()
            }
        ));
    }

    #[test]
    fn correction_key_parses_supported_single_keys() {
        assert_eq!(parse_correction_key("LeftMeta"), Some(Key::MetaLeft));
        assert_eq!(parse_correction_key("RightMeta"), Some(Key::MetaRight));
        assert_eq!(parse_correction_key("F7"), Some(Key::F7));
        assert_eq!(parse_correction_key("Cmd"), None);
    }
}
