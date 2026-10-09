use std::cell::{Cell, OnceCell, RefCell};
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use block2::StackBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSApplication, NSApplicationActivationOptions,
    NSApplicationActivationPolicy, NSApplicationDelegate, NSImageScaling, NSMenu, NSMenuItem,
    NSStatusBar, NSStatusItem, NSTextDelegate, NSTextViewDelegate, NSWindowDelegate, NSWorkspace,
    NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectNSDelayedPerforming,
    NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer, NSURL,
};

mod settings_presentation;
mod startup_windows;

use settings_presentation::{SettingsPresentation, SettingsPresentationStep};
use startup_windows::StartupWindows;

use crate::audio::{validate_mic_config, AudioConfigApplyEffect, AudioController};
use crate::config::{self, Config};
use crate::deepgram_connection::{
    DeepgramCheckRequest, DeepgramCheckUpdate, DeepgramConnectionController,
};
use crate::hotkey_binding::{format_hotkey_binding, parse_hotkey_binding, parse_key};
use crate::hotkey_capture::{
    capture_outcome_message, HotkeyCaptureController, HotkeyCaptureOutcome, HotkeyCapturePreview,
    HotkeyCaptureTarget,
};
use crate::icon::{make_application_icon, make_status_bar_active_icon, make_status_bar_icon};
use crate::overlay::{OverlayStyle, OverlayWindow};
use crate::permissions::{self, GlobalHotkeyPermissions};
use crate::permissions_dialog::PermissionsDialog;
use crate::settings::LiveConfigStore;
use crate::settings_window::form::Prompt;
use crate::settings_window::helpers::{
    manual_model_fetch_message, settings_load_status, unreadable_model_cache_message,
};
use crate::settings_window::{SettingsWindow, SAVE_BUTTON_TITLE};
use crate::state::{
    AppState, MicMeterSnapshot, OverlayText, STATE_BUFFER_READY, STATE_ERROR, STATE_IDLE,
    STATE_PROCESSING, STATE_RECORDING, STATE_TRANSFORMING,
};
use crate::transformation_models::{
    api_key_belongs_to_provider, ModelListPlan, ModelListTrigger, TransformationModelAction,
    TransformationModelUpdate, TransformationModelsController, TransformationProviderRequest,
};

const APP_DISPLAY_NAME: &str = "simple-ptt";
const GITHUB_REPO_URL: &str = "https://github.com/alexgorbatchev/simple-ptt";
const NS_VARIABLE_STATUS_ITEM_LENGTH: f64 = -1.0;

pub struct Ivars {
    app_updater: OnceCell<Option<crate::updater::AppUpdater>>,
    audio_controller: AudioController,
    initial_audio_error: Option<String>,
    config_store: LiveConfigStore,
    deepgram_connection_controller: DeepgramConnectionController,
    hotkey_capture_controller: HotkeyCaptureController,
    state: Arc<AppState>,
    overlay_style: OverlayStyle,
    active_status_bar_icon: Retained<objc2_app_kit::NSImage>,
    idle_status_bar_icon: Retained<objc2_app_kit::NSImage>,
    overlay_window: OnceCell<OverlayWindow>,
    permissions_dialog: OnceCell<PermissionsDialog>,
    startup_hotkey_permissions: GlobalHotkeyPermissions,
    accessibility_permission_requested: Cell<bool>,
    input_monitoring_permission_requested: Cell<bool>,
    microphone_permission_requested: Cell<bool>,
    settings_window: OnceCell<SettingsWindow>,
    settings_presentation: Cell<SettingsPresentation>,
    permissions_dialog_returns_in_front_of_settings: Cell<bool>,
    transformation_models_controller: TransformationModelsController,
    status_item: OnceCell<Retained<NSStatusItem>>,
    status_poll: RefCell<StatusPollState>,
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

            let config_file_missing = config_file_is_missing(self.ivars().config_store.path());
            let deepgram_api_key_missing = self
                .ivars()
                .config_store
                .current()
                .resolve_deepgram_api_key()
                .is_err();
            let audio_startup_failed = self.ivars().initial_audio_error.is_some();
            let startup_permissions_missing = !self.ivars().startup_hotkey_permissions.all_granted();
            let startup_windows = StartupWindows::plan(
                config_file_missing,
                deepgram_api_key_missing,
                audio_startup_failed,
                startup_permissions_missing,
            );

