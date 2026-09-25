//! The settings window: a toolbar-style `NSTabViewController` with one pane per
//! settings area, above a button bar with the status text and the Cancel and
//! Save buttons, which stay visible whichever pane is selected.
//!
//! Layout is Auto Layout only: each pane is an `NSGridView` form (or a stack of
//! prompt editors), and the window's minimum content size is the fitting size
//! of the largest pane plus the button bar. Values move between the controls
//! and `Config` through the AppKit-free `form::SettingsForm`.

pub mod actions;
mod controls;
pub mod form;
mod grid;
pub mod helpers;
mod panes;
mod popups;

pub use controls::{settings_font, style_button_bezel_and_text};

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSImage,
    NSLayoutAttribute, NSLayoutConstraintOrientation, NSLayoutManager, NSLayoutPriorityDefaultLow,
    NSStackView, NSTabViewController, NSTabViewControllerTabStyle, NSTabViewItem, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSViewController, NSWindow, NSWindowDelegate,
    NSWindowStyleMask, NSWindowToolbarStyle,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSRect, NSSize, NSString};

use crate::config::Config;
use crate::hotkey_capture::HotkeyCaptureTarget;
use crate::state::MicMeterSnapshot;
use actions::SettingsAction;
use controls::{for_auto_layout, push_button, set_capture_button_state, status_label};
use form::SettingsForm;
use helpers::environment_hint_message;
use panes::activate;
use panes::deepgram::{DeepgramEnvironmentHints, DeepgramPane};
use panes::general::GeneralPane;
use panes::microphone::MicrophonePane;
use panes::prompts::PromptsPane;
use panes::transformation::TransformationPane;

/// Title of the button that saves every pane to the config file. Startup
/// alerts name this button, so they build their text from this constant.
pub const SAVE_BUTTON_TITLE: &str = "Save";

const BUTTON_BAR_MARGIN: f64 = 20.0;
const BUTTON_BAR_SPACING: f64 = 12.0;
const STATUS_MAXIMUM_LINES: isize = 3;

#[derive(Debug)]
pub struct SettingsWindow {
    window: Retained<NSWindow>,
    tab_view_controller: Retained<NSTabViewController>,
    general: GeneralPane,
    microphone: MicrophonePane,
    deepgram: DeepgramPane,
    transformation: TransformationPane,
    prompts: PromptsPane,
    status_text_field: Retained<NSTextField>,
    hotkey_capture_restore_value: RefCell<Option<(HotkeyCaptureTarget, String)>>,
}

