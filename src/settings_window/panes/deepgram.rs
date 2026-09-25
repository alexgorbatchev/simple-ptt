use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSButton, NSPopUpButton, NSTextField, NSView};

use super::{pane_view, PaneContentHeight};
use crate::settings_window::actions::SettingsAction;
use crate::settings_window::controls::{
    fixed_width, minimum_width, number_field, pop_up_button, push_button, selected_title,
    set_text_value, set_unsigned_value, text_field, text_value, unsigned_value, NumberFieldKind,
    FIELD_MIN_WIDTH, NUMBER_FIELD_WIDTH,
};
use crate::settings_window::form::{optional_text, DeepgramForm};
use crate::settings_window::grid::{ControlWidth, FormGrid, HintRow};
use crate::settings_window::popups::populate_deepgram_model_popup;

const KEYTERMS_HINT: &str = "Comma-separated (e.g. 'macOS, GitHub')";

/// Environment variable hints shown below the credential fields.
#[derive(Debug, Default)]
pub struct DeepgramEnvironmentHints {
    pub api_key: Option<String>,
    pub project_id: Option<String>,
}

#[derive(Debug)]
pub struct DeepgramPane {
    api_key_field: Retained<NSTextField>,
    pub api_key_check_button: Retained<NSButton>,
    api_key_env_hint: HintRow,
    project_id_field: Retained<NSTextField>,
    project_id_env_hint: HintRow,
    language_field: Retained<NSTextField>,
    keyterms_field: Retained<NSTextField>,
    model_popup: Retained<NSPopUpButton>,
    endpointing_ms_field: Retained<NSTextField>,
    utterance_end_ms_field: Retained<NSTextField>,
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
        let project_id_field = text_field(mtm);
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

        let grid = FormGrid::new(mtm, 1);
        grid.add_row(
            "API key:",
            &api_key_field,
            ControlWidth::Fill,
            &[&api_key_check_button],
        );
        let api_key_env_hint = grid.add_hint_row(None);
        grid.add_row("Project ID:", &project_id_field, ControlWidth::Fill, &[]);
        let project_id_env_hint = grid.add_hint_row(None);
        grid.add_row("Language:", &language_field, ControlWidth::Fill, &[]);
        grid.add_row("Keyterms:", &keyterms_field, ControlWidth::Fill, &[]);
        grid.add_hint_row(Some(KEYTERMS_HINT));
        grid.add_row("Model:", &model_popup, ControlWidth::Intrinsic, &[]);
        grid.add_row(
            "Endpointing (ms):",
            &endpointing_ms_field,
            ControlWidth::Intrinsic,
            &[],
        );
        grid.add_row(
            "Utterance end (ms):",
            &utterance_end_ms_field,
            ControlWidth::Intrinsic,
            &[],
        );

        let pane = Self {
            api_key_field,
            api_key_check_button,
            api_key_env_hint,
            project_id_field,
            project_id_env_hint,
            language_field,
            keyterms_field,
            model_popup,
            endpointing_ms_field,
            utterance_end_ms_field,
        };
        let view = pane_view(mtm, grid.view(), PaneContentHeight::Fitting);
        (pane, view)
    }

    pub fn load(&self, form: &DeepgramForm, hints: DeepgramEnvironmentHints) {
        set_text_value(&self.api_key_field, &form.api_key);
        self.api_key_env_hint.set_text(hints.api_key.as_deref());
        set_text_value(&self.project_id_field, &form.project_id);
        self.project_id_env_hint
            .set_text(hints.project_id.as_deref());
        set_text_value(&self.language_field, &form.language);
        set_text_value(&self.keyterms_field, &form.keyterms);
        populate_deepgram_model_popup(&self.model_popup, form.model_title.as_deref());
        set_unsigned_value(&self.endpointing_ms_field, form.endpointing_ms);
        set_unsigned_value(&self.utterance_end_ms_field, form.utterance_end_ms);
    }

    pub fn read(&self) -> DeepgramForm {
        DeepgramForm {
            api_key: text_value(&self.api_key_field),
            project_id: text_value(&self.project_id_field),
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

    pub fn project_id_value(&self) -> Option<String> {
        optional_text(&text_value(&self.project_id_field))
    }
}
