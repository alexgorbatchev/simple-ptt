use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSButton, NSPopUpButton, NSTextField, NSView};

use super::{pane_view, PaneContentHeight};
use crate::config::Config;
use crate::settings_window::actions::SettingsAction;
use crate::settings_window::controls::{
    checkbox, decimal_value, fixed_width, hotkey_display_field, is_checked, minimum_width,
    number_field, pop_up_button, push_button, selected_title, set_checked, set_decimal_value,
    set_placeholder, set_text_value, set_unsigned_value, text_value, unsigned_value,
    NumberFieldKind, ResetButtons, FIELD_MIN_WIDTH, NUMBER_FIELD_WIDTH,
};
use crate::settings_window::defaulted::DefaultedRow;
use crate::settings_window::form::GeneralForm;
use crate::settings_window::grid::{ControlWidth, FormGrid};
use crate::settings_window::labels;
use crate::settings_window::popups::{
    available_font_family_names, populate_font_name_popup, populate_meter_style_popup,
};

const HOLD_THRESHOLD_HINT: &str =
    "A shorter press of the dictation shortcut toggles recording; holding it longer records until release.";

#[derive(Debug)]
pub struct GeneralPane {
    pub hotkey_field: Retained<NSTextField>,
    pub hotkey_capture_button: Retained<NSButton>,
    hold_ms_field: Retained<NSTextField>,
    pub correction_key_field: Retained<NSTextField>,
    pub correction_key_capture_button: Retained<NSButton>,
    font_name_popup: Retained<NSPopUpButton>,
    font_size_field: Retained<NSTextField>,
    footer_font_size_field: Retained<NSTextField>,
    meter_style_popup: Retained<NSPopUpButton>,
    auto_check_updates_checkbox: Retained<NSButton>,
    start_on_login_checkbox: Retained<NSButton>,
    pub reset_buttons: ResetButtons,
}

