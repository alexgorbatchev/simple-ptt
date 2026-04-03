use std::cell::OnceCell;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSImageScaling, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSWorkspace,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSURL,
};

use crate::audio::{validate_mic_config, AudioConfigApplyEffect, AudioController};
use crate::billing::BillingController;
use crate::config::{self, Config};
use crate::hotkey_binding::{format_hotkey_binding, parse_hotkey_binding};
use crate::hotkey_capture::{
    capture_outcome_message, HotkeyCaptureController, HotkeyCaptureOutcome, HotkeyCapturePreview,
    HotkeyCaptureTarget,
};
use crate::icon::{make_application_icon, make_status_bar_active_icon, make_status_bar_icon};
use crate::overlay::{OverlayStyle, OverlayWindow};
use crate::settings::LiveConfigStore;
use crate::settings_window::SettingsWindow;
use crate::state::{
    AppState, MicMeterSnapshot, STATE_BUFFER_READY, STATE_ERROR, STATE_IDLE, STATE_PROCESSING,
    STATE_RECORDING, STATE_TRANSFORMING,
};

const APP_DISPLAY_NAME: &str = "simple-ptt";
const GITHUB_REPO_URL: &str = "https://github.com/alexgorbatchev/simple-ptt";
const NS_VARIABLE_STATUS_ITEM_LENGTH: f64 = -1.0;

pub struct Ivars {
    audio_controller: AudioController,
    billing_controller: BillingController,
    billing_menu_item: OnceCell<Retained<NSMenuItem>>,
    config_store: LiveConfigStore,
    hotkey_capture_controller: HotkeyCaptureController,
    overlay_style: OverlayStyle,
    active_status_bar_icon: Retained<objc2_app_kit::NSImage>,
    idle_status_bar_icon: Retained<objc2_app_kit::NSImage>,
    overlay_window: OnceCell<OverlayWindow>,
    settings_window: OnceCell<SettingsWindow>,
    status_item: OnceCell<Retained<NSStatusItem>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppDelegate"]
    #[ivars = Ivars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = MainThreadMarker::from(self);
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

            let status_bar = NSStatusBar::systemStatusBar();
            let status_item = status_bar.statusItemWithLength(NS_VARIABLE_STATUS_ITEM_LENGTH);

            let application_icon = make_application_icon(mtm);
            unsafe {
                app.setApplicationIconImage(Some(&application_icon));
            }

            if let Some(button) = status_item.button(mtm) {
                button.setImage(Some(&self.ivars().idle_status_bar_icon));
                button.setImageScaling(NSImageScaling::ScaleProportionallyDown);
                button.setTitle(ns_string!(""));
                button.setContentTintColor(None);
            }

            let menu = NSMenu::new(mtm);

