pub mod actions;
pub mod builder;
pub mod helpers;

pub use builder::*;
pub use helpers::*;

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{msg_send, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSColor, NSComboBox,
    NSControlStateValueOff, NSControlStateValueOn, NSPopUpButton, NSScrollView, NSSlider,
    NSTextField, NSTextView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSPoint, NSRect, NSSize, NSString,
};

use crate::audio::{available_audio_input_devices, AvailableAudioInputDevices};
use crate::config::Config;
use crate::hotkey_capture::HotkeyCaptureTarget;
use crate::state::MicMeterSnapshot;
use crate::ui_meter::UiMeterView;
use actions::SettingsAction;

#[derive(Debug)]
pub struct SettingsWindow {
    window: Retained<NSWindow>,
    scroll_view: Retained<NSScrollView>,
    ui_hotkey_field: Retained<NSTextField>,
    ui_hotkey_capture_button: Retained<NSButton>,
    ui_correction_key_field: Retained<NSTextField>,
    ui_correction_key_capture_button: Retained<NSButton>,
    ui_meter_style_popup: Retained<NSPopUpButton>,
    mic_audio_device_popup: Retained<NSPopUpButton>,
    mic_audio_device_options: RefCell<Vec<MicAudioDeviceOption>>,
    mic_sample_rate_field: Retained<NSTextField>,
    mic_hold_ms_field: Retained<NSTextField>,
    mic_gain_slider: Retained<NSSlider>,
    mic_gain_label: Retained<NSTextField>,
    mic_preview_meter_view: UiMeterView,
    deepgram_api_key_field: Retained<NSTextField>,
    deepgram_api_key_env_hint_field: Retained<NSTextField>,
    deepgram_project_id_field: Retained<NSTextField>,
    deepgram_project_id_env_hint_field: Retained<NSTextField>,
    deepgram_language_field: Retained<NSTextField>,
    deepgram_keyterms_field: Retained<NSTextField>,
    deepgram_model_popup: Retained<NSPopUpButton>,
    deepgram_endpointing_ms_field: Retained<NSTextField>,
    deepgram_utterance_end_ms_field: Retained<NSTextField>,
    transformation_hotkey_field: Retained<NSTextField>,
    transformation_hotkey_capture_button: Retained<NSButton>,
    transformation_auto_checkbox: Retained<NSButton>,
    transformation_provider_popup: Retained<NSPopUpButton>,
    transformation_api_key_field: Retained<NSTextField>,
    transformation_api_key_env_hint_field: Retained<NSTextField>,
    transformation_model_combo_box: Retained<NSComboBox>,
    transformation_model_refresh_button: Retained<NSButton>,
    transformation_model_check_button: Retained<NSButton>,
    transformation_system_prompt_view: Retained<NSTextView>,
    transformation_correction_system_prompt_view: Retained<NSTextView>,
    ui_auto_check_updates_checkbox: Retained<NSButton>,
    ui_start_on_login_checkbox: Retained<NSButton>,
    ui_font_name_popup: Retained<NSPopUpButton>,
    ui_font_size_field: Retained<NSTextField>,
    ui_footer_font_size_field: Retained<NSTextField>,
    deepgram_api_key_check_button: Retained<NSButton>,
    status_text_field: Retained<NSTextField>,
    _save_button: Retained<NSButton>,
    _cancel_button: Retained<NSButton>,
    hotkey_capture_restore_value: RefCell<Option<(HotkeyCaptureTarget, String)>>,
}

