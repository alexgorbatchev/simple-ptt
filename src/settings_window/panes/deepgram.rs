use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSButton, NSPopUpButton, NSTextField, NSView};

use super::{pane_view, PaneContentHeight};
use crate::config::Config;
use crate::settings_window::actions::SettingsAction;
use crate::settings_window::controls::{
    fixed_width, minimum_width, number_field, pop_up_button, push_button, selected_title,
    set_placeholder, set_text_value, set_unsigned_value, text_field, text_value, unsigned_value,
    NumberFieldKind, ResetButtons, FIELD_MIN_WIDTH, NUMBER_FIELD_WIDTH,
};
use crate::settings_window::defaulted::DefaultedRow;
use crate::settings_window::form::{optional_text, DeepgramForm};
use crate::settings_window::grid::{ControlWidth, FormGrid, HintRow};
use crate::settings_window::labels;
use crate::settings_window::popups::populate_deepgram_model_popup;

const KEYTERMS_HINT: &str = "Comma-separated (e.g. 'macOS, GitHub')";

/// Environment variable hint shown below the API key field.
#[derive(Debug, Default)]
pub struct DeepgramEnvironmentHints {
    pub api_key: Option<String>,
}

#[derive(Debug)]
pub struct DeepgramPane {
    api_key_field: Retained<NSTextField>,
    pub api_key_check_button: Retained<NSButton>,
    api_key_env_hint: HintRow,
    language_field: Retained<NSTextField>,
    keyterms_field: Retained<NSTextField>,
    model_popup: Retained<NSPopUpButton>,
    endpointing_ms_field: Retained<NSTextField>,
    utterance_end_ms_field: Retained<NSTextField>,
    pub reset_buttons: ResetButtons,
}

impl DeepgramPane {
    pub fn new(mtm: MainThreadMarker, target: &AnyObject) -> (Self, Retained<NSView>) {
        let api_key_field = text_field(mtm);
        let api_key_check_button = push_button(
            mtm,
            "Check",
            target,
            SettingsAction::CheckDeepgramConnection,
        );
        let language_field = text_field(mtm);
        let keyterms_field = text_field(mtm);
        let model_popup = pop_up_button(mtm);
        let u16_field_kind = NumberFieldKind::UnsignedInteger {
            maximum: u64::from(u16::MAX),
        };
        let endpointing_ms_field = number_field(mtm, u16_field_kind);
        let utterance_end_ms_field = number_field(mtm, u16_field_kind);

        minimum_width(&api_key_field, FIELD_MIN_WIDTH);
        fixed_width(&endpointing_ms_field, NUMBER_FIELD_WIDTH);
        fixed_width(&utterance_end_ms_field, NUMBER_FIELD_WIDTH);

        let defaults = Config::default().deepgram;
        set_placeholder(&language_field, &defaults.language);
        set_placeholder(&endpointing_ms_field, &defaults.endpointing_ms.to_string());
        set_placeholder(
            &utterance_end_ms_field,
            &defaults.utterance_end_ms.to_string(),
        );
        let mut reset_buttons = ResetButtons::default();
        let language_reset = reset_buttons.add(mtm, target, DefaultedRow::DeepgramLanguage);
        let model_reset = reset_buttons.add(mtm, target, DefaultedRow::DeepgramModel);
        let endpointing_reset = reset_buttons.add(mtm, target, DefaultedRow::Endpointing);
        let utterance_end_reset = reset_buttons.add(mtm, target, DefaultedRow::UtteranceEnd);

        let grid = FormGrid::new(mtm, 1);
        grid.add_row(
            &labels::DEEPGRAM_API_KEY.row_title(),
            &api_key_field,
            ControlWidth::Fill,
            &[&api_key_check_button],
        );
        let api_key_env_hint = grid.add_hint_row(None);
        grid.add_row(
            &labels::DEEPGRAM_LANGUAGE.row_title(),
            &language_field,
            ControlWidth::Fill,
            &[&language_reset],
        );
        grid.add_row(
            &labels::DEEPGRAM_KEYTERMS.row_title(),
            &keyterms_field,
            ControlWidth::Fill,
            &[],
        );
        grid.add_hint_row(Some(KEYTERMS_HINT));
        grid.add_row(
            &labels::DEEPGRAM_MODEL.row_title(),
            &model_popup,
            ControlWidth::Intrinsic,
            &[&model_reset],
        );
        grid.add_row(
            &labels::ENDPOINTING.row_title(),
            &endpointing_ms_field,
            ControlWidth::Intrinsic,
            &[&endpointing_reset],
        );
        grid.add_row(
            &labels::UTTERANCE_END.row_title(),
            &utterance_end_ms_field,
            ControlWidth::Intrinsic,
            &[&utterance_end_reset],
        );

        let pane = Self {
            api_key_field,
            api_key_check_button,
            api_key_env_hint,
            language_field,
            keyterms_field,
            model_popup,
            endpointing_ms_field,
            utterance_end_ms_field,
            reset_buttons,
        };
        let view = pane_view(mtm, grid.view(), PaneContentHeight::Fitting);
        (pane, view)
    }

    pub fn load(&self, form: &DeepgramForm, hints: DeepgramEnvironmentHints) {
        set_text_value(&self.api_key_field, &form.api_key);
        self.api_key_env_hint.set_text(hints.api_key.as_deref());
        set_text_value(&self.language_field, &form.language);
        set_text_value(&self.keyterms_field, &form.keyterms);
        populate_deepgram_model_popup(&self.model_popup, form.model_title.as_deref());
        set_unsigned_value(&self.endpointing_ms_field, form.endpointing_ms);
        set_unsigned_value(&self.utterance_end_ms_field, form.utterance_end_ms);
    }

    /// Shows `row`'s value from `form` after a reset.
    pub fn apply_row(&self, row: DefaultedRow, form: &DeepgramForm) {
        match row {
            DefaultedRow::DeepgramLanguage => set_text_value(&self.language_field, &form.language),
            DefaultedRow::DeepgramModel => {
                populate_deepgram_model_popup(&self.model_popup, form.model_title.as_deref())
            }
            DefaultedRow::Endpointing => {
                set_unsigned_value(&self.endpointing_ms_field, form.endpointing_ms)
            }
            DefaultedRow::UtteranceEnd => {
                set_unsigned_value(&self.utterance_end_ms_field, form.utterance_end_ms)
            }
            _ => {}
        }
    }

    pub fn read(&self) -> DeepgramForm {
        DeepgramForm {
            api_key: text_value(&self.api_key_field),
            language: text_value(&self.language_field),
            keyterms: text_value(&self.keyterms_field),
            model_title: selected_title(&self.model_popup),
            endpointing_ms: unsigned_value(&self.endpointing_ms_field),
            utterance_end_ms: unsigned_value(&self.utterance_end_ms_field),
        }
    }

    pub fn api_key_value(&self) -> Option<String> {
        optional_text(&text_value(&self.api_key_field))
    }
}