            if startup_windows.settings || startup_windows.permissions_dialog {
                app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
                app.activate();
            } else {
                app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            }

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
            app.setMainMenu(Some(&make_hidden_main_menu(self, mtm)));

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
                    ns_string!("")
                )
            };
            unsafe {
                settings_item.setTarget(Some(self));
            }
            menu.addItem(&settings_item);

            let check_updates_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!("Check for Updates…"),
                    Some(sel!(checkForUpdates:)),
                    ns_string!(""),
                )
            };

            let app_updater = crate::updater::AppUpdater::init(mtm);
            if let Some(ref updater) = app_updater {
                updater.set_automatically_checks_for_updates(
                    self.ivars().config_store.current().ui.auto_check_updates,
                );
                unsafe {
                    check_updates_item.setTarget(Some(updater.controller()));
                }
            } else {
                unsafe {
                    check_updates_item.setTarget(Some(self));
                }
            }
            menu.addItem(&check_updates_item);
            let _ = self.ivars().app_updater.set(app_updater);

            let permissions_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!("Application Permissions…"),
                    Some(sel!(openPermissions:)),
                    ns_string!("")
                )
            };
            unsafe {
                permissions_item.setTarget(Some(self));
            }
            menu.addItem(&permissions_item);

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

            let overlay_window = OverlayWindow::new(
                mtm,
                &self.ivars().overlay_style,
                Arc::clone(&self.ivars().state),
            );
            overlay_window.set_delegate(ProtocolObject::from_ref(self));
            self.ivars()
                .overlay_window
                .set(overlay_window)
                .expect("overlay window must only be set once");
            self.ivars()
                .status_item
                .set(status_item)
                .expect("status item must only be set once");
            let settings_window = SettingsWindow::new(mtm, self, ProtocolObject::from_ref(self));
            self.ivars()
                .settings_window
                .set(settings_window)
                .expect("settings window must only be set once");
            self.ivars()
                .permissions_dialog
                .set(PermissionsDialog::new(self, mtm))
                .expect("permissions dialog must only be set once");

            if startup_permissions_missing {
                self.sync_hotkey_permissions_ui();
            }
            if startup_windows.settings {
                self.ivars()
                    .permissions_dialog_returns_in_front_of_settings
                    .set(startup_windows.permissions_dialog_returns_in_front_of_settings());
                self.present_settings_window(self.ivars().initial_audio_error.as_deref());
            }

            if startup_windows.permissions_dialog {
                self.present_startup_hotkey_permissions_window();
            }

            if startup_windows.missing_config_alert {
                show_modal_alert(
                    "simple-ptt didn't find a config.toml yet",
                    &missing_config_alert_text(self.ivars().config_store.path()),
                );
            }

            if startup_windows.audio_failure_alert {
                if let Some(audio_error) = self.ivars().initial_audio_error.as_deref() {
                    show_modal_alert(
                        "simple-ptt couldn't start audio input",
                        &audio_startup_failure_alert_text(audio_error),
                    );
                }
            }

            log::info!("menu bar initialized");
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _notification: &NSNotification) {
            let step =
                self.advance_settings_presentation(SettingsPresentation::app_did_become_active);
            self.perform_settings_presentation_step(step);
        }
    }

    impl AppDelegate {
        // AppDelegate must implement a method for every
        // `settings_window::actions::SettingsAction::selector()`; enforced by the
        // `app_delegate_implements_every_settings_window_action` test.
        #[unsafe(method(micGainSliderChanged:))]
        fn mic_gain_slider_changed(&self, _sender: Option<&AnyObject>) {
            let Some(settings_window) = self.ivars().settings_window.get() else {
                return;
            };
            let gain = settings_window.mic_gain_slider_value();
            self.ivars().state.set_preview_mic_gain(Some(gain));
            settings_window.update_mic_gain_label(gain);
        }

        #[unsafe(method(micAudioDeviceChanged:))]
        fn mic_audio_device_changed(&self, _sender: Option<&AnyObject>) {
            let Some(settings_window) = self.ivars().settings_window.get() else {
                return;
            };
            let device = settings_window.mic_audio_device_value();
            self.ivars().audio_controller.set_preview_audio_device(device);
            self.ivars().audio_controller.apply_pending_if_idle();
        }

        // Target of the repeating main-run-loop timer scheduled by
        // `setup_status_polling`; its selector is `status_poll_selector()`.
        #[unsafe(method(pollStatus:))]
        fn poll_status(&self, _timer: &NSTimer) {
            self.run_status_poll_tick(MainThreadMarker::from(self));
        }

        // Performed in the default run loop mode by
        // `perform_settings_presentation_step`; its selector is
        // `settings_presentation_turn_selector()`.
        #[unsafe(method(runSettingsPresentationTurn))]
        fn run_settings_presentation_turn(&self) {
            let app_is_active =
                NSApplication::sharedApplication(MainThreadMarker::from(self)).isActive();
            // A key nonactivating panel can make AppKit report active while
            // another process is still frontmost. Settings needs both.
            let app_is_frontmost =
                objc2_app_kit::NSRunningApplication::currentApplication().isActive();
            let step = self.advance_settings_presentation(|presentation| {
                presentation.default_mode_turn(app_is_active, app_is_frontmost)
            });
            self.perform_settings_presentation_step(step);
        }

        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, sender: Option<&AnyObject>) {
            self.promote_for_window_presentation();
            if let Some(Some(ref updater)) = self.ivars().app_updater.get() {
                updater.check_for_updates(sender);
            }
        }

        #[unsafe(method(openGitHubRepo:))]
        fn open_github_repo(&self, _sender: Option<&AnyObject>) {
            self.open_github_repo_url();
        }

        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            // A request from the menu puts Settings in front, even if the
            // launch-time request that would have kept the permissions dialog
            // in front is still waiting for activation.
            self.ivars()
                .permissions_dialog_returns_in_front_of_settings
                .set(false);
            self.present_settings_window(None);
        }

        #[unsafe(method(openPermissions:))]
        fn open_permissions(&self, _sender: Option<&AnyObject>) {
            self.sync_hotkey_permissions_ui();
            self.present_hotkey_permissions_window();
        }

        #[unsafe(method(requestAccessibilityPermission:))]
        fn request_accessibility_permission(&self, _sender: Option<&AnyObject>) {
            self.ivars().accessibility_permission_requested.set(true);
            self.deactivate_for_system_settings_transition();
            if let Err(error) =
                self.open_system_settings_and_activate(permissions::accessibility_settings_urls())
            {
                log::error!("failed to open Accessibility settings: {}", error);
            }
            self.sync_hotkey_permissions_ui();
        }

        #[unsafe(method(requestMicrophonePermission:))]
        fn request_microphone_permission(&self, _sender: Option<&AnyObject>) {
            let already_requested = self.ivars().microphone_permission_requested.get();
            self.ivars().microphone_permission_requested.set(true);

            let status = permissions::microphone_authorization_status();
            if matches!(
                status,
                permissions::AVAuthorizationStatus::Denied
                    | permissions::AVAuthorizationStatus::Restricted
            ) || already_requested
            {
                if let Err(error) =
                    self.open_system_settings_and_activate(permissions::microphone_settings_urls())
                {
                    log::error!("failed to open microphone settings: {}", error);
                }
            }

            if let Err(error) = permissions::request_microphone_access(|granted| {
                if granted {
                    log::info!("microphone access granted by user");
                } else {
                    log::info!("microphone access denied by user");
                }
            }) {
                log::error!("failed to request microphone access: {}", error);
            }

            self.sync_hotkey_permissions_ui();
        }

        #[unsafe(method(resetHotkeyPermissions:))]
        fn reset_hotkey_permissions(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = permissions::reset_application_permissions_and_relaunch() {
                log::error!("failed to reset macOS permissions: {}", error);

                show_modal_alert(
                    "simple-ptt couldn't reset permissions properly",
                    &format!(
                        concat!(
                            "The process manager failed to background the reset script.\n\n",
                            "Error: {}"
                        ),
                        error
                    ),
                );
                return;
            }

            let app = NSApplication::sharedApplication(MainThreadMarker::from(self));
            app.terminate(None);
        }

        #[unsafe(method(recheckHotkeyPermissions:))]
        fn recheck_hotkey_permissions(&self, _sender: Option<&AnyObject>) {
            self.sync_hotkey_permissions_ui();
            self.activate_audio_if_ready();
        }

        #[unsafe(method(quitFromPermissionsDialog:))]
        fn quit_from_permissions_dialog(&self, _sender: Option<&AnyObject>) {
            let app = NSApplication::sharedApplication(MainThreadMarker::from(self));
            let flow = self.current_hotkey_permission_flow();

            if flow.relaunch_required() {
                if let Err(error) = permissions::relaunch_current_application() {
                    log::error!("failed to relaunch after permission grant: {}", error);
                    show_modal_alert(
                        "simple-ptt couldn't relaunch itself",
                        &format!(
                            concat!(
                                "Permissions are granted, but simple-ptt failed to reopen automatically.\n\n",
                                "Quit and launch the app again manually.\n\n",
                                "Error: {}"
                            ),
                            error
                        ),
                    );
                    return;
                }

                app.terminate(None);
                return;
            }

            if flow.all_granted() {
                let Some(permissions_dialog) = self.ivars().permissions_dialog.get() else {
                    return;
                };
                permissions_dialog.hide();
                self.restore_accessory_activation_policy_if_possible();
                self.activate_audio_if_ready();
                return;
            }

            app.terminate(None);
        }

        #[unsafe(method(captureRecordHotkey:))]
        fn capture_record_hotkey(&self, _sender: Option<&AnyObject>) {
            self.begin_hotkey_capture(HotkeyCaptureTarget::Record);
        }

        #[unsafe(method(captureTransformHotkey:))]
        fn capture_transform_hotkey(&self, _sender: Option<&AnyObject>) {
            self.begin_hotkey_capture(HotkeyCaptureTarget::Transform);
        }

        #[unsafe(method(captureCorrectionKey:))]
        fn capture_correction_key(&self, _sender: Option<&AnyObject>) {
            self.begin_hotkey_capture(HotkeyCaptureTarget::Correction);
        }

        #[unsafe(method(transformationProviderChanged:))]
        fn transformation_provider_changed(&self, _sender: Option<&AnyObject>) {
            self.sync_transformation_provider_ui(ModelListTrigger::ProviderChanged);
        }

        #[unsafe(method(refreshTransformationModels:))]
        fn refresh_transformation_models(&self, _sender: Option<&AnyObject>) {
            self.start_transformation_model_action(TransformationModelAction::Refresh);
        }

        #[unsafe(method(checkTransformationProvider:))]
        fn check_transformation_provider(&self, _sender: Option<&AnyObject>) {
            self.start_transformation_model_action(TransformationModelAction::Check);
        }

        #[unsafe(method(checkDeepgramConnection:))]
        fn check_deepgram_connection(&self, _sender: Option<&AnyObject>) {
            self.start_deepgram_connection_check();
        }

        #[unsafe(method(resetDictationPrompt:))]
        fn reset_dictation_prompt(&self, _sender: Option<&AnyObject>) {
            self.reset_prompt_to_default(Prompt::Dictation);
        }

        #[unsafe(method(resetCorrectionPrompt:))]
        fn reset_correction_prompt(&self, _sender: Option<&AnyObject>) {
            self.reset_prompt_to_default(Prompt::Correction);
        }

        #[unsafe(method(cancelSettingsPressed:))]
        fn cancel_settings_pressed(&self, _sender: Option<&AnyObject>) {
            self.cancel_settings();
        }

        #[unsafe(method(applySettingsPressed:))]
        fn apply_settings_pressed(&self, _sender: Option<&AnyObject>) {
            self.save_settings();
        }
    }

    unsafe impl NSWindowDelegate for AppDelegate {
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            self.ivars()
                .hotkey_capture_controller
                .set_settings_window_visible(true);
        }

        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            self.disable_settings_window_hotkey_blocking();
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            self.settings_window_closed();
        }
    }

    unsafe impl NSTextViewDelegate for AppDelegate {}
    unsafe impl NSTextDelegate for AppDelegate {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, _notification: &NSNotification) {
            if let Some(overlay_window) = self.ivars().overlay_window.get() {
                overlay_window.apply_text_edit();
            }
        }
    }
);

