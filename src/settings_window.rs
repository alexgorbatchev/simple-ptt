use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{sel, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSButton, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSScrollView,
    NSTextAlignment, NSTextField, NSTextView, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use crate::config::{Config, UiMeterStyle};
use crate::hotkey_capture::HotkeyCaptureTarget;

const WINDOW_HEIGHT: f64 = 760.0;
const WINDOW_WIDTH: f64 = 760.0;
const CONTENT_HEIGHT: f64 = 1480.0;
const HORIZONTAL_PADDING: f64 = 20.0;
const LABEL_WIDTH: f64 = 180.0;
const FIELD_HEIGHT: f64 = 24.0;
const FIELD_WIDTH: f64 = 500.0;
const HOTKEY_FIELD_WIDTH: f64 = 390.0;
const CAPTURE_BUTTON_WIDTH: f64 = 100.0;
const CAPTURE_BUTTON_GAP: f64 = 10.0;
const FIELD_X: f64 = HORIZONTAL_PADDING + LABEL_WIDTH + 12.0;
const ROW_GAP: f64 = 10.0;
const SECTION_GAP: f64 = 20.0;
const SECTION_HEIGHT: f64 = 22.0;
const PROMPT_HEIGHT: f64 = 270.0;
const STATUS_HEIGHT: f64 = 40.0;
const ENV_HINT_HEIGHT: f64 = 18.0;
const SETTINGS_FONT_SIZE: f64 = 12.0;
const SETTINGS_FONT_WEIGHT: f64 = 0.0;

#[derive(Debug)]
pub struct SettingsWindow {
    window: Retained<NSWindow>,
    scroll_view: Retained<NSScrollView>,
    path_text_field: Retained<NSTextField>,
    status_text_field: Retained<NSTextField>,
    ui_hotkey_field: Retained<NSTextField>,
    ui_hotkey_capture_button: Retained<NSButton>,
    ui_font_name_field: Retained<NSTextField>,
    ui_font_size_field: Retained<NSTextField>,
    ui_footer_font_size_field: Retained<NSTextField>,
    ui_meter_style_field: Retained<NSTextField>,
    mic_audio_device_field: Retained<NSTextField>,
    mic_sample_rate_field: Retained<NSTextField>,
    mic_gain_field: Retained<NSTextField>,
    mic_hold_ms_field: Retained<NSTextField>,
    deepgram_api_key_field: Retained<NSTextField>,
    deepgram_api_key_env_hint_field: Retained<NSTextField>,
    deepgram_project_id_field: Retained<NSTextField>,
    deepgram_project_id_env_hint_field: Retained<NSTextField>,
    deepgram_language_field: Retained<NSTextField>,
    deepgram_model_field: Retained<NSTextField>,
    deepgram_endpointing_ms_field: Retained<NSTextField>,
    deepgram_utterance_end_ms_field: Retained<NSTextField>,
    transformation_hotkey_field: Retained<NSTextField>,
    transformation_hotkey_capture_button: Retained<NSButton>,
    transformation_auto_checkbox: Retained<NSButton>,
    transformation_provider_field: Retained<NSTextField>,
    transformation_api_key_field: Retained<NSTextField>,
    transformation_api_key_env_hint_field: Retained<NSTextField>,
    transformation_model_field: Retained<NSTextField>,
    transformation_system_prompt_view: Retained<NSTextView>,
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

        let mut current_y = CONTENT_HEIGHT - 28.0;
        let path_title = make_section_title(mtm, "Config file");
        set_view_frame(
            &*path_title,
            HORIZONTAL_PADDING,
            current_y,
            220.0,
            SECTION_HEIGHT,
        );
        content_view.addSubview(&path_title);
        current_y -= SECTION_HEIGHT + 6.0;

        let path_text_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        configure_wrapping_label(&path_text_field);
        set_view_frame(
            &path_text_field,
            HORIZONTAL_PADDING,
            current_y - 32.0,
            WINDOW_WIDTH - (HORIZONTAL_PADDING * 2.0),
            36.0,
        );
        content_view.addSubview(&path_text_field);
        current_y -= 54.0;

        current_y = add_section_title(&content_view, mtm, current_y, "UI");
        let (ui_hotkey_field, ui_hotkey_capture_button) = add_labeled_text_field_with_button(
            &content_view,
            target,
            mtm,
            &mut current_y,
            "Record hotkey",
            "Capture…",
            sel!(captureRecordHotkey:),
        );
        let ui_font_name_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Font name");
        let ui_font_size_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Font size");
        let ui_footer_font_size_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Footer font size");
        let ui_meter_style_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Meter style");

        current_y = add_section_title(&content_view, mtm, current_y, "Microphone");
        let mic_audio_device_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Audio device");
        let mic_sample_rate_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Sample rate");
        let mic_gain_field = add_labeled_text_field(&content_view, mtm, &mut current_y, "Gain");
        let mic_hold_ms_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Hold ms");

        current_y = add_section_title(&content_view, mtm, current_y, "Deepgram");
        let (deepgram_api_key_field, deepgram_api_key_env_hint_field) =
            add_labeled_text_field_with_hint(&content_view, mtm, &mut current_y, "API key");
        let (deepgram_project_id_field, deepgram_project_id_env_hint_field) =
            add_labeled_text_field_with_hint(&content_view, mtm, &mut current_y, "Project ID");
        let deepgram_language_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Language");
        let deepgram_model_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Model");
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
        let transformation_provider_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Provider");
        let (transformation_api_key_field, transformation_api_key_env_hint_field) =
            add_labeled_text_field_with_hint(&content_view, mtm, &mut current_y, "API key");
        let transformation_model_field =
            add_labeled_text_field(&content_view, mtm, &mut current_y, "Model");
        let transformation_system_prompt_view =
            add_prompt_editor(&content_view, mtm, &mut current_y, "System prompt");

        let save_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Save and Apply"),
                Some(target),
                Some(sel!(saveSettings:)),
                mtm,
            )
        };
        set_view_frame(&*save_button, WINDOW_WIDTH - 170.0, 10.0, 150.0, 30.0);
        root_view.addSubview(&save_button);

        let status_text_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        configure_wrapping_label(&status_text_field);
        status_text_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
        set_view_frame(
            &*status_text_field,
            20.0,
            6.0,
            WINDOW_WIDTH - 200.0,
            STATUS_HEIGHT,
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
            ui_hotkey_field,
            ui_hotkey_capture_button,
            ui_font_name_field,
            ui_font_size_field,
            ui_footer_font_size_field,
            ui_meter_style_field,
            mic_audio_device_field,
            mic_sample_rate_field,
            mic_gain_field,
            mic_hold_ms_field,
            deepgram_api_key_field,
            deepgram_api_key_env_hint_field,
            deepgram_project_id_field,
            deepgram_project_id_env_hint_field,
            deepgram_language_field,
            deepgram_model_field,
            deepgram_endpointing_ms_field,
            deepgram_utterance_end_ms_field,
            transformation_hotkey_field,
            transformation_hotkey_capture_button,
            transformation_auto_checkbox,
            transformation_provider_field,
            transformation_api_key_field,
            transformation_api_key_env_hint_field,
            transformation_model_field,
            transformation_system_prompt_view,
        }
    }

    pub fn show(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.activate();
        self.window.makeKeyAndOrderFront(None);
        let _ = self.window.makeFirstResponder(Some(&*self.ui_hotkey_field));
        self.scroll_to_top();
    }

    pub fn load_from_config(&self, config: &Config, config_path: &str) {
        self.path_text_field
            .setStringValue(&NSString::from_str(config_path));
        self.ui_hotkey_field
            .setStringValue(&NSString::from_str(&config.ui.hotkey));
        self.ui_font_name_field.setStringValue(&NSString::from_str(
            config.ui.font_name.as_deref().unwrap_or(""),
        ));
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
        self.ui_meter_style_field
            .setStringValue(&NSString::from_str(match config.ui.meter_style {
                UiMeterStyle::None => "none",
                UiMeterStyle::AnimatedHeight => "animated-height",
                UiMeterStyle::AnimatedColor => "animated-color",
            }));

        self.mic_audio_device_field
            .setStringValue(&NSString::from_str(
                config.mic.audio_device.as_deref().unwrap_or(""),
            ));
        self.mic_sample_rate_field
            .setStringValue(&NSString::from_str(&config.mic.sample_rate.to_string()));
        self.mic_gain_field
            .setStringValue(&NSString::from_str(&config.mic.gain.to_string()));
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
        self.deepgram_model_field
            .setStringValue(&NSString::from_str(&config.deepgram.model));
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
        self.transformation_provider_field
            .setStringValue(&NSString::from_str(
                config.transformation.provider.as_deref().unwrap_or(""),
            ));
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
        self.transformation_model_field
            .setStringValue(&NSString::from_str(&config.transformation.model));
        self.transformation_system_prompt_view
            .setString(&NSString::from_str(&config.transformation.system_prompt));
        self.set_status("");
        self.scroll_to_top();
    }

    pub fn read_config(&self) -> Result<Config, String> {
        Ok(Config {
            ui: crate::config::UiConfig {
                hotkey: read_required_string(&self.ui_hotkey_field, "Record hotkey")?,
                font_name: read_optional_string(&self.ui_font_name_field),
                font_size: read_required_f64(&self.ui_font_size_field, "Font size")?,
                footer_font_size: read_optional_f64(
                    &self.ui_footer_font_size_field,
                    "Footer font size",
                )?,
                meter_style: parse_meter_style(&read_required_string(
                    &self.ui_meter_style_field,
                    "Meter style",
                )?)?,
            },
            mic: crate::config::MicConfig {
                audio_device: read_optional_string(&self.mic_audio_device_field),
                sample_rate: read_required_u32(&self.mic_sample_rate_field, "Sample rate")?,
                gain: read_required_f32(&self.mic_gain_field, "Gain")?,
                hold_ms: read_required_u64(&self.mic_hold_ms_field, "Hold ms")?,
            },
            deepgram: crate::config::DeepgramConfig {
                api_key: read_optional_string(&self.deepgram_api_key_field),
                project_id: read_optional_string(&self.deepgram_project_id_field),
                language: read_required_string(&self.deepgram_language_field, "Deepgram language")?,
                model: read_required_string(&self.deepgram_model_field, "Deepgram model")?,
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
                provider: read_optional_string(&self.transformation_provider_field),
                api_key: read_optional_string(&self.transformation_api_key_field),
                model: read_required_string(
                    &self.transformation_model_field,
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

    pub fn begin_hotkey_capture(&self, target: HotkeyCaptureTarget) {
        self.set_hotkey_capture_state(Some(target));
        self.set_status("Press a key, or Esc to cancel");
    }

    pub fn finish_hotkey_capture(&self) {
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
    let title_field = make_section_title(mtm, title);
    set_view_frame(
        &*title_field,
        HORIZONTAL_PADDING,
        current_y,
        260.0,
        SECTION_HEIGHT,
    );
    content_view.addSubview(&title_field);
    current_y - SECTION_HEIGHT - ROW_GAP
}

fn make_section_title(mtm: MainThreadMarker, title: &str) -> Retained<NSTextField> {
    let title_field = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    title_field.setFont(Some(&settings_font()));
    title_field.setTextColor(Some(&NSColor::labelColor()));
    title_field
}

fn add_labeled_text_field(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> Retained<NSTextField> {
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

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    set_view_frame(
        &*text_field,
        FIELD_X,
        *current_y - 2.0,
        FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    text_field
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

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    set_view_frame(
        &*text_field,
        FIELD_X,
        *current_y - 2.0,
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

fn add_labeled_text_field_with_hint(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> (Retained<NSTextField>, Retained<NSTextField>) {
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

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    set_view_frame(
        &*text_field,
        FIELD_X,
        *current_y - 2.0,
        FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    let hint_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    configure_wrapping_label(&hint_field);
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
    let prompt_view = NSTextView::initWithFrame(
        NSTextView::alloc(mtm),
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(FIELD_WIDTH, PROMPT_HEIGHT),
        ),
    );
    prompt_view.setEditable(true);
    prompt_view.setSelectable(true);
    prompt_view.setDrawsBackground(false);
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

fn settings_font() -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(SETTINGS_FONT_SIZE, SETTINGS_FONT_WEIGHT)
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

fn read_required_f32(field: &NSTextField, field_name: &str) -> Result<f32, String> {
    read_required_string(field, field_name)?
        .parse::<f32>()
        .map_err(|error| format!("{} must be a number: {}", field_name, error))
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