impl SettingsWindow {
    pub fn new(
        mtm: MainThreadMarker,
        target: &AnyObject,
        delegate: &ProtocolObject<dyn NSWindowDelegate>,
    ) -> Self {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::ZERO,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(ns_string!("Settings"));
        window.setToolbarStyle(NSWindowToolbarStyle::Preference);
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setDelegate(Some(delegate));

        let (general, general_view) = GeneralPane::new(mtm, target);
        let (microphone, microphone_view) = MicrophonePane::new(mtm, target);
        let (deepgram, deepgram_view) = DeepgramPane::new(mtm, target);
        let (transformation, transformation_view) = TransformationPane::new(mtm, target);
        let (prompts, prompts_view) = PromptsPane::new(mtm);
        let pane_views = [
            ("General", "gearshape", general_view),
            ("Microphone", "mic", microphone_view),
            ("Deepgram", "waveform", deepgram_view),
            ("Transformation", "wand.and.stars", transformation_view),
            ("Prompts", "text.bubble", prompts_view),
        ];

        let tab_view_controller = NSTabViewController::new(mtm);
        tab_view_controller.setTabStyle(NSTabViewControllerTabStyle::Toolbar);
        for (title, symbol_name, view) in &pane_views {
            tab_view_controller.addTabViewItem(&pane_tab_view_item(mtm, title, symbol_name, view));
        }

        let status_text_field = status_label(mtm, STATUS_MAXIMUM_LINES);
        let cancel_button = push_button(mtm, "Cancel", target, SettingsAction::CancelSettings);
        let save_button = push_button(mtm, SAVE_BUTTON_TITLE, target, SettingsAction::SaveSettings);
        let button_bar = button_bar(mtm, &status_text_field, &cancel_button, &save_button);

        let root_view = NSView::new(mtm);
        let tab_view = for_auto_layout(tab_view_controller.view());
        let separator = for_auto_layout(NSBox::new(mtm));
        separator.setBoxType(NSBoxType::Separator);
        root_view.addSubview(&tab_view);
        root_view.addSubview(&separator);
        root_view.addSubview(&button_bar);

        // Every pane must fit whichever pane is selected, so the tab view is at
        // least as large as the largest pane's fitting size.
        let largest_pane_size = pane_views.iter().fold(NSSize::ZERO, |size, (_, _, view)| {
            let fitting_size = view.fittingSize();
            NSSize::new(
                size.width.max(fitting_size.width),
                size.height.max(fitting_size.height),
            )
        });
        activate(&[
            tab_view
                .topAnchor()
                .constraintEqualToAnchor(&root_view.topAnchor()),
            tab_view
                .leadingAnchor()
                .constraintEqualToAnchor(&root_view.leadingAnchor()),
            tab_view
                .trailingAnchor()
                .constraintEqualToAnchor(&root_view.trailingAnchor()),
            tab_view
                .widthAnchor()
                .constraintGreaterThanOrEqualToConstant(largest_pane_size.width),
            tab_view
                .heightAnchor()
                .constraintGreaterThanOrEqualToConstant(largest_pane_size.height),
            separator
                .topAnchor()
                .constraintEqualToAnchor(&tab_view.bottomAnchor()),
            separator
                .leadingAnchor()
                .constraintEqualToAnchor(&root_view.leadingAnchor()),
            separator
                .trailingAnchor()
                .constraintEqualToAnchor(&root_view.trailingAnchor()),
            button_bar
                .topAnchor()
                .constraintEqualToAnchor_constant(&separator.bottomAnchor(), BUTTON_BAR_SPACING),
            button_bar
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&root_view.leadingAnchor(), BUTTON_BAR_MARGIN),
            button_bar
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&root_view.trailingAnchor(), -BUTTON_BAR_MARGIN),
            button_bar
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&root_view.bottomAnchor(), -BUTTON_BAR_SPACING),
        ]);

        let root_view_controller = NSViewController::new(mtm);
        root_view_controller.setView(&root_view);
        root_view_controller.addChildViewController(&tab_view_controller);
        window.setContentViewController(Some(&root_view_controller));

        let minimum_content_size = root_view.fittingSize();
        window.setContentMinSize(minimum_content_size);
        window.setContentSize(minimum_content_size);
        window.center();

        let window_obj = Self {
            window,
            tab_view_controller,
            general,
            microphone,
            deepgram,
            transformation,
            prompts,
            status_text_field,
            hotkey_capture_restore_value: RefCell::new(None),
        };

        window_obj.set_hotkey_capture_state(None);
        window_obj
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    pub fn show(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
        self.tab_view_controller.setSelectedTabViewItemIndex(0);
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
        let _ = self
            .window
            .makeFirstResponder(Some(&*self.general.hotkey_field));
    }

    pub fn update_meter(&self, meter: Option<MicMeterSnapshot>) {
        self.microphone.update_meter(meter.unwrap_or_default());
    }

    pub fn hide(&self) {
        self.window.orderOut(None);
    }

    /// Loads `config` into every pane, discarding any edit in progress.
    pub fn load_from_config(
        &self,
        config: &Config,
        audio_device_status_message: Option<&str>,
    ) -> Result<(), String> {
        // SAFETY: `nil` asks the window to end editing of any field.
        unsafe { self.window.endEditingFor(None) };

        let form = SettingsForm::from_config(config);
        self.general
            .load(MainThreadMarker::from(&*self.window), &form.general);
        self.microphone.load(&form.microphone)?;
        self.deepgram.load(
            &form.deepgram,
            DeepgramEnvironmentHints {
                api_key: config
                    .deepgram_api_key_env_var_in_use()
                    .map(environment_hint_message),
                project_id: config
                    .deepgram_project_id_env_var_in_use()
                    .map(environment_hint_message),
            },
        );
        self.transformation.load(
            &form.transformation,
            config
                .transformation_api_key_env_var_in_use()
                .map(environment_hint_message),
        );
        self.prompts.load(&form.prompts);
        self.set_status(audio_device_status_message.unwrap_or(""));
        Ok(())
    }

    /// Commits the edit in progress and reads every pane into a `Config`.
    pub fn read_config(&self) -> Result<Config, String> {
        // Ending editing runs the field's formatter; a field whose text the
        // formatter rejects keeps first responder status and fails here.
        if !self.window.makeFirstResponder(Some(&self.window)) {
            return Err("Correct the value in the field being edited before saving.".to_owned());
        }
        self.read_form().to_config()
    }

    fn read_form(&self) -> SettingsForm {
        SettingsForm {
            general: self.general.read(),
            microphone: self.microphone.read(),
            deepgram: self.deepgram.read(),
            transformation: self.transformation.read(),
            prompts: self.prompts.read(),
        }
    }

    pub fn set_status(&self, message: &str) {
        self.status_text_field
            .setStringValue(&NSString::from_str(message));
        // The label truncates after `STATUS_MAXIMUM_LINES`; the tooltip keeps
        // the whole message readable.
        self.status_text_field.setToolTip(
            (!message.is_empty())
                .then(|| NSString::from_str(message))
                .as_deref(),
        );
    }

    pub fn update_mic_gain_label(&self, gain_db: f32) {
        self.microphone.update_gain_label(gain_db);
    }

    pub fn mic_gain_slider_value(&self) -> f32 {
        self.microphone.gain_slider_value()
    }

    pub fn mic_audio_device_value(&self) -> Option<String> {
        self.microphone.audio_device_value()
    }

    pub fn sync_transformation_api_key_env_hint(&self) {
        let hint = crate::config::transformation_api_key_env_var_in_use(
            self.transformation_provider_value().as_deref(),
            self.transformation_api_key_value().as_deref(),
        )
        .map(environment_hint_message);
        self.transformation.set_api_key_env_hint(hint);
    }

    pub fn deepgram_api_key_value(&self) -> Option<String> {
        self.deepgram.api_key_value()
    }

    pub fn deepgram_project_id_value(&self) -> Option<String> {
        self.deepgram.project_id_value()
    }

    pub fn transformation_provider_value(&self) -> Option<String> {
        self.transformation.provider_value()
    }

    pub fn transformation_api_key_value(&self) -> Option<String> {
        self.transformation.api_key_value()
    }

    pub fn transformation_model_value(&self) -> String {
        self.transformation.model_value()
    }

    pub fn populate_transformation_model_values(&self, models: &[String]) {
        self.transformation.populate_model_values(models);
    }

    pub fn set_transformation_model_controls_enabled(&self, enabled: bool) {
        self.transformation.set_model_controls_enabled(enabled);
    }

    pub fn set_transformation_check_result(&self, success: bool) {
        style_check_result(&self.transformation.model_check_button, success);
    }

    pub fn set_deepgram_check_result(&self, success: bool) {
        style_check_result(&self.deepgram.api_key_check_button, success);
    }

    pub fn reset_transformation_check_button(&self) {
        style_button_bezel_and_text(&self.transformation.model_check_button, "Check", None, None);
    }

    pub fn reset_deepgram_check_button(&self) {
        style_button_bezel_and_text(&self.deepgram.api_key_check_button, "Check", None, None);
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
        self.hotkey_field(target).stringValue().to_string()
    }

    pub fn set_hotkey_value(&self, target: HotkeyCaptureTarget, value: &str) {
        self.hotkey_field(target)
            .setStringValue(&NSString::from_str(value));
    }

    fn hotkey_field(&self, target: HotkeyCaptureTarget) -> &NSTextField {
        match target {
            HotkeyCaptureTarget::Record => &self.general.hotkey_field,
            HotkeyCaptureTarget::Correction => &self.general.correction_key_field,
            HotkeyCaptureTarget::Transform => &self.transformation.hotkey_field,
        }
    }

    fn set_hotkey_capture_state(&self, active_target: Option<HotkeyCaptureTarget>) {
        for (target, button) in [
            (
                HotkeyCaptureTarget::Record,
                &self.general.hotkey_capture_button,
            ),
            (
                HotkeyCaptureTarget::Correction,
                &self.general.correction_key_capture_button,
            ),
            (
                HotkeyCaptureTarget::Transform,
                &self.transformation.hotkey_capture_button,
            ),
        ] {
            set_capture_button_state(
                button,
                active_target == Some(target),
                active_target.is_none(),
            );
        }
    }
}