impl AppDelegate {
    pub fn new(
        mtm: MainThreadMarker,
        overlay_style: OverlayStyle,
        config_store: LiveConfigStore,
        startup_hotkey_permissions: GlobalHotkeyPermissions,
        initial_audio_error: Option<String>,
        hotkey_capture_controller: HotkeyCaptureController,
        transformation_models_controller: TransformationModelsController,
        deepgram_connection_controller: DeepgramConnectionController,
        audio_controller: AudioController,
        state: Arc<AppState>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            app_updater: OnceCell::new(),
            audio_controller,
            initial_audio_error,
            config_store,
            deepgram_connection_controller,
            hotkey_capture_controller,
            state,
            overlay_style,
            active_status_bar_icon: make_status_bar_active_icon(mtm),
            idle_status_bar_icon: make_status_bar_icon(mtm),
            overlay_window: OnceCell::new(),
            permissions_dialog: OnceCell::new(),
            startup_hotkey_permissions,
            accessibility_permission_requested: Cell::new(false),
            input_monitoring_permission_requested: Cell::new(false),
            microphone_permission_requested: Cell::new(false),
            settings_window: OnceCell::new(),
            settings_presentation: Cell::new(SettingsPresentation::default()),
            permissions_dialog_returns_in_front_of_settings: Cell::new(false),
            transformation_models_controller,
            status_item: OnceCell::new(),
            status_poll: RefCell::new(StatusPollState::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn current_hotkey_permission_flow(&self) -> permissions::GlobalHotkeyPermissionFlow {
        permissions::resolve_global_hotkey_permission_flow(
            self.ivars().startup_hotkey_permissions,
            self.ivars().accessibility_permission_requested.get(),
            self.ivars().input_monitoring_permission_requested.get(),
            self.ivars().microphone_permission_requested.get(),
        )
    }

    fn sync_hotkey_permissions_ui(&self) {
        self.ensure_input_monitoring_access_requested();

        let Some(permissions_dialog) = self.ivars().permissions_dialog.get() else {
            return;
        };

        permissions_dialog.sync(&self.current_hotkey_permission_flow());
    }

    fn ensure_input_monitoring_access_requested(&self) {
        let flow = self.current_hotkey_permission_flow();
        if flow.permissions.input_monitoring_granted
            || self.ivars().input_monitoring_permission_requested.get()
        {
            return;
        }

        self.ivars().input_monitoring_permission_requested.set(true);
        if let Err(error) = permissions::request_input_monitoring_access() {
            log::error!(
                "failed to request background Input Monitoring access: {}",
                error
            );
        }
    }

    fn activate_audio_if_ready(&self) {
        let flow = self.current_hotkey_permission_flow();
        if flow.relaunch_required()
            || !flow.permissions.hotkey_permissions_granted()
            || !flow.permissions.microphone_granted
        {
            return;
        }

        if let Err(error) = self.ivars().audio_controller.ensure_input_stream_ready() {
            log::error!(
                "failed to activate audio input after microphone grant: {}",
                error
            );
            self.ivars().state.report_error(error.to_string());
            if let Some(settings_window) = self.ivars().settings_window.get() {
                settings_window.set_status(&error);
            }
            show_modal_alert(
                "simple-ptt couldn't start audio input",
                &format!(
                    concat!(
                        "Audio capture failed to initialize after microphone permission grant.\n\n",
                        "Please check your audio device settings.\n\n",
                        "Error: {}"
                    ),
                    error
                ),
            );
        }
    }

    fn open_github_repo_url(&self) {
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

    fn promote_for_window_presentation(&self) {
        let app = NSApplication::sharedApplication(MainThreadMarker::from(self));
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.activate();
    }

    fn restore_accessory_activation_policy_if_possible(&self) {
        if self.settings_window_is_visible()
            || self.ivars().settings_presentation.get().is_pending()
            || self.permissions_dialog_is_visible()
        {
            return;
        }

        let app = NSApplication::sharedApplication(MainThreadMarker::from(self));
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }

    fn settings_window_is_visible(&self) -> bool {
        self.ivars()
            .settings_window
            .get()
            .map(SettingsWindow::is_visible)
            .unwrap_or(false)
    }

    fn permissions_dialog_is_visible(&self) -> bool {
        self.ivars()
            .permissions_dialog
            .get()
            .map(PermissionsDialog::is_visible)
            .unwrap_or(false)
    }

    fn present_hotkey_permissions_window(&self) {
        let Some(permissions_dialog) = self.ivars().permissions_dialog.get() else {
            return;
        };

        self.promote_for_window_presentation();
        permissions_dialog.show(MainThreadMarker::from(self));
    }

    fn present_startup_hotkey_permissions_window(&self) {
        let Some(permissions_dialog) = self.ivars().permissions_dialog.get() else {
            return;
        };

        self.promote_for_window_presentation();
        permissions_dialog.show_startup(MainThreadMarker::from(self));
    }

    fn deactivate_for_system_settings_transition(&self) {
        let app = NSApplication::sharedApplication(MainThreadMarker::from(self));
        app.deactivate();
    }

    fn open_system_settings_and_activate(&self, urls: [&str; 2]) -> Result<(), String> {
        let workspace = NSWorkspace::sharedWorkspace();
        self.open_system_settings_url_with_workspace(&workspace, urls[0])
            .or_else(|primary_error| {
                self.open_system_settings_url_with_workspace(&workspace, urls[1])
                    .map_err(|fallback_error| {
                        format!(
                            "primary URL failed: {}; fallback URL failed: {}",
                            primary_error, fallback_error
                        )
                    })
            })
    }

    fn open_system_settings_url_with_workspace(
        &self,
        workspace: &NSWorkspace,
        url: &str,
    ) -> Result<(), String> {
        let url_string = NSString::from_str(url);
        let Some(ns_url) = NSURL::URLWithString(&url_string) else {
            return Err(format!("invalid System Settings URL: {}", url));
        };

        let configuration = NSWorkspaceOpenConfiguration::configuration();
        configuration.setActivates(true);
        configuration.setHides(false);
        configuration.setHidesOthers(false);
        configuration.setPromptsUserIfNeeded(true);

        let launch_result = std::rc::Rc::new(std::cell::RefCell::new(None::<Result<(), String>>));
        let completion_result = launch_result.clone();
        let completion = StackBlock::new(
            move |running_app: *mut objc2_app_kit::NSRunningApplication,
                  error: *mut objc2_foundation::NSError| {
                if !error.is_null() {
                    let error = unsafe { &*error };
                    completion_result
                        .borrow_mut()
                        .replace(Err(error.localizedDescription().to_string()));
                    return;
                }

                if running_app.is_null() {
                    completion_result.borrow_mut().replace(Err(
                        "System Settings launch returned no application instance".to_owned(),
                    ));
                    return;
                }

                let running_app = unsafe { &*running_app };
                running_app.unhide();
                let _ = running_app
                    .activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
                completion_result.borrow_mut().replace(Ok(()));
            },
        );

        workspace.openURL_configuration_completionHandler(
            &ns_url,
            &configuration,
            Some(&completion),
        );

        let result = launch_result.borrow_mut().take().unwrap_or_else(|| {
            Err("System Settings launch did not report completion synchronously".to_owned())
        });
        result
    }

    /// Loads the config file into Settings and presents the window. The
    /// status area shows `status_message`, any load problem, and the
    /// transformation model list status.
    fn present_settings_window(&self, status_message: Option<&str>) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        self.ivars().hotkey_capture_controller.cancel();
        settings_window.cancel_hotkey_capture();

        let current_file_config = self.ivars().config_store.current_file();
        let load_problems = settings_window.load_from_config(&current_file_config);
        let transformation_status =
            self.sync_transformation_provider_controls(ModelListTrigger::Resync);
        settings_window.set_status(&settings_load_status(
            status_message,
            &load_problems,
            transformation_status.as_deref(),
        ));

        if self
            .ivars()
            .settings_presentation
            .get()
            .is_awaiting_activation()
        {
            log::warn!(
                "macOS has not activated simple-ptt since Settings was last requested; requesting activation again"
            );
        }
        // A regular app appears in the Dock, so the user can also activate it
        // there if macOS declines the activation request.
        NSApplication::sharedApplication(MainThreadMarker::from(self))
            .setActivationPolicy(NSApplicationActivationPolicy::Regular);
        let step = self.advance_settings_presentation(SettingsPresentation::request);
        self.perform_settings_presentation_step(step);
    }

    /// Every way of leaving Settings (close button, Cancel, Save) ends here.
    fn settings_window_closed(&self) {
        self.disable_settings_window_hotkey_blocking();
        self.cancel_pending_settings_presentation();
        self.restore_accessory_activation_policy_if_possible();
    }

    /// Called when Settings closes, so that a request still waiting for
    /// activation does not reopen it on the next activation.
    fn cancel_pending_settings_presentation(&self) {
        let mut presentation = self.ivars().settings_presentation.get();
        presentation.cancel();
        self.ivars().settings_presentation.set(presentation);
        self.ivars()
            .permissions_dialog_returns_in_front_of_settings
            .set(false);
        // SAFETY: `self` is a live `AppDelegate`, the selector is the valid
        // selector it implements, and nil is the argument that
        // `perform_settings_presentation_step` scheduled the turn with, so this
        // matches and drops that turn.
        unsafe {
            NSObject::cancelPreviousPerformRequestsWithTarget_selector_object(
                self,
                settings_presentation_turn_selector(),
                None,
            );
        }
    }

    fn advance_settings_presentation(
        &self,
        transition: impl FnOnce(&mut SettingsPresentation) -> SettingsPresentationStep,
    ) -> SettingsPresentationStep {
        let mut presentation = self.ivars().settings_presentation.get();
        let step = transition(&mut presentation);
        self.ivars().settings_presentation.set(presentation);
        step
    }

    /// Carries out a step decided by `SettingsPresentation`. The new state is
    /// stored before this runs, so `applicationDidBecomeActive:` sees
    /// `AwaitingActivation` even if AppKit delivers it before the activation
    /// request returns.
    fn perform_settings_presentation_step(&self, step: SettingsPresentationStep) {
        match step {
            SettingsPresentationStep::Wait => {}
            SettingsPresentationStep::ScheduleDefaultModeTurn => {
                // `performSelector:withObject:afterDelay:` runs the selector only
                // when the run loop is in the default mode. Menu tracking runs it
                // in the event tracking mode, so the turn comes after the status
                // item menu has been dismissed.
                // SAFETY: `AppDelegate` implements
                // `settings_presentation_turn_selector()`, which takes no
                // argument, so the nil argument is not passed to it. `main`
                // holds the delegate for the whole `NSApplication::run`.
                unsafe {
                    self.performSelector_withObject_afterDelay(
                        settings_presentation_turn_selector(),
                        None,
                        0.0,
                    );
                }
            }
            SettingsPresentationStep::RequestActivation => {
                log::info!(
                    "requested app activation; Settings opens once macOS activates simple-ptt"
                );
                // Request process activation, even if the nonactivating overlay
                // has already made NSApplication report itself as active.
                objc2_app_kit::NSRunningApplication::currentApplication()
                    .activateWithOptions(NSApplicationActivationOptions::empty());
            }
            SettingsPresentationStep::Present => {
                let Some(settings_window) = self.ivars().settings_window.get() else {
                    return;
                };
                self.ivars().state.set_settings_window_visible(true);
                settings_window.show();
                if self
                    .ivars()
                    .permissions_dialog_returns_in_front_of_settings
                    .replace(false)
                    && self.permissions_dialog_is_visible()
                {
                    self.present_startup_hotkey_permissions_window();
                }
            }
        }
    }

    fn disable_settings_window_hotkey_blocking(&self) {
        self.ivars().hotkey_capture_controller.cancel();
        self.ivars()
            .hotkey_capture_controller
            .set_settings_window_visible(false);
        self.ivars().state.set_settings_window_visible(false);
        self.ivars().state.set_preview_mic_gain(None);
        self.ivars().audio_controller.clear_preview_audio_device();
        self.ivars().audio_controller.apply_pending_if_idle();

        if let Some(settings_window) = self.ivars().settings_window.get() {
            settings_window.cancel_hotkey_capture();
        }
    }

    fn current_transformation_provider_request(
        &self,
    ) -> Result<TransformationProviderRequest, String> {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return Err("settings window is not available".to_owned());
        };

        let provider = settings_window
            .transformation_provider_value()
            .ok_or_else(|| "Choose a transformation provider first.".to_owned())?;
        let resolved_api_key = config::resolve_transformation_api_key_for_provider(
            Some(provider.as_str()),
            settings_window.transformation_api_key_value().as_deref(),
        );

        Ok(TransformationProviderRequest::new(
            provider,
            resolved_api_key,
            settings_window.transformation_model_value(),
        ))
    }

    fn current_deepgram_check_request(&self) -> Result<DeepgramCheckRequest, String> {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return Err("settings window is not available".to_owned());
        };

        let mut effective_config = self.ivars().config_store.current_file();
        effective_config.deepgram.api_key = settings_window.deepgram_api_key_value();

        Ok(DeepgramCheckRequest::new(effective_config.resolve_deepgram_api_key()?))
    }

