use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSButton, NSComboBox, NSPopUpButton, NSTextField, NSView};

use super::{pane_view, PaneContentHeight};
use crate::settings_window::actions::SettingsAction;
use crate::settings_window::controls::{
    checkbox, combo_box, hotkey_display_field, is_checked, minimum_width,
    pop_up_button_with_action, push_button, selected_title, set_checked, set_text_value,
    text_field, text_value, FIELD_MIN_WIDTH,
};
use crate::settings_window::form::{
    optional_popup_value, optional_text, TransformationForm, TRANSFORMATION_PROVIDER_DISABLED_LABEL,
};
use crate::settings_window::grid::{ControlWidth, FormGrid, HintRow};
use crate::settings_window::popups::{
    populate_combo_box_with_values, populate_transformation_provider_popup,
};

const AUTO_TRANSFORM_HINT: &str =
    "When enabled, releasing the dictation shortcut runs transformation automatically.";

#[derive(Debug)]
pub struct TransformationPane {
    pub hotkey_field: Retained<NSTextField>,
    pub hotkey_capture_button: Retained<NSButton>,
    auto_checkbox: Retained<NSButton>,
    provider_popup: Retained<NSPopUpButton>,
    api_key_field: Retained<NSTextField>,
    api_key_env_hint: HintRow,
    model_combo_box: Retained<NSComboBox>,
    model_refresh_button: Retained<NSButton>,
    pub model_check_button: Retained<NSButton>,
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
        let model_combo_box = combo_box(mtm);
        let model_refresh_button = push_button(
            mtm,
            "Fetch models",
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

        let grid = FormGrid::new(mtm, 2);
        grid.add_row(
            "Transform shortcut:",
            &hotkey_field,
            ControlWidth::Fill,
            &[&hotkey_capture_button],
        );
        grid.add_detail_row(&auto_checkbox, ControlWidth::Intrinsic);
        grid.add_hint_row(Some(AUTO_TRANSFORM_HINT));
        grid.add_row("Provider:", &provider_popup, ControlWidth::Intrinsic, &[]);
        grid.add_row("API key:", &api_key_field, ControlWidth::Fill, &[]);
        let api_key_env_hint = grid.add_hint_row(None);
        grid.add_row(
            "Model:",
            &model_combo_box,
            ControlWidth::Fill,
            &[&model_refresh_button, &model_check_button],
        );

        let pane = Self {
            hotkey_field,
            hotkey_capture_button,
            auto_checkbox,
            provider_popup,
            api_key_field,
            api_key_env_hint,
            model_combo_box,
            model_refresh_button,
            model_check_button,
        };
        let view = pane_view(mtm, grid.view(), PaneContentHeight::Fitting);
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
        self.api_key_env_hint.set_text(api_key_env_hint.as_deref());
        populate_combo_box_with_values(&self.model_combo_box, &[], &form.model);
        self.set_model_controls_enabled(self.provider_value().is_some());
    }

    pub fn read(&self) -> TransformationForm {
        TransformationForm {
            hotkey: text_value(&self.hotkey_field),
            auto: is_checked(&self.auto_checkbox),
            provider_title: selected_title(&self.provider_popup),
            api_key: text_value(&self.api_key_field),
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

    pub fn set_model_controls_enabled(&self, enabled: bool) {
        self.model_combo_box.setEnabled(enabled);
        self.model_refresh_button.setEnabled(enabled);
        self.model_check_button.setEnabled(enabled);
    }
}