fn style_check_result(button: &NSButton, success: bool) {
    let bezel = if success {
        NSColor::systemGreenColor()
    } else {
        NSColor::systemRedColor()
    };
    style_button_bezel_and_text(button, "Check", Some(bezel), Some(NSColor::whiteColor()));
}

fn pane_tab_view_item(
    mtm: MainThreadMarker,
    title: &str,
    symbol_name: &str,
    view: &NSView,
) -> Retained<NSTabViewItem> {
    let title = NSString::from_str(title);
    let view_controller = NSViewController::new(mtm);
    view_controller.setView(view);
    view_controller.setTitle(Some(&title));

    let item = NSTabViewItem::tabViewItemWithViewController(&view_controller);
    item.setLabel(&title);
    item.setImage(
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol_name),
            Some(&title),
        )
        .as_deref(),
    );
    item
}

/// Status text on the leading side and Cancel/Save on the trailing side. The
/// bar is always tall enough for `STATUS_MAXIMUM_LINES` of status text, so a
/// longer message does not resize the window.
fn button_bar(
    mtm: MainThreadMarker,
    status_text_field: &NSTextField,
    cancel_button: &NSButton,
    save_button: &NSButton,
) -> Retained<NSStackView> {
    let bar = for_auto_layout(NSStackView::new(mtm));
    bar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    bar.setAlignment(NSLayoutAttribute::CenterY);
    for view in [
        status_text_field as &NSView,
        cancel_button as &NSView,
        save_button as &NSView,
    ] {
        bar.addArrangedSubview(view);
    }
    status_text_field.setContentHuggingPriority_forOrientation(
        NSLayoutPriorityDefaultLow,
        NSLayoutConstraintOrientation::Horizontal,
    );

    let status_font = status_text_field.font().expect("labels always have a font");
    let status_line_height = NSLayoutManager::new().defaultLineHeightForFont(&status_font);
    bar.heightAnchor()
        .constraintGreaterThanOrEqualToConstant(status_line_height * STATUS_MAXIMUM_LINES as f64)
        .setActive(true);
    bar
}