    fn sync_transformation_provider_ui(&self, trigger: ModelListTrigger) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };
        if let Some(message) = self.sync_transformation_provider_controls(trigger) {
            settings_window.set_status(&message);
        }
    }

    /// Syncs the transformation controls with the selected provider and
    /// returns the status message for the result, or `None` to leave the
    /// status unchanged. After a provider change, a provider with no cached
    /// models has them fetched (see `plan_model_list`).
    #[must_use = "the transformation status must be shown in the status area"]
    fn sync_transformation_provider_controls(&self, trigger: ModelListTrigger) -> Option<String> {
        let settings_window = self.ivars().settings_window.get()?;

        settings_window.sync_transformation_api_key_env_hint();
        let provider_selected = settings_window.transformation_provider_value().is_some();
        settings_window.set_transformation_model_controls_enabled(provider_selected);
        if !provider_selected {
            settings_window.populate_transformation_model_values(&[]);
            return Some(String::new());
        }

        let request = self.current_transformation_provider_request().ok()?;
        let saved = self.ivars().config_store.current_file().transformation;
        let api_key_belongs = api_key_belongs_to_provider(
            &request,
            settings_window.transformation_api_key_value().as_deref(),
            saved.provider.as_deref(),
            saved.api_key.as_deref(),
        );
        let plan = self
            .ivars()
            .transformation_models_controller
            .plan_model_list(trigger, &request, api_key_belongs);
        // Without cached models the list is emptied, so it does not keep
        // offering the previous provider's models.
        let (models, message) = match plan {
            ModelListPlan::UseCached { models, message } => (models, message),
            ModelListPlan::StartFetch => (
                Vec::new(),
                self.begin_transformation_model_action(TransformationModelAction::Refresh, request),
            ),
            ModelListPlan::AwaitFetch(action) => {
                (Vec::new(), action.progress_message(&request.provider))
            }
            ModelListPlan::NeedsManualFetch(reason) => (
                Vec::new(),
                manual_model_fetch_message(&request.provider, reason),
            ),
            ModelListPlan::CacheUnreadable(error) => {
                (Vec::new(), unreadable_model_cache_message(&error))
            }
            ModelListPlan::ShowError(message) => (Vec::new(), message),
        };
        settings_window.populate_transformation_model_values(&models);
        Some(message)
    }

    fn start_transformation_model_action(&self, action: TransformationModelAction) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        let status_message = match self.current_transformation_provider_request() {
            Ok(request) => self.begin_transformation_model_action(action, request),
            Err(error) => error,
        };
        settings_window.set_status(&status_message);
    }

    /// Starts `action` for `request` and returns the status message to show
    /// while it runs.
    #[must_use = "the action's progress must be shown in the status area"]
    fn begin_transformation_model_action(
        &self,
        action: TransformationModelAction,
        request: TransformationProviderRequest,
    ) -> String {
        if let (TransformationModelAction::Check, Some(settings_window)) =
            (action, self.ivars().settings_window.get())
        {
            settings_window.reset_transformation_check_button();
        }
        let status_message = action.progress_message(&request.provider);
        self.ivars()
            .transformation_models_controller
            .start_action(action, request);
        status_message
    }

    fn start_deepgram_connection_check(&self) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        let request = match self.current_deepgram_check_request() {
            Ok(request) => request,
            Err(error) => {
                settings_window.set_status(&error);
                return;
            }
        };

        settings_window.reset_deepgram_check_button();
        settings_window.set_status("Checking Deepgram connection…");
        self.ivars()
            .deepgram_connection_controller
            .start_check(request);
    }

    fn handle_pending_transformation_model_updates(&self) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        while let Some(update) = self.ivars().transformation_models_controller.take_update() {
            let current_request = self.current_transformation_provider_request().ok();
            if !update.applies_to(current_request.as_ref()) {
                continue;
            }

            if let Some(success) = update.check_result() {
                settings_window.set_transformation_check_result(success);
            }
            match update {
                TransformationModelUpdate::ModelsRefreshed {
                    models, message, ..
                }
                | TransformationModelUpdate::ConnectionChecked {
                    models, message, ..
                } => {
                    settings_window.populate_transformation_model_values(&models);
                    settings_window.set_status(&message);
                }
                TransformationModelUpdate::ActionFailed { message, .. } => {
                    settings_window.set_status(&message);
                }
            }
        }
    }

    fn handle_pending_deepgram_check_updates(&self) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        while let Some(update) = self.ivars().deepgram_connection_controller.take_update() {
            let current_request = self.current_deepgram_check_request().ok();
            let update_request = match &update {
                DeepgramCheckUpdate::ConnectionChecked { request, .. }
                | DeepgramCheckUpdate::ActionFailed { request, .. } => request,
            };
            if current_request
                .as_ref()
                .map(|request| request.same_source_as(update_request))
                != Some(true)
            {
                continue;
            }

            match update {
                DeepgramCheckUpdate::ConnectionChecked { message, .. } => {
                    settings_window.set_status(&message);
                    settings_window.set_deepgram_check_result(true);
                }
                DeepgramCheckUpdate::ActionFailed { message, .. } => {
                    settings_window.set_status(&message);
                    settings_window.set_deepgram_check_result(false);
                }
            }
        }
    }

    fn reset_prompt_to_default(&self, prompt: Prompt) {
        if let Some(settings_window) = self.ivars().settings_window.get() {
            settings_window.reset_prompt_to_default(prompt);
        }
    }

    fn cancel_settings(&self) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        let previous_file_config = self.ivars().config_store.current_file();
        let load_problems = settings_window.load_from_config(&previous_file_config);
        settings_window.set_status(&settings_load_status(None, &load_problems, None));
        settings_window.hide();
        self.settings_window_closed();
    }

    fn save_settings(&self) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        self.ivars().hotkey_capture_controller.cancel();
        settings_window.cancel_hotkey_capture();

        let (proposed_config, prompt_resets) = match settings_window.read_config() {
            Ok(read) => read,
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
        let config_file_snapshot =
            match config::ConfigFileSnapshot::take(self.ivars().config_store.path()) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    settings_window.set_status(&error);
                    show_modal_alert("Couldn't save settings", &error);
                    return;
                }
            };

        if let Err(error) = config::save_config(
            self.ivars().config_store.path(),
            &proposed_config,
            prompt_resets,
        ) {
            settings_window.set_status(&error);
            show_modal_alert("Couldn't save settings", &error);
            return;
        }

        crate::auto_launch::apply_auto_launch_config(runtime_config.ui.start_on_login);

        if let Some(Some(ref updater)) = self.ivars().app_updater.get() {
            updater.set_automatically_checks_for_updates(runtime_config.ui.auto_check_updates);
        }

        self.ivars().audio_controller.clear_preview_audio_device();

        let audio_apply_effect = match self.ivars().audio_controller.apply_mic_config(&runtime_config.mic) {
            Ok(effect) => effect,
            Err(error) => {
                let error = match config_file_snapshot.restore() {
                    Ok(()) => error,
                    Err(restore_error) => format!(
                        "{}; also failed to restore the previous config file, so it keeps the settings that failed to apply: {}",
                        error, restore_error
                    ),
                };
                settings_window.set_status(&error);
                show_modal_alert("Couldn't apply audio settings", &error);
                return;
            }
        };

        let audio_message = match audio_apply_effect {
            AudioConfigApplyEffect::AppliedNow => "audio changes applied now",
            AudioConfigApplyEffect::DeferredUntilRecordingStops => {
                "audio device/sample-rate changes will apply after the current recording stops"
            }
        };
        log::info!("settings saved and applied ({})", audio_message);

        self.ivars()
            .config_store
            .replace(proposed_config.clone(), runtime_config.clone());
        if let Some(overlay_window) = self.ivars().overlay_window.get() {
            overlay_window.apply_style(&overlay_style_from_config(&runtime_config));
        }

        self.sync_transformation_provider_ui(ModelListTrigger::Resync);
        settings_window.hide();
        self.settings_window_closed();
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

                let conflicting_target = [
                    HotkeyCaptureTarget::Record,
                    HotkeyCaptureTarget::Correction,
                    HotkeyCaptureTarget::Transform,
                ]
                .into_iter()
                .filter(|candidate| *candidate != target)
                .find(|candidate| settings_window.hotkey_value(*candidate) == captured_name);
                if conflicting_target.is_some() {
                    settings_window.cancel_hotkey_capture();
                    settings_window.set_status(
                        "Record, correction, and transform triggers must be different.",
                    );
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

    fn run_status_poll_tick(&self, mtm: MainThreadMarker) {
        let ivars = self.ivars();
        ivars.audio_controller.sync_stream_state();

        let settings_requested = ivars.state.take_settings_request();
        let snapshot = UiSnapshot::capture(&ivars.state);
        let controller_updates_pending = settings_requested
            || ivars.hotkey_capture_controller.has_pending_ui_update()
            || ivars
                .transformation_models_controller
                .has_pending_ui_update()
            || ivars.deepgram_connection_controller.has_pending_ui_update();
        // The poll state borrow ends with this statement, before `update_ui` runs.
        let outcome = ivars
            .status_poll
            .borrow_mut()
            .advance(&snapshot, controller_updates_pending);
        let StatusPollOutcome::Refresh { ui_changed } = outcome else {
            return;
        };

        if ui_changed {
            let label = match snapshot.state {
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
                snapshot.overlay_text.text.len()
            );
        }

        self.update_ui(
            mtm,
            snapshot.state,
            snapshot.overlay_dismissed,
            &snapshot.overlay_text,
            &snapshot.overlay_error_text,
            &snapshot.overlay_correction_text,
            snapshot.overlay_correction_active,
            snapshot.overlay_text_opacity,
            snapshot.mic_meter,
            snapshot.capturing_audio,
        );
        if settings_requested {
            self.ivars()
                .permissions_dialog_returns_in_front_of_settings
                .set(false);
            self.present_settings_window(None);
        }
    }

    pub fn update_ui(
        &self,
        mtm: MainThreadMarker,
        state: u8,
        overlay_dismissed: bool,
        overlay_text: &OverlayText,
        overlay_error_text: &str,
        overlay_correction_text: &OverlayText,
        overlay_correction_active: bool,
        overlay_text_opacity: f64,
        mic_meter: MicMeterSnapshot,
        capturing_audio: bool,
    ) {
        self.handle_pending_hotkey_capture_preview();
        self.handle_pending_hotkey_capture();
        self.handle_pending_transformation_model_updates();
        self.handle_pending_deepgram_check_updates();
        self.ivars().audio_controller.apply_pending_if_idle();
        update_status_item(self, mtm, state);

        if self.ivars().state.is_settings_window_visible() {
            if let Some(settings_window) = self.ivars().settings_window.get() {
                settings_window.update_meter(Some(mic_meter));
            }
        }

        update_overlay_window(
            self,
            mtm,
            state,
            overlay_dismissed,
            overlay_text,
            overlay_error_text,
            overlay_correction_text,
            overlay_correction_active,
            overlay_text_opacity,
            mic_meter,
            capturing_audio,
        );
    }
}