            let title_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&format!(
                        "{} — Version {}",
                        APP_DISPLAY_NAME,
                        env!("CARGO_PKG_VERSION")
                    )),
                    Some(sel!(openGitHubRepo:)),
                    ns_string!(""),
                )
            };
            unsafe {
                title_item.setTarget(Some(self));
            }
            menu.addItem(&title_item);

            let settings_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!("Settings…"),
                    Some(sel!(openSettings:)),
                    ns_string!(",")
                )
            };
            unsafe {
                settings_item.setTarget(Some(self));
            }
            menu.addItem(&settings_item);

            let billing_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!(""),
                    None,
                    ns_string!(""),
                )
            };
            billing_item.setEnabled(false);
            billing_item.setHidden(true);
            menu.addItem(&billing_item);

            let quit_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!("Quit"),
                    Some(sel!(terminate:)),
                    ns_string!("q"),
                )
            };
            menu.addItem(&quit_item);

            status_item.setMenu(Some(&menu));

            self.ivars()
                .overlay_window
                .set(OverlayWindow::new(mtm, &self.ivars().overlay_style))
                .expect("overlay window must only be set once");
            self.ivars()
                .status_item
                .set(status_item)
                .expect("status item must only be set once");
            self.ivars()
                .billing_menu_item
                .set(billing_item)
                .expect("billing item must only be set once");
            self.ivars()
                .settings_window
                .set(SettingsWindow::new(self, mtm))
                .expect("settings window must only be set once");

            let config_file_missing = config_file_is_missing(self.ivars().config_store.path());
            let deepgram_api_key_missing = self
                .ivars()
                .config_store
                .current()
                .resolve_deepgram_api_key()
                .is_err();

            if config_file_missing || deepgram_api_key_missing {
                self.present_settings_window();
            }

            if !deepgram_api_key_missing && config_file_missing {
                show_modal_alert(
                    "simple-ptt didn't find a config.toml yet",
                    &format!(
                        concat!(
                            "Settings opened so you can create one on first launch.\n",
                            "Review the defaults, then click Save and Apply to write:\n\n",
                            "{}\n\n",
                            "simple-ptt is a menu bar app, so a successful launch appears in the menu bar rather than the Dock."
                        ),
                        self.ivars().config_store.path().display()
                    ),
                );
            }

            log::info!("menu bar initialized");
        }
    }

    impl AppDelegate {
        #[unsafe(method(openGitHubRepo:))]
        fn open_github_repo(&self, _sender: Option<&AnyObject>) {
            let github_url = NSString::from_str(GITHUB_REPO_URL);
            let Some(url) = NSURL::URLWithString(&github_url) else {
                log::error!("invalid GitHub URL configured: {}", GITHUB_REPO_URL);
                return;
            };

            let opened = NSWorkspace::sharedWorkspace().openURL(&url);
            if !opened {
                log::error!("failed to open GitHub URL: {}", GITHUB_REPO_URL);
            }
        }

        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            self.present_settings_window();
        }

        #[unsafe(method(captureRecordHotkey:))]
        fn capture_record_hotkey(&self, _sender: Option<&AnyObject>) {
            self.begin_hotkey_capture(HotkeyCaptureTarget::Record);
        }

        #[unsafe(method(captureTransformHotkey:))]
        fn capture_transform_hotkey(&self, _sender: Option<&AnyObject>) {
            self.begin_hotkey_capture(HotkeyCaptureTarget::Transform);
        }

        #[unsafe(method(saveSettings:))]
        fn save_settings(&self, _sender: Option<&AnyObject>) {
            let Some(settings_window) = self.ivars().settings_window.get() else {
                return;
            };

            self.ivars().hotkey_capture_controller.cancel();
            settings_window.cancel_hotkey_capture();

            let proposed_config = match settings_window.read_config() {
                Ok(config) => config,
                Err(error) => {
                    settings_window.set_status(&error);
                    show_modal_alert("Couldn't save settings", &error);
                    return;
                }
            };

            if let Err(error) = validate_settings_config(&proposed_config) {
                settings_window.set_status(&error);
                show_modal_alert("Couldn't save settings", &error);
                return;
            }

            let runtime_config = config::materialize_runtime_config(&proposed_config);
            let previous_file_config = self.ivars().config_store.current_file();

            if let Err(error) = config::save_config(self.ivars().config_store.path(), &proposed_config)
            {
                settings_window.set_status(&error);
                show_modal_alert("Couldn't save settings", &error);
                return;
            }

            let audio_apply_effect = match self.ivars().audio_controller.apply_mic_config(&runtime_config.mic) {
                Ok(effect) => effect,
                Err(error) => {
                    let _ = config::save_config(
                        self.ivars().config_store.path(),
                        &previous_file_config,
                    );
                    settings_window.set_status(&error);
                    show_modal_alert("Couldn't apply audio settings", &error);
                    return;
                }
            };

            self.ivars()
                .config_store
                .replace(proposed_config.clone(), runtime_config.clone());
            self.ivars().billing_controller.refresh_month_to_date_spend();
            if let Some(overlay_window) = self.ivars().overlay_window.get() {
                overlay_window.apply_style(&overlay_style_from_config(&runtime_config));
            }

            let audio_message = match audio_apply_effect {
                AudioConfigApplyEffect::AppliedNow => "audio changes applied now",
                AudioConfigApplyEffect::DeferredUntilRecordingStops => {
                    "audio device/sample-rate changes will apply after the current recording stops"
                }
            };
            settings_window.load_from_config(
                &proposed_config,
                &self.ivars().config_store.path().display().to_string(),
            );
            settings_window.set_status(&format!("Saved and applied settings. {}.", audio_message));
        }
    }
);

