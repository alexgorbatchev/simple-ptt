use std::sync::Arc;

use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSString};

use crate::deepgram_connection::DeepgramConnectionController;
use crate::hotkey_capture::HotkeyCaptureController;
use crate::icon::{make_status_bar_active_icon, make_status_bar_icon};
use crate::state::{
    AppState, DeepgramConnectionStatus, MicMeterSnapshot, STATE_BUFFER_READY, STATE_IDLE,
    STATE_PROCESSING, STATE_RECORDING, STATE_TRANSFORMING,
};
use crate::transformation_models::TransformationModelsController;
use super::helpers::billing_menu_text;
use super::AppDelegate;

pub struct UiUpdate {
    pub delegate_addr: usize,
    pub deepgram_connection_status: DeepgramConnectionStatus,
    pub mic_meter: MicMeterSnapshot,
    pub mic_meter_changed: bool,
    pub overlay_dismissed: bool,
    pub overlay_footer_text: Arc<str>,
    pub overlay_correction_active: bool,
    pub overlay_correction_text: Arc<str>,
    pub overlay_text: Arc<str>,
    pub overlay_error_text: Arc<str>,
    pub overlay_text_opacity: f64,
    pub state: u8,
}

pub fn setup_status_polling(
    delegate: Retained<AppDelegate>,
    state: Arc<AppState>,
    hotkey_capture_controller: HotkeyCaptureController,
    transformation_models_controller: TransformationModelsController,
    deepgram_connection_controller: DeepgramConnectionController,
) {
    let delegate_addr = Retained::as_ptr(&delegate) as usize;
    std::mem::forget(delegate);

    std::thread::Builder::new()
        .name("ui-poller".into())
        .spawn(move || {
            let mut last_deepgram_connection_status = DeepgramConnectionStatus::Unknown;
            let mut last_mic_meter = MicMeterSnapshot::default();
            let mut last_overlay_dismissed = false;
            let mut last_overlay_footer_text: Arc<str> = Arc::from("");
            let mut last_overlay_correction_active = false;
            let mut last_overlay_correction_text: Arc<str> = Arc::from("");
            let mut last_overlay_text: Arc<str> = Arc::from("");
            let mut last_overlay_error_text: Arc<str> = Arc::from("");
            let mut last_overlay_text_opacity = 1.0;
            let mut last_state = STATE_IDLE;
            let mut frame_count = 0u64;

            loop {
                std::thread::sleep(std::time::Duration::from_millis(75));
                let delegate_ref = unsafe { &*(delegate_addr as *const AppDelegate) };
                delegate_ref.ivars().audio_controller.sync_stream_state();

                frame_count += 1;
                let current_state = state.get_state();
                let current_deepgram_connection_status = state.deepgram_connection_status();
                let current_mic_meter = state.mic_meter_snapshot();
                let current_overlay_dismissed = state.is_overlay_dismissed();
                let current_overlay_footer_text = state.overlay_footer_text();
                let current_overlay_correction_active = state.is_overlay_correction_active();
                let current_overlay_correction_text = state.overlay_correction_text();
                let current_overlay_text = state.overlay_text();
                let current_overlay_error_text = state.overlay_error_text();
                let current_overlay_text_opacity = state.overlay_text_opacity();
                let ui_changed = current_state != last_state
                    || current_deepgram_connection_status != last_deepgram_connection_status
                    || current_overlay_dismissed != last_overlay_dismissed
                    || !Arc::ptr_eq(&current_overlay_footer_text, &last_overlay_footer_text)
                    || current_overlay_correction_active != last_overlay_correction_active
                    || !Arc::ptr_eq(
                        &current_overlay_correction_text,
                        &last_overlay_correction_text,
                    )
                    || !Arc::ptr_eq(&current_overlay_text, &last_overlay_text)
                    || !Arc::ptr_eq(&current_overlay_error_text, &last_overlay_error_text)
                    || (current_overlay_text_opacity - last_overlay_text_opacity).abs()
                        > f64::EPSILON;
                let mic_meter_changed = current_mic_meter != last_mic_meter;

                last_state = current_state;
                last_deepgram_connection_status = current_deepgram_connection_status;
                last_mic_meter = current_mic_meter;
                last_overlay_dismissed = current_overlay_dismissed;
                last_overlay_footer_text = Arc::clone(&current_overlay_footer_text);
                last_overlay_correction_active = current_overlay_correction_active;
                last_overlay_correction_text = Arc::clone(&current_overlay_correction_text);
                last_overlay_text = Arc::clone(&current_overlay_text);
                last_overlay_error_text = Arc::clone(&current_overlay_error_text);
                last_overlay_text_opacity = current_overlay_text_opacity;

                if ui_changed {
                    log::debug!(
                        "polling loop state transition -> state={}, connection={:?}, opacity={:.2}, text_len={}, correction_len={}, frame={}",
                        current_state,
                        current_deepgram_connection_status,
                        current_overlay_text_opacity,
                        current_overlay_text.len(),
                        current_overlay_correction_text.len(),
                        frame_count
                    );
                }

                if ui_changed || mic_meter_changed {
                    let update = Box::new(UiUpdate {
                        delegate_addr,
                        deepgram_connection_status: current_deepgram_connection_status,
                        mic_meter: current_mic_meter,
                        mic_meter_changed,
                        overlay_dismissed: current_overlay_dismissed,
                        overlay_footer_text: current_overlay_footer_text,
                        overlay_correction_active: current_overlay_correction_active,
                        overlay_correction_text: current_overlay_correction_text,
                        overlay_text: current_overlay_text,
                        overlay_error_text: current_overlay_error_text,
                        overlay_text_opacity: current_overlay_text_opacity,
                        state: current_state,
                    });

                    extern "C" {
                        static _dispatch_main_q: std::ffi::c_void;
                        fn dispatch_async_f(
                            queue: *const std::ffi::c_void,
                            context: *mut std::ffi::c_void,
                            work: extern "C" fn(*mut std::ffi::c_void),
                        );
                    }

                    unsafe {
                        dispatch_async_f(
                            &_dispatch_main_q,
                            Box::into_raw(update) as *mut std::ffi::c_void,
                            perform_ui_update,
                        );
                    }
                }

                if hotkey_capture_controller.has_pending_ui_update() {
                    delegate_ref.dispatch_to_main_thread(|delegate| {
                        delegate.apply_hotkey_capture_ui_update();
                    });
                }

                if transformation_models_controller.has_pending_ui_update() {
                    delegate_ref.dispatch_to_main_thread(|delegate| {
                        delegate.apply_transformation_models_ui_update();
                    });
                }

                if deepgram_connection_controller.has_pending_ui_update() {
                    delegate_ref.dispatch_to_main_thread(|delegate| {
                        delegate.apply_deepgram_connection_ui_update();
                    });
                }
            }
        })
        .expect("failed to spawn UI status polling thread");
}

