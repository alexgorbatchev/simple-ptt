use std::ffi::c_void;
use std::ptr::NonNull;
use std::thread;
use std::time::{Duration, Instant};

use dispatch2::DispatchQueue;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};
use objc2_foundation::{MainThreadMarker, NSString};

use super::{publish_or_hand_over, Publication};
use crate::state::{
    AppState, RecordingGeneration, STATE_BUFFER_READY, STATE_IDLE, STATE_PROCESSING,
};

const COMMAND_FLAGGED_V_KEYCODE: u16 = 9;
const KEY_EVENT_DELAY_MS: u64 = 20;
const PASTE_HANDOFF_POLL_INTERVAL_MS: u64 = 10;
const PASTE_HANDOFF_TIMEOUT_MS: u64 = 750;
const PASTEBOARD_SETTLE_DELAY_MS: u64 = 80;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasteStep {
    WaitForOverlayHide,
    WaitForAppDeactivation,
    WriteClipboard,
    WaitForPasteboardSettle,
    SendPasteShortcut,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasteDiagnostics {
    pub state: u8,
    pub overlay_window_visible: bool,
    pub app_active: bool,
}

pub fn query_paste_diagnostics(state: &AppState) -> PasteDiagnostics {
    let app_active = is_main_app_active();

    PasteDiagnostics {
        state: state.get_state(),
        overlay_window_visible: state.is_overlay_window_visible(),
        app_active,
    }
}

fn is_main_app_active() -> bool {
    objc2_app_kit::NSRunningApplication::currentApplication().isActive()
}

pub fn describe_paste_diagnostics(diagnostics: &PasteDiagnostics) -> String {
    format!(
        "state={}, overlay_visible={}, app_active={}",
        diagnostics.state, diagnostics.overlay_window_visible, diagnostics.app_active
    )
}

/// Pastes the text of the recording `generation`. The overlay is dismissed
/// for the paste while the recording stays current, so no recording starts
/// and the cancel shortcuts cancel nothing until the paste's outcome is
/// known. A failed paste returns the text to the overlay as a ready buffer,
/// so it is never lost. An abort requested before the paste discards the
/// text, and a recording started before it continues the text.
pub(super) fn paste_buffered_text(
    state: &AppState,
    generation: RecordingGeneration,
    buffered_text: &mut String,
    handed_over: &mut String,
) {
    let mut paste = None;
    let publication =
        publish_or_hand_over(state, generation, handed_over, buffered_text, |recording| {
            recording.begin_paste();
            paste = Some(RunningPaste {
                state,
                unpasted: Some(buffered_text.clone()),
            });
            // The paste waits for the overlay to hide before pasting.
            state.dismiss_overlay();
            state.clear_overlay_text();
            state.clear_overlay_correction_text();
            state.set_overlay_correction_active(false);
            state.set_overlay_text_opacity(1.0);
            state.set_state(STATE_PROCESSING);
        });
    let Some(mut paste) = paste else {
        if publication == Publication::Cancelled {
            log::info!("discarding buffered text because abort was requested");
        }
        buffered_text.clear();
        return;
    };

    if let Err(error) = run_paste_sequence(state, buffered_text.as_str()) {
        log::error!("failed to copy/paste buffered text: {}", error);
        return;
    }
    paste.unpasted = None;
    buffered_text.clear();
    log::info!("buffer pasted successfully");
}

/// A paste `CurrentRecording::begin_paste` marked. Dropping it ends the
/// paste and writes its outcome while holding the recording: idle once the
/// text is pasted, or else the text back in the overlay as a ready buffer.
/// A paste that panics therefore counts as failed instead of leaving the app
/// processing with every record press ignored.
struct RunningPaste<'a> {
    state: &'a AppState,
    /// The text, until it is pasted.
    unpasted: Option<String>,
}

impl Drop for RunningPaste<'_> {
    fn drop(&mut self) {
        let state = self.state;
        let _recording = state.end_paste();
        match self.unpasted.take() {
            None => state.set_state(STATE_IDLE),
            Some(text) => {
                state.set_overlay_text(text);
                state.set_overlay_text_opacity(1.0);
                state.restore_overlay();
                state.set_state(STATE_BUFFER_READY);
            }
        }
    }
}

/// Runs `work` during the next paste sequence on this thread, as if it
/// happened while the paste waited for the overlay to hide.
#[cfg(test)]
pub(super) fn during_next_paste(work: impl FnOnce() + 'static) {
    DURING_PASTE.with(|during_paste| *during_paste.borrow_mut() = Some(Box::new(work)));
}