impl AppDelegate {
    pub fn new(
        mtm: MainThreadMarker,
        overlay_style: OverlayStyle,
        config_store: LiveConfigStore,
        hotkey_capture_controller: HotkeyCaptureController,
        billing_controller: BillingController,
        audio_controller: AudioController,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            audio_controller,
            billing_controller,
            billing_menu_item: OnceCell::new(),
            config_store,
            hotkey_capture_controller,
            overlay_style,
            active_status_bar_icon: make_status_bar_active_icon(mtm),
            idle_status_bar_icon: make_status_bar_icon(mtm),
            overlay_window: OnceCell::new(),
            settings_window: OnceCell::new(),
            status_item: OnceCell::new(),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn present_settings_window(&self) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        self.ivars().hotkey_capture_controller.cancel();
        settings_window.cancel_hotkey_capture();

        let current_file_config = self.ivars().config_store.current_file();
        settings_window.load_from_config(
            &current_file_config,
            &self.ivars().config_store.path().display().to_string(),
        );
        settings_window.show(MainThreadMarker::from(self));
    }

    fn begin_hotkey_capture(&self, target: HotkeyCaptureTarget) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        self.ivars().hotkey_capture_controller.cancel();
        settings_window.cancel_hotkey_capture();
        self.ivars().hotkey_capture_controller.begin_capture(target);
        settings_window.begin_hotkey_capture(target);
    }

    fn handle_pending_hotkey_capture_preview(&self) {
        let Some(HotkeyCapturePreview { target, text }) =
            self.ivars().hotkey_capture_controller.take_preview()
        else {
            return;
        };
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        settings_window.set_hotkey_capture_preview(target, &text);
    }

    fn handle_pending_hotkey_capture(&self) {
        let Some(outcome) = self.ivars().hotkey_capture_controller.take_outcome() else {
            return;
        };
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        match outcome {
            HotkeyCaptureOutcome::Cancelled { .. } => {
                settings_window.cancel_hotkey_capture();
                settings_window.set_status("Hotkey capture canceled.");
            }
            HotkeyCaptureOutcome::Captured { target, binding } => {
                let Some(captured_name) = format_hotkey_binding(binding) else {
                    settings_window.cancel_hotkey_capture();
                    settings_window.set_status("That hotkey is not supported.");
                    return;
                };

                let other_target = match target {
                    HotkeyCaptureTarget::Record => HotkeyCaptureTarget::Transform,
                    HotkeyCaptureTarget::Transform => HotkeyCaptureTarget::Record,
                };
                let other_hotkey = settings_window.hotkey_value(other_target);
                if parse_hotkey_binding(other_hotkey.as_str()).ok() == Some(binding) {
                    settings_window.cancel_hotkey_capture();
                    settings_window.set_status("Record and transform hotkeys must be different.");
                    return;
                }

                settings_window.finish_hotkey_capture();
                settings_window.set_hotkey_value(target, &captured_name);
                if let Some(message) = capture_outcome_message(outcome) {
                    settings_window.set_status(&message);
                }
            }
        }
    }

    pub fn update_ui(
        &self,
        mtm: MainThreadMarker,
        state: u8,
        overlay_text: &str,
        overlay_text_opacity: f64,
        overlay_footer_text: &str,
        mic_meter: MicMeterSnapshot,
    ) {
        self.handle_pending_hotkey_capture_preview();
        self.handle_pending_hotkey_capture();
        self.ivars().audio_controller.apply_pending_if_idle();
        update_status_item(self, mtm, state);
        update_billing_menu_item(self, overlay_footer_text);
        update_overlay_window(
            self,
            mtm,
            state,
            overlay_text,
            overlay_text_opacity,
            overlay_footer_text,
            mic_meter,
        );
    }
}

fn config_file_is_missing(path: &Path) -> bool {
    matches!(
        std::fs::metadata(path),
        Err(error) if error.kind() == ErrorKind::NotFound
    )
}