impl SettingsWindow {
    pub fn new(
        mtm: MainThreadMarker,
        target: &objc2::runtime::AnyObject,
        delegate: &ProtocolObject<dyn NSWindowDelegate>,
    ) -> Self {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
                ),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };

        window.setTitle(ns_string!("Settings"));
        window.center();
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setDelegate(Some(delegate));

        let main_content_view = window.contentView().unwrap();

        let scroll_view = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, BUTTON_BAR_Y + BUTTON_BAR_HEIGHT + 10.0),
                NSSize::new(
                    WINDOW_WIDTH,
                    WINDOW_HEIGHT - (BUTTON_BAR_Y + BUTTON_BAR_HEIGHT + 10.0),
                ),
            ),
        );
        scroll_view.setHasVerticalScroller(true);
        scroll_view.setHasHorizontalScroller(false);
        scroll_view.setAutohidesScrollers(true);
        scroll_view.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
        unsafe {
            let _: () = msg_send![&scroll_view, setHorizontalScrollElasticity: 1isize];
        }

        let content_view = unsafe {
            let view: Retained<SettingsScrollContentView> = msg_send![
                SettingsScrollContentView::alloc(mtm),
                initWithFrame: NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(WINDOW_WIDTH - 24.0, CONTENT_HEIGHT),
                )
            ];
            Retained::into_super(view)
        };

        let mut current_y = CONTENT_HEIGHT - CONTENT_TOP_PADDING;

        current_y = add_section_title(&content_view, mtm, current_y, "Hotkeys & Interface");
        let (ui_hotkey_field, ui_hotkey_capture_button) = add_labeled_hotkey_field(
            &content_view,
            target,
            mtm,
            &mut current_y,
            "Dictation shortcut",
            SettingsAction::CaptureRecordHotkey,
        );
        let (ui_correction_key_field, ui_correction_key_capture_button) =
            add_labeled_hotkey_field(
                &content_view,
                target,
                mtm,
                &mut current_y,
                "Correction key",
                SettingsAction::CaptureCorrectionKey,
            );
        let available_font_names = available_font_family_names(mtm);
        let ui_font_name_popup =
            add_labeled_pop_up_button(&content_view, mtm, &mut current_y, "Overlay font");
        populate_font_name_popup(&ui_font_name_popup, &available_font_names, None);

        let ui_font_size_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Font size");
        let ui_footer_font_size_field = add_labeled_text_field(
            &content_view,
            mtm,
            &mut current_y,
            "Footer font size",
        );
        let ui_meter_style_popup =
            add_labeled_pop_up_button(&content_view, mtm, &mut current_y, "Meter style");

        current_y = add_section_title(&content_view, mtm, current_y, "Microphone");
        let mic_audio_device_popup = add_labeled_pop_up_button_with_action(
            &content_view,
            target,
            mtm,
            &mut current_y,
            "Audio device",
            SettingsAction::MicAudioDeviceChanged,
        );
        let mic_sample_rate_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Sample rate");
        let (mic_gain_label, mic_gain_slider, mic_preview_meter_view) =
            add_labeled_slider_with_meter(
                &content_view,
                target,
                mtm,
                &mut current_y,
                "Mic gain (dB)",
                SettingsAction::MicGainSliderChanged,
            );
        let mic_hold_ms_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Silence pad (ms)");
        let _mic_always_on_checkbox = add_checkbox(
            &content_view,
            mtm,
            &mut current_y,
            "Keep microphone connection open",
        );

        current_y = add_section_title(&content_view, mtm, current_y, "Deepgram");
        let (deepgram_api_key_field, deepgram_api_key_check_button, deepgram_api_key_env_hint_field) =
            add_labeled_text_field_with_hint_and_button(
                &content_view,
                target,
                mtm,
                &mut current_y,
                "API key",
                "Check",
                SettingsAction::CheckDeepgramConnection,
            );
        let (deepgram_project_id_field, deepgram_project_id_env_hint_field) =
            add_labeled_text_field_with_hint(
                &content_view,
                mtm,
                &mut current_y,
                "Project ID",
            );
        let deepgram_language_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Language");
        let deepgram_keyterms_field = add_labeled_text_field_with_hint(
            &content_view,
            mtm,
            &mut current_y,
            "Keyterms",
        )
        .0;
        let deepgram_model_popup =
            add_labeled_pop_up_button(&content_view, mtm, &mut current_y, "Model");
        let deepgram_endpointing_ms_field = add_labeled_text_field(
            &content_view,
            mtm,
            &mut current_y,
            "Endpointing (ms)",
        );
        let deepgram_utterance_end_ms_field = add_labeled_text_field(
            &content_view,
            mtm,
            &mut current_y,
            "Utterance end (ms)",
        );

        current_y = add_section_title(&content_view, mtm, current_y, "Prompt Transformation");
        let (transformation_hotkey_field, transformation_hotkey_capture_button) =
            add_labeled_hotkey_field(
                &content_view,
                target,
                mtm,
                &mut current_y,
                "Transform shortcut",
                SettingsAction::CaptureTransformHotkey,
            );
        let transformation_auto_checkbox = add_checkbox_with_hint(
            &content_view,
            mtm,
            &mut current_y,
            "Auto-transform on shortcut release",
            "When enabled, releasing the dictation shortcut runs transformation automatically.",
        );
        let transformation_provider_popup = add_labeled_pop_up_button_with_action(
            &content_view,
            target,
            mtm,
            &mut current_y,
            "Provider",
            SettingsAction::TransformationProviderChanged,
        );
        let (transformation_api_key_field, transformation_api_key_env_hint_field) =
            add_labeled_text_field_with_hint(
                &content_view,
                mtm,
                &mut current_y,
                "API key",
            );
        let (
            transformation_model_combo_box,
            transformation_model_refresh_button,
            transformation_model_check_button,
        ) = add_labeled_combo_box_with_buttons(
            &content_view,
            target,
            mtm,
            &mut current_y,
            "Model",
            "Fetch models",
            SettingsAction::RefreshTransformationModels,
            "Check",
            SettingsAction::CheckTransformationProvider,
        );
        let transformation_system_prompt_view = add_prompt_editor(
            &content_view,
            mtm,
            &mut current_y,
            "Dictation prompt",
        );
        let transformation_correction_system_prompt_view = add_prompt_editor(
            &content_view,
            mtm,
            &mut current_y,
            "Correction prompt",
        );

        current_y = add_section_title(&content_view, mtm, current_y, "System");
        let ui_auto_check_updates_checkbox = add_checkbox(
            &content_view,
            mtm,
            &mut current_y,
            "Automatically check for updates",
        );
        let ui_start_on_login_checkbox = add_checkbox(
            &content_view,
            mtm,
            &mut current_y,
            "Start automatically on login",
        );

        scroll_view.setDocumentView(Some(&content_view));
        main_content_view.addSubview(&scroll_view);

        let status_text_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        configure_wrapping_label(&status_text_field);
        set_view_frame(
            &*status_text_field,
            HORIZONTAL_PADDING,
            BUTTON_BAR_Y + 5.0,
            WINDOW_WIDTH - (HORIZONTAL_PADDING * 2.0) - 180.0,
            STATUS_HEIGHT,
        );
        main_content_view.addSubview(&status_text_field);

        let button_width = 80.0;
        let save_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Save"),
                Some(target),
                Some(SettingsAction::SaveSettings.selector()),
                mtm,
            )
        };
        save_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*save_button,
            WINDOW_WIDTH - HORIZONTAL_PADDING - button_width,
            BUTTON_BAR_Y,
            button_width,
            BUTTON_BAR_HEIGHT,
        );

        let cancel_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Cancel"),
                Some(target),
                Some(SettingsAction::CancelSettings.selector()),
                mtm,
            )
        };
        cancel_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*cancel_button,
            WINDOW_WIDTH - HORIZONTAL_PADDING - (button_width * 2.0) - 10.0,
            BUTTON_BAR_Y,
            button_width,
            BUTTON_BAR_HEIGHT,
        );

        main_content_view.addSubview(&save_button);
        main_content_view.addSubview(&cancel_button);

        let window_obj = Self {
            window,
            scroll_view,
            ui_hotkey_field,
            ui_hotkey_capture_button,
            ui_correction_key_field,
            ui_correction_key_capture_button,
            ui_meter_style_popup,
            mic_audio_device_popup,
            mic_audio_device_options: RefCell::new(Vec::new()),
            mic_sample_rate_field,
            mic_hold_ms_field,
            mic_gain_slider,
            mic_gain_label,
            mic_preview_meter_view,
            deepgram_api_key_field,
            deepgram_api_key_env_hint_field,
            deepgram_project_id_field,
            deepgram_project_id_env_hint_field,
            deepgram_language_field,
            deepgram_keyterms_field,
            deepgram_model_popup,
            deepgram_endpointing_ms_field,
            deepgram_utterance_end_ms_field,
            transformation_hotkey_field,
            transformation_hotkey_capture_button,
            transformation_auto_checkbox,
            transformation_provider_popup,
            transformation_api_key_field,
            transformation_api_key_env_hint_field,
            transformation_model_combo_box,
            transformation_model_refresh_button,
            transformation_model_check_button,
            transformation_system_prompt_view,
            transformation_correction_system_prompt_view,
            ui_auto_check_updates_checkbox,
            ui_start_on_login_checkbox,
            ui_font_name_popup,
            ui_font_size_field,
            ui_footer_font_size_field,
            deepgram_api_key_check_button,
            status_text_field,
            _save_button: save_button,
            _cancel_button: cancel_button,
            hotkey_capture_restore_value: RefCell::new(None),
        };

        window_obj.set_hotkey_capture_state(None);
        window_obj
    }

    #[allow(dead_code)]
    pub fn window(&self) -> &NSWindow {
        &self.window
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    pub fn show(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
        let _ = self.window.makeFirstResponder(Some(&*self.ui_hotkey_field));
        self.scroll_to_top();
    }

    pub fn show_startup(&self, mtm: MainThreadMarker) {
        self.show(mtm);
    }

    pub fn update_meter(&self, meter: Option<MicMeterSnapshot>) {
        self.update_preview_mic_meter(meter);
    }

    pub fn hide(&self) {
        self.window.orderOut(None);
    }

    pub fn load_from_config(
        &self,
        config: &Config,
        audio_device_status_message: Option<&str>,
    ) -> Result<(), String> {
        self.ui_auto_check_updates_checkbox
            .setState(if config.ui.auto_check_updates {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        self.ui_start_on_login_checkbox.setState(if config.ui.start_on_login {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.ui_hotkey_field
            .setStringValue(&NSString::from_str(&config.ui.hotkey));
        self.ui_correction_key_field
            .setStringValue(&NSString::from_str(&config.ui.correction_key));

        let available_font_names =
            available_font_family_names(MainThreadMarker::from(&*self.window));
        populate_font_name_popup(
            &self.ui_font_name_popup,
            &available_font_names,
            config.ui.font_name.as_deref(),
        );

        self.ui_font_size_field
            .setStringValue(&NSString::from_str(&config.ui.font_size.to_string()));
        self.ui_footer_font_size_field.setStringValue(&NSString::from_str(
            &config
                .ui
                .footer_font_size
                .map(|val| val.to_string())
                .unwrap_or_default(),
        ));
        populate_meter_style_popup(&self.ui_meter_style_popup, config.ui.meter_style);

        self.populate_mic_audio_device_popup(config.mic.audio_device.as_deref())?;
        self.mic_sample_rate_field
            .setStringValue(&NSString::from_str(&config.mic.sample_rate.to_string()));
        self.mic_gain_slider.setDoubleValue(f64::from(config.mic.gain));
        self.mic_gain_label
            .setStringValue(&NSString::from_str(&format!(
                "{:.1}",
                config.mic.gain / 1.5
            )));
        self.mic_hold_ms_field
            .setStringValue(&NSString::from_str(&config.mic.hold_ms.to_string()));

        self.deepgram_api_key_field
            .setStringValue(&NSString::from_str(
                config.deepgram.api_key.as_deref().unwrap_or(""),
            ));
        set_hint_text(
            &self.deepgram_api_key_env_hint_field,
            config
                .deepgram_api_key_env_var_in_use()
                .map(environment_hint_message),
        );
        self.deepgram_project_id_field
            .setStringValue(&NSString::from_str(
                config.deepgram.project_id.as_deref().unwrap_or(""),
            ));
        set_hint_text(
            &self.deepgram_project_id_env_hint_field,
            config
                .deepgram_project_id_env_var_in_use()
                .map(environment_hint_message),
        );
        self.deepgram_language_field
            .setStringValue(&NSString::from_str(&config.deepgram.language));
        self.deepgram_keyterms_field
            .setStringValue(&NSString::from_str(&config.deepgram.keyterms.join(", ")));
        populate_deepgram_model_popup(&self.deepgram_model_popup, &config.deepgram.model);
        self.deepgram_endpointing_ms_field
            .setStringValue(&NSString::from_str(
                &config.deepgram.endpointing_ms.to_string(),
            ));
        self.deepgram_utterance_end_ms_field
            .setStringValue(&NSString::from_str(
                &config.deepgram.utterance_end_ms.to_string(),
            ));

        self.transformation_hotkey_field
            .setStringValue(&NSString::from_str(&config.transformation.hotkey));
        self.transformation_auto_checkbox
            .setState(if config.transformation.auto {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        populate_transformation_provider_popup(
            &self.transformation_provider_popup,
            config.transformation.provider.as_deref(),
        );
        self.transformation_api_key_field
            .setStringValue(&NSString::from_str(
                config.transformation.api_key.as_deref().unwrap_or(""),
            ));
        set_hint_text(
            &self.transformation_api_key_env_hint_field,
            config
                .transformation_api_key_env_var_in_use()
                .map(environment_hint_message),
        );
        populate_combo_box_with_values(
            &self.transformation_model_combo_box,
            &[],
            &config.transformation.model,
        );
        self.set_transformation_model_controls_enabled(
            self.transformation_provider_value().is_some(),
        );
        self.transformation_system_prompt_view
            .setString(&NSString::from_str(&config.transformation.system_prompt));
        self.transformation_correction_system_prompt_view
            .setString(&NSString::from_str(
                &config.transformation.correction_system_prompt,
            ));
        self.set_status(audio_device_status_message.unwrap_or(""));
        self.scroll_to_top();
        Ok(())
    }

    pub fn read_config(&self) -> Result<Config, String> {
        Ok(Config {
            ui: crate::config::UiConfig {
                start_on_login: self.ui_start_on_login_checkbox.state() == NSControlStateValueOn,
                auto_check_updates: self.ui_auto_check_updates_checkbox.state() == NSControlStateValueOn,
                hotkey: read_required_string(&self.ui_hotkey_field, "Record hotkey")?,
                correction_key: read_required_string(
                    &self.ui_correction_key_field,
                    "Correction key",
                )?,
                font_name: read_optional_pop_up_button_string(&self.ui_font_name_popup),
                font_size: read_required_f64(&self.ui_font_size_field, "Font size")?,
                footer_font_size: read_optional_f64(
                    &self.ui_footer_font_size_field,
                    "Footer font size",
                )?,
                meter_style: parse_meter_style(&read_required_pop_up_button_string(
                    &self.ui_meter_style_popup,
                    "Meter style",
                )?)?,
            },
            mic: crate::config::MicConfig {
                audio_device: self.mic_audio_device_value(),
                sample_rate: read_required_u32(&self.mic_sample_rate_field, "Sample rate")?,
                gain: self.mic_gain_slider_value(),
                hold_ms: read_required_u64(&self.mic_hold_ms_field, "Hold ms")?,
                always_on: true,
            },
            deepgram: crate::config::DeepgramConfig {
                api_key: read_optional_string(&self.deepgram_api_key_field),
                project_id: read_optional_string(&self.deepgram_project_id_field),
                language: read_required_string(&self.deepgram_language_field, "Deepgram language")?,
                keyterms: read_optional_string(&self.deepgram_keyterms_field)
                    .map(|s| {
                        s.split(',')
                            .map(|k| k.trim().to_string())
                            .filter(|k| !k.is_empty())
                            .collect()
                    })
                    .unwrap_or_default(),
                model: read_required_pop_up_button_string(
                    &self.deepgram_model_popup,
                    "Deepgram model",
                )?,
                endpointing_ms: read_required_u16(
                    &self.deepgram_endpointing_ms_field,
                    "Endpointing ms",
                )?,
                utterance_end_ms: read_required_u16(
                    &self.deepgram_utterance_end_ms_field,
                    "Utterance end ms",
                )?,
            },
            transformation: crate::config::TransformationConfig {
                hotkey: read_required_string(
                    &self.transformation_hotkey_field,
                    "Transform hotkey",
                )?,
                auto: self.transformation_auto_checkbox.state() == NSControlStateValueOn,
                provider: read_optional_provider_pop_up_button_string(
                    &self.transformation_provider_popup,
                ),
                api_key: read_optional_string(&self.transformation_api_key_field),
                model: read_required_combo_box_string(
                    &self.transformation_model_combo_box,
                    "Transformation model",
                )?,
                system_prompt: self.transformation_system_prompt_view.string().to_string(),
                correction_system_prompt: self
                    .transformation_correction_system_prompt_view
                    .string()
                    .to_string(),
            },
        })
    }

    pub fn set_status(&self, message: &str) {
        self.status_text_field
            .setStringValue(&NSString::from_str(message));
    }

    pub fn update_preview_mic_meter(&self, meter: Option<MicMeterSnapshot>) {
        let meter = meter.unwrap_or_default();
        self.mic_preview_meter_view.update(meter, 180.0);
    }

    pub fn update_mic_gain_label(&self, gain_db: f32) {
        self.mic_gain_label
            .setStringValue(&NSString::from_str(&format!("{:.1} dB", gain_db)));
    }

    pub fn mic_gain_slider_value(&self) -> f32 {
        self.mic_gain_slider.doubleValue() as f32
    }

    pub fn sync_transformation_api_key_env_hint(&self) {
        let hint = crate::config::transformation_api_key_env_var_in_use(
            self.transformation_provider_value().as_deref(),
            self.transformation_api_key_value().as_deref(),
        )
        .map(environment_hint_message);
        set_hint_text(&self.transformation_api_key_env_hint_field, hint);
    }

    pub fn deepgram_api_key_value(&self) -> Option<String> {
        read_optional_string(&self.deepgram_api_key_field)
    }

    pub fn deepgram_project_id_value(&self) -> Option<String> {
        read_optional_string(&self.deepgram_project_id_field)
    }

    pub fn transformation_provider_value(&self) -> Option<String> {
        read_optional_provider_pop_up_button_string(&self.transformation_provider_popup)
    }

    pub fn transformation_api_key_value(&self) -> Option<String> {
        read_optional_string(&self.transformation_api_key_field)
    }

    pub fn transformation_model_value(&self) -> String {
        self.transformation_model_combo_box
            .stringValue()
            .to_string()
    }

    pub fn populate_transformation_model_values(&self, models: &[String]) {
        let selected_model = self.transformation_model_value();
        populate_combo_box_with_values(
            &self.transformation_model_combo_box,
            models,
            selected_model.as_str(),
        );
    }

    pub fn set_transformation_model_controls_enabled(&self, enabled: bool) {
        self.transformation_model_combo_box.setEnabled(enabled);
        self.transformation_model_refresh_button.setEnabled(enabled);
        self.transformation_model_check_button.setEnabled(enabled);
    }

    pub fn set_transformation_check_result(&self, success: bool) {
        let (bezel, text) = if success {
            (
                Some(NSColor::systemGreenColor()),
                Some(NSColor::whiteColor()),
            )
        } else {
            (Some(NSColor::systemRedColor()), Some(NSColor::whiteColor()))
        };
        style_button_bezel_and_text(
            &self.transformation_model_check_button,
            "Check",
            bezel,
            text,
        );
    }

    pub fn set_deepgram_check_result(&self, success: bool) {
        let (bezel, text) = if success {
            (
                Some(NSColor::systemGreenColor()),
                Some(NSColor::whiteColor()),
            )
        } else {
            (Some(NSColor::systemRedColor()), Some(NSColor::whiteColor()))
        };
        style_button_bezel_and_text(&self.deepgram_api_key_check_button, "Check", bezel, text);
    }

    pub fn reset_transformation_check_button(&self) {
        style_button_bezel_and_text(&self.transformation_model_check_button, "Check", None, None);
    }

    pub fn reset_deepgram_check_button(&self) {
        style_button_bezel_and_text(&self.deepgram_api_key_check_button, "Check", None, None);
    }

    pub fn begin_hotkey_capture(&self, target: HotkeyCaptureTarget) {
        self.hotkey_capture_restore_value
            .replace(Some((target, self.hotkey_value(target))));
        self.set_hotkey_capture_state(Some(target));
        self.set_hotkey_value(target, "");
        self.set_status("Press a key, or Esc to cancel");
    }

    pub fn set_hotkey_capture_preview(&self, target: HotkeyCaptureTarget, value: &str) {
        self.set_hotkey_value(target, value);
    }

    #[allow(dead_code)]
    pub fn set_hotkey_capture_outcome(&self, target: HotkeyCaptureTarget, value: &str) {
        self.set_hotkey_capture_state(None);
        self.set_hotkey_value(target, value);
    }

    pub fn cancel_hotkey_capture(&self) {
        if let Some((target, value)) = self.hotkey_capture_restore_value.borrow_mut().take() {
            self.set_hotkey_value(target, &value);
        }
        self.set_hotkey_capture_state(None);
    }

    pub fn finish_hotkey_capture(&self) {
        self.hotkey_capture_restore_value.borrow_mut().take();
        self.set_hotkey_capture_state(None);
    }

    pub fn hotkey_value(&self, target: HotkeyCaptureTarget) -> String {
        match target {
            HotkeyCaptureTarget::Record => self.ui_hotkey_field.stringValue().to_string(),
            HotkeyCaptureTarget::Correction => {
                self.ui_correction_key_field.stringValue().to_string()
            }
            HotkeyCaptureTarget::Transform => {
                self.transformation_hotkey_field.stringValue().to_string()
            }
        }
    }

    fn populate_mic_audio_device_popup(
        &self,
        configured_audio_device: Option<&str>,
    ) -> Result<(), String> {
        let available_audio_input_devices = available_audio_input_devices();
        let (audio_device_options, selected_audio_device_title) = mic_audio_device_popup_state(
            available_audio_input_devices
                .clone()
                .unwrap_or(AvailableAudioInputDevices {
                    default_device_name: None,
                    choices: Vec::new(),
                }),
            configured_audio_device,
        );

        self.mic_audio_device_popup.removeAllItems();
        for option in &audio_device_options {
            self.mic_audio_device_popup
                .addItemWithTitle(&NSString::from_str(&option.title));
        }
        self.mic_audio_device_popup
            .selectItemWithTitle(&NSString::from_str(&selected_audio_device_title));
        self.mic_audio_device_options.replace(audio_device_options);

        available_audio_input_devices.map(|_| ())
    }

    pub fn mic_audio_device_value(&self) -> Option<String> {
        let selected_title = self
            .mic_audio_device_popup
            .titleOfSelectedItem()
            .map(|selected_title| selected_title.to_string())?;

        if let Some(option) = self
            .mic_audio_device_options
            .borrow()
            .iter()
            .find(|option| option.title == selected_title)
        {
            return option.value.clone();
        }

        let trimmed_value = selected_title.trim();
        if trimmed_value.is_empty() {
            None
        } else {
            Some(trimmed_value.to_owned())
        }
    }

    pub fn set_hotkey_value(&self, target: HotkeyCaptureTarget, value: &str) {
        match target {
            HotkeyCaptureTarget::Record => {
                self.ui_hotkey_field
                    .setStringValue(&NSString::from_str(value));
            }
            HotkeyCaptureTarget::Correction => {
                self.ui_correction_key_field
                    .setStringValue(&NSString::from_str(value));
            }
            HotkeyCaptureTarget::Transform => {
                self.transformation_hotkey_field
                    .setStringValue(&NSString::from_str(value));
            }
        }
    }

    fn set_hotkey_capture_state(&self, active_target: Option<HotkeyCaptureTarget>) {
        set_capture_button_state(
            &self.ui_hotkey_capture_button,
            active_target == Some(HotkeyCaptureTarget::Record),
            active_target.is_none(),
        );
        set_capture_button_state(
            &self.ui_correction_key_capture_button,
            active_target == Some(HotkeyCaptureTarget::Correction),
            active_target.is_none(),
        );
        set_capture_button_state(
            &self.transformation_hotkey_capture_button,
            active_target == Some(HotkeyCaptureTarget::Transform),
            active_target.is_none(),
        );
    }

    fn scroll_to_top(&self) {
        let clip_view = self.scroll_view.contentView();
        let Some(document_view) = self.scroll_view.documentView() else {
            return;
        };

        let document_height = document_view.frame().size.height;
        let visible_height = self.scroll_view.contentSize().height;
        let top_origin_y = (document_height - visible_height).max(0.0);
        clip_view.scrollToPoint(NSPoint::new(0.0, top_origin_y));
        self.scroll_view.reflectScrolledClipView(&clip_view);
    }
}