#[cfg(test)]
thread_local! {
    static DURING_PASTE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

pub fn run_paste_sequence(state: &AppState, text: &str) -> Result<(), String> {
    // Tests never write the clipboard or send Cmd+V to the frontmost app;
    // they count the attempt and see it fail.
    if cfg!(test) {
        #[cfg(test)]
        {
            state.record_paste_attempt();
            if let Some(work) = DURING_PASTE.with(|during_paste| during_paste.borrow_mut().take()) {
                work();
            }
        }
        return Err("paste is disabled in tests".to_owned());
    }

    log::info!(
        "paste handoff start: chars={}, diagnostics={}",
        text.chars().count(),
        describe_paste_diagnostics(&query_paste_diagnostics(state))
    );

    for step in [
        PasteStep::WaitForOverlayHide,
        PasteStep::WaitForAppDeactivation,
        PasteStep::WriteClipboard,
        PasteStep::WaitForPasteboardSettle,
        PasteStep::SendPasteShortcut,
    ] {
        match step {
            PasteStep::WaitForOverlayHide => {
                wait_for_paste_condition(state, step, |diagnostics| {
                    !diagnostics.overlay_window_visible
                })?;
            }
            PasteStep::WaitForAppDeactivation => {
                wait_for_paste_condition(state, step, |diagnostics| !diagnostics.app_active)?;
            }
            PasteStep::WriteClipboard => {
                write_clipboard_text(text)?;
                log::info!("paste step=write-clipboard chars={}", text.chars().count());
            }
            PasteStep::WaitForPasteboardSettle => {
                thread::sleep(Duration::from_millis(PASTEBOARD_SETTLE_DELAY_MS));
            }
            PasteStep::SendPasteShortcut => {
                let diagnostics = query_paste_diagnostics(state);
                log::info!(
                    "paste step=send-shortcut diagnostics={}",
                    describe_paste_diagnostics(&diagnostics)
                );
                send_paste_shortcut()?;
            }
        }
    }

    Ok(())
}

fn wait_for_paste_condition(
    state: &AppState,
    step: PasteStep,
    condition: impl Fn(&PasteDiagnostics) -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(PASTE_HANDOFF_TIMEOUT_MS);

    loop {
        let diagnostics = query_paste_diagnostics(state);
        if condition(&diagnostics) {
            log::info!(
                "paste step={:?} ready diagnostics={}",
                step,
                describe_paste_diagnostics(&diagnostics)
            );
            return Ok(());
        }

        if Instant::now() >= deadline {
            let details = describe_paste_diagnostics(&diagnostics);
            log::warn!("paste step={:?} timed out diagnostics={}", step, details);
            return Err(format!(
                "timed out while preparing paste ({:?}); {}",
                step, details
            ));
        }

        thread::sleep(Duration::from_millis(PASTE_HANDOFF_POLL_INTERVAL_MS));
    }
}

pub fn send_paste_shortcut() -> Result<(), String> {
    let key_down = create_keyboard_event(COMMAND_FLAGGED_V_KEYCODE, true)?;
    CGEvent::set_flags(Some(&key_down), CGEventFlags::MaskCommand);
    CGEvent::post(
        CGEventTapLocation::AnnotatedSessionEventTap,
        Some(&key_down),
    );
    thread::sleep(Duration::from_millis(KEY_EVENT_DELAY_MS));

    let key_up = create_keyboard_event(COMMAND_FLAGGED_V_KEYCODE, false)?;
    CGEvent::set_flags(Some(&key_up), CGEventFlags::MaskCommand);
    CGEvent::post(CGEventTapLocation::AnnotatedSessionEventTap, Some(&key_up));
    thread::sleep(Duration::from_millis(KEY_EVENT_DELAY_MS));

    Ok(())
}

fn create_keyboard_event(virtual_key: u16, key_down: bool) -> Result<CFRetained<CGEvent>, String> {
    extern "C-unwind" {
        fn CGEventCreateKeyboardEvent(
            source: *const c_void,
            virtual_key: u16,
            key_down: bool,
        ) -> Option<NonNull<CGEvent>>;
    }

    let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null(), virtual_key, key_down) }
        .ok_or_else(|| {
            format!(
                "failed to create synthetic keyboard event for keycode {} ({})",
                virtual_key,
                if key_down { "down" } else { "up" }
            )
        })?;

    Ok(unsafe { CFRetained::from_raw(event) })
}

/// Runs `work` on the main thread, where AppKit's pasteboard must be used,
/// and waits for it. The worker threads that read and write the clipboard
/// call this; on the main thread it would deadlock, waiting on its own queue.
fn on_main_thread<T: Send>(work: impl Send + FnOnce() -> T) -> T {
    debug_assert!(
        MainThreadMarker::new().is_none(),
        "the clipboard is reached through the main queue from worker threads only"
    );
    let mut result = None;
    DispatchQueue::main().exec_sync(|| result = Some(work()));
    result.expect("the main queue ran the clipboard work")
}

pub fn read_clipboard_text() -> Result<String, String> {
    on_main_thread(|| {
        NSPasteboard::generalPasteboard()
            .stringForType(unsafe { NSPasteboardTypeString })
            .map(|text| text.to_string())
    })
    .ok_or_else(|| "clipboard does not currently contain plain text".to_owned())
}

pub fn write_clipboard_text(text: &str) -> Result<(), String> {
    let written = on_main_thread(|| {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
    });
    if written {
        Ok(())
    } else {
        Err("failed to update the macOS general pasteboard".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    #[test]
    fn query_paste_diagnostics_runs_on_background_thread_without_panicking() {
        let state = AppState::new();
        let handle = std::thread::spawn(move || query_paste_diagnostics(&state));

        let diagnostics = handle.join().expect("background thread must not panic");
        assert_eq!(diagnostics.state, crate::state::STATE_IDLE);
    }
}