pub fn overlay_style_from_config(config: &Config) -> OverlayStyle {
    let overlay_font_size = if config.ui.font_size.is_finite() && config.ui.font_size > 0.0 {
        config.ui.font_size
    } else {
        12.0
    };
    let overlay_footer_font_size = match config.ui.footer_font_size {
        Some(footer_font_size) if footer_font_size.is_finite() && footer_font_size > 0.0 => {
            footer_font_size
        }
        Some(_) | None => overlay_font_size * 0.6,
    };
    let transformation_hotkey = config
        .resolve_transformation_config()
        .ok()
        .map(|_| config.transformation.hotkey.as_str());

    OverlayStyle {
        font_name: config.ui.font_name.clone(),
        font_size: overlay_font_size,
        footer_font_size: overlay_footer_font_size,
        meter_style: config.ui.meter_style,
        transformation_hint: transformation_hotkey.map(|hotkey| {
            format!(
                "{}: transform {}: paste ESC: cancel",
                hotkey, config.ui.hotkey
            )
        }),
    }
}

fn validate_settings_config(config: &Config) -> Result<(), String> {
    let record_hotkey = parse_hotkey_binding(config.ui.hotkey.as_str())
        .map_err(|error| format!("record hotkey is invalid: {}", error))?;

    if !config.ui.font_size.is_finite() || config.ui.font_size <= 0.0 {
        return Err("Font size must be a positive number".to_owned());
    }

    if let Some(footer_font_size) = config.ui.footer_font_size {
        if !footer_font_size.is_finite() || footer_font_size <= 0.0 {
            return Err("Footer font size must be a positive number".to_owned());
        }
    }

    if !config.mic.gain.is_finite() {
        return Err("Gain must be a finite number".to_owned());
    }

    validate_mic_config(&config.mic)?;

    config.resolve_deepgram_api_key()?;

    if let Some(provider) = config.transformation.provider.as_deref().map(str::trim) {
        if !provider.is_empty() {
            let transform_hotkey = parse_hotkey_binding(config.transformation.hotkey.as_str())
                .map_err(|error| format!("transform hotkey is invalid: {}", error))?;
            if record_hotkey == transform_hotkey {
                return Err("record and transform hotkeys must be different".to_owned());
            }
            config.resolve_transformation_config()?;
        }
    }

    Ok(())
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

fn billing_menu_text(overlay_footer_text: &str) -> Option<&str> {
    let trimmed_overlay_footer_text = overlay_footer_text.trim();
    if trimmed_overlay_footer_text.starts_with("Billing (")
        && trimmed_overlay_footer_text.contains(": $")
    {
        Some(trimmed_overlay_footer_text)
    } else {
        None
    }
}

fn update_status_item(delegate: &AppDelegate, mtm: MainThreadMarker, state: u8) {
    if let Some(status_item) = delegate.ivars().status_item.get() {
        if let Some(button) = status_item.button(mtm) {
            let is_active = matches!(
                state,
                STATE_RECORDING | STATE_PROCESSING | STATE_BUFFER_READY | STATE_TRANSFORMING
            );
            let icon = if is_active {
                &delegate.ivars().active_status_bar_icon
            } else {
                &delegate.ivars().idle_status_bar_icon
            };
            button.setImage(Some(icon));
            button.setContentTintColor(None);
        }
    }
}

fn update_overlay_window(
    delegate: &AppDelegate,
    mtm: MainThreadMarker,
    state: u8,
    overlay_text: &str,
    overlay_text_opacity: f64,
    overlay_footer_text: &str,
    mic_meter: MicMeterSnapshot,
) {
    if let Some(overlay_window) = delegate.ivars().overlay_window.get() {
        overlay_window.update(
            mtm,
            state,
            overlay_text,
            overlay_text_opacity,
            overlay_footer_text,
            mic_meter,
        );
    }
}

extern "C" {
    static _dispatch_main_q: std::ffi::c_void;
    fn dispatch_async_f(
        queue: *const std::ffi::c_void,
        context: *mut std::ffi::c_void,
        work: extern "C" fn(*mut std::ffi::c_void),
    );
}

struct UiUpdate {
    delegate_addr: usize,
    mic_meter: MicMeterSnapshot,
    overlay_footer_text: String,
    overlay_text: String,
    overlay_text_opacity: f64,
    state: u8,
}

extern "C" fn perform_ui_update(ctx: *mut std::ffi::c_void) {
    let update = unsafe { Box::from_raw(ctx as *mut UiUpdate) };
    let mtm = MainThreadMarker::new().expect("perform_ui_update must run on main thread");
    let delegate = unsafe { &*(update.delegate_addr as *const AppDelegate) };
    delegate.update_ui(
        mtm,
        update.state,
        &update.overlay_text,
        update.overlay_text_opacity,
        &update.overlay_footer_text,
        update.mic_meter,
    );
}

fn show_modal_alert(message_text: &str, informative_text: &str) {
    let mtm = MainThreadMarker::new().expect("must run on main thread");
    let app = NSApplication::sharedApplication(mtm);
    let previous_activation_policy = app.activationPolicy();
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.activate();

    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(message_text));
    alert.setInformativeText(&NSString::from_str(informative_text));
    alert.addButtonWithTitle(ns_string!("OK"));
    alert.runModal();

    app.setActivationPolicy(previous_activation_policy);
}

