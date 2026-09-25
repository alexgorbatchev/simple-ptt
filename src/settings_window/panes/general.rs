use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSButton, NSPopUpButton, NSTextField, NSView};

use super::{pane_view, PaneContentHeight};
use crate::settings_window::actions::SettingsAction;
use crate::settings_window::controls::{
    checkbox, decimal_value, fixed_width, hotkey_display_field, is_checked, minimum_width,
    number_field, pop_up_button, push_button, selected_title, set_checked, set_decimal_value,
    set_text_value, text_value, NumberFieldKind, FIELD_MIN_WIDTH, NUMBER_FIELD_WIDTH,
};
use crate::settings_window::form::GeneralForm;
use crate::settings_window::grid::{ControlWidth, FormGrid};
use crate::settings_window::popups::{
    available_font_family_names, populate_font_name_popup, populate_meter_style_popup,
};

#[derive(Debug)]
pub struct GeneralPane {
    pub hotkey_field: Retained<NSTextField>,
    pub hotkey_capture_button: Retained<NSButton>,
    pub correction_key_field: Retained<NSTextField>,
    pub correction_key_capture_button: Retained<NSButton>,
    font_name_popup: Retained<NSPopUpButton>,
    font_size_field: Retained<NSTextField>,
    footer_font_size_field: Retained<NSTextField>,
    meter_style_popup: Retained<NSPopUpButton>,
    auto_check_updates_checkbox: Retained<NSButton>,
    start_on_login_checkbox: Retained<NSButton>,
}

impl GeneralPane {
    pub fn new(mtm: MainThreadMarker, target: &AnyObject) -> (Self, Retained<NSView>) {
        let hotkey_field = hotkey_display_field(mtm);
        let hotkey_capture_button =
            push_button(mtm, "Capture…", target, SettingsAction::CaptureRecordHotkey);
        let correction_key_field = hotkey_display_field(mtm);
        let correction_key_capture_button = push_button(
            mtm,
            "Capture…",
            target,
            SettingsAction::CaptureCorrectionKey,
        );
        let font_name_popup = pop_up_button(mtm);
        let font_size_field = number_field(mtm, NumberFieldKind::Decimal);
        let footer_font_size_field = number_field(mtm, NumberFieldKind::Decimal);
        let meter_style_popup = pop_up_button(mtm);
        let auto_check_updates_checkbox = checkbox(mtm, "Automatically check for updates");
        let start_on_login_checkbox = checkbox(mtm, "Start automatically on login");

        minimum_width(&hotkey_field, FIELD_MIN_WIDTH);
        fixed_width(&font_size_field, NUMBER_FIELD_WIDTH);
        fixed_width(&footer_font_size_field, NUMBER_FIELD_WIDTH);

        let grid = FormGrid::new(mtm, 1);
        grid.add_section_header("Shortcuts");
        grid.add_row(
            "Dictation shortcut:",
            &hotkey_field,
            ControlWidth::Fill,
            &[&hotkey_capture_button],
        );
        grid.add_row(
            "Correction key:",
            &correction_key_field,
            ControlWidth::Fill,
            &[&correction_key_capture_button],
        );
        grid.add_section_header("Overlay");
        grid.add_row("Font:", &font_name_popup, ControlWidth::Intrinsic, &[]);
        grid.add_row("Font size:", &font_size_field, ControlWidth::Intrinsic, &[]);
        grid.add_row(
            "Footer font size:",
            &footer_font_size_field,
            ControlWidth::Intrinsic,
            &[],
        );
        grid.add_row(
            "Meter style:",
            &meter_style_popup,
            ControlWidth::Intrinsic,
            &[],
        );
        grid.add_section_header("System");
        grid.add_detail_row(&auto_check_updates_checkbox, ControlWidth::Intrinsic);
        grid.add_detail_row(&start_on_login_checkbox, ControlWidth::Intrinsic);

        let pane = Self {
            hotkey_field,
            hotkey_capture_button,
            correction_key_field,
            correction_key_capture_button,
            font_name_popup,
            font_size_field,
            footer_font_size_field,
            meter_style_popup,
            auto_check_updates_checkbox,
            start_on_login_checkbox,
        };
        let view = pane_view(mtm, grid.view(), PaneContentHeight::Fitting);
        (pane, view)
    }

    pub fn load(&self, mtm: MainThreadMarker, form: &GeneralForm) {
        set_text_value(&self.hotkey_field, &form.hotkey);
        set_text_value(&self.correction_key_field, &form.correction_key);
        populate_font_name_popup(
            &self.font_name_popup,
            &available_font_family_names(mtm),
            form.font_name_title.as_deref(),
        );
        set_decimal_value(&self.font_size_field, form.font_size);
        set_decimal_value(&self.footer_font_size_field, form.footer_font_size);
        populate_meter_style_popup(&self.meter_style_popup, form.meter_style_title.as_deref());
        set_checked(&self.auto_check_updates_checkbox, form.auto_check_updates);
        set_checked(&self.start_on_login_checkbox, form.start_on_login);
    }

    pub fn read(&self) -> GeneralForm {
        GeneralForm {
            hotkey: text_value(&self.hotkey_field),
            correction_key: text_value(&self.correction_key_field),
            font_name_title: selected_title(&self.font_name_popup),
            font_size: decimal_value(&self.font_size_field),
            footer_font_size: decimal_value(&self.footer_font_size_field),
            meter_style_title: selected_title(&self.meter_style_popup),
            auto_check_updates: is_checked(&self.auto_check_updates_checkbox),
            start_on_login: is_checked(&self.start_on_login_checkbox),
        }
    }
}
