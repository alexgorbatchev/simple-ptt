use std::cell::{Cell, OnceCell};
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
    NSStatusBar, NSStatusItem, NSWindowDelegate, NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSURL,
};

use crate::audio::{validate_mic_config, AudioConfigApplyEffect, AudioController};
use crate::billing::BillingController;
use crate::config::{self, Config};
use crate::deepgram_connection::{
    DeepgramCheckRequest, DeepgramCheckUpdate, DeepgramConnectionController,
};
use crate::hotkey_binding::{format_hotkey_binding, parse_hotkey_binding};
use crate::hotkey_capture::{
    capture_outcome_message, HotkeyCaptureController, HotkeyCaptureOutcome, HotkeyCapturePreview,
    HotkeyCaptureTarget,
};
use crate::icon::{make_application_icon, make_status_bar_active_icon, make_status_bar_icon};
use crate::overlay::{OverlayStyle, OverlayWindow};
use crate::permissions::{self, GlobalHotkeyPermissions};
use crate::permissions_dialog::PermissionsDialog;
use crate::settings::LiveConfigStore;
use crate::settings_window::SettingsWindow;
use crate::state::{
    AppState, MicMeterSnapshot, STATE_BUFFER_READY, STATE_ERROR, STATE_IDLE, STATE_PROCESSING,
    STATE_RECORDING, STATE_TRANSFORMING,
};
use crate::transformation_models::{
    TransformationModelAction, TransformationModelUpdate, TransformationModelsController,
    TransformationProviderRequest,
};

const APP_DISPLAY_NAME: &str = "simple-ptt";
const GITHUB_REPO_URL: &str = "https://github.com/alexgorbatchev/simple-ptt";
const NS_VARIABLE_STATUS_ITEM_LENGTH: f64 = -1.0;

pub struct Ivars {
    audio_controller: AudioController,
    initial_audio_error: Option<String>,
    billing_controller: BillingController,
    billing_menu_item: OnceCell<Retained<NSMenuItem>>,
    config_store: LiveConfigStore,
    deepgram_connection_controller: DeepgramConnectionController,
    hotkey_capture_controller: HotkeyCaptureController,
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
    transformation_models_controller: TransformationModelsController,
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

            let config_file_missing = config_file_is_missing(self.ivars().config_store.path());
            let deepgram_api_key_missing = self
                .ivars()
                .config_store
                .current()
                .resolve_deepgram_api_key()
                .is_err();
            let audio_startup_failed = self.ivars().initial_audio_error.is_some();
            let startup_permissions_missing = !self.ivars().startup_hotkey_permissions.all_granted();
            let startup_ui_required = config_file_missing
                || deepgram_api_key_missing
                || audio_startup_failed
                || startup_permissions_missing;

