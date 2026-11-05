#[path = "hotkey_macos.rs"]
mod platform;

use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use self::platform::run_hotkey_event_loop;
use crate::audio::NO_MICROPHONE_MESSAGE;
use crate::config::Config;
use crate::correction_binding::CorrectionBinding;
use crate::hotkey_binding::{parse_hotkey_binding, HotkeyBinding, HotkeyModifiers};
use crate::hotkey_capture::HotkeyCaptureController;
use crate::key::Key;
use crate::settings::LiveConfigStore;
use crate::state::{
    AppState, STATE_BUFFER_READY, STATE_ERROR, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING,
    STATE_TRANSFORMING,
};
use crate::transcription::{StartSessionError, TranscriptionController};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordHotkeyAction {
    StartRecording,
    StopAndPaste,
    StopAndTransformAndPaste,
}

#[derive(Clone, Copy)]
enum CorrectionOrigin {
    Dictation,
    Buffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HotkeyEvent {
    /// A key went down, with the modifiers held as it did (for a modifier
    /// key, the others), read from the event itself.
    KeyPress(Key, HotkeyModifiers),
    /// A key went up, with the modifiers still held after its release.
    KeyRelease(Key, HotkeyModifiers),
}

struct CurrentHotkeyConfig {
    auto_transform_enabled: bool,
    correction_key: Option<CorrectionBinding>,
    hold_ms: u64,
    /// Why recording cannot start with the configured `mic.gain`.
    mic_gain_problem: Option<String>,
    record_hotkey: Option<HotkeyBinding>,
    transform_hotkey: Option<HotkeyBinding>,
}

/// The bindings in effect for a config; `None` is a binding that is off.
/// The hotkey thread and the overlay's shortcut hint both read this, so the
/// hint advertises only shortcuts that work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActiveBindings {
    pub record_hotkey: Option<HotkeyBinding>,
    pub correction_key: Option<CorrectionBinding>,
    pub transform_hotkey: Option<HotkeyBinding>,
    /// Whether the transformation config resolved, independent of whether
    /// its hotkey is on.
    pub transformation_resolved: bool,
}

impl ActiveBindings {
    /// A binding that does not parse is off, and so is one that
    /// `config::problems` reports as sharing keys with another: the
    /// correction key when it overlaps the record hotkey, and the transform
    /// hotkey when transformation does not resolve, when it equals the record
    /// hotkey, or when it overlaps the configured correction key.
    pub fn from_config(config: &Config) -> Self {
        let record_hotkey = parse_hotkey_binding(&config.ui.hotkey).ok();
        let configured_correction_key = parse_correction_key(config.ui.correction_key.as_str());
        let correction_key = configured_correction_key
            .filter(|key| !record_hotkey.is_some_and(|record_hotkey| key.overlaps(record_hotkey)));
        let transformation_resolved = config.resolve_transformation_config().is_ok();
        let transform_hotkey = parse_hotkey_binding(&config.transformation.hotkey)
            .ok()
            .filter(|transform_hotkey| {
                transformation_resolved
                    && Some(*transform_hotkey) != record_hotkey
                    && !configured_correction_key.is_some_and(|key| key.overlaps(*transform_hotkey))
            });

        Self {
            record_hotkey,
            correction_key,
            transform_hotkey,
            transformation_resolved,
        }
    }
}

/// What the hotkey thread remembers between key events.
#[derive(Default)]
pub(crate) struct HotkeyTracking {
    press_time: Cell<Option<Instant>>,
    record_hotkey_action: Cell<Option<RecordHotkeyAction>>,
    correction_key_origin: Cell<Option<CorrectionOrigin>>,
    transform_hotkey_is_down: Cell<bool>,
    clipboard_insert_is_down: Cell<bool>,
}

pub fn spawn_hotkey_thread(
    state: Arc<AppState>,
    controller: TranscriptionController,
    config_store: LiveConfigStore,
    hotkey_capture_controller: HotkeyCaptureController,
) {
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || {
            log::info!("hotkey thread started");
            let tracking = HotkeyTracking::default();

            if let Err(error) = run_hotkey_event_loop(move |event| {
                let settings_window_visible = hotkey_capture_controller.settings_window_visible();
                match event {
                    HotkeyEvent::KeyPress(key, current_modifiers) => {
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
                                &controller,
                                &tracking,
                            )
                        }
                    }
                    HotkeyEvent::KeyRelease(key, current_modifiers) => {
                        if hotkey_capture_controller.handle_key_release(key, current_modifiers) {
                            true
                        } else if settings_window_visible {
                            false
                        } else {
                            handle_key_release(
                                key,
                                current_modifiers,
                                &config_store,
                                &state,
                                &controller,
                                &tracking,
                            )
                        }
                    }
                }
            }) {
                log::error!("global hotkey tap failed: {}", error);
            }
        })
        .expect("failed to spawn hotkey thread");
}