pub fn show_startup_error_dialog(message_text: &str, informative_text: &str) {
    show_modal_alert(message_text, informative_text);
}

#[cfg(test)]
mod tests {
    use super::config_file_is_missing;

    #[test]
    fn config_file_is_missing_only_reports_not_found_paths() {
        let path = std::env::temp_dir().join(format!(
            "simple-ptt-missing-config-{}-{}.toml",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));

        let _ = std::fs::remove_file(&path);
        assert!(config_file_is_missing(&path));

        std::fs::write(&path, "[ui]\nhotkey = \"F5\"\n").unwrap();
        assert!(!config_file_is_missing(&path));
        std::fs::remove_file(&path).unwrap();
    }
}

pub fn setup_status_polling(
    delegate: Retained<AppDelegate>,
    state: Arc<AppState>,
    hotkey_capture_controller: HotkeyCaptureController,
) {
    let delegate_addr = Retained::as_ptr(&delegate) as usize;
    std::mem::forget(delegate);

    std::thread::Builder::new()
        .name("ui-poller".into())
        .spawn(move || {
            let mut last_mic_meter = MicMeterSnapshot::default();
            let mut last_overlay_footer_text = String::new();
            let mut last_overlay_text = String::new();
            let mut last_overlay_text_opacity = 1.0;
            let mut last_state = STATE_IDLE;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(75));
                let current_state = state.get_state();
                let current_mic_meter = state.mic_meter_snapshot();
                let current_overlay_footer_text = state.overlay_footer_text();
                let current_overlay_text = state.overlay_text();
                let current_overlay_text_opacity = state.overlay_text_opacity();
                let ui_changed = current_state != last_state
                    || current_overlay_footer_text != last_overlay_footer_text
                    || current_overlay_text != last_overlay_text
                    || (current_overlay_text_opacity - last_overlay_text_opacity).abs()
                        > f64::EPSILON;
                let mic_meter_changed = current_mic_meter != last_mic_meter;
                let should_animate_meter = current_state == STATE_RECORDING;
                let should_animate_overlay =
                    matches!(current_state, STATE_PROCESSING | STATE_TRANSFORMING);
                let hotkey_capture_update_pending =
                    hotkey_capture_controller.has_pending_ui_update();
                if !ui_changed
                    && !mic_meter_changed
                    && !should_animate_meter
                    && !should_animate_overlay
                    && !hotkey_capture_update_pending
                {
                    continue;
                }

                last_state = current_state;
                last_mic_meter = current_mic_meter;
                last_overlay_footer_text = current_overlay_footer_text.clone();
                last_overlay_text = current_overlay_text.clone();
                last_overlay_text_opacity = current_overlay_text_opacity;

                if ui_changed {
                    let label = match current_state {
                        STATE_RECORDING => "recording",
                        STATE_PROCESSING => "processing",
                        STATE_BUFFER_READY => "buffer-ready",
                        STATE_TRANSFORMING => "transforming",
                        STATE_ERROR => "error",
                        _ => "idle",
                    };
                    log::info!(
                        "ui update: state={}, transcript_len={}",
                        label,
                        current_overlay_text.len()
                    );
                }

                let update = Box::new(UiUpdate {
                    delegate_addr,
                    mic_meter: current_mic_meter,
                    overlay_footer_text: current_overlay_footer_text,
                    overlay_text: current_overlay_text,
                    overlay_text_opacity: current_overlay_text_opacity,
                    state: current_state,
                });
                unsafe {
                    dispatch_async_f(
                        &_dispatch_main_q,
                        Box::into_raw(update) as *mut std::ffi::c_void,
                        perform_ui_update,
                    );
                }
            }
        })
        .expect("failed to spawn ui-poller thread");
}
