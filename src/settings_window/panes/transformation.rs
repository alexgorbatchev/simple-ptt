use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSButton, NSComboBox, NSLayoutAttribute, NSPopUpButton, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::NSString;

use super::prompt_editors::PromptEditors;
use super::{activate, pane_view, PaneContentHeight};
use crate::config::Config;
use crate::settings_window::actions::SettingsAction;
use crate::settings_window::controls::{
    checkbox, combo_box, for_auto_layout, hotkey_display_field, is_checked, minimum_width,
    pop_up_button_with_action, push_button, selected_title, set_checked, set_placeholder,
    set_text_value, text_field, text_value, ResetButtons, FIELD_MIN_WIDTH,
};
use crate::settings_window::defaulted::DefaultedRow;
use crate::settings_window::form::{
    optional_popup_value, optional_text, transformation_model_placeholder, TransformationForm,
    TRANSFORMATION_PROVIDER_DISABLED_LABEL,
};
use crate::settings_window::grid::{ControlWidth, FormGrid, HintRow, RowAlignment};
use crate::settings_window::labels;
use crate::settings_window::popups::{
    populate_combo_box_with_values, populate_transformation_provider_popup,
};
use crate::settings_window::FETCH_MODELS_BUTTON_TITLE;

const AUTO_TRANSFORM_HINT: &str =
    "When enabled, releasing the dictation shortcut runs transformation automatically.";

/// Space between the form and the prompt editors, and between the two editors.
const SECTION_SPACING: f64 = 16.0;

/// The transformation shortcut, provider and model form, above the prompt
/// editors, which fill the rest of the pane.
#[derive(Debug)]
pub struct TransformationPane {
    pub hotkey_field: Retained<NSTextField>,
    pub hotkey_capture_button: Retained<NSButton>,
    auto_checkbox: Retained<NSButton>,
    provider_popup: Retained<NSPopUpButton>,
    api_key_field: Retained<NSTextField>,
    ollama_base_url_field: Retained<NSTextField>,
    api_key_env_hint: HintRow,
    model_combo_box: Retained<NSComboBox>,
    model_refresh_button: Retained<NSButton>,
    pub model_check_button: Retained<NSButton>,
    pub prompts: PromptEditors,
    pub reset_buttons: ResetButtons,
}

impl TransformationPane {
    pub fn new(mtm: MainThreadMarker, target: &AnyObject) -> (Self, Retained<NSView>) {
        let hotkey_field = hotkey_display_field(mtm);
        let hotkey_capture_button = push_button(
            mtm,
            "Capture…",
            target,
            SettingsAction::CaptureTransformHotkey,
        );
        let auto_checkbox = checkbox(mtm, "Auto-transform on shortcut release");
        let provider_popup =
            pop_up_button_with_action(mtm, target, SettingsAction::TransformationProviderChanged);
        let api_key_field = text_field(mtm);
        let ollama_base_url_field = text_field(mtm);
        set_placeholder(
            &ollama_base_url_field,
            crate::config::DEFAULT_OLLAMA_BASE_URL,
        );
        let model_combo_box = combo_box(mtm);
        let model_refresh_button = push_button(
            mtm,
            FETCH_MODELS_BUTTON_TITLE,
            target,
            SettingsAction::RefreshTransformationModels,
        );
        let model_check_button = push_button(
            mtm,
            "Check",
            target,
            SettingsAction::CheckTransformationProvider,
        );

        minimum_width(&hotkey_field, FIELD_MIN_WIDTH);

        set_placeholder(&hotkey_field, &Config::default().transformation.hotkey);
        let mut reset_buttons = ResetButtons::default();
        let hotkey_reset = reset_buttons.add(mtm, target, DefaultedRow::TransformShortcut);
        let auto_reset = reset_buttons.add(mtm, target, DefaultedRow::AutoTransform);

        let grid = FormGrid::new(mtm, 2);
        grid.add_row(
            &labels::TRANSFORM_SHORTCUT.row_title(),
            &hotkey_field,
            ControlWidth::Fill,
            &[&hotkey_capture_button, &hotkey_reset],
        );
        grid.add_detail_row_with_accessories(
            &auto_checkbox,
            ControlWidth::Intrinsic,
            &[&auto_reset],
        );
        grid.add_hint_row(Some(AUTO_TRANSFORM_HINT));
        grid.add_row(
            &labels::TRANSFORMATION_PROVIDER.row_title(),
            &provider_popup,
            ControlWidth::Intrinsic,
            &[],
        );
        grid.add_row(
            &labels::TRANSFORMATION_API_KEY.row_title(),
            &api_key_field,
            ControlWidth::Fill,
            &[],
        );
        let api_key_env_hint = grid.add_hint_row(None);
        grid.add_row(
            &labels::OLLAMA_SERVER.row_title(),
            &ollama_base_url_field,
            ControlWidth::Fill,
            &[],
        );
        // NSComboBox reports a first baseline 4pt above the text it draws (as
        // measured on macOS 26), so a first-baseline row sets it 4pt below its
        // label and buttons. Centering puts every view of the row on one line,
        // and the label's text on the combo box's drawn text.
        grid.add_aligned_row(
            &labels::TRANSFORMATION_MODEL.row_title(),
            &model_combo_box,
            ControlWidth::Fill,
            &[&model_refresh_button, &model_check_button],
            RowAlignment::Center,
        );

        let stack = for_auto_layout(NSStackView::new(mtm));
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(SECTION_SPACING);
        stack.addArrangedSubview(grid.view());
        activate(&[grid
            .view()
            .widthAnchor()
            .constraintEqualToAnchor(&stack.widthAnchor())]);
        let prompts = PromptEditors::new(mtm, target, &stack);

        let pane = Self {
            hotkey_field,
            hotkey_capture_button,
            auto_checkbox,
            provider_popup,
            api_key_field,
            ollama_base_url_field,
            api_key_env_hint,
            model_combo_box,
            model_refresh_button,
            model_check_button,
            prompts,
            reset_buttons,
        };
        let view = pane_view(mtm, &stack, PaneContentHeight::Fill);
        (pane, view)
    }