pub(crate) fn handle_key_press(
    key: Key,
    current_modifiers: HotkeyModifiers,
    config_store: &LiveConfigStore,
    state: &AppState,
    controller: &TranscriptionController,
    tracking: &HotkeyTracking,
) -> bool {
    let HotkeyTracking {
        press_time,
        record_hotkey_action,
        correction_key_origin,
        transform_hotkey_is_down,
        clipboard_insert_is_down,
    } = tracking;
    let hotkey_config = current_hotkey_config(config_store);

    let open_settings = key == Key::Comma
        && current_modifiers.meta
        && !current_modifiers.shift
        && !current_modifiers.control
        && !current_modifiers.alt
        && state.is_overlay_window_visible()
        && !state.is_overlay_dismissed();
    let escape_abort = key == Key::Escape
        && !hotkey_config
            .record_hotkey
            .map(|binding| binding.matches_press(Key::Escape, current_modifiers))
            .unwrap_or(false);
    if open_settings || escape_abort {
        if !open_settings
            && !matches!(
                state.get_state(),
                STATE_RECORDING
                    | STATE_PROCESSING
                    | STATE_BUFFER_READY
                    | STATE_TRANSFORMING
                    | STATE_ERROR
            )
        {
            return false;
        }

        // The work in progress uses the abort up: the recording's start or
        // its finish, the buffer's discard, or the transformation. Idle and
        // error have no work in progress to cancel, so they request no abort,
        // and a paste is left to finish, as the key goes to the app pasted
        // into.
        let Some(current_state) = state.cancel_work_in_progress() else {
            return false;
        };
        state.dismiss_overlay();
        state.clear_overlay_text();
        state.set_overlay_text_opacity(1.0);
        press_time.set(None);
        record_hotkey_action.set(None);
        correction_key_origin.set(None);
        transform_hotkey_is_down.set(false);
        clipboard_insert_is_down.set(false);

        match current_state {
            STATE_RECORDING => {
                if state.is_overlay_correction_active() {
                    match controller.stop_correction_session_and_apply() {
                        Ok(()) => log::info!("aborting correction recording"),
                        Err(error) => {
                            log::error!("failed to abort correction recording: {}", error);
                            state.report_error(error.to_string());
                        }
                    }
                } else {
                    abort_recording(state, controller, "overlay cancellation");
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

        if open_settings {
            state.request_settings();
        }
        return true;
    }

    if hotkey_config
        .correction_key
        .is_some_and(|binding| binding.matches_press(key, current_modifiers))
    {
        if correction_key_origin.get().is_some() {
            return true;
        }

        let current_state = state.get_state();
        if matches!(current_state, STATE_PROCESSING | STATE_TRANSFORMING) {
            correction_key_origin.set(None);
            log::info!("ignoring correction while background work is still running");
            return false;
        }

        let has_annotation_text = !state.overlay_text().trim().is_empty();

        match current_state {
            STATE_RECORDING if state.is_overlay_correction_active() => {
                correction_key_origin.set(Some(CorrectionOrigin::Dictation));
                return true;
            }
            STATE_RECORDING | STATE_BUFFER_READY if !has_annotation_text => {
                correction_key_origin.set(None);
                log::info!("ignoring correction because no narrated annotation is available");
                return false;
            }
            STATE_RECORDING | STATE_BUFFER_READY => {
                correction_key_origin.set(Some(if current_state == STATE_RECORDING {
                    CorrectionOrigin::Dictation
                } else {
                    CorrectionOrigin::Buffer
                }));
                state.restore_overlay();
                state.set_overlay_correction_active(true);
                state.clear_overlay_correction_text();
                state.set_overlay_text_opacity(1.0);
                state.set_state(STATE_RECORDING);
                match controller.start_correction_session() {
                    Ok(()) => {
                        log::info!("correction recording started");
                    }
                    Err(start_error) => {
                        correction_key_origin.set(None);
                        log::error!("failed to start correction: {}", start_error);
                        state.report_error(start_error.to_string());
                    }
                }
            }
            _ => {
                correction_key_origin.set(None);
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
        // A paste normally keeps the state processing; an error reported
        // during it must not let a press change anything before the paste's
        // outcome is known.
        if state.is_pasting() {
            log::info!("ignoring record hotkey while the previous recording is pasted");
            return true;
        }

        let action = match current_state {
            STATE_IDLE | STATE_ERROR | STATE_BUFFER_READY => {
                match start_recording(
                    state,
                    controller,
                    current_state,
                    hotkey_config.mic_gain_problem.as_deref(),
                ) {
                    Some(action) => Some(action),
                    None => return true,
                }
            }
            STATE_RECORDING => Some(if hotkey_config.auto_transform_enabled {
                RecordHotkeyAction::StopAndTransformAndPaste
            } else {
                RecordHotkeyAction::StopAndPaste
            }),
            _ => None,
        };

        if action.is_some() {
            press_time.set(Some(Instant::now()));
            record_hotkey_action.set(action);
            return true;
        }

        return false;
    }

    if hotkey_config
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
                state.report_error(error.to_string());
            }
        }

        return true;
    }

    false
}

pub(crate) fn handle_key_release(
    key: Key,
    current_modifiers: HotkeyModifiers,
    config_store: &LiveConfigStore,
    state: &AppState,
    controller: &TranscriptionController,
    tracking: &HotkeyTracking,
) -> bool {
    let HotkeyTracking {
        press_time,
        record_hotkey_action,
        correction_key_origin,
        transform_hotkey_is_down,
        clipboard_insert_is_down,
    } = tracking;
    let hotkey_config = current_hotkey_config(config_store);

    if hotkey_config
        .correction_key
        .is_some_and(|binding| binding.matches_release(key, current_modifiers))
    {
        let Some(origin) = correction_key_origin.replace(None) else {
            return false;
        };

        if state.is_overlay_correction_active() {
            // Publish the resume intent before the worker can finish. It
            // keeps capture active while this command waits in the queue.
            state.set_dictation_resuming(matches!(origin, CorrectionOrigin::Dictation));
            state.set_overlay_correction_active(false);
            state.set_state(STATE_PROCESSING);
            match controller.stop_correction_session_and_apply() {
                Ok(()) => {
                    log::info!("stopping correction and applying it");
                }
                Err(error) => {
                    state.set_dictation_resuming(false);
                    log::error!("failed to stop correction: {}", error);
                    state.report_error(error.to_string());
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
        }

        return true;
    }

    if key == Key::KeyV && clipboard_insert_is_down.replace(false) {
        return true;
    }

    if hotkey_config
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
            STATE_RECORDING => {
                stop_recording_and_transform_and_resume(state, controller, "transform hotkey")
            }
            STATE_BUFFER_READY => {
                state.set_state(STATE_TRANSFORMING);
                match controller.transform_buffer() {
                    Ok(()) => {
                        log::info!("transforming buffered text");
                    }
                    Err(error) => {
                        log::error!("failed to start transformation: {}", error);
                        state.report_error(error.to_string());
                    }
                }
            }
            _ => {}
        }

        return true;
    }

    false
}

fn current_hotkey_config(config_store: &LiveConfigStore) -> CurrentHotkeyConfig {
    let config = config_store.current();
    let ActiveBindings {
        record_hotkey,
        correction_key,
        transform_hotkey,
        transformation_resolved,
    } = ActiveBindings::from_config(&config);

    CurrentHotkeyConfig {
        auto_transform_enabled: config.transformation.auto && transformation_resolved,
        hold_ms: config.mic.hold_ms,
        mic_gain_problem: crate::config::mic_gain_problem(config.mic.gain).map(|problem| {
            format!(
                "{}. Fix it in Settings, then record again.",
                problem.describe(crate::settings_window::labels::settings_name)
            )
        }),
        correction_key,
        record_hotkey,
        transform_hotkey,
    }
}

fn is_clipboard_insert_shortcut(key: Key, current_modifiers: HotkeyModifiers) -> bool {
    key == Key::KeyV
        && current_modifiers.meta
        && !current_modifiers.shift
        && !current_modifiers.control
        && !current_modifiers.alt
}

fn parse_correction_key(raw: &str) -> Option<CorrectionBinding> {
    CorrectionBinding::parse(raw).ok()
}

fn stop_recording_and_paste(state: &AppState, controller: &TranscriptionController, reason: &str) {
    if !state.is_recording() || state.is_overlay_correction_active() {
        if state.is_overlay_correction_active() {
            log::info!(
                "ignoring record hotkey paste release while correction is active ({})",
                reason
            );
        }
        return;
    }

    state.set_overlay_text_opacity(1.0);
    state.begin_finishing_dictation();
    match controller.stop_session_and_paste() {
        Ok(()) => {
            log::info!("recording stopped ({})", reason);
        }
        Err(stop_error) => {
            log::error!("failed to stop recording: {}", stop_error);
            state.report_error(stop_error.to_string());
        }
    }
}

fn stop_recording_and_transform_and_paste(
    state: &AppState,
    controller: &TranscriptionController,
    reason: &str,
) {
    if !state.is_recording() || state.is_overlay_correction_active() {
        if state.is_overlay_correction_active() {
            log::info!("ignoring record hotkey transform-and-paste release while correction is active ({})", reason);
        }
        return;
    }

    state.set_overlay_text_opacity(1.0);
    state.begin_finishing_dictation();
    match controller.stop_session_and_transform_and_paste() {
        Ok(()) => {
            log::info!("recording stopped ({})", reason);
        }
        Err(stop_error) => {
            log::error!("failed to stop recording: {}", stop_error);
            state.report_error(stop_error.to_string());
        }
    }
}

/// Starts recording from `current_state`, or shows why it cannot: with no
/// microphone, or a gain the audio stream refuses, the overlay explains that
/// instead of recording silence.
fn start_recording(
    state: &AppState,
    controller: &TranscriptionController,
    current_state: u8,
    mic_gain_problem: Option<&str>,
) -> Option<RecordHotkeyAction> {
    if let Some(problem) = mic_gain_problem {
        log::warn!(
            "record hotkey pressed with an invalid gain (state {})",
            current_state
        );
        state.report_error(problem);
        return None;
    }
    if !state.is_microphone_available() {
        log::warn!(
            "record hotkey pressed with no microphone available (state {})",
            current_state
        );
        state.report_error(NO_MICROPHONE_MESSAGE);
        return None;
    }
    // Starting the recording also restores the overlay and clears what the
    // new recording does not continue.
    match controller.start_session(state) {
        Ok(()) => {
            log::info!("recording started");
            Some(RecordHotkeyAction::StartRecording)
        }
        Err(StartSessionError::PasteRunning) => {
            log::info!("ignoring record hotkey while the previous recording is pasted");
            None
        }
        Err(start_error) => {
            log::error!("failed to start recording: {}", start_error);
            state.report_error(start_error.to_string());
            None
        }
    }
}

/// Transforms the transcript so far and resumes dictation after it. Audio
/// capture carries on meanwhile (`AppState::set_dictation_resuming`), so
/// what is said while the transformation runs joins the transformed text; the
/// transcription worker ends the mark when it is done.
fn stop_recording_and_transform_and_resume(
    state: &AppState,
    controller: &TranscriptionController,
    reason: &str,
) {
    state.set_dictation_resuming(true);
    state.set_overlay_text_opacity(1.0);
    state.set_state(STATE_PROCESSING);
    match controller.stop_session_and_transform_and_resume() {
        Ok(()) => {
            log::info!(
                "stopping recording, transforming buffered text, and resuming ({})",
                reason
            );
        }
        Err(error) => {
            log::error!("failed to stop recording for transformation: {}", error);
            state.set_dictation_resuming(false);
            state.report_error(error.to_string());
        }
    }
}

/// Ends the dictation recording and throws its transcript away.
fn abort_recording(state: &AppState, controller: &TranscriptionController, reason: &str) {
    if !state.is_recording() {
        return;
    }

    match controller.abort_session() {
        Ok(()) => {
            state.set_state(STATE_IDLE);
            log::info!("recording aborted ({})", reason);
        }
        Err(stop_error) => {
            log::error!("failed to abort recording: {}", stop_error);
            state.report_error(stop_error.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_clipboard_insert_shortcut, parse_correction_key, stop_recording_and_paste,
        stop_recording_and_transform_and_paste, stop_recording_and_transform_and_resume,
    };

    fn test_controller(
        state: &std::sync::Arc<crate::state::AppState>,
    ) -> crate::transcription::TranscriptionController {
        let config = crate::config::Config::default();
        let config_store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from("/tmp/config.toml"),
        );
        crate::transcription::spawn_transcription_thread(state.clone(), config_store)
    }

    #[test]
    fn the_record_key_explains_a_missing_microphone_instead_of_recording() {
        use crate::audio::NO_MICROPHONE_MESSAGE;
        use crate::state::{AppState, STATE_ERROR, STATE_IDLE};

        let state = AppState::new();
        state.set_state(STATE_IDLE);
        state.set_microphone_available(false);
        let controller = crate::transcription::TranscriptionController::without_worker();

        assert_eq!(
            super::start_recording(&state, &controller, STATE_IDLE, None),
            None
        );

        assert_eq!(state.get_state(), STATE_ERROR);
        assert_eq!(&*state.overlay_error_text(), NO_MICROPHONE_MESSAGE);
        assert!(!state.is_overlay_dismissed());
    }

    #[test]
    fn escape_on_an_error_dismisses_it_without_leaving_an_abort_behind() {
        use crate::key::Key;
        use crate::state::{AppState, STATE_IDLE};

        let state = AppState::new();
        state.report_error(
            "failed to start Deepgram session: HTTP error: 500 Internal Server Error",
        );
        let config = crate::config::Config::default();
        let config_store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from("/tmp/config.toml"),
        );
        let controller = crate::transcription::TranscriptionController::without_worker();

        let handled = super::handle_key_press(
            Key::Escape,
            crate::hotkey_binding::HotkeyModifiers::default(),
            &config_store,
            &state,
            &controller,
            &super::HotkeyTracking::default(),
        );

        assert!(handled);
        assert_eq!(state.get_state(), STATE_IDLE);
        assert!(state.is_overlay_dismissed());
        assert!(state.overlay_error_text().is_empty());
        // An error has no work in progress for an abort to cancel.
        assert!(!state.is_abort_requested());
    }

    #[test]
    fn command_comma_discards_the_overlay_and_requests_settings() {
        use super::{
            AppState, Instant, LiveConfigStore, TranscriptionController, STATE_BUFFER_READY,
            STATE_ERROR, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING, STATE_TRANSFORMING,
        };
        for (current_state, correcting) in [
            (STATE_RECORDING, false),
            (STATE_RECORDING, true),
            (STATE_BUFFER_READY, false),
            (STATE_PROCESSING, false),
            (STATE_TRANSFORMING, false),
            (STATE_ERROR, false),
            (STATE_IDLE, false),
        ] {
            let state = AppState::new();
            state.set_state(current_state);
            state.set_overlay_correction_active(correcting);
            state.set_overlay_window_visible(true);
            state.set_overlay_text("Do not paste or transform this narration");
            let config = crate::config::Config::default();
            let store = LiveConfigStore::new(
                config.clone(),
                config,
                std::path::PathBuf::from(".tmp/overlay-settings/config.toml"),
            );
            let controller = TranscriptionController::without_worker();
            let tracking = super::HotkeyTracking::default();
            tracking.press_time.set(Some(Instant::now()));
            tracking
                .record_hotkey_action
                .set(Some(super::RecordHotkeyAction::StopAndTransformAndPaste));
            tracking
                .correction_key_origin
                .set(Some(super::CorrectionOrigin::Dictation));
            tracking.transform_hotkey_is_down.set(true);
            tracking.clipboard_insert_is_down.set(true);

            assert!(super::handle_key_press(
                Key::Comma,
                HotkeyModifiers {
                    meta: true,
                    ..HotkeyModifiers::default()
                },
                &store,
                &state,
                &controller,
                &tracking,
            ));

            assert!(state.is_overlay_dismissed());
            assert!(state.overlay_text().is_empty());
            assert!(state.take_settings_request());
            assert!(
                !state.take_settings_request(),
                "present Settings once per request"
            );
            assert!(tracking.press_time.get().is_none());
            assert!(tracking.record_hotkey_action.get().is_none());
            assert!(tracking.correction_key_origin.get().is_none());
            assert!(!tracking.transform_hotkey_is_down.get());
            assert!(!tracking.clipboard_insert_is_down.get());
            assert_eq!(
                state.is_abort_requested(),
                !matches!(current_state, STATE_IDLE | STATE_ERROR)
            );

            // Releasing held record, transform, or correction keys after the
            // shortcut must not trigger another operation.
            for key in [Key::F5, Key::F6, Key::AltLeft] {
                super::handle_key_release(
                    key,
                    HotkeyModifiers::default(),
                    &store,
                    &state,
                    &controller,
                    &tracking,
                );
            }
            assert!(state.overlay_text().is_empty());
            assert!(state.is_overlay_dismissed());
        }
    }

    #[test]
    fn command_comma_leaves_other_apps_and_other_comma_chords_alone() {
        use super::{AppState, LiveConfigStore, TranscriptionController, STATE_RECORDING};
        let command = HotkeyModifiers {
            meta: true,
            ..HotkeyModifiers::default()
        };
        for (visible, dismissed, modifiers) in [
            (false, false, command),
            (true, true, command),
            (true, false, HotkeyModifiers::default()),
            (
                true,
                false,
                HotkeyModifiers {
                    shift: true,
                    ..command
                },
            ),
            (
                true,
                false,
                HotkeyModifiers {
                    alt: true,
                    ..command
                },
            ),
            (
                true,
                false,
                HotkeyModifiers {
                    control: true,
                    ..command
                },
            ),
        ] {
            let state = AppState::new();
            state.set_state(STATE_RECORDING);
            state.set_overlay_window_visible(visible);
            state.set_overlay_text("Keep this narration");
            if dismissed {
                state.dismiss_overlay();
            }
            let config = crate::config::Config::default();
            let store = LiveConfigStore::new(
                config.clone(),
                config,
                std::path::PathBuf::from(".tmp/overlay-settings/config.toml"),
            );
            assert!(!super::handle_key_press(
                Key::Comma,
                modifiers,
                &store,
                &state,
                &TranscriptionController::without_worker(),
                &super::HotkeyTracking::default(),
            ));
            assert_eq!(&*state.overlay_text(), "Keep this narration");
            assert!(!state.is_abort_requested());
            assert!(!state.take_settings_request());
        }
    }

    #[test]
    fn command_comma_discards_an_empty_worker_buffer_without_leaving_an_abort() {
        use super::{AppState, Instant, LiveConfigStore, STATE_BUFFER_READY, STATE_IDLE};
        let state = AppState::new();
        state.set_state(STATE_BUFFER_READY);
        state.set_overlay_window_visible(true);
        state.set_overlay_text("Only the editable overlay holds this text");
        let config = crate::config::Config::default();
        let store = LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from(".tmp/overlay-settings/config.toml"),
        );
        let controller =
            crate::transcription::spawn_transcription_thread(state.clone(), store.clone());
        assert!(super::handle_key_press(
            Key::Comma,
            HotkeyModifiers {
                meta: true,
                ..HotkeyModifiers::default()
            },
            &store,
            &state,
            &controller,
            &super::HotkeyTracking::default(),
        ));
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while state.get_state() != STATE_IDLE && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(state.get_state(), STATE_IDLE);
        assert!(state.overlay_text().is_empty());
        assert!(state.is_overlay_dismissed());
        assert!(!state.is_abort_requested());
        assert!(state.take_settings_request());
    }

    #[test]
    fn the_record_key_names_an_invalid_gain_instead_of_recording() {
        use crate::state::{AppState, STATE_ERROR, STATE_IDLE};

        let state = AppState::new();
        state.set_state(STATE_IDLE);
        let controller = crate::transcription::TranscriptionController::without_worker();
        let problem = "Microphone > Mic gain (dB): \"40 dB\" is outside the 0 to 10 dB range. Fix it in Settings, then record again.";

        assert_eq!(
            super::start_recording(&state, &controller, STATE_IDLE, Some(problem)),
            None
        );

        assert_eq!(state.get_state(), STATE_ERROR);
        assert_eq!(&*state.overlay_error_text(), problem);
    }

    fn hotkey_config_for(config: crate::config::Config) -> super::CurrentHotkeyConfig {
        let store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from("/tmp/config.toml"),
        );
        super::current_hotkey_config(&store)
    }

    #[test]
    fn an_invalid_gain_is_carried_into_the_hotkey_config() {
        let mut config = crate::config::Config::default();
        assert_eq!(hotkey_config_for(config.clone()).mic_gain_problem, None);

        config.mic.gain = 40.0;
        assert_eq!(
            hotkey_config_for(config).mic_gain_problem.as_deref(),
            Some("Microphone > Mic gain (dB): \"40 dB\" is outside the 0 to 10 dB range. Fix it in Settings, then record again.")
        );
    }

    #[test]
    fn a_correction_key_inside_the_record_hotkey_is_off() {
        let mut config = crate::config::Config::default();
        config.ui.hotkey = "Cmd+F5".to_owned();
        config.ui.correction_key = "LeftMeta".to_owned();

        let hotkey_config = hotkey_config_for(config);

        assert!(hotkey_config.record_hotkey.is_some());
        assert_eq!(hotkey_config.correction_key, None);
    }

    #[test]
    fn auto_transform_follows_transformation_resolution_not_the_transform_hotkey() {
        let mut config = crate::config::Config::default();
        config.transformation.auto = true;
        assert!(!hotkey_config_for(config.clone()).auto_transform_enabled);

        config.transformation.provider = Some("openai".to_owned());
        config.transformation.api_key = Some("test-key".to_owned());
        config.transformation.hotkey = config.ui.hotkey.clone();
        let hotkey_config = hotkey_config_for(config.clone());
        assert_eq!(hotkey_config.transform_hotkey, None);
        assert!(hotkey_config.auto_transform_enabled);

        config.transformation.auto = false;
        assert!(!hotkey_config_for(config).auto_transform_enabled);
    }

    #[test]
    fn a_transform_hotkey_sharing_a_key_with_the_correction_key_is_off() {
        let mut config = crate::config::Config::default();
        config.transformation.provider = Some("openai".to_owned());
        config.transformation.api_key = Some("test-key".to_owned());
        assert!(hotkey_config_for(config.clone()).transform_hotkey.is_some());

        config.ui.correction_key = "F6".to_owned();
        config.transformation.hotkey = "Shift+F6".to_owned();
        let hotkey_config = hotkey_config_for(config);

        assert!(hotkey_config.correction_key.is_some());
        assert_eq!(hotkey_config.transform_hotkey, None);
    }

    #[test]
    fn the_record_key_starts_recording_when_a_microphone_is_available() {
        use crate::state::{AppState, STATE_IDLE, STATE_RECORDING};

        let state = AppState::new();
        state.set_state(STATE_IDLE);
        state.dismiss_overlay();
        let controller = crate::transcription::TranscriptionController::without_worker();

        assert_eq!(
            super::start_recording(&state, &controller, STATE_IDLE, None),
            Some(super::RecordHotkeyAction::StartRecording)
        );
        assert_eq!(state.get_state(), STATE_RECORDING);
        assert!(!state.is_overlay_dismissed());
    }

    #[test]
    fn the_transform_hotkey_mid_dictation_keeps_capturing_audio() {
        use crate::state::{AppState, STATE_PROCESSING, STATE_RECORDING};

        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        // Its commands wait, so the state is seen before the worker acts.
        let controller = crate::transcription::TranscriptionController::without_worker();

        stop_recording_and_transform_and_resume(&state, &controller, "test");

        assert_eq!(state.get_state(), STATE_PROCESSING);
        assert!(state.is_dictation_resuming());
        assert!(state.is_capturing_audio());
    }

    #[test]
    fn the_worker_ends_resuming_when_the_transform_and_resume_is_done() {
        use crate::state::{AppState, STATE_RECORDING};

        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        let controller = test_controller(&state);

        stop_recording_and_transform_and_resume(&state, &controller, "test");

        // There is no session to transform, so the worker is done at once.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.is_dictation_resuming() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!state.is_dictation_resuming());
    }
    use crate::hotkey_binding::HotkeyModifiers;
    use crate::key::Key;

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
        assert_eq!(
            parse_correction_key("LeftMeta"),
            Some(crate::correction_binding::CorrectionBinding::Key(
                Key::MetaLeft
            ))
        );
        assert_eq!(
            parse_correction_key("RightMeta"),
            Some(crate::correction_binding::CorrectionBinding::Key(
                Key::MetaRight
            ))
        );
        assert_eq!(
            parse_correction_key("F7"),
            Some(crate::correction_binding::CorrectionBinding::Key(Key::F7))
        );
        assert_eq!(parse_correction_key("Cmd"), None);
    }

    #[test]
    fn default_correction_requires_option_and_command() {
        for (first, second) in [
            (Key::AltLeft, Key::MetaLeft),
            (Key::MetaRight, Key::AltRight),
        ] {
            let state = crate::state::AppState::new();
            state.set_state(crate::state::STATE_BUFFER_READY);
            state.set_overlay_text("Keep this narration");
            let config = crate::config::Config::default();
            let store = crate::settings::LiveConfigStore::new(
                config.clone(),
                config,
                ".tmp/correction-chord/config.toml".into(),
            );
            let controller = crate::transcription::TranscriptionController::without_worker();
            let tracking = super::HotkeyTracking::default();
            let press = |key, modifiers| {
                super::handle_key_press(key, modifiers, &store, &state, &controller, &tracking)
            };

            assert!(
                !press(first, HotkeyModifiers::default()),
                "one modifier must not start correction"
            );
            assert!(!state.is_overlay_correction_active());
            assert!(press(
                second,
                HotkeyModifiers::default().with_key_pressed(first)
            ));
            assert!(state.is_overlay_correction_active());
            assert_eq!(&*state.overlay_text(), "Keep this narration");
        }
    }

    #[test]
    fn explicit_single_correction_key_leaves_command_shortcuts_available() {
        let state = crate::state::AppState::new();
        state.set_state(crate::state::STATE_RECORDING);
        state.set_overlay_text("Keep this narration");
        let mut config = crate::config::Config::default();
        config.ui.correction_key = "LeftAlt".to_owned();
        let store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from(".tmp/correction/config.toml"),
        );
        let controller = crate::transcription::TranscriptionController::without_worker();
        let tracking = super::HotkeyTracking::default();
        let press = |key, modifiers| {
            super::handle_key_press(key, modifiers, &store, &state, &controller, &tracking)
        };
        let release = |key| {
            super::handle_key_release(
                key,
                HotkeyModifiers::default(),
                &store,
                &state,
                &controller,
                &tracking,
            )
        };
        let command = HotkeyModifiers {
            meta: true,
            ..HotkeyModifiers::default()
        };

        assert!(!press(Key::MetaLeft, HotkeyModifiers::default()));
        assert!(!state.is_overlay_correction_active());
        assert!(!press(Key::KeyC, command));
        assert!(press(Key::KeyV, command));
        assert!(tracking.clipboard_insert_is_down.get());
        assert!(release(Key::KeyV));
        assert!(!release(Key::MetaLeft));
        assert!(press(Key::AltLeft, HotkeyModifiers::default()));
        assert!(state.is_overlay_correction_active());
        assert!(release(Key::AltLeft));
        assert!(!state.is_overlay_correction_active());
        assert_eq!(state.get_state(), crate::state::STATE_PROCESSING);
        assert!(state.is_capturing_audio());
        assert_eq!(&*state.overlay_text(), "Keep this narration");

        state.set_dictation_resuming(false);
        state.set_state(crate::state::STATE_BUFFER_READY);
        assert!(press(Key::AltLeft, HotkeyModifiers::default()));
        assert!(release(Key::AltLeft));
        assert!(!state.is_capturing_audio());
        assert_eq!(&*state.overlay_text(), "Keep this narration");
    }

    #[test]
    fn finishing_dictation_keeps_the_transcript_readable_during_transformation() {
        use crate::state::{AppState, STATE_PROCESSING, STATE_RECORDING};
        use crate::transcription::TranscriptionController;

        let state = AppState::new();
        state.set_overlay_text("Keep my narration visible.");
        state.set_state(STATE_RECORDING);
        let controller = TranscriptionController::without_worker();

        stop_recording_and_transform_and_paste(&state, &controller, "test");

        assert_eq!(state.get_state(), STATE_PROCESSING);
        assert_eq!(&*state.overlay_text(), "Keep my narration visible.");
        assert_eq!(state.overlay_text_opacity(), 1.0);
        assert!(state.is_finishing_dictation());
        assert!(!state.is_overlay_dismissed());
        state.set_state(crate::state::STATE_TRANSFORMING);
        assert!(state.is_finishing_dictation());
        state.report_error("Transformation failed");
        assert!(!state.is_finishing_dictation());
    }

    #[test]
    fn finishing_empty_dictation_dismisses_without_waiting_or_discarding_final_audio() {
        use crate::state::{AppState, STATE_PROCESSING, STATE_RECORDING, STATE_TRANSFORMING};
        use crate::transcription::TranscriptionController;

        for text in ["", " \n\t "] {
            for finish in [
                stop_recording_and_paste,
                stop_recording_and_transform_and_paste,
            ] {
                let state = AppState::new();
                state.set_overlay_text(text);
                state.set_state(STATE_RECORDING);
                state.set_overlay_window_visible(true);
                let controller = TranscriptionController::without_worker();

                finish(&state, &controller, "test");

                assert_eq!(state.get_state(), STATE_PROCESSING);
                assert!(state.is_overlay_dismissed());
                assert!(!state.is_finishing_dictation());
                assert!(!state.is_abort_requested());
                assert!(state.is_overlay_window_visible());

                state.set_overlay_window_visible(false);
                state.set_overlay_text("Final speech can arrive after F5.");
                state.set_state(STATE_TRANSFORMING);
                assert!(state.is_overlay_dismissed());
                assert!(!state.is_finishing_dictation());
                assert_eq!(&*state.overlay_text(), "Final speech can arrive after F5.");

                state.report_error("Session failed");
                assert!(!state.is_overlay_dismissed());
            }
        }
    }

    #[test]
    fn transforming_and_resuming_keeps_the_transcript_readable() {
        use crate::state::{AppState, STATE_PROCESSING, STATE_RECORDING};
        use crate::transcription::TranscriptionController;

        let state = AppState::new();
        state.set_overlay_text("Keep my narration visible.");
        state.set_state(STATE_RECORDING);
        let controller = TranscriptionController::without_worker();

        stop_recording_and_transform_and_resume(&state, &controller, "test");

        assert_eq!(state.get_state(), STATE_PROCESSING);
        assert!(state.is_dictation_resuming());
        assert_eq!(&*state.overlay_text(), "Keep my narration visible.");
        assert_eq!(state.overlay_text_opacity(), 1.0);
        assert!(!state.is_finishing_dictation());
    }

    #[test]
    fn releasing_record_hotkey_while_correction_is_active_is_ignored() {
        use crate::state::{AppState, STATE_RECORDING};

        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        state.set_overlay_correction_active(true);

        let config = crate::config::Config::default();
        let config_store = crate::settings::LiveConfigStore::new(
            config.clone(),
            config,
            std::path::PathBuf::from("/tmp/config.toml"),
        );
        let controller =
            crate::transcription::spawn_transcription_thread(state.clone(), config_store);

        stop_recording_and_transform_and_paste(&state, &controller, "test");
        stop_recording_and_paste(&state, &controller, "test");

        // State MUST remain STATE_RECORDING (not changed to STATE_PROCESSING)
        assert_eq!(state.get_state(), STATE_RECORDING);
    }
}