extern "C" fn perform_ui_update(ctx: *mut std::ffi::c_void) {
    let update = unsafe { Box::from_raw(ctx as *mut UiUpdate) };
    let delegate = unsafe { &*(update.delegate_addr as *const AppDelegate) };
    let mtm = MainThreadMarker::new().expect("must be on main thread");

    update_status_item(delegate, mtm, update.state);
    update_billing_menu_item(delegate, &update.overlay_footer_text);
    if update.mic_meter_changed {
        if let Some(settings_window) = delegate.ivars().settings_window.get() {
            settings_window.update_meter(Some(update.mic_meter));
        }
    }
    update_overlay_window(
        delegate,
        update.state,
        update.deepgram_connection_status,
        update.overlay_dismissed,
        &update.overlay_text,
        &update.overlay_error_text,
        &update.overlay_correction_text,
        update.overlay_correction_active,
        update.overlay_text_opacity,
        &update.overlay_footer_text,
        update.mic_meter,
    );
}

fn update_status_item(delegate: &AppDelegate, mtm: MainThreadMarker, state: u8) {
    if let Some(status_item) = delegate.ivars().status_item.get() {
        if let Some(button) = status_item.button(mtm) {
            let is_active = matches!(
                state,
                STATE_RECORDING | STATE_PROCESSING | STATE_BUFFER_READY | STATE_TRANSFORMING
            );

            update_status_item_icon(&button, mtm, is_active);
        }
    }
}

fn update_status_item_icon(
    button: &objc2_app_kit::NSStatusBarButton,
    mtm: MainThreadMarker,
    is_active: bool,
) {
    let active_icon_retained = delegate_icon_cache().get(is_active, mtm);
    button.setImage(Some(&active_icon_retained));
}

fn update_billing_menu_item(delegate: &AppDelegate, overlay_footer_text: &str) {
    let Some(billing_menu_item) = delegate.ivars().billing_menu_item.get() else {
        return;
    };

    match billing_menu_text(overlay_footer_text) {
        Some(billing_text) => {
            billing_menu_item.setTitle(&NSString::from_str(billing_text));
            billing_menu_item.setHidden(false);
        }
        None => billing_menu_item.setHidden(true),
    }
}

fn update_overlay_window(
    delegate: &AppDelegate,
    state: u8,
    deepgram_connection_status: DeepgramConnectionStatus,
    overlay_dismissed: bool,
    overlay_text: &str,
    overlay_error_text: &str,
    overlay_correction_text: &str,
    overlay_correction_active: bool,
    overlay_text_opacity: f64,
    overlay_footer_text: &str,
    mic_meter: MicMeterSnapshot,
) {
    let Some(overlay_window) = delegate.ivars().overlay_window.get() else {
        return;
    };

    overlay_window.update(
        state,
        deepgram_connection_status,
        overlay_dismissed,
        overlay_text,
        overlay_error_text,
        overlay_correction_text,
        overlay_correction_active,
        overlay_text_opacity,
        overlay_footer_text,
        mic_meter,
    );
}

struct IconCache {
    active_icon: std::sync::OnceLock<Retained<objc2_app_kit::NSImage>>,
    inactive_icon: std::sync::OnceLock<Retained<objc2_app_kit::NSImage>>,
}

impl IconCache {
    fn get(&self, is_active: bool, mtm: MainThreadMarker) -> Retained<objc2_app_kit::NSImage> {
        if is_active {
            self.active_icon
                .get_or_init(|| make_status_bar_active_icon(mtm))
                .clone()
        } else {
            self.inactive_icon
                .get_or_init(|| make_status_bar_icon(mtm))
                .clone()
        }
    }
}

fn delegate_icon_cache() -> &'static IconCache {
    static CACHE: std::sync::OnceLock<IconCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| IconCache {
        active_icon: std::sync::OnceLock::new(),
        inactive_icon: std::sync::OnceLock::new(),
    })
}