fn config_file_is_missing(path: &Path) -> bool {
    matches!(
        std::fs::metadata(path),
        Err(error) if error.kind() == ErrorKind::NotFound
    )
}

// The alert texts name the settings Save button through `SAVE_BUTTON_TITLE`.
// `concat!` accepts only literals, and `format!` cannot implicitly capture
// `{SAVE_BUTTON_TITLE}` from a format string expanded from a macro, so the
// title is passed as an explicit named argument.
fn missing_config_alert_text(config_path: &Path) -> String {
    format!(
        concat!(
            "Settings opens so you can create one on first launch.\n",
            "Review the defaults, then click {save_button} to write:\n\n",
            "{config_path}\n\n",
            "simple-ptt is a menu bar app, so a successful launch appears in the menu bar rather than the Dock."
        ),
        save_button = SAVE_BUTTON_TITLE,
        config_path = config_path.display(),
    )
}

fn audio_startup_failure_alert_text(audio_error: &str) -> String {
    format!(
        concat!(
            "The app launched so you can fix the microphone settings, but audio capture is currently unavailable.\n\n",
            "Open Settings, choose a valid input device, and click {save_button}.\n\n",
            "Error: {audio_error}"
        ),
        save_button = SAVE_BUTTON_TITLE,
        audio_error = audio_error,
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
        Some(_) | None => 10.0,
    };
    let transformation_hotkey = config
        .resolve_transformation_config()
        .ok()
        .map(|_| config.transformation.hotkey.as_str());
    let correction_key_label = correction_key_hint_label(config.ui.correction_key.as_str());
    let shortcut_hint = Some(match transformation_hotkey {
        Some(hotkey) => format!(
            "<Hold {}> correction <{}> transform <{}> paste <Cmd+V> insert <ESC> cancel",
            correction_key_label, hotkey, config.ui.hotkey
        ),
        None => format!(
            "<Hold {}> correction <{}> paste <Cmd+V> insert <ESC> cancel",
            correction_key_label, config.ui.hotkey
        ),
    });

    OverlayStyle {
        font_name: config.ui.font_name.clone(),
        font_size: overlay_font_size,
        footer_font_size: overlay_footer_font_size,
        meter_style: config.ui.meter_style,
        shortcut_hint,
    }
}