impl GeneralPane {
    pub fn new(mtm: MainThreadMarker, target: &AnyObject) -> (Self, Retained<NSView>) {
        let hotkey_field = hotkey_display_field(mtm);
        let hotkey_capture_button =
            push_button(mtm, "Capture…", target, SettingsAction::CaptureRecordHotkey);
        // `NSNumberFormatter` rejects integers above `i64::MAX`, so that is the
        // largest hold threshold the field can take.
        let hold_ms_field = number_field(
            mtm,
            NumberFieldKind::UnsignedInteger {
                maximum: i64::MAX as u64,
            },
        );
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

        let defaults = Config::default();
        set_placeholder(&hotkey_field, &defaults.ui.hotkey);
        set_placeholder(&hold_ms_field, &defaults.mic.hold_ms.to_string());
        set_placeholder(&correction_key_field, &defaults.ui.correction_key);
        set_placeholder(&font_size_field, &defaults.ui.font_size.to_string());
        let mut reset_buttons = ResetButtons::default();
        let hotkey_reset = reset_buttons.add(mtm, target, DefaultedRow::DictationShortcut);
        let hold_ms_reset = reset_buttons.add(mtm, target, DefaultedRow::HoldThreshold);
        let correction_key_reset = reset_buttons.add(mtm, target, DefaultedRow::CorrectionKey);
        let font_size_reset = reset_buttons.add(mtm, target, DefaultedRow::FontSize);
        let meter_style_reset = reset_buttons.add(mtm, target, DefaultedRow::MeterStyle);
        let auto_check_updates_reset =
            reset_buttons.add(mtm, target, DefaultedRow::AutoCheckUpdates);
        let start_on_login_reset = reset_buttons.add(mtm, target, DefaultedRow::StartOnLogin);

        minimum_width(&hotkey_field, FIELD_MIN_WIDTH);
        fixed_width(&hold_ms_field, NUMBER_FIELD_WIDTH);
        fixed_width(&font_size_field, NUMBER_FIELD_WIDTH);
        fixed_width(&footer_font_size_field, NUMBER_FIELD_WIDTH);

        let grid = FormGrid::new(mtm, 2);
        grid.add_section_header("Shortcuts");
        grid.add_row(
            &labels::DICTATION_SHORTCUT.row_title(),
            &hotkey_field,
            ControlWidth::Fill,
            &[&hotkey_capture_button, &hotkey_reset],
        );
        grid.add_row(
            &labels::HOLD_THRESHOLD.row_title(),
            &hold_ms_field,
            ControlWidth::Intrinsic,
            &[&hold_ms_reset],
        );
        grid.add_hint_row(Some(HOLD_THRESHOLD_HINT));
        grid.add_row(
            &labels::CORRECTION_KEY.row_title(),
            &correction_key_field,
            ControlWidth::Fill,
            &[&correction_key_capture_button, &correction_key_reset],
        );
        grid.add_section_header("Overlay");
        grid.add_row(
            &labels::FONT.row_title(),
            &font_name_popup,
            ControlWidth::Intrinsic,
            &[],
        );
        grid.add_row(
            &labels::FONT_SIZE.row_title(),
            &font_size_field,
            ControlWidth::Intrinsic,
            &[&font_size_reset],
        );
        grid.add_row(
            &labels::FOOTER_FONT_SIZE.row_title(),
            &footer_font_size_field,
            ControlWidth::Intrinsic,
            &[],
        );
        grid.add_row(
            &labels::METER_STYLE.row_title(),
            &meter_style_popup,
            ControlWidth::Intrinsic,
            &[&meter_style_reset],
        );
        grid.add_section_header("System");
        grid.add_detail_row_with_accessories(
            &auto_check_updates_checkbox,
            ControlWidth::Intrinsic,
            &[&auto_check_updates_reset],
        );
        grid.add_detail_row_with_accessories(
            &start_on_login_checkbox,
            ControlWidth::Intrinsic,
            &[&start_on_login_reset],
        );

        let pane = Self {
            hotkey_field,
            hotkey_capture_button,
            hold_ms_field,
            correction_key_field,
            correction_key_capture_button,
            font_name_popup,
            font_size_field,
            footer_font_size_field,
            meter_style_popup,
            auto_check_updates_checkbox,
            start_on_login_checkbox,
            reset_buttons,
        };
        let view = pane_view(mtm, grid.view(), PaneContentHeight::Fitting);
        (pane, view)
    }

    pub fn load(&self, mtm: MainThreadMarker, form: &GeneralForm) {
        set_text_value(&self.hotkey_field, &form.hotkey);
        set_unsigned_value(&self.hold_ms_field, form.hold_ms);
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

    /// Shows `row`'s value from `form` after a reset.
    pub fn apply_row(&self, row: DefaultedRow, form: &GeneralForm) {
        match row {
            DefaultedRow::DictationShortcut => set_text_value(&self.hotkey_field, &form.hotkey),
            DefaultedRow::HoldThreshold => set_unsigned_value(&self.hold_ms_field, form.hold_ms),
            DefaultedRow::CorrectionKey => {
                set_text_value(&self.correction_key_field, &form.correction_key)
            }
            DefaultedRow::FontSize => set_decimal_value(&self.font_size_field, form.font_size),
            DefaultedRow::MeterStyle => populate_meter_style_popup(
                &self.meter_style_popup,
                form.meter_style_title.as_deref(),
            ),
            DefaultedRow::AutoCheckUpdates => {
                set_checked(&self.auto_check_updates_checkbox, form.auto_check_updates)
            }
            DefaultedRow::StartOnLogin => {
                set_checked(&self.start_on_login_checkbox, form.start_on_login)
            }
            _ => {}
        }
    }

    pub fn read(&self) -> GeneralForm {
        GeneralForm {
            hotkey: text_value(&self.hotkey_field),
            hold_ms: unsigned_value(&self.hold_ms_field),
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
