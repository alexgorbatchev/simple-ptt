use std::ffi::c_void;
use std::ptr::NonNull;
use std::thread;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSPasteboard, NSPasteboardTypeString};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};
use objc2_foundation::NSString;

use crate::state::{AppState, STATE_BUFFER_READY, STATE_IDLE};

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
    let mtm = MainThreadMarker::new().expect("must be on main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.isActive()
}

pub fn describe_paste_diagnostics(diagnostics: &PasteDiagnostics) -> String {
    format!(
        "state={}, overlay_visible={}, app_active={}",
        diagnostics.state, diagnostics.overlay_window_visible, diagnostics.app_active
    )
}

pub fn flush_buffered_text_or_paste(state: &AppState, buffered_text: &mut String, paste: bool) {
    if state.consume_abort_request() {
        log::info!("discarding buffered text because abort was requested");
        buffered_text.clear();
        state.clear_overlay_text();
        state.clear_overlay_correction_text();
        state.set_overlay_correction_active(false);
        state.set_overlay_text_opacity(1.0);
        state.set_state(STATE_IDLE);
        return;
    }

    if !paste {
        log::info!("buffering transcript for manual paste");
        state.set_overlay_text(buffered_text.clone());
        state.clear_overlay_correction_text();
        state.set_overlay_correction_active(false);
        state.set_overlay_text_opacity(1.0);
        state.set_state(STATE_BUFFER_READY);
        return;
    }

    state.clear_overlay_text();
    state.clear_overlay_correction_text();
    state.set_overlay_correction_active(false);
    state.set_overlay_text_opacity(1.0);
    state.set_state(STATE_IDLE);

    match run_paste_sequence(state, buffered_text.as_str()) {
        Ok(()) => {
            buffered_text.clear();
            state.clear_abort_request();
            log::info!("buffer pasted successfully");
        }
        Err(error) => {
            if state.consume_abort_request() {
                log::info!("discarding buffered text because abort was requested during paste");
                buffered_text.clear();
                return;
            }

            log::error!("failed to copy/paste buffered text: {}", error);
            state.set_overlay_text(buffered_text.clone());
            state.set_overlay_text_opacity(1.0);
            state.set_state(STATE_BUFFER_READY);
        }
    }
}

pub fn run_paste_sequence(state: &AppState, text: &str) -> Result<(), String> {
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

struct ClipboardReadRequest {
    text: Option<String>,
}

struct ClipboardWriteRequest {
    text: String,
    success: bool,
}

extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_sync_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

extern "C" fn perform_clipboard_read(context: *mut c_void) {
    let request = unsafe { &mut *(context as *mut ClipboardReadRequest) };
    let pasteboard = NSPasteboard::generalPasteboard();
    request.text = pasteboard
        .stringForType(unsafe { NSPasteboardTypeString })
        .map(|text| text.to_string());
}

pub fn read_clipboard_text() -> Result<String, String> {
    let mut request = Box::new(ClipboardReadRequest { text: None });

    unsafe {
        dispatch_sync_f(
            &_dispatch_main_q,
            (&mut *request) as *mut ClipboardReadRequest as *mut c_void,
            perform_clipboard_read,
        );
    }

    request
        .text
        .ok_or_else(|| "clipboard does not currently contain plain text".to_owned())
}

extern "C" fn perform_clipboard_write(context: *mut c_void) {
    let request = unsafe { &mut *(context as *mut ClipboardWriteRequest) };
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();

    let ns_text = NSString::from_str(&request.text);
    request.success = pasteboard.setString_forType(&ns_text, unsafe { NSPasteboardTypeString });
}

pub fn write_clipboard_text(text: &str) -> Result<(), String> {
    let mut request = Box::new(ClipboardWriteRequest {
        text: text.to_owned(),
        success: false,
    });

    unsafe {
        dispatch_sync_f(
            &_dispatch_main_q,
            (&mut *request) as *mut ClipboardWriteRequest as *mut c_void,
            perform_clipboard_write,
        );
    }

    if request.success {
        Ok(())
    } else {
        Err("failed to update the macOS general pasteboard".to_owned())
    }
}