fn validate_settings_config(config: &Config) -> Result<(), String> {
    let record_hotkey = parse_hotkey_binding(config.ui.hotkey.as_str())
        .map_err(|error| format!("record hotkey is invalid: {}", error))?;
    let correction_key = parse_correction_key(config.ui.correction_key.as_str())?;

    if hotkey_uses_key(record_hotkey, correction_key) {
        return Err("record hotkey and correction key must be different".to_owned());
    }

    if !config.ui.font_size.is_finite() || config.ui.font_size <= 0.0 {
        return Err("Font size must be a positive number".to_owned());
    }

    if let Some(footer_font_size) = config.ui.footer_font_size {
        if !footer_font_size.is_finite() || footer_font_size <= 0.0 {
            return Err("Footer font size must be a positive number".to_owned());
        }
    }

    config::validate_mic_gain(config.mic.gain)?;

    validate_mic_config(&config.mic)?;

    config.resolve_deepgram_api_key()?;

    if let Some(provider) = config.transformation.provider.as_deref().map(str::trim) {
        if !provider.is_empty() {
            let transform_hotkey = parse_hotkey_binding(config.transformation.hotkey.as_str())
                .map_err(|error| format!("transform hotkey is invalid: {}", error))?;
            if record_hotkey == transform_hotkey {
                return Err("record and transform hotkeys must be different".to_owned());
            }
            if hotkey_uses_key(transform_hotkey, correction_key) {
                return Err("transform hotkey and correction key must be different".to_owned());
            }
            config.resolve_transformation_config()?;
        }
    }

    Ok(())
}

fn parse_correction_key(raw: &str) -> Result<crate::key::Key, String> {
    parse_key(raw.trim()).ok_or_else(|| {
        "correction key must be a single supported key such as LeftMeta, RightMeta, LeftAlt, or F7"
            .to_owned()
    })
}

fn correction_key_hint_label(raw: &str) -> String {
    match raw.trim() {
        value
            if value.eq_ignore_ascii_case("LeftMeta")
                || value.eq_ignore_ascii_case("RightMeta") =>
        {
            "Cmd".to_owned()
        }
        value
            if value.eq_ignore_ascii_case("LeftAlt") || value.eq_ignore_ascii_case("RightAlt") =>
        {
            "Alt".to_owned()
        }
        value
            if value.eq_ignore_ascii_case("LeftControl")
                || value.eq_ignore_ascii_case("RightControl") =>
        {
            "Ctrl".to_owned()
        }
        value
            if value.eq_ignore_ascii_case("LeftShift")
                || value.eq_ignore_ascii_case("RightShift") =>
        {
            "Shift".to_owned()
        }
        value if value.eq_ignore_ascii_case("Escape") => "Esc".to_owned(),
        value => value.to_owned(),
    }
}

fn hotkey_uses_key(binding: crate::hotkey_binding::HotkeyBinding, key: crate::key::Key) -> bool {
    if binding.key == key {
        return true;
    }

    match key {
        crate::key::Key::ShiftLeft | crate::key::Key::ShiftRight => binding.modifiers.shift,
        crate::key::Key::ControlLeft | crate::key::Key::ControlRight => binding.modifiers.control,
        crate::key::Key::AltLeft | crate::key::Key::AltRight => binding.modifiers.alt,
        crate::key::Key::MetaLeft | crate::key::Key::MetaRight => binding.modifiers.meta,
        _ => false,
    }
}

fn make_hidden_main_menu(delegate: &AppDelegate, mtm: MainThreadMarker) -> Retained<NSMenu> {
    let main_menu = NSMenu::new(mtm);

    let app_menu_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(APP_DISPLAY_NAME),
            None,
            ns_string!(""),
        )
    };
    let app_menu = NSMenu::new(mtm);
    let about_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(&format!("About {}", APP_DISPLAY_NAME)),
            Some(sel!(openGitHubRepo:)),
            ns_string!(""),
        )
    };
    app_menu.addItem(&about_item);
    let app_separator_item = NSMenuItem::separatorItem(mtm);
    app_menu.addItem(&app_separator_item);
    let hide_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Hide"),
            Some(sel!(hide:)),
            ns_string!("h"),
        )
    };
    app_menu.addItem(&hide_item);
    let hide_others_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Hide Others"),
            Some(sel!(hideOtherApplications:)),
            ns_string!("h"),
        )
    };
    app_menu.addItem(&hide_others_item);
    if let Some(hide_others_item) = app_menu.itemAtIndex(3) {
        hide_others_item.setKeyEquivalentModifierMask(
            objc2_app_kit::NSEventModifierFlags::Command
                | objc2_app_kit::NSEventModifierFlags::Option,
        );
    }
    let quit_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Quit"),
            Some(sel!(terminate:)),
            ns_string!("q"),
        )
    };
    app_menu.addItem(&quit_item);
    app_menu_item.setSubmenu(Some(&app_menu));
    main_menu.addItem(&app_menu_item);

    let edit_menu_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Edit"),
            None,
            ns_string!(""),
        )
    };
    let edit_menu = NSMenu::new(mtm);
    for (title, action, key) in [
        ("Undo", sel!(undo:), "z"),
        ("Redo", sel!(redo:), "Z"),
        ("Cut", sel!(cut:), "x"),
        ("Copy", sel!(copy:), "c"),
        ("Paste", sel!(paste:), "v"),
        ("Select All", sel!(selectAll:), "a"),
    ] {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        edit_menu.addItem(&item);
    }
    if let Some(redo_item) = edit_menu.itemAtIndex(1) {
        redo_item.setKeyEquivalentModifierMask(
            objc2_app_kit::NSEventModifierFlags::Command
                | objc2_app_kit::NSEventModifierFlags::Shift,
        );
    }
    edit_menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), 2);
    edit_menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), 6);
    edit_menu_item.setSubmenu(Some(&edit_menu));
    main_menu.addItem(&edit_menu_item);

    unsafe {
        about_item.setTarget(Some(delegate));
    }

    main_menu
}

fn deepgram_status_title(state: u8, waiting: bool) -> &'static str {
    if waiting && matches!(state, STATE_RECORDING | STATE_PROCESSING) {
        "Waiting…"
    } else {
        ""
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
            let title = deepgram_status_title(state, delegate.ivars().state.is_deepgram_waiting());
            if button.title().to_string() != title {
                button.setTitle(&NSString::from_str(title));
            }
        }
    }
}

