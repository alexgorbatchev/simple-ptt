//! Pane titles and row labels of the settings window. The panes build their
//! rows from these, and every error that names a field names it by the same
//! pane and label, so the message points at a row the user can find.

use std::fmt;

pub const GENERAL_PANE_TITLE: &str = "General";
pub const MICROPHONE_PANE_TITLE: &str = "Microphone";
pub const DEEPGRAM_PANE_TITLE: &str = "Deepgram";
pub const TRANSFORMATION_PANE_TITLE: &str = "Transformation";

/// A settings row, named in errors as `Pane > Label`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsField {
    pub pane: &'static str,
    pub label: &'static str,
}

impl SettingsField {
    const fn new(pane: &'static str, label: &'static str) -> Self {
        Self { pane, label }
    }

    /// The row title shown in the pane's grid.
    pub fn row_title(self) -> String {
        format!("{}:", self.label)
    }
}

impl fmt::Display for SettingsField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} > {}", self.pane, self.label)
    }
}

pub const DICTATION_SHORTCUT: SettingsField =
    SettingsField::new(GENERAL_PANE_TITLE, "Dictation shortcut");
pub const HOLD_THRESHOLD: SettingsField =
    SettingsField::new(GENERAL_PANE_TITLE, "Hold threshold (ms)");
pub const CORRECTION_KEY: SettingsField = SettingsField::new(GENERAL_PANE_TITLE, "Correction key");
pub const FONT: SettingsField = SettingsField::new(GENERAL_PANE_TITLE, "Font");
pub const FONT_SIZE: SettingsField = SettingsField::new(GENERAL_PANE_TITLE, "Font size");
pub const FOOTER_FONT_SIZE: SettingsField =
    SettingsField::new(GENERAL_PANE_TITLE, "Footer font size");
pub const METER_STYLE: SettingsField = SettingsField::new(GENERAL_PANE_TITLE, "Meter style");

pub const AUDIO_DEVICE: SettingsField = SettingsField::new(MICROPHONE_PANE_TITLE, "Audio device");
pub const SAMPLE_RATE: SettingsField = SettingsField::new(MICROPHONE_PANE_TITLE, "Sample rate");
pub const MIC_GAIN: SettingsField = SettingsField::new(MICROPHONE_PANE_TITLE, "Mic gain (dB)");

pub const DEEPGRAM_API_KEY: SettingsField = SettingsField::new(DEEPGRAM_PANE_TITLE, "API key");
pub const DEEPGRAM_LANGUAGE: SettingsField = SettingsField::new(DEEPGRAM_PANE_TITLE, "Language");
pub const DEEPGRAM_KEYTERMS: SettingsField = SettingsField::new(DEEPGRAM_PANE_TITLE, "Keyterms");
pub const DEEPGRAM_MODEL: SettingsField = SettingsField::new(DEEPGRAM_PANE_TITLE, "Model");
pub const ENDPOINTING: SettingsField = SettingsField::new(DEEPGRAM_PANE_TITLE, "Endpointing (ms)");
pub const UTTERANCE_END: SettingsField =
    SettingsField::new(DEEPGRAM_PANE_TITLE, "Utterance end (ms)");

pub const TRANSFORM_SHORTCUT: SettingsField =
    SettingsField::new(TRANSFORMATION_PANE_TITLE, "Transform shortcut");
pub const TRANSFORMATION_PROVIDER: SettingsField =
    SettingsField::new(TRANSFORMATION_PANE_TITLE, "Provider");
pub const TRANSFORMATION_API_KEY: SettingsField =
    SettingsField::new(TRANSFORMATION_PANE_TITLE, "API key");
pub const TRANSFORMATION_MODEL: SettingsField =
    SettingsField::new(TRANSFORMATION_PANE_TITLE, "Model");
pub const OLLAMA_SERVER: SettingsField =
    SettingsField::new(TRANSFORMATION_PANE_TITLE, "Ollama server");

/// The row that edits a config key `config::problems` can report.
pub fn field_for_config_key(key: &str) -> Option<SettingsField> {
    Some(match key {
        "ui.hotkey" => DICTATION_SHORTCUT,
        "ui.correction_key" => CORRECTION_KEY,
        "ui.font_size" => FONT_SIZE,
        "ui.footer_font_size" => FOOTER_FONT_SIZE,
        "mic.gain" => MIC_GAIN,
        "transformation.hotkey" => TRANSFORM_SHORTCUT,
        "transformation.provider" => TRANSFORMATION_PROVIDER,
        "transformation.model" => TRANSFORMATION_MODEL,
        "transformation.ollama_base_url" => OLLAMA_SERVER,
        _ => return None,
    })
}

/// A config key named the way the settings window names its row, or the key
/// itself when no row edits it.
pub fn settings_name(key: &str) -> String {
    field_for_config_key(key).map_or_else(|| key.to_owned(), |field| field.to_string())
}

#[cfg(test)]
mod tests {
    use super::{field_for_config_key, settings_name, DICTATION_SHORTCUT, HOLD_THRESHOLD};
    use crate::config::{problems, Config};

    #[test]
    fn fields_are_named_by_pane_and_label() {
        assert_eq!(DICTATION_SHORTCUT.to_string(), "General > Dictation shortcut");
        assert_eq!(HOLD_THRESHOLD.row_title(), "Hold threshold (ms):");
    }

    #[test]
    fn every_key_config_problems_reports_has_a_settings_row() {
        let mut config = Config::default();
        config.ui.hotkey = "Nope".to_owned();
        config.ui.correction_key = "Nope".to_owned();
        config.ui.font_size = 0.0;
        config.ui.footer_font_size = Some(0.0);
        config.mic.gain = 40.0;
        config.transformation.provider = Some("ollama".to_owned());
        config.transformation.hotkey = "Nope".to_owned();
        config.transformation.ollama_base_url = Some("nope".to_owned());
        let mut keys: Vec<_> = problems(&config).iter().map(|problem| problem.key).collect();
        config.transformation.provider = Some("galadriel".to_owned());
        keys.extend(problems(&config).iter().map(|problem| problem.key));

        for key in keys {
            assert!(field_for_config_key(key).is_some(), "{key} has no settings row");
        }
    }

    #[test]
    fn a_key_without_a_row_keeps_its_config_name() {
        assert_eq!(settings_name("ui.hotkey"), "General > Dictation shortcut");
        assert_eq!(settings_name("deepgram.keyterms"), "deepgram.keyterms");
    }
}