            if startup_ui_required {
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
                    ns_string!(",")
                )
            };
            unsafe {
                settings_item.setTarget(Some(self));
            }
            menu.addItem(&settings_item);

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
            let settings_window = SettingsWindow::new(self, mtm);
            settings_window.set_delegate(ProtocolObject::from_ref(self));
            self.ivars()
                .settings_window
                .set(settings_window)
                .expect("settings window must only be set once");
            self.ivars()
                .permissions_dialog
                .set(PermissionsDialog::new(self, mtm))
                .expect("permissions dialog must only be set once");

            if config_file_missing || deepgram_api_key_missing || audio_startup_failed {
                self.present_startup_settings_window();
            }

            if startup_permissions_missing {
                self.sync_hotkey_permissions_ui();
                self.present_startup_hotkey_permissions_window();
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

            if let Some(audio_error) = self.ivars().initial_audio_error.as_deref() {
                if let Some(settings_window) = self.ivars().settings_window.get() {
                    settings_window.set_status(audio_error);
                }
                show_modal_alert(
                    "simple-ptt couldn't start audio input",
                    &format!(
                        concat!(
                            "The app launched so you can fix the microphone settings, but audio capture is currently unavailable.\n\n",
                            "Open Settings, choose a valid input device, and click Save and Apply.\n\n",
                            "Error: {}"
                        ),
                        audio_error
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
            let flow = self.current_hotkey_permission_flow();
            let result = if matches!(
                flow.microphone_state,
                permissions::GlobalHotkeyPermissionState::Requested
            ) {
                self.open_system_settings_and_activate(permissions::microphone_settings_urls())
            } else {
                self.ivars().microphone_permission_requested.set(true);
                permissions::request_microphone_access()
            };

            if let Err(error) = result {
                log::error!("failed to request microphone access: {}", error);
            }
            self.sync_hotkey_permissions_ui();
        }

        #[unsafe(method(resetHotkeyPermissions:))]
        fn reset_hotkey_permissions(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = permissions::reset_application_permissions() {
                log::error!("failed to reset macOS permissions: {}", error);
            }

            self.ivars().accessibility_permission_requested.set(false);
            self.ivars().input_monitoring_permission_requested.set(false);
            self.ivars().microphone_permission_requested.set(false);
            self.sync_hotkey_permissions_ui();
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

        #[unsafe(method(transformationProviderChanged:))]
        fn transformation_provider_changed(&self, _sender: Option<&AnyObject>) {
            self.sync_transformation_provider_ui();
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

        #[unsafe(method(cancelSettings:))]
        fn cancel_settings(&self, _sender: Option<&AnyObject>) {
            let Some(settings_window) = self.ivars().settings_window.get() else {
                return;
            };

            self.disable_settings_window_hotkey_blocking();
            settings_window.hide();
            self.restore_accessory_activation_policy_if_possible();
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
            self.sync_transformation_provider_ui();
            settings_window.set_status(&format!("Saved and applied settings. {}.", audio_message));
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
            self.disable_settings_window_hotkey_blocking();
            self.restore_accessory_activation_policy_if_possible();
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
        billing_controller: BillingController,
        audio_controller: AudioController,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            audio_controller,
            initial_audio_error,
            billing_controller,
            billing_menu_item: OnceCell::new(),
            config_store,
            deepgram_connection_controller,
            hotkey_capture_controller,
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
            transformation_models_controller,
            status_item: OnceCell::new(),
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
            if let Some(settings_window) = self.ivars().settings_window.get() {
                settings_window.set_status(&error);
            }
        }
    }

    fn promote_for_window_presentation(&self) {
        let app = NSApplication::sharedApplication(MainThreadMarker::from(self));
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.activate();
    }

    fn restore_accessory_activation_policy_if_possible(&self) {
        if self.settings_window_is_visible() || self.permissions_dialog_is_visible() {
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
        self.sync_transformation_provider_ui();
        self.promote_for_window_presentation();
        settings_window.show(MainThreadMarker::from(self));
    }

    fn present_startup_settings_window(&self) {
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
        self.sync_transformation_provider_ui();
        self.promote_for_window_presentation();
        settings_window.show_startup(MainThreadMarker::from(self));
    }

    fn disable_settings_window_hotkey_blocking(&self) {
        self.ivars().hotkey_capture_controller.cancel();
        self.ivars()
            .hotkey_capture_controller
            .set_settings_window_visible(false);

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
        effective_config.deepgram.project_id = settings_window.deepgram_project_id_value();

        Ok(DeepgramCheckRequest::new(
            effective_config.resolve_deepgram_api_key()?,
            effective_config.resolve_deepgram_project_id(),
        ))
    }

    fn sync_transformation_provider_ui(&self) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        settings_window.sync_transformation_api_key_env_hint();
        let provider_selected = settings_window.transformation_provider_value().is_some();
        settings_window.set_transformation_model_controls_enabled(provider_selected);
        if !provider_selected {
            settings_window.populate_transformation_model_values(&[]);
            settings_window.set_status("");
            return;
        }

        if let Ok(request) = self.current_transformation_provider_request() {
            match self
                .ivars()
                .transformation_models_controller
                .load_cached_models_now(request)
            {
                TransformationModelUpdate::CachedModelsLoaded {
                    models, message, ..
                } => {
                    settings_window.populate_transformation_model_values(&models);
                    settings_window.set_status(&message);
                }
                TransformationModelUpdate::ActionFailed { message, .. } => {
                    settings_window.set_status(&message);
                }
                TransformationModelUpdate::ModelsRefreshed { .. }
                | TransformationModelUpdate::ConnectionChecked { .. } => {}
            }
        }
    }

    fn start_transformation_model_action(&self, action: TransformationModelAction) {
        let Some(settings_window) = self.ivars().settings_window.get() else {
            return;
        };

        let request = match self.current_transformation_provider_request() {
            Ok(request) => request,
            Err(error) => {
                settings_window.set_status(&error);
                return;
            }
        };

        let status_message = match action {
            TransformationModelAction::Refresh => {
                format!("Refreshing models for {}…", request.provider)
            }
            TransformationModelAction::Check => {
                format!("Checking {} connection…", request.provider)
            }
        };
        settings_window.set_status(&status_message);
        self.ivars()
            .transformation_models_controller
            .start_action(action, request);
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
            let update_request = match &update {
                TransformationModelUpdate::CachedModelsLoaded { request, .. }
                | TransformationModelUpdate::ModelsRefreshed { request, .. }
                | TransformationModelUpdate::ConnectionChecked { request, .. }
                | TransformationModelUpdate::ActionFailed { request, .. } => request,
            };
            if current_request
                .as_ref()
                .map(|request| request.same_source_as(update_request))
                != Some(true)
            {
                continue;
            }

            match update {
                TransformationModelUpdate::CachedModelsLoaded {
                    models, message, ..
                }
                | TransformationModelUpdate::ModelsRefreshed {
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
                DeepgramCheckUpdate::ConnectionChecked { message, .. }
                | DeepgramCheckUpdate::ActionFailed { message, .. } => {
                    settings_window.set_status(&message);
                }
            }
        }
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
        self.handle_pending_transformation_model_updates();
        self.handle_pending_deepgram_check_updates();
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
        Some(_) | None => 10.0,
    };
    let transformation_hotkey = config
        .resolve_transformation_config()
        .ok()
        .map(|_| config.transformation.hotkey.as_str());
    let shortcut_hint = Some(match transformation_hotkey {
        Some(hotkey) => format!(
            "<{}> transform <{}> paste <Cmd+V> insert <ESC> cancel",
            hotkey, config.ui.hotkey
        ),
        None => format!("<{}> paste <Cmd+V> insert <ESC> cancel", config.ui.hotkey),
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
    if trimmed_overlay_footer_text.starts_with("Deepgram (")
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
    overlay_footer_text: Arc<str>,
    overlay_text: Arc<str>,
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
    use super::{billing_menu_text, config_file_is_missing, overlay_style_from_config};
    use crate::config::Config;

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
            Some("<F5> paste <Cmd+V> insert <ESC> cancel")
        );
    }

    #[test]
    fn overlay_style_includes_transform_shortcut_when_transformation_is_configured() {
        let mut config = Config::default();
        config.transformation.provider = Some("openai".to_owned());

        let style = overlay_style_from_config(&config);

        assert_eq!(
            style.shortcut_hint.as_deref(),
            Some("<F6> transform <F5> paste <Cmd+V> insert <ESC> cancel")
        );
    }

    #[test]
    fn billing_menu_text_accepts_deepgram_monthly_spend_label() {
        assert_eq!(
            billing_menu_text("Deepgram (Apr 2026): $12.34"),
            Some("Deepgram (Apr 2026): $12.34")
        );
        assert_eq!(billing_menu_text("Billing (Apr 2026): $12.34"), None);
    }
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
            let mut last_mic_meter = MicMeterSnapshot::default();
            let mut last_overlay_footer_text: Arc<str> = Arc::from("");
            let mut last_overlay_text: Arc<str> = Arc::from("");
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
                    || !Arc::ptr_eq(&current_overlay_footer_text, &last_overlay_footer_text)
                    || !Arc::ptr_eq(&current_overlay_text, &last_overlay_text)
                    || (current_overlay_text_opacity - last_overlay_text_opacity).abs()
                        > f64::EPSILON;
                let mic_meter_changed = current_mic_meter != last_mic_meter;
                let should_animate_meter = current_state == STATE_RECORDING;
                let should_animate_overlay =
                    matches!(current_state, STATE_PROCESSING | STATE_TRANSFORMING);
                let hotkey_capture_update_pending =
                    hotkey_capture_controller.has_pending_ui_update();
                let transformation_models_update_pending =
                    transformation_models_controller.has_pending_ui_update();
                let deepgram_connection_update_pending =
                    deepgram_connection_controller.has_pending_ui_update();
                if !ui_changed
                    && !mic_meter_changed
                    && !should_animate_meter
                    && !should_animate_overlay
                    && !hotkey_capture_update_pending
                    && !transformation_models_update_pending
                    && !deepgram_connection_update_pending
                {
                    continue;
                }

                last_state = current_state;
                last_mic_meter = current_mic_meter;
                last_overlay_footer_text = Arc::clone(&current_overlay_footer_text);
                last_overlay_text = Arc::clone(&current_overlay_text);
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
