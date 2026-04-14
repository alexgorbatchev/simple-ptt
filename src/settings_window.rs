use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{msg_send, sel, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSColor, NSComboBox,
    NSControlStateValueOff, NSControlStateValueOn, NSFont, NSFontManager, NSPopUpButton,
    NSScrollView, NSSlider, NSTextAlignment, NSTextField, NSTextView, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::CALayer;

use crate::audio::{available_audio_input_devices, AvailableAudioInputDevices};
use crate::config::{Config, UiMeterStyle};
use crate::hotkey_capture::HotkeyCaptureTarget;
use crate::state::MicMeterSnapshot;
use crate::ui_meter::UiMeterView;

const WINDOW_HEIGHT: f64 = 760.0;
const WINDOW_WIDTH: f64 = 760.0;
const CONTENT_HEIGHT: f64 = 1330.0;
const CONTENT_TOP_PADDING: f64 = 28.0;
const HORIZONTAL_PADDING: f64 = 20.0;
const LABEL_WIDTH: f64 = 180.0;
const FIELD_HEIGHT: f64 = 24.0;
const FIELD_WIDTH: f64 = 500.0;
const HOTKEY_FIELD_WIDTH: f64 = 390.0;
const CAPTURE_BUTTON_WIDTH: f64 = 100.0;
const CAPTURE_BUTTON_GAP: f64 = 10.0;
const MODEL_COMBO_BOX_WIDTH: f64 = 320.0;
const MODEL_ACTION_BUTTON_WIDTH: f64 = 80.0;
const MODEL_ACTION_BUTTON_GAP: f64 = 10.0;
const FIELD_WITH_ACTION_BUTTON_WIDTH: f64 =
    FIELD_WIDTH - MODEL_ACTION_BUTTON_GAP - MODEL_ACTION_BUTTON_WIDTH;
const FIELD_X: f64 = HORIZONTAL_PADDING + LABEL_WIDTH + 12.0;
const ROW_GAP: f64 = 10.0;
const SECTION_BREAK_GAP: f64 = 12.0;
const SECTION_GAP: f64 = 20.0;
const SECTION_HEIGHT: f64 = 22.0;
const SECTION_TITLE_BOTTOM_GAP: f64 = 10.0;
const PROMPT_HEIGHT: f64 = 270.0;
const STATUS_HEIGHT: f64 = 40.0;
const BUTTON_BAR_Y: f64 = 10.0;
const BUTTON_BAR_HEIGHT: f64 = 30.0;
const ENV_HINT_HEIGHT: f64 = 18.0;
const SETTINGS_FONT_SIZE: f64 = 12.0;
const SETTINGS_FONT_WEIGHT: f64 = 0.0;
const SETTINGS_HINT_FONT_SCALE: f64 = 0.8;
const SETTINGS_SECTION_TITLE_FONT_WEIGHT: f64 = 0.4;
const INPUT_BORDER_HIGHLIGHT_LEVEL: f64 = 0.5;
const INPUT_BORDER_WIDTH: f64 = 1.0;
const INPUT_CORNER_RADIUS: f64 = 6.0;
const LABEL_VISUAL_CENTER_NUDGE: f64 = -4.0;
const SYSTEM_DEFAULT_FONT_LABEL: &str = "System default";
const SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL: &str = "System default";
const TRANSFORMATION_PROVIDER_DISABLED_LABEL: &str = "Disabled";

#[derive(Clone, Debug, Eq, PartialEq)]
struct MicAudioDeviceOption {
    title: String,
    value: Option<String>,
}

// Source: Deepgram model docs and live streaming docs.
// - https://developers.deepgram.com/docs/model
// - https://developers.deepgram.com/docs/live-streaming-audio
// Whisper and custom model IDs are intentionally excluded here because this app
// uses live streaming transcription and Whisper is documented as pre-recorded-only,
// while custom IDs cannot be enumerated statically.
const DEEPGRAM_MODEL_OPTIONS: &[&str] = &[
    "flux",
    "nova-3",
    "nova-3-general",
    "nova-3-medical",
    "nova-2-general",
    "nova-2-meeting",
    "nova-2-phonecall",
    "nova-2-voicemail",
    "nova-2-finance",
    "nova-2-conversationalai",
    "nova-2-video",
    "nova-2-medical",
    "nova-2-drivethru",
    "nova-2-automotive",
    "nova-2-atc",
    "nova-general",
    "nova-phonecall",
    "enhanced-general",
    "enhanced-meeting",
    "enhanced-phonecall",
    "enhanced-finance",
    "base-general",
    "base-meeting",
    "base-phonecall",
    "base-voicemail",
    "base-finance",
    "base-conversationalai",
    "base-video",
];

#[derive(Debug)]
pub struct SettingsWindow {
    window: Retained<NSWindow>,
    scroll_view: Retained<NSScrollView>,
    path_text_field: Retained<NSTextField>,
    status_text_field: Retained<NSTextField>,
    ui_start_on_login_checkbox: Retained<NSButton>,
    ui_hotkey_field: Retained<NSTextField>,
    ui_hotkey_capture_button: Retained<NSButton>,
    ui_font_name_popup: Retained<NSPopUpButton>,
    ui_font_size_field: Retained<NSTextField>,
    ui_footer_font_size_field: Retained<NSTextField>,
    ui_meter_style_popup: Retained<NSPopUpButton>,
    mic_audio_device_popup: Retained<NSPopUpButton>,
    mic_audio_device_options: RefCell<Vec<MicAudioDeviceOption>>,
    mic_sample_rate_field: Retained<NSTextField>,
    mic_gain_slider: Retained<NSSlider>,
    mic_gain_label_field: Retained<NSTextField>,
    ui_meter_view: UiMeterView,
    mic_hold_ms_field: Retained<NSTextField>,
    deepgram_api_key_field: Retained<NSTextField>,
    deepgram_api_key_env_hint_field: Retained<NSTextField>,
    deepgram_project_id_field: Retained<NSTextField>,
    deepgram_project_id_env_hint_field: Retained<NSTextField>,
    deepgram_language_field: Retained<NSTextField>,
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
    available_font_family_names: Vec<String>,
    hotkey_capture_restore_value: RefCell<Option<(HotkeyCaptureTarget, String)>>,
}

impl SettingsWindow {
    pub fn new(target: &AnyObject, mtm: MainThreadMarker) -> Self {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
                ),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("simple-ptt Settings"));
        window.center();
        window.setContentMinSize(NSSize::new(WINDOW_WIDTH, 620.0));

        let root_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
            ),
        );
        root_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        let scroll_view = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, STATUS_HEIGHT + 16.0),
                NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT - STATUS_HEIGHT - 16.0),
            ),
        );
        scroll_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll_view.setDrawsBackground(false);
        scroll_view.setHasVerticalScroller(true);
        scroll_view.setHasHorizontalScroller(false);

        let content_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(WINDOW_WIDTH, CONTENT_HEIGHT),
            ),
        );
        content_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);

        let mut current_y = CONTENT_HEIGHT - CONTENT_TOP_PADDING;
        let path_row_y = current_y - 2.0;
        let path_title = NSTextField::labelWithString(&NSString::from_str("Config file"), mtm);
        path_title.setFont(Some(&settings_font()));
        path_title.setTextColor(Some(&NSColor::secondaryLabelColor()));
        set_view_frame(
            &*path_title,
            HORIZONTAL_PADDING,
            path_row_y,
            LABEL_WIDTH,
            FIELD_HEIGHT,
        );
        content_view.addSubview(&path_title);

        let path_text_field = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        path_text_field.setFont(Some(&settings_font()));
        path_text_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
        set_view_frame(
            &path_text_field,
            FIELD_X,
            path_row_y,
            WINDOW_WIDTH - FIELD_X - HORIZONTAL_PADDING,
            FIELD_HEIGHT,
        );
        content_view.addSubview(&path_text_field);
        current_y -= FIELD_HEIGHT + ROW_GAP;

        let available_font_family_names = available_font_family_names(mtm);

        current_y = add_section_title(&content_view, mtm, current_y, "UI");
        let ui_start_on_login_checkbox =
            add_checkbox(&content_view, mtm, &mut current_y, "Start on login");
        let (ui_hotkey_field, ui_hotkey_capture_button) = add_labeled_text_field_with_button(
            &content_view,
            target,
            mtm,
            &mut current_y,
            "Record hotkey",
            "Capture…",
            sel!(captureRecordHotkey:),
        );
        let ui_font_name_popup =
            add_labeled_pop_up_button(&content_view, mtm, &mut current_y, "Font name");
        let ui_font_size_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Font size");
        let ui_footer_font_size_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Footer font size");
        let ui_meter_style_popup =
            add_labeled_pop_up_button(&content_view, mtm, &mut current_y, "Meter style");

        current_y = add_section_title(&content_view, mtm, current_y, "Microphone");
        let mic_audio_device_popup =
            add_labeled_pop_up_button(&content_view, mtm, &mut current_y, "Audio device");
        let mic_sample_rate_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Sample rate");
        let mic_hold_ms_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Hold ms");
        let (mic_gain_label_field, mic_gain_slider, ui_meter_view) = add_labeled_slider_with_meter(
            &content_view,
            target,
            mtm,
            &mut current_y,
            "Gain",
            sel!(micGainSliderChanged:),
        );

        current_y = add_section_title(&content_view, mtm, current_y, "Deepgram");
        let (
            deepgram_api_key_field,
            _deepgram_api_key_check_button,
            deepgram_api_key_env_hint_field,
        ) = add_labeled_text_field_with_hint_and_button(
            &content_view,
            target,
            mtm,
            &mut current_y,
            "API key",
            "Check",
            sel!(checkDeepgramConnection:),
        );
        let (deepgram_project_id_field, deepgram_project_id_env_hint_field) =
            add_labeled_text_field_with_hint(&content_view, mtm, &mut current_y, "Project ID");
        let deepgram_language_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Language");
        let deepgram_model_popup =
            add_labeled_pop_up_button(&content_view, mtm, &mut current_y, "Model");
        let deepgram_endpointing_ms_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Endpointing ms");
        let deepgram_utterance_end_ms_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Utterance end ms");

        current_y = add_section_title(&content_view, mtm, current_y, "Transformation");
        let (transformation_hotkey_field, transformation_hotkey_capture_button) =
            add_labeled_text_field_with_button(
                &content_view,
                target,
                mtm,
                &mut current_y,
                "Transform hotkey",
                "Capture…",
                sel!(captureTransformHotkey:),
            );
        let transformation_auto_checkbox = add_checkbox(
            &content_view,
            mtm,
            &mut current_y,
            "Auto-transform when stopping with the record hotkey",
        );
        let transformation_provider_popup =
            add_labeled_pop_up_button(&content_view, mtm, &mut current_y, "Provider");
        unsafe {
            transformation_provider_popup.setTarget(Some(target));
            transformation_provider_popup.setAction(Some(sel!(transformationProviderChanged:)));
        }
        let (transformation_api_key_field, transformation_api_key_env_hint_field) =
            add_labeled_text_field_with_hint(&content_view, mtm, &mut current_y, "API key");
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
            "Refresh",
            sel!(refreshTransformationModels:),
            "Check",
            sel!(checkTransformationProvider:),
        );
        let transformation_system_prompt_view =
            add_prompt_editor(&content_view, mtm, &mut current_y, "System prompt");

        let right_edge = FIELD_X + FIELD_WIDTH;
        let save_button_width = 150.0;
        let save_button_x = right_edge - save_button_width;
        let close_button_width = 100.0;
        let close_button_x = save_button_x - 10.0 - close_button_width;

        let close_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Close"),
                Some(target),
                Some(sel!(cancelSettings:)),
                mtm,
            )
        };
        close_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*close_button,
            close_button_x,
            BUTTON_BAR_Y,
            close_button_width,
            BUTTON_BAR_HEIGHT,
        );
        root_view.addSubview(&close_button);

        let save_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Save and Apply"),
                Some(target),
                Some(sel!(saveSettings:)),
                mtm,
            )
        };
        save_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*save_button,
            save_button_x,
            BUTTON_BAR_Y,
            save_button_width,
            BUTTON_BAR_HEIGHT,
        );
        root_view.addSubview(&save_button);

        let status_text_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        configure_wrapping_label(&status_text_field);
        status_text_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
        set_view_frame(
            &*status_text_field,
            HORIZONTAL_PADDING,
            vertically_centered_label_y(BUTTON_BAR_Y, BUTTON_BAR_HEIGHT),
            close_button_x - HORIZONTAL_PADDING - 20.0,
            FIELD_HEIGHT,
        );
        root_view.addSubview(&status_text_field);

        scroll_view.setDocumentView(Some(&content_view));
        root_view.addSubview(&scroll_view);
        window.setContentView(Some(&root_view));
        window.setInitialFirstResponder(Some(&ui_hotkey_field));

        Self {
            window,
            scroll_view,
            path_text_field,
            status_text_field,
            ui_start_on_login_checkbox,
            ui_hotkey_field,
            ui_hotkey_capture_button,
            ui_font_name_popup,
            ui_font_size_field,
            ui_footer_font_size_field,
            ui_meter_style_popup,
            mic_audio_device_popup,
            mic_audio_device_options: RefCell::new(Vec::new()),
            mic_sample_rate_field,
            mic_gain_slider,
            mic_gain_label_field,
            ui_meter_view,
            mic_hold_ms_field,
            deepgram_api_key_field,
            deepgram_api_key_env_hint_field,
            deepgram_project_id_field,
            deepgram_project_id_env_hint_field,
            deepgram_language_field,
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
            available_font_family_names,
            hotkey_capture_restore_value: RefCell::new(None),
        }
    }

    pub fn set_delegate(&self, delegate: &ProtocolObject<dyn NSWindowDelegate>) {
        self.window.setDelegate(Some(delegate));
    }

    pub fn show(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
        self.window.makeKeyAndOrderFront(None);
        let _ = self.window.makeFirstResponder(Some(&*self.ui_hotkey_field));
        self.scroll_to_top();
    }

    pub fn show_startup(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
        let _ = self.window.makeFirstResponder(Some(&*self.ui_hotkey_field));
        self.scroll_to_top();
    }

    pub fn hide(&self) {
        self.window.orderOut(None);
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    pub fn update_meter(&self, mic_meter: MicMeterSnapshot) {
        self.ui_meter_view.update(mic_meter, 180.0);
    }

    pub fn mic_gain_slider_value(&self) -> f32 {
        self.mic_gain_slider.doubleValue() as f32 * 1.5
    }

    pub fn update_mic_gain_label(&self, gain: f32) {
        self.mic_gain_label_field
            .setStringValue(&NSString::from_str(&format!("{:.1}", gain / 1.5)));
    }

    pub fn load_from_config(&self, config: &Config, config_path: &str) {
        let audio_device_status_message = self
            .populate_mic_audio_device_popup(config.mic.audio_device.as_deref())
            .err()
            .map(|error| format!("Couldn't refresh audio devices: {}", error));

        self.path_text_field
            .setStringValue(&NSString::from_str(config_path));
        self.ui_start_on_login_checkbox
            .setState(if config.ui.start_on_login {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        self.ui_hotkey_field
            .setStringValue(&NSString::from_str(&config.ui.hotkey));
        populate_font_name_popup(
            &self.ui_font_name_popup,
            &self.available_font_family_names,
            config.ui.font_name.as_deref(),
        );
        self.ui_font_size_field
            .setStringValue(&NSString::from_str(&config.ui.font_size.to_string()));
        self.ui_footer_font_size_field
            .setStringValue(&NSString::from_str(
                &config
                    .ui
                    .footer_font_size
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            ));
        populate_meter_style_popup(&self.ui_meter_style_popup, config.ui.meter_style);

        self.mic_sample_rate_field
            .setStringValue(&NSString::from_str(&config.mic.sample_rate.to_string()));
        self.mic_gain_slider
            .setDoubleValue((config.mic.gain / 1.5) as f64);
        self.mic_gain_label_field
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
        self.set_status(audio_device_status_message.as_deref().unwrap_or(""));
        self.scroll_to_top();
    }

    pub fn read_config(&self) -> Result<Config, String> {
        Ok(Config {
            ui: crate::config::UiConfig {
                start_on_login: self.ui_start_on_login_checkbox.state() == NSControlStateValueOn,
                hotkey: read_required_string(&self.ui_hotkey_field, "Record hotkey")?,
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
            },
            deepgram: crate::config::DeepgramConfig {
                api_key: read_optional_string(&self.deepgram_api_key_field),
                project_id: read_optional_string(&self.deepgram_project_id_field),
                language: read_required_string(&self.deepgram_language_field, "Deepgram language")?,
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
            },
        })
    }

    pub fn set_status(&self, message: &str) {
        self.status_text_field
            .setStringValue(&NSString::from_str(message));
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

    fn mic_audio_device_value(&self) -> Option<String> {
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

fn add_section_title(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: f64,
    title: &str,
) -> f64 {
    let title_y = current_y - SECTION_BREAK_GAP;
    let title_field = make_section_title(mtm, title);
    set_view_frame(
        &*title_field,
        HORIZONTAL_PADDING,
        title_y,
        260.0,
        SECTION_HEIGHT,
    );
    content_view.addSubview(&title_field);
    title_y - SECTION_HEIGHT - SECTION_TITLE_BOTTOM_GAP
}

fn make_section_title(mtm: MainThreadMarker, title: &str) -> Retained<NSTextField> {
    let title_field = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    title_field.setFont(Some(&settings_section_title_font()));
    title_field.setTextColor(Some(&NSColor::labelColor()));
    title_field
}

fn add_labeled_text_field(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> Retained<NSTextField> {
    let text_field_y = *current_y - 2.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(text_field_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    configure_input_border(&text_field);
    set_view_frame(
        &*text_field,
        FIELD_X,
        text_field_y,
        FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    text_field
}

fn add_labeled_slider_with_meter(
    content_view: &NSView,
    target: &AnyObject,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
    slider_action: objc2::runtime::Sel,
) -> (Retained<NSTextField>, Retained<NSSlider>, UiMeterView) {
    let base_y = *current_y - 2.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(base_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let slider =
        unsafe { NSSlider::sliderWithTarget_action(Some(target), Some(slider_action), mtm) };
    slider.setMinValue(0.0);
    slider.setMaxValue(10.0);
    slider.setContinuous(true);
    let slider_width = 150.0;
    set_view_frame(&*slider, FIELD_X, base_y, slider_width, FIELD_HEIGHT);
    content_view.addSubview(&slider);

    let value_field = NSTextField::labelWithString(&NSString::from_str("3.0"), mtm);
    value_field.setFont(Some(&settings_font()));
    let value_width = 40.0;
    set_view_frame(
        &*value_field,
        FIELD_X + slider_width + 10.0,
        vertically_centered_label_y(base_y, FIELD_HEIGHT),
        value_width,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&value_field);

    let meter = UiMeterView::new(mtm, UiMeterStyle::AnimatedColor);
    let meter_height = crate::ui_meter::meter_container_height(UiMeterStyle::AnimatedColor);
    let meter_width = 180.0;
    set_view_frame(
        meter.view(),
        FIELD_X + slider_width + 10.0 + value_width + 10.0,
        base_y + (FIELD_HEIGHT - meter_height) / 2.0,
        meter_width,
        meter_height,
    );
    content_view.addSubview(meter.view());

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    (value_field, slider, meter)
}

fn add_labeled_text_field_with_button(
    content_view: &NSView,
    target: &AnyObject,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
    button_title: &str,
    action: objc2::runtime::Sel,
) -> (Retained<NSTextField>, Retained<NSButton>) {
    let text_field_y = *current_y - 2.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(text_field_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    configure_input_border(&text_field);
    set_view_frame(
        &*text_field,
        FIELD_X,
        text_field_y,
        HOTKEY_FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(button_title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    button.setFont(Some(&settings_font()));
    set_view_frame(
        &*button,
        FIELD_X + HOTKEY_FIELD_WIDTH + CAPTURE_BUTTON_GAP,
        *current_y - 4.0,
        CAPTURE_BUTTON_WIDTH,
        FIELD_HEIGHT + 4.0,
    );
    content_view.addSubview(&button);

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    (text_field, button)
}

fn add_labeled_combo_box_with_buttons(
    content_view: &NSView,
    target: &AnyObject,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
    leading_button_title: &str,
    leading_action: objc2::runtime::Sel,
    trailing_button_title: &str,
    trailing_action: objc2::runtime::Sel,
) -> (Retained<NSComboBox>, Retained<NSButton>, Retained<NSButton>) {
    let combo_box_y = *current_y - 2.0;
    let button_y = *current_y - 4.0;
    let button_height = FIELD_HEIGHT + 4.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(combo_box_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let combo_box = NSComboBox::initWithFrame(
        NSComboBox::alloc(mtm),
        NSRect::new(
            NSPoint::new(FIELD_X, combo_box_y),
            NSSize::new(MODEL_COMBO_BOX_WIDTH, FIELD_HEIGHT),
        ),
    );
    combo_box.setFont(Some(&settings_font()));
    combo_box.setCompletes(true);
    combo_box.setNumberOfVisibleItems(20);
    configure_input_border(&combo_box);
    content_view.addSubview(&combo_box);

    let leading_button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(leading_button_title),
            Some(target),
            Some(leading_action),
            mtm,
        )
    };
    leading_button.setFont(Some(&settings_font()));
    set_view_frame(
        &*leading_button,
        FIELD_X + MODEL_COMBO_BOX_WIDTH + MODEL_ACTION_BUTTON_GAP,
        button_y,
        MODEL_ACTION_BUTTON_WIDTH,
        button_height,
    );
    content_view.addSubview(&leading_button);

    let trailing_button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(trailing_button_title),
            Some(target),
            Some(trailing_action),
            mtm,
        )
    };
    trailing_button.setFont(Some(&settings_font()));
    set_view_frame(
        &*trailing_button,
        FIELD_X
            + MODEL_COMBO_BOX_WIDTH
            + MODEL_ACTION_BUTTON_GAP
            + MODEL_ACTION_BUTTON_WIDTH
            + MODEL_ACTION_BUTTON_GAP,
        button_y,
        MODEL_ACTION_BUTTON_WIDTH,
        button_height,
    );
    content_view.addSubview(&trailing_button);

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    (combo_box, leading_button, trailing_button)
}

fn add_labeled_pop_up_button(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> Retained<NSPopUpButton> {
    let popup_button_y = *current_y - 3.0;
    let popup_button_height = FIELD_HEIGHT + 6.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(popup_button_y, popup_button_height),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let popup_button = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(
            NSPoint::new(FIELD_X, popup_button_y),
            NSSize::new(FIELD_WIDTH, popup_button_height),
        ),
        false,
    );
    popup_button.setFont(Some(&settings_font()));
    configure_input_border(&popup_button);
    content_view.addSubview(&popup_button);

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    popup_button
}

fn add_labeled_text_field_with_hint_and_button(
    content_view: &NSView,
    target: &AnyObject,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
    button_title: &str,
    action: objc2::runtime::Sel,
) -> (
    Retained<NSTextField>,
    Retained<NSButton>,
    Retained<NSTextField>,
) {
    let text_field_y = *current_y - 2.0;
    let button_y = *current_y - 4.0;
    let button_height = FIELD_HEIGHT + 4.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(text_field_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    configure_input_border(&text_field);
    set_view_frame(
        &*text_field,
        FIELD_X,
        text_field_y,
        FIELD_WITH_ACTION_BUTTON_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(button_title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    button.setFont(Some(&settings_font()));
    set_view_frame(
        &*button,
        FIELD_X + FIELD_WITH_ACTION_BUTTON_WIDTH + MODEL_ACTION_BUTTON_GAP,
        button_y,
        MODEL_ACTION_BUTTON_WIDTH,
        button_height,
    );
    content_view.addSubview(&button);

    let hint_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    configure_hint_label(&hint_field);
    hint_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &hint_field,
        FIELD_X,
        *current_y - ENV_HINT_HEIGHT - 4.0,
        FIELD_WIDTH,
        ENV_HINT_HEIGHT,
    );
    content_view.addSubview(&hint_field);

    *current_y -= FIELD_HEIGHT + ENV_HINT_HEIGHT + ROW_GAP;
    (text_field, button, hint_field)
}

fn add_labeled_text_field_with_hint(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> (Retained<NSTextField>, Retained<NSTextField>) {
    let text_field_y = *current_y - 2.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(text_field_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    configure_input_border(&text_field);
    set_view_frame(
        &*text_field,
        FIELD_X,
        text_field_y,
        FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    let hint_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    configure_hint_label(&hint_field);
    hint_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &hint_field,
        FIELD_X,
        *current_y - ENV_HINT_HEIGHT - 4.0,
        FIELD_WIDTH,
        ENV_HINT_HEIGHT,
    );
    content_view.addSubview(&hint_field);

    *current_y -= FIELD_HEIGHT + ENV_HINT_HEIGHT + ROW_GAP;
    (text_field, hint_field)
}

fn add_checkbox(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    title: &str,
) -> Retained<NSButton> {
    let checkbox = unsafe {
        NSButton::checkboxWithTitle_target_action(&NSString::from_str(title), None, None, mtm)
    };
    checkbox.setFont(Some(&settings_font()));
    set_view_frame(
        &*checkbox,
        FIELD_X,
        *current_y - 2.0,
        FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&checkbox);
    *current_y -= FIELD_HEIGHT + ROW_GAP;
    checkbox
}

fn add_prompt_editor(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> Retained<NSTextView> {
    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        *current_y,
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let prompt_scroll_view = NSScrollView::initWithFrame(
        NSScrollView::alloc(mtm),
        NSRect::new(
            NSPoint::new(FIELD_X, *current_y - PROMPT_HEIGHT + 20.0),
            NSSize::new(FIELD_WIDTH, PROMPT_HEIGHT),
        ),
    );
    prompt_scroll_view.setHasVerticalScroller(true);
    prompt_scroll_view.setHasHorizontalScroller(false);
    prompt_scroll_view.setDrawsBackground(true);
    configure_input_border(&prompt_scroll_view);
    let prompt_view = NSTextView::initWithFrame(
        NSTextView::alloc(mtm),
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(FIELD_WIDTH, PROMPT_HEIGHT),
        ),
    );
    prompt_view.setEditable(true);
    prompt_view.setSelectable(true);
    prompt_view.setDrawsBackground(true);
    prompt_view.setBackgroundColor(&NSColor::textBackgroundColor());
    prompt_view.setFont(Some(&settings_font()));
    prompt_view.setHorizontallyResizable(false);
    prompt_view.setVerticallyResizable(true);
    prompt_scroll_view.setDocumentView(Some(&prompt_view));
    content_view.addSubview(&prompt_scroll_view);

    *current_y -= PROMPT_HEIGHT + SECTION_GAP;
    prompt_view
}

fn configure_wrapping_label(label: &NSTextField) {
    label.setDrawsBackground(false);
    label.setBordered(false);
    label.setBezeled(false);
    label.setEditable(false);
    label.setSelectable(false);
    label.setFont(Some(&settings_font()));
    if let Some(cell) = label.cell() {
        cell.setAlignment(NSTextAlignment::Left);
    }
}

fn configure_hint_label(label: &NSTextField) {
    label.setDrawsBackground(false);
    label.setBordered(false);
    label.setBezeled(false);
    label.setEditable(false);
    label.setSelectable(false);
    label.setFont(Some(&settings_hint_font()));
    if let Some(cell) = label.cell() {
        cell.setAlignment(NSTextAlignment::Left);
    }
}

pub(crate) fn settings_font() -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(SETTINGS_FONT_SIZE, SETTINGS_FONT_WEIGHT)
}

fn settings_hint_font() -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(
        SETTINGS_FONT_SIZE * SETTINGS_HINT_FONT_SCALE,
        SETTINGS_FONT_WEIGHT,
    )
}

fn settings_section_title_font() -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(
        SETTINGS_FONT_SIZE,
        SETTINGS_SECTION_TITLE_FONT_WEIGHT,
    )
}

fn input_border_color() -> Retained<NSColor> {
    NSColor::separatorColor()
        .highlightWithLevel(INPUT_BORDER_HIGHLIGHT_LEVEL)
        .unwrap_or_else(NSColor::separatorColor)
}

fn configure_input_border(view: &AnyObject) {
    unsafe {
        let _: () = msg_send![view, setWantsLayer: true];
        let layer: Option<Retained<CALayer>> = msg_send![view, layer];
        let Some(layer) = layer else {
            return;
        };

        layer.setCornerRadius(INPUT_CORNER_RADIUS);
        layer.setBorderWidth(INPUT_BORDER_WIDTH);
        let border_color = input_border_color();
        let border_cg_color = border_color.CGColor();
        layer.setBorderColor(Some(&border_cg_color));
    }
}

fn available_font_family_names(mtm: MainThreadMarker) -> Vec<String> {
    let font_manager = NSFontManager::sharedFontManager(mtm);
    let font_families = font_manager.availableFontFamilies();
    let mut names = (0..font_families.count())
        .map(|index| font_families.objectAtIndex(index).to_string())
        .collect::<Vec<_>>();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names.dedup();
    names
}

fn populate_font_name_popup(
    popup_button: &NSPopUpButton,
    available_font_family_names: &[String],
    selected_font_name: Option<&str>,
) {
    popup_button.removeAllItems();
    popup_button.addItemWithTitle(&NSString::from_str(SYSTEM_DEFAULT_FONT_LABEL));
    for font_name in available_font_family_names {
        popup_button.addItemWithTitle(&NSString::from_str(font_name));
    }

    let Some(selected_font_name) = selected_font_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        popup_button.selectItemAtIndex(0);
        return;
    };

    let selected_font_name = NSString::from_str(selected_font_name);
    if popup_button.indexOfItemWithTitle(&selected_font_name) < 0 {
        popup_button.insertItemWithTitle_atIndex(&selected_font_name, 1);
    }
    popup_button.selectItemWithTitle(&selected_font_name);
}

fn mic_audio_device_popup_state(
    available_audio_input_devices: AvailableAudioInputDevices,
    configured_audio_device: Option<&str>,
) -> (Vec<MicAudioDeviceOption>, String) {
    let default_audio_device_title =
        default_audio_device_title(available_audio_input_devices.default_device_name.as_deref());
    let mut audio_device_options = vec![MicAudioDeviceOption {
        title: default_audio_device_title.clone(),
        value: None,
    }];
    audio_device_options.extend(
        available_audio_input_devices
            .choices
            .into_iter()
            .map(|choice| MicAudioDeviceOption {
                title: choice.label,
                value: Some(choice.value),
            }),
    );

    let Some(configured_audio_device) = configured_audio_device
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return (audio_device_options, default_audio_device_title);
    };

    if let Some(selected_audio_device_title) =
        find_mic_audio_device_option_title(&audio_device_options, configured_audio_device)
            .map(str::to_owned)
    {
        return (audio_device_options, selected_audio_device_title);
    }

    audio_device_options.insert(
        1,
        MicAudioDeviceOption {
            title: configured_audio_device.to_owned(),
            value: Some(configured_audio_device.to_owned()),
        },
    );
    (audio_device_options, configured_audio_device.to_owned())
}

fn find_mic_audio_device_option_title<'a>(
    audio_device_options: &'a [MicAudioDeviceOption],
    configured_audio_device: &str,
) -> Option<&'a str> {
    if is_system_default_audio_device_value(configured_audio_device) {
        return audio_device_options
            .first()
            .map(|option| option.title.as_str());
    }

    audio_device_options
        .iter()
        .find(|option| {
            option.title == configured_audio_device
                || option.value.as_deref() == Some(configured_audio_device)
                || option
                    .value
                    .as_deref()
                    .map(|value| value.eq_ignore_ascii_case(configured_audio_device))
                    .unwrap_or(false)
        })
        .map(|option| option.title.as_str())
}

fn is_system_default_audio_device_value(value: &str) -> bool {
    let trimmed_value = value.trim();
    trimmed_value == SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL
        || trimmed_value
            .strip_prefix(SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL)
            .map(|suffix| suffix.starts_with(" (") && suffix.ends_with(')'))
            .unwrap_or(false)
}

fn default_audio_device_title(default_device_name: Option<&str>) -> String {
    match default_device_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(default_device_name) => {
            format!(
                "{} ({})",
                SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL, default_device_name
            )
        }
        None => SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL.to_owned(),
    }
}

fn populate_meter_style_popup(popup_button: &NSPopUpButton, selected_meter_style: UiMeterStyle) {
    popup_button.removeAllItems();
    popup_button.addItemWithTitle(ns_string!("animated-color"));
    popup_button.addItemWithTitle(ns_string!("animated-height"));
    popup_button.addItemWithTitle(ns_string!("none"));

    let selected_title = NSString::from_str(match selected_meter_style {
        UiMeterStyle::AnimatedColor => "animated-color",
        UiMeterStyle::AnimatedHeight => "animated-height",
        UiMeterStyle::None => "none",
    });
    popup_button.selectItemWithTitle(&selected_title);
}

fn populate_combo_box_with_values(combo_box: &NSComboBox, values: &[String], selected_value: &str) {
    combo_box.removeAllItems();
    for value in values {
        let string = NSString::from_str(value);
        unsafe {
            combo_box.addItemWithObjectValue(&*string);
        }
    }
    combo_box.setStringValue(&NSString::from_str(selected_value.trim()));
}

fn populate_transformation_provider_popup(
    popup_button: &NSPopUpButton,
    selected_provider: Option<&str>,
) {
    popup_button.removeAllItems();
    popup_button.addItemWithTitle(&NSString::from_str(TRANSFORMATION_PROVIDER_DISABLED_LABEL));
    for provider in crate::config::supported_transformation_providers() {
        popup_button.addItemWithTitle(&NSString::from_str(provider));
    }

    let Some(selected_provider) = selected_provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        popup_button.selectItemAtIndex(0);
        return;
    };

    let selected_provider = NSString::from_str(selected_provider);
    if popup_button.indexOfItemWithTitle(&selected_provider) < 0 {
        popup_button.insertItemWithTitle_atIndex(&selected_provider, 1);
    }
    popup_button.selectItemWithTitle(&selected_provider);
}

fn populate_deepgram_model_popup(popup_button: &NSPopUpButton, selected_model: &str) {
    popup_button.removeAllItems();
    for model in DEEPGRAM_MODEL_OPTIONS {
        popup_button.addItemWithTitle(&NSString::from_str(model));
    }

    let selected_model = selected_model.trim();
    if selected_model.is_empty() {
        popup_button.selectItemWithTitle(&NSString::from_str("nova-3"));
        return;
    }

    let selected_model = NSString::from_str(selected_model);
    if popup_button.indexOfItemWithTitle(&selected_model) < 0 {
        popup_button.insertItemWithTitle_atIndex(&selected_model, 0);
    }
    popup_button.selectItemWithTitle(&selected_model);
}

fn environment_hint_message(variable_name: &str) -> String {
    format!("Using ${} from environment.", variable_name)
}

fn set_capture_button_state(button: &NSButton, is_active: bool, is_enabled: bool) {
    button.setTitle(if is_active {
        ns_string!("Capturing…")
    } else {
        ns_string!("Capture…")
    });
    button.setEnabled(is_enabled);
}

fn set_hint_text(label: &NSTextField, message: Option<String>) {
    label.setStringValue(&NSString::from_str(message.as_deref().unwrap_or("")));
}

fn vertically_centered_label_y(control_y: f64, control_height: f64) -> f64 {
    control_y + ((control_height - FIELD_HEIGHT) / 2.0) + LABEL_VISUAL_CENTER_NUDGE
}

fn set_view_frame(view: &AnyObject, x: f64, y: f64, width: f64, height: f64) {
    unsafe {
        let _: () = objc2::msg_send![view, setFrame: NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))];
    }
}

fn read_required_string(field: &NSTextField, field_name: &str) -> Result<String, String> {
    let value = field.stringValue().to_string();
    let trimmed_value = value.trim();
    if trimmed_value.is_empty() {
        return Err(format!("{} is required", field_name));
    }
    Ok(trimmed_value.to_owned())
}

fn read_optional_string(field: &NSTextField) -> Option<String> {
    let value = field.stringValue().to_string();
    let trimmed_value = value.trim();
    if trimmed_value.is_empty() {
        None
    } else {
        Some(trimmed_value.to_owned())
    }
}

fn read_optional_pop_up_button_string(popup_button: &NSPopUpButton) -> Option<String> {
    let selected_title = popup_button.titleOfSelectedItem()?.to_string();
    let trimmed_value = selected_title.trim();
    if trimmed_value.is_empty() || trimmed_value == SYSTEM_DEFAULT_FONT_LABEL {
        None
    } else {
        Some(trimmed_value.to_owned())
    }
}

fn read_optional_provider_pop_up_button_string(popup_button: &NSPopUpButton) -> Option<String> {
    let selected_title = popup_button.titleOfSelectedItem()?.to_string();
    let trimmed_value = selected_title.trim();
    if trimmed_value.is_empty() || trimmed_value == TRANSFORMATION_PROVIDER_DISABLED_LABEL {
        None
    } else {
        Some(trimmed_value.to_owned())
    }
}

fn read_required_combo_box_string(
    combo_box: &NSComboBox,
    field_name: &str,
) -> Result<String, String> {
    let value = combo_box.stringValue().to_string();
    let trimmed_value = value.trim();
    if trimmed_value.is_empty() {
        Err(format!("{} is required", field_name))
    } else {
        Ok(trimmed_value.to_owned())
    }
}

fn read_required_pop_up_button_string(
    popup_button: &NSPopUpButton,
    field_name: &str,
) -> Result<String, String> {
    let selected_title = popup_button
        .titleOfSelectedItem()
        .ok_or_else(|| format!("{} is required", field_name))?
        .to_string();
    let trimmed_value = selected_title.trim();
    if trimmed_value.is_empty() {
        Err(format!("{} is required", field_name))
    } else {
        Ok(trimmed_value.to_owned())
    }
}

fn read_required_f64(field: &NSTextField, field_name: &str) -> Result<f64, String> {
    read_required_string(field, field_name)?
        .parse::<f64>()
        .map_err(|error| format!("{} must be a number: {}", field_name, error))
}

fn read_optional_f64(field: &NSTextField, field_name: &str) -> Result<Option<f64>, String> {
    match read_optional_string(field) {
        Some(value) => value
            .parse::<f64>()
            .map(Some)
            .map_err(|error| format!("{} must be a number: {}", field_name, error)),
        None => Ok(None),
    }
}

fn read_required_u32(field: &NSTextField, field_name: &str) -> Result<u32, String> {
    read_required_string(field, field_name)?
        .parse::<u32>()
        .map_err(|error| format!("{} must be an unsigned integer: {}", field_name, error))
}

fn read_required_u64(field: &NSTextField, field_name: &str) -> Result<u64, String> {
    read_required_string(field, field_name)?
        .parse::<u64>()
        .map_err(|error| format!("{} must be an unsigned integer: {}", field_name, error))
}

fn read_required_u16(field: &NSTextField, field_name: &str) -> Result<u16, String> {
    read_required_string(field, field_name)?
        .parse::<u16>()
        .map_err(|error| format!("{} must be an unsigned integer: {}", field_name, error))
}

fn parse_meter_style(raw_value: &str) -> Result<UiMeterStyle, String> {
    match raw_value.trim() {
        "none" => Ok(UiMeterStyle::None),
        "animated-height" => Ok(UiMeterStyle::AnimatedHeight),
        "animated-color" => Ok(UiMeterStyle::AnimatedColor),
        other_value => Err(format!(
            "Meter style must be one of: animated-color, animated-height, none (got '{}')",
            other_value
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        default_audio_device_title, find_mic_audio_device_option_title,
        is_system_default_audio_device_value, mic_audio_device_popup_state, MicAudioDeviceOption,
        SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL,
    };
    use crate::audio::{AudioInputDeviceChoice, AvailableAudioInputDevices};

    #[test]
    fn default_audio_device_title_includes_detected_default_name() {
        assert_eq!(
            default_audio_device_title(Some("MacBook Pro Microphone")),
            "System default (MacBook Pro Microphone)"
        );
    }

    #[test]
    fn mic_audio_device_popup_state_selects_default_when_unconfigured() {
        let (options, selected_title) = mic_audio_device_popup_state(
            AvailableAudioInputDevices {
                default_device_name: Some("MacBook Pro Microphone".to_owned()),
                choices: vec![AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                }],
            },
            None,
        );

        assert_eq!(
            options,
            vec![
                MicAudioDeviceOption {
                    title: "System default (MacBook Pro Microphone)".to_owned(),
                    value: None,
                },
                MicAudioDeviceOption {
                    title: "Shure MV7".to_owned(),
                    value: Some("Shure MV7".to_owned()),
                },
            ]
        );
        assert_eq!(selected_title, "System default (MacBook Pro Microphone)");
    }

    #[test]
    fn mic_audio_device_popup_state_matches_configured_name_case_insensitively() {
        let (options, selected_title) = mic_audio_device_popup_state(
            AvailableAudioInputDevices {
                default_device_name: None,
                choices: vec![AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                }],
            },
            Some("shure mv7"),
        );

        assert_eq!(
            options,
            vec![
                MicAudioDeviceOption {
                    title: SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL.to_owned(),
                    value: None,
                },
                MicAudioDeviceOption {
                    title: "Shure MV7".to_owned(),
                    value: Some("Shure MV7".to_owned()),
                },
            ]
        );
        assert_eq!(selected_title, "Shure MV7");
    }

    #[test]
    fn mic_audio_device_popup_state_inserts_missing_configured_value() {
        let (options, selected_title) = mic_audio_device_popup_state(
            AvailableAudioInputDevices {
                default_device_name: None,
                choices: vec![AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                }],
            },
            Some("Missing Mic"),
        );

        assert_eq!(
            options,
            vec![
                MicAudioDeviceOption {
                    title: SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL.to_owned(),
                    value: None,
                },
                MicAudioDeviceOption {
                    title: "Missing Mic".to_owned(),
                    value: Some("Missing Mic".to_owned()),
                },
                MicAudioDeviceOption {
                    title: "Shure MV7".to_owned(),
                    value: Some("Shure MV7".to_owned()),
                },
            ]
        );
        assert_eq!(selected_title, "Missing Mic");
    }

    #[test]
    fn system_default_audio_device_value_is_recognized_from_decorated_title() {
        assert!(is_system_default_audio_device_value(
            "System default (MacBook Pro Microphone)"
        ));
        assert!(is_system_default_audio_device_value("System default"));
        assert!(!is_system_default_audio_device_value("Shure MV7"));
    }

    #[test]
    fn popup_state_maps_legacy_decorated_default_value_back_to_default_option() {
        let options = vec![
            MicAudioDeviceOption {
                title: "System default (MacBook Pro Microphone)".to_owned(),
                value: None,
            },
            MicAudioDeviceOption {
                title: "Shure MV7".to_owned(),
                value: Some("Shure MV7".to_owned()),
            },
        ];

        assert_eq!(
            find_mic_audio_device_option_title(&options, "System default (MacBook Pro Microphone)"),
            Some("System default (MacBook Pro Microphone)")
        );
    }
}