    pub fn load(&self, form: &TransformationForm, api_key_env_hint: Option<String>) {
        set_text_value(&self.hotkey_field, &form.hotkey);
        set_checked(&self.auto_checkbox, form.auto);
        populate_transformation_provider_popup(
            &self.provider_popup,
            form.provider_title.as_deref(),
        );
        set_text_value(&self.api_key_field, &form.api_key);
        set_text_value(&self.ollama_base_url_field, &form.ollama_base_url);
        self.api_key_env_hint.set_text(api_key_env_hint.as_deref());
        populate_combo_box_with_values(&self.model_combo_box, &[], &form.model);
        self.set_model_controls_enabled(self.provider_value().is_some());
        self.sync_model_placeholder();
    }

    /// Shows `row`'s value from `form` after a reset.
    pub fn apply_row(&self, row: DefaultedRow, form: &TransformationForm) {
        match row {
            DefaultedRow::TransformShortcut => set_text_value(&self.hotkey_field, &form.hotkey),
            DefaultedRow::AutoTransform => set_checked(&self.auto_checkbox, form.auto),
            _ => {}
        }
    }

    pub fn read(&self) -> TransformationForm {
        TransformationForm {
            hotkey: text_value(&self.hotkey_field),
            auto: is_checked(&self.auto_checkbox),
            provider_title: selected_title(&self.provider_popup),
            api_key: text_value(&self.api_key_field),
            ollama_base_url: text_value(&self.ollama_base_url_field),
            model: self.model_value(),
        }
    }

    pub fn provider_value(&self) -> Option<String> {
        optional_popup_value(
            selected_title(&self.provider_popup).as_deref(),
            TRANSFORMATION_PROVIDER_DISABLED_LABEL,
        )
    }

    pub fn api_key_value(&self) -> Option<String> {
        optional_text(&text_value(&self.api_key_field))
    }

    pub fn ollama_base_url_value(&self) -> Option<String> {
        optional_text(&text_value(&self.ollama_base_url_field))
    }

    /// The Ollama server field applies only while `ollama` is the provider.
    pub fn set_ollama_server_enabled(&self, enabled: bool) {
        self.ollama_base_url_field.setEnabled(enabled);
    }

    pub fn model_value(&self) -> String {
        self.model_combo_box.stringValue().to_string()
    }

    pub fn set_api_key_env_hint(&self, hint: Option<String>) {
        self.api_key_env_hint.set_text(hint.as_deref());
    }

    pub fn populate_model_values(&self, models: &[String]) {
        let selected_model = self.model_value();
        populate_combo_box_with_values(&self.model_combo_box, models, selected_model.as_str());
    }

    /// Shows in the empty model field the model it stands for with the
    /// selected provider.
    pub fn sync_model_placeholder(&self) {
        let placeholder = transformation_model_placeholder(self.provider_value().as_deref());
        self.model_combo_box
            .setPlaceholderString(Some(&NSString::from_str(&placeholder)));
    }

    pub fn set_model_controls_enabled(&self, enabled: bool) {
        self.model_combo_box.setEnabled(enabled);
        self.model_refresh_button.setEnabled(enabled);
        self.model_check_button.setEnabled(enabled);
    }
}