fn update_overlay_window(
    delegate: &AppDelegate,
    mtm: MainThreadMarker,
    state: u8,
    overlay_dismissed: bool,
    overlay_text: &OverlayText,
    overlay_error_text: &str,
    overlay_correction_text: &OverlayText,
    overlay_correction_active: bool,
    overlay_text_opacity: f64,
    mic_meter: MicMeterSnapshot,
    capturing_audio: bool,
) {
    if let Some(overlay_window) = delegate.ivars().overlay_window.get() {
        overlay_window.update(
            mtm,
            state,
            overlay_dismissed,
            overlay_text,
            overlay_error_text,
            overlay_correction_text,
            overlay_correction_active,
            overlay_text_opacity,
            mic_meter,
            capturing_audio,
        );
    }
}

const STATUS_POLL_INTERVAL_SECONDS: f64 = 0.075;
// Refresh at least every ~1.5 seconds (20 ticks of 75 ms) so `update_ui` can
// recheck the current audio input device default if it changed.
const STATUS_POLL_BACKGROUND_REFRESH_TICKS: u64 = 20;

/// The `AppState` values the status poll compares between ticks.
#[derive(Clone)]
struct UiSnapshot {
    state: u8,
    deepgram_waiting: bool,
    mic_meter: MicMeterSnapshot,
    overlay_dismissed: bool,
    overlay_correction_active: bool,
    overlay_correction_text: OverlayText,
    overlay_text: OverlayText,
    overlay_error_text: Arc<str>,
    overlay_text_opacity: f64,
    /// `AppState::is_capturing_audio`: the meter runs while it holds.
    capturing_audio: bool,
}

impl UiSnapshot {
    fn initial() -> Self {
        Self {
            state: STATE_IDLE,
            deepgram_waiting: false,
            mic_meter: MicMeterSnapshot::default(),
            overlay_dismissed: false,
            overlay_correction_active: false,
            overlay_correction_text: OverlayText::default(),
            overlay_text: OverlayText::default(),
            overlay_error_text: Arc::from(""),
            overlay_text_opacity: 1.0,
            capturing_audio: false,
        }
    }

    fn capture(state: &AppState) -> Self {
        Self {
            state: state.get_state(),
            deepgram_waiting: state.is_deepgram_waiting(),
            mic_meter: state.mic_meter_snapshot(),
            overlay_dismissed: state.is_overlay_dismissed(),
            overlay_correction_active: state.is_overlay_correction_active(),
            overlay_correction_text: state.overlay_correction_text_snapshot(),
            overlay_text: state.overlay_text_snapshot(),
            overlay_error_text: state.overlay_error_text(),
            overlay_text_opacity: state.overlay_text_opacity(),
            capturing_audio: state.is_capturing_audio(),
        }
    }

    /// Text fields compare by `Arc` identity: `AppState` hands out a new `Arc`
    /// whenever the text is replaced.
    fn ui_differs_from(&self, other: &Self) -> bool {
        self.state != other.state
            || self.deepgram_waiting != other.deepgram_waiting
            || self.overlay_dismissed != other.overlay_dismissed
            || self.overlay_correction_active != other.overlay_correction_active
            || !Arc::ptr_eq(
                &self.overlay_correction_text.text,
                &other.overlay_correction_text.text,
            )
            || !Arc::ptr_eq(&self.overlay_text.text, &other.overlay_text.text)
            || !Arc::ptr_eq(&self.overlay_error_text, &other.overlay_error_text)
            || (self.overlay_text_opacity - other.overlay_text_opacity).abs() > f64::EPSILON
            || self.capturing_audio != other.capturing_audio
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusPollOutcome {
    Skip,
    Refresh { ui_changed: bool },
}

/// Per-tick bookkeeping for the status poll, owned by `AppDelegate`.
struct StatusPollState {
    last: UiSnapshot,
    frame_count: u64,
}

impl StatusPollState {
    fn new() -> Self {
        Self {
            last: UiSnapshot::initial(),
            frame_count: 0,
        }
    }

    /// Records `current` as the latest snapshot and decides whether this tick
    /// must refresh the UI.
    fn advance(
        &mut self,
        current: &UiSnapshot,
        controller_updates_pending: bool,
    ) -> StatusPollOutcome {
        self.frame_count += 1;
        let ui_changed = current.ui_differs_from(&self.last);
        let mic_meter_changed = current.mic_meter != self.last.mic_meter;
        let should_animate_meter = current.capturing_audio;
        let should_animate_overlay = matches!(current.state, STATE_PROCESSING | STATE_TRANSFORMING)
            && !current.overlay_dismissed;
        let background_refresh_due = self.frame_count % STATUS_POLL_BACKGROUND_REFRESH_TICKS == 0;

        if !ui_changed
            && !mic_meter_changed
            && !should_animate_meter
            && !should_animate_overlay
            && !controller_updates_pending
            && !background_refresh_due
        {
            return StatusPollOutcome::Skip;
        }

        self.last = current.clone();
        StatusPollOutcome::Refresh { ui_changed }
    }
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
    use objc2::{sel, ClassType};

    use super::{
        audio_startup_failure_alert_text, config_file_is_missing,
        deepgram_status_title, missing_config_alert_text, overlay_style_from_config,
        settings_presentation_turn_selector, status_poll_selector, validate_settings_config,
        AppDelegate, StatusPollOutcome, StatusPollState, UiSnapshot,
        STATUS_POLL_BACKGROUND_REFRESH_TICKS,
    };
    use crate::config::Config;
    use crate::settings_window::actions::SettingsAction;
    use crate::settings_window::SAVE_BUTTON_TITLE;
    use crate::state::{
        AppState, MicMeterSnapshot, OverlayText, STATE_BUFFER_READY, STATE_ERROR, STATE_IDLE,
        STATE_PROCESSING, STATE_RECORDING, STATE_TRANSFORMING,
    };

    #[test]
    fn missing_config_alert_names_the_save_button_as_titled() {
        let text = missing_config_alert_text(std::path::Path::new("/tmp/config.toml"));

        assert!(
            text.contains(&format!("click {SAVE_BUTTON_TITLE}")),
            "{text}"
        );
        assert!(
            text.contains(&format!("click {SAVE_BUTTON_TITLE} to write:")),
            "{text}"
        );
        assert!(!text.contains("Save and Apply"), "{text}");
        assert!(text.contains("/tmp/config.toml"), "{text}");
        // The alert is modal, so it shows before the deferred Settings window.
        assert!(text.starts_with("Settings opens so"), "{text}");
    }

    #[test]
    fn audio_startup_failure_alert_names_the_save_button_as_titled() {
        let text = audio_startup_failure_alert_text("no input device");

        assert!(
            text.contains(&format!("click {SAVE_BUTTON_TITLE}")),
            "{text}"
        );
        assert!(
            text.contains(&format!("click {SAVE_BUTTON_TITLE}.")),
            "{text}"
        );
        assert!(!text.contains("Save and Apply"), "{text}");
        assert!(text.ends_with("Error: no input device"), "{text}");
    }

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

    #[test]
    fn overlay_style_includes_clipboard_shortcut_without_transformation() {
        let config = Config::default();

        let style = overlay_style_from_config(&config);

        assert_eq!(
            style.shortcut_hint.as_deref(),
            Some("<Hold Alt> correction <F5> paste <Cmd+V> insert <ESC> cancel")
        );
    }

    #[test]
    fn menu_bar_wait_notice_follows_transcription_progress() {
        let state = AppState::new();
        for phase in [STATE_RECORDING, STATE_PROCESSING] {
            state.set_state(phase);
            assert_eq!(deepgram_status_title(state.get_state(), state.is_deepgram_waiting()), "");
            state.set_deepgram_waiting(true);
            state.dismiss_overlay();
            assert_eq!(deepgram_status_title(state.get_state(), state.is_deepgram_waiting()), "Waiting…");
            state.set_deepgram_waiting(false);
            assert_eq!(deepgram_status_title(state.get_state(), state.is_deepgram_waiting()), "");
        }
        state.set_deepgram_waiting(true);
        for phase in [STATE_IDLE, STATE_ERROR, STATE_BUFFER_READY, STATE_TRANSFORMING] {
            state.set_state(phase);
            assert_eq!(deepgram_status_title(state.get_state(), state.is_deepgram_waiting()), "");
        }
    }

    #[test]
    fn overlay_style_includes_transform_shortcut_when_transformation_is_configured() {
        let mut config = Config::default();
        config.transformation.provider = Some("openai".to_owned());

        let style = overlay_style_from_config(&config);

        assert_eq!(
            style.shortcut_hint.as_deref(),
            Some("<Hold Alt> correction <F6> transform <F5> paste <Cmd+V> insert <ESC> cancel")
        );
    }

    #[test]
    fn overlay_style_uses_configured_correction_key_label() {
        let mut config = Config::default();
        config.ui.correction_key = "RightAlt".to_owned();

        let style = overlay_style_from_config(&config);

        assert_eq!(
            style.shortcut_hint.as_deref(),
            Some("<Hold Alt> correction <F5> paste <Cmd+V> insert <ESC> cancel")
        );
    }

    #[test]
    fn validate_settings_rejects_correction_key_that_overlaps_record_hotkey() {
        let mut config = Config::default();
        config.ui.hotkey = "Cmd+F5".to_owned();
        config.ui.correction_key = "LeftMeta".to_owned();

        assert_eq!(
            validate_settings_config(&config).unwrap_err(),
            "record hotkey and correction key must be different"
        );
    }

    #[test]
    fn validate_settings_rejects_mic_gain_outside_slider_range() {
        for gain in [-0.5, 10.5, f32::NAN, f32::INFINITY] {
            let mut config = Config::default();
            config.mic.gain = gain;

            assert_eq!(
                validate_settings_config(&config).unwrap_err(),
                "Gain must be between 0 and 10 dB"
            );
        }
    }

    #[test]
    fn app_delegate_implements_every_settings_window_action() {
        let delegate_class = AppDelegate::class();
        let missing: Vec<_> = SettingsAction::ALL
            .iter()
            .filter(|action| !delegate_class.responds_to(action.selector()))
            .map(|action| (*action, action.selector()))
            .collect();

        assert!(
            missing.is_empty(),
            "AppDelegate does not implement settings actions: {missing:?}"
        );
    }

    #[test]
    fn app_delegate_implements_status_poll_timer_selector() {
        assert!(
            AppDelegate::class().responds_to(status_poll_selector()),
            "AppDelegate does not implement the status poll timer selector {:?}",
            status_poll_selector()
        );
    }

    #[test]
    fn app_delegate_implements_settings_presentation_selectors() {
        let delegate_class = AppDelegate::class();

        assert!(
            delegate_class.responds_to(settings_presentation_turn_selector()),
            "AppDelegate does not implement the settings presentation turn selector {:?}",
            settings_presentation_turn_selector()
        );
        assert!(
            delegate_class.responds_to(sel!(applicationDidBecomeActive:)),
            "AppDelegate does not implement applicationDidBecomeActive:"
        );
    }

    fn idle_snapshot() -> UiSnapshot {
        UiSnapshot::initial()
    }

    #[test]
    fn a_snapshot_counts_a_resuming_dictation_as_capturing_audio() {
        let state = AppState::new();
        state.set_state(STATE_RECORDING);
        state.set_dictation_resuming(true);
        state.set_state(STATE_TRANSFORMING);

        assert!(UiSnapshot::capture(&state).capturing_audio);
    }

    #[test]
    fn a_deepgram_wait_refreshes_the_ui_when_it_starts_and_when_it_clears() {
        let state = AppState::new();
        let mut poll = StatusPollState::new();
        poll.advance(&UiSnapshot::capture(&state), false);
        state.set_deepgram_waiting(true);
        assert_eq!(poll.advance(&UiSnapshot::capture(&state), false), StatusPollOutcome::Refresh { ui_changed: true });
        state.set_deepgram_waiting(false);
        assert_eq!(poll.advance(&UiSnapshot::capture(&state), false), StatusPollOutcome::Refresh { ui_changed: true });
    }

    #[test]
    fn status_poll_refreshes_when_audio_capture_starts_or_stops() {
        let mut poll_state = StatusPollState::new();
        let mut snapshot = idle_snapshot();
        poll_state.advance(&snapshot, false);

        snapshot.capturing_audio = true;

        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Refresh { ui_changed: true }
        );
    }

    #[test]
    fn status_poll_refreshes_on_first_tick_because_text_identity_differs() {
        let mut poll_state = StatusPollState::new();

        assert_eq!(
            poll_state.advance(&idle_snapshot(), false),
            StatusPollOutcome::Refresh { ui_changed: true }
        );
    }

    #[test]
    fn status_poll_skips_unchanged_idle_ticks_until_background_refresh() {
        let mut poll_state = StatusPollState::new();
        let snapshot = idle_snapshot();
        poll_state.advance(&snapshot, false);

        for tick in 2..STATUS_POLL_BACKGROUND_REFRESH_TICKS {
            assert_eq!(
                poll_state.advance(&snapshot, false),
                StatusPollOutcome::Skip,
                "tick {tick} should not refresh the UI"
            );
        }
        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Refresh { ui_changed: false }
        );
    }

    #[test]
    fn status_poll_refreshes_once_when_ui_state_changes() {
        let mut poll_state = StatusPollState::new();
        let mut snapshot = idle_snapshot();
        poll_state.advance(&snapshot, false);
        snapshot.state = STATE_ERROR;

        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Refresh { ui_changed: true }
        );
        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Skip
        );
    }

    #[test]
    fn status_poll_treats_replaced_overlay_text_as_changed() {
        let mut poll_state = StatusPollState::new();
        let mut snapshot = idle_snapshot();
        poll_state.advance(&snapshot, false);
        snapshot.overlay_text = OverlayText::default();

        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Refresh { ui_changed: true }
        );
    }

    #[test]
    fn status_poll_refreshes_every_tick_while_recording() {
        let mut poll_state = StatusPollState::new();
        let mut snapshot = idle_snapshot();
        snapshot.state = STATE_RECORDING;
        snapshot.capturing_audio = true;

        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Refresh { ui_changed: true }
        );
        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Refresh { ui_changed: false }
        );
    }

    #[test]
    fn status_poll_animates_processing_overlay_only_while_visible() {
        let mut poll_state = StatusPollState::new();
        let mut snapshot = idle_snapshot();
        snapshot.state = STATE_PROCESSING;
        poll_state.advance(&snapshot, false);

        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Refresh { ui_changed: false }
        );

        snapshot.overlay_dismissed = true;
        poll_state.advance(&snapshot, false);
        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Skip
        );
    }

    #[test]
    fn status_poll_refreshes_for_mic_meter_changes_and_pending_controller_updates() {
        let mut poll_state = StatusPollState::new();
        let mut snapshot = idle_snapshot();
        poll_state.advance(&snapshot, false);
        snapshot.mic_meter = MicMeterSnapshot {
            level: 10,
            ..MicMeterSnapshot::default()
        };

        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Refresh { ui_changed: false }
        );
        assert_eq!(
            poll_state.advance(&snapshot, false),
            StatusPollOutcome::Skip
        );
        assert_eq!(
            poll_state.advance(&snapshot, true),
            StatusPollOutcome::Refresh { ui_changed: false }
        );
    }
}

fn status_poll_selector() -> objc2::runtime::Sel {
    sel!(pollStatus:)
}

fn settings_presentation_turn_selector() -> objc2::runtime::Sel {
    sel!(runSettingsPresentationTurn)
}

/// Schedules the repeating status poll on the main run loop.
///
/// Every tick runs on the main thread, so it reaches the main-thread-only
/// `AppDelegate` (and the `!Send` `AudioController` it owns) through a normal
/// `&AppDelegate`. The timer is added in `NSRunLoopCommonModes`, the modes in
/// which the main run loop also drains the main dispatch queue, so it keeps
/// firing while menus are tracked or modal alerts run.
pub fn setup_status_polling(delegate: &AppDelegate) {
    // SAFETY: `delegate` is an `AppDelegate`, which implements
    // `status_poll_selector()` taking the firing `NSTimer`; `userInfo` is nil.
    // The timer retains its target until invalidated and the run loop retains
    // the timer, so the delegate outlives every tick.
    let timer = unsafe {
        NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            STATUS_POLL_INTERVAL_SECONDS,
            delegate,
            status_poll_selector(),
            None,
            true,
        )
    };
    // SAFETY: `NSRunLoop` is not thread-safe; this runs on the main thread (a
    // `&AppDelegate` only exists there), which owns the main run loop.
    // `NSRunLoopCommonModes` is an immutable Foundation constant, and the timer
    // is freshly created and not yet scheduled on any run loop.
    unsafe {
        NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
    }
}
