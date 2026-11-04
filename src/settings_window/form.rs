//! Plain model of the values shown in the settings window, one struct per
//! settings area: one per pane, and `PromptsForm` for the prompt editors that
//! the Transformation pane shows below its form. The pane views only copy
//! values between their controls and these structs; every conversion to and
//! from `Config` (trimming, required and optional values, popup sentinel
//! titles, integer ranges) lives here so it can be tested without AppKit.
//!
//! Numbers are `None` when their field is empty. The fields' `NSNumberFormatter`
//! rejects anything that is not a number at entry, so only emptiness and the
//! config type's range remain to be checked on save.

use std::collections::BTreeSet;

use super::defaulted::DefaultedRow;
use super::labels::{
    settings_name, SettingsField, DEEPGRAM_MODEL, ENDPOINTING, HOLD_THRESHOLD, METER_STYLE,
    SAMPLE_RATE, UTTERANCE_END,
};
use crate::config::{
    default_transformation_correction_system_prompt, default_transformation_system_prompt,
    unsupported_provider_problem, Config, DeepgramConfig, DefaultResets, MicConfig,
    TransformationConfig, TransformationProvider, UiConfig, UiMeterStyle,
};

/// The font popup's title for an unset `ui.font_name`: the overlay then uses
/// the monospaced system font (`default_overlay_font` in `src/overlay/mod.rs`).
pub const SYSTEM_MONOSPACED_FONT_LABEL: &str = "System monospaced";
pub const TRANSFORMATION_PROVIDER_DISABLED_LABEL: &str = "Disabled";

/// Meter styles in popup order, with the popup title of each.
pub const METER_STYLE_TITLES: &[(UiMeterStyle, &str)] = &[
    (UiMeterStyle::Pills, "pills"),
    (UiMeterStyle::AnimatedColor, "animated-color"),
    (UiMeterStyle::AnimatedHeight, "animated-height"),
    (UiMeterStyle::None, "none"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsForm {
    /// Defaulted rows reset with their Reset button since the form was
    /// loaded; saved with the config so `save_config` drops their keys.
    pub resets: DefaultResets,
    pub general: GeneralForm,
    pub microphone: MicrophoneForm,
    pub deepgram: DeepgramForm,
    pub transformation: TransformationForm,
    pub prompts: PromptsForm,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneralForm {
    pub hotkey: String,
    pub hold_ms: Option<u64>,
    pub correction_key: String,
    /// Selected title of the overlay font popup.
    pub font_name_title: Option<String>,
    pub font_size: Option<f64>,
    pub footer_font_size: Option<f64>,
    /// Selected title of the meter style popup.
    pub meter_style_title: Option<String>,
    pub auto_check_updates: bool,
    pub start_on_login: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MicrophoneForm {
    /// Device value of the selected audio device option; `None` is the
    /// system default device.
    pub audio_device: Option<String>,
    pub sample_rate: Option<u64>,
    pub gain: f32,
    pub always_on: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeepgramForm {
    pub api_key: String,
    pub language: String,
    /// Comma-separated keyterms.
    pub keyterms: String,
    /// Selected title of the Deepgram model popup.
    pub model_title: Option<String>,
    pub endpointing_ms: Option<u64>,
    pub utterance_end_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransformationForm {
    pub hotkey: String,
    pub auto: bool,
    /// Selected title of the provider popup.
    pub provider_title: Option<String>,
    pub api_key: String,
    /// Blank for `DEFAULT_OLLAMA_BASE_URL`.
    pub ollama_base_url: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromptsForm {
    pub system_prompt: String,
    pub correction_system_prompt: String,
    /// Prompts reset to their default since the form was loaded; saved with
    /// the config so `save_config` drops their keys (see `DefaultResets`).
    pub resets: DefaultResets,
}

/// One of the prompt editors of the Transformation pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Prompt {
    Dictation,
    Correction,
}

impl PromptsForm {
    pub fn text(&self, prompt: Prompt) -> &str {
        match prompt {
            Prompt::Dictation => &self.system_prompt,
            Prompt::Correction => &self.correction_system_prompt,
        }
    }

    /// Replaces the prompt with its built-in default and records the reset.
    pub fn reset_to_default(&mut self, prompt: Prompt) {
        match prompt {
            Prompt::Dictation => {
                self.system_prompt = default_transformation_system_prompt();
                self.resets.insert("transformation.system_prompt");
            }
            Prompt::Correction => {
                self.correction_system_prompt = default_transformation_correction_system_prompt();
                self.resets
                    .insert("transformation.correction_system_prompt");
            }
        }
    }
}

impl SettingsForm {
    pub fn from_config(config: &Config) -> Self {
        Self {
            resets: DefaultResets::default(),
            general: GeneralForm {
                hotkey: config.ui.hotkey.clone(),
                hold_ms: Some(config.mic.hold_ms),
                correction_key: config.ui.correction_key.clone(),
                font_name_title: Some(
                    config
                        .ui
                        .font_name
                        .clone()
                        .unwrap_or_else(|| SYSTEM_MONOSPACED_FONT_LABEL.to_owned()),
                ),
                font_size: Some(config.ui.font_size),
                footer_font_size: config.ui.footer_font_size,
                meter_style_title: Some(meter_style_title(config.ui.meter_style).to_owned()),
                auto_check_updates: config.ui.auto_check_updates,
                start_on_login: config.ui.start_on_login,
            },
            microphone: MicrophoneForm {
                audio_device: config.mic.audio_device.clone(),
                sample_rate: Some(u64::from(config.mic.sample_rate)),
                gain: config.mic.gain,
                always_on: config.mic.always_on,
            },
            deepgram: DeepgramForm {
                api_key: config.deepgram.api_key.clone().unwrap_or_default(),
                language: config.deepgram.language.clone(),
                keyterms: config.deepgram.keyterms.join(", "),
                model_title: Some(config.deepgram.model.clone()),
                endpointing_ms: Some(u64::from(config.deepgram.endpointing_ms)),
                utterance_end_ms: Some(u64::from(config.deepgram.utterance_end_ms)),
            },
            transformation: TransformationForm {
                hotkey: config.transformation.hotkey.clone(),
                auto: config.transformation.auto,
                provider_title: Some(
                    config
                        .transformation
                        .provider
                        .clone()
                        .unwrap_or_else(|| TRANSFORMATION_PROVIDER_DISABLED_LABEL.to_owned()),
                ),
                api_key: config.transformation.api_key.clone().unwrap_or_default(),
                ollama_base_url: config
                    .transformation
                    .ollama_base_url
                    .clone()
                    .unwrap_or_default(),
                model: config.transformation.model.clone(),
            },
            prompts: PromptsForm {
                system_prompt: config.transformation.system_prompt.clone(),
                correction_system_prompt: config.transformation.correction_system_prompt.clone(),
                resets: DefaultResets::default(),
            },
        }
    }

    /// Blanks each text, number, and shortcut field whose key `config.toml`
    /// does not have (`written`), so the field shows the default it follows
    /// as its placeholder.
    pub fn follow_defaults_not_in(mut self, written: &BTreeSet<&str>) -> Self {
        for row in DefaultedRow::ALL {
            if !written.contains(row.config_key()) && row_is_blank_while_default(row) {
                self.blank_row(row);
            }
        }
        self
    }

    /// Puts the row back to its built-in default and records the reset for
    /// the next Save: a text, number, or shortcut field is blanked, any other
    /// control takes the default value.
    pub fn reset_row(&mut self, row: DefaultedRow) {
        let defaults = Config::default();
        match row {
            DefaultedRow::MeterStyle => {
                self.general.meter_style_title =
                    Some(meter_style_title(defaults.ui.meter_style).to_owned())
            }
            DefaultedRow::AutoCheckUpdates => {
                self.general.auto_check_updates = defaults.ui.auto_check_updates
            }
            DefaultedRow::StartOnLogin => self.general.start_on_login = defaults.ui.start_on_login,
            DefaultedRow::MicGain => self.microphone.gain = defaults.mic.gain,
            DefaultedRow::AlwaysOn => self.microphone.always_on = defaults.mic.always_on,
            DefaultedRow::DeepgramModel => {
                self.deepgram.model_title = Some(defaults.deepgram.model.clone())
            }
            DefaultedRow::AutoTransform => self.transformation.auto = defaults.transformation.auto,
            _ => self.blank_row(row),
        }
        self.resets.insert(row.config_key());
    }

    fn blank_row(&mut self, row: DefaultedRow) {
        match row {
            DefaultedRow::DictationShortcut => self.general.hotkey.clear(),
            DefaultedRow::HoldThreshold => self.general.hold_ms = None,
            DefaultedRow::CorrectionKey => self.general.correction_key.clear(),
            DefaultedRow::FontSize => self.general.font_size = None,
            DefaultedRow::SampleRate => self.microphone.sample_rate = None,
            DefaultedRow::DeepgramLanguage => self.deepgram.language.clear(),
            DefaultedRow::Endpointing => self.deepgram.endpointing_ms = None,
            DefaultedRow::UtteranceEnd => self.deepgram.utterance_end_ms = None,
            DefaultedRow::TransformShortcut => self.transformation.hotkey.clear(),
            _ => unreachable!("{row:?} has no blank form"),
        }
    }

    /// The keys `save_config` drops while they equal their default: rows
    /// reset since loading, reset prompts, and blank fields that follow the
    /// default.
    pub fn resets(&self) -> DefaultResets {
        let mut resets = self.resets.clone();
        resets.extend(&self.prompts.resets);
        for row in DefaultedRow::ALL {
            if row_is_blank_while_default(row) && self.row_is_blank(row) {
                resets.insert(row.config_key());
            }
        }
        resets
    }

    fn row_is_blank(&self, row: DefaultedRow) -> bool {
        match row {
            DefaultedRow::DictationShortcut => self.general.hotkey.trim().is_empty(),
            DefaultedRow::HoldThreshold => self.general.hold_ms.is_none(),
            DefaultedRow::CorrectionKey => self.general.correction_key.trim().is_empty(),
            DefaultedRow::FontSize => self.general.font_size.is_none(),
            DefaultedRow::SampleRate => self.microphone.sample_rate.is_none(),
            DefaultedRow::DeepgramLanguage => self.deepgram.language.trim().is_empty(),
            DefaultedRow::Endpointing => self.deepgram.endpointing_ms.is_none(),
            DefaultedRow::UtteranceEnd => self.deepgram.utterance_end_ms.is_none(),
            DefaultedRow::TransformShortcut => self.transformation.hotkey.trim().is_empty(),
            _ => false,
        }
    }

    /// The config the form edits. A blank text, number, or shortcut field
    /// takes its built-in default; `resets` says which keys follow it.
    pub fn to_config(&self) -> Result<Config, String> {
        let defaults = Config::default();
        let general = &self.general;
        let microphone = &self.microphone;
        let deepgram = &self.deepgram;
        let transformation = &self.transformation;

        Ok(Config {
            ui: UiConfig {
                start_on_login: general.start_on_login,
                auto_check_updates: general.auto_check_updates,
                hotkey: text_or_default(&general.hotkey, &defaults.ui.hotkey),
                correction_key: text_or_default(
                    &general.correction_key,
                    &defaults.ui.correction_key,
                ),
                font_name: optional_popup_value(
                    general.font_name_title.as_deref(),
                    SYSTEM_MONOSPACED_FONT_LABEL,
                ),
                font_size: general.font_size.unwrap_or(defaults.ui.font_size),
                footer_font_size: general.footer_font_size,
                meter_style: parse_meter_style(&required_popup_title(
                    general.meter_style_title.as_deref(),
                    METER_STYLE,
                )?)?,
            },
            mic: MicConfig {
                audio_device: microphone.audio_device.clone(),
                sample_rate: integer_or_default(
                    microphone.sample_rate,
                    defaults.mic.sample_rate,
                    SAMPLE_RATE,
                )?,
                gain: microphone.gain,
                hold_ms: integer_or_default(general.hold_ms, defaults.mic.hold_ms, HOLD_THRESHOLD)?,
                always_on: microphone.always_on,
            },
            deepgram: DeepgramConfig {
                api_key: optional_text(&deepgram.api_key),
                language: text_or_default(&deepgram.language, &defaults.deepgram.language),
                keyterms: parse_keyterms(&deepgram.keyterms),
                model: required_popup_title(deepgram.model_title.as_deref(), DEEPGRAM_MODEL)?,
                endpointing_ms: integer_or_default(
                    deepgram.endpointing_ms,
                    defaults.deepgram.endpointing_ms,
                    ENDPOINTING,
                )?,
                utterance_end_ms: integer_or_default(
                    deepgram.utterance_end_ms,
                    defaults.deepgram.utterance_end_ms,
                    UTTERANCE_END,
                )?,
            },
            transformation: TransformationConfig {
                hotkey: text_or_default(&transformation.hotkey, &defaults.transformation.hotkey),
                auto: transformation.auto,
                provider: optional_popup_value(
                    transformation.provider_title.as_deref(),
                    TRANSFORMATION_PROVIDER_DISABLED_LABEL,
                )
                .map(|provider| {
                    provider
                        .parse::<TransformationProvider>()
                        .map(|provider| provider.to_string())
                        .map_err(|_| {
                            unsupported_provider_problem(&provider).describe(settings_name)
                        })
                })
                .transpose()?,
                api_key: optional_text(&transformation.api_key),
                ollama_base_url: optional_text(&transformation.ollama_base_url),
                model: transformation.model.trim().to_owned(),
                system_prompt: self.prompts.system_prompt.clone(),
                correction_system_prompt: self.prompts.correction_system_prompt.clone(),
            },
        })
    }
}

/// Placeholder of the model field for the provider popup's value: the model
/// a blank field stands for, or that the provider needs one.
pub fn transformation_model_placeholder(provider: Option<&str>) -> String {
    match provider.map(str::parse::<TransformationProvider>) {
        Some(Ok(provider)) => match provider.default_model() {
            Some(model) => format!("Default: {model}"),
            None => format!("Required for {provider}"),
        },
        Some(Err(_)) | None => String::new(),
    }
}

/// Trimmed text, or `None` when the text is blank.
pub fn optional_text(value: &str) -> Option<String> {
    let trimmed_value = value.trim();
    if trimmed_value.is_empty() {
        None
    } else {
        Some(trimmed_value.to_owned())
    }
}

/// Trimmed popup title, or `None` when nothing, a blank title, or the popup's
/// `unset_title` sentinel item is selected.
pub fn optional_popup_value(selected_title: Option<&str>, unset_title: &str) -> Option<String> {
    optional_text(selected_title?).filter(|value| value != unset_title)
}

pub fn meter_style_title(meter_style: UiMeterStyle) -> &'static str {
    METER_STYLE_TITLES
        .iter()
        .find(|(style, _)| *style == meter_style)
        .map(|(_, title)| *title)
        .expect("every meter style has a popup title")
}

/// Mic gain label text, shown both when the window loads a config and while
/// the gain slider moves. `mic.gain` is in decibels.
pub fn mic_gain_label(gain_db: f32) -> String {
    format!("{:.1} dB", gain_db)
}

fn required_text(value: &str, field: SettingsField) -> Result<String, String> {
    optional_text(value).ok_or_else(|| format!("{}: required", field))
}

fn required_popup_title(
    selected_title: Option<&str>,
    field: SettingsField,
) -> Result<String, String> {
    required_text(selected_title.unwrap_or_default(), field)
}

/// Whether the row's field is blank while it follows its default, rather
/// than showing the default value.
fn row_is_blank_while_default(row: DefaultedRow) -> bool {
    !matches!(
        row,
        DefaultedRow::MeterStyle
            | DefaultedRow::AutoCheckUpdates
            | DefaultedRow::StartOnLogin
            | DefaultedRow::MicGain
            | DefaultedRow::AlwaysOn
            | DefaultedRow::DeepgramModel
            | DefaultedRow::AutoTransform
    )
}

/// Trimmed text, or `default` for a blank field.
fn text_or_default(value: &str, default: &str) -> String {
    optional_text(value).unwrap_or_else(|| default.to_owned())
}

fn integer_or_default<T>(value: Option<u64>, default: T, field: SettingsField) -> Result<T, String>
where
    T: TryFrom<u64>,
    T::Error: std::fmt::Display,
{
    match value {
        Some(value) => T::try_from(value)
            .map_err(|error| format!("{}: must be an unsigned integer: {}", field, error)),
        None => Ok(default),
    }
}

fn parse_keyterms(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|keyterm| !keyterm.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_meter_style(raw_value: &str) -> Result<UiMeterStyle, String> {
    let raw_value = raw_value.trim();
    METER_STYLE_TITLES
        .iter()
        .find(|(_, title)| *title == raw_value)
        .map(|(style, _)| *style)
        .ok_or_else(|| {
            format!(
                "{}: must be one of: {} (got \"{}\")",
                METER_STYLE,
                METER_STYLE_TITLES
                    .iter()
                    .map(|(_, title)| *title)
                    .collect::<Vec<_>>()
                    .join(", "),
                raw_value
            )
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        mic_gain_label, DeepgramForm, GeneralForm, MicrophoneForm, Prompt, PromptsForm,
        SettingsForm, TransformationForm, METER_STYLE_TITLES,
    };
    use crate::config::{
        default_transformation_correction_system_prompt, default_transformation_system_prompt,
        Config, DeepgramConfig, DefaultResets, MicConfig, TransformationConfig, UiConfig,
        UiMeterStyle,
    };
    use crate::settings_window::defaulted::DefaultedRow;

    /// The error a form edit should produce, and the edit.
    type FormEditCase = (&'static str, fn(&mut SettingsForm));

    fn customized_config() -> Config {
        Config {
            ui: UiConfig {
                start_on_login: true,
                auto_check_updates: false,
                hotkey: "Cmd+F9".to_owned(),
                correction_key: "Alt+Cmd".to_owned(),
                font_name: Some("Menlo".to_owned()),
                font_size: 15.5,
                footer_font_size: Some(10.25),
                meter_style: UiMeterStyle::AnimatedHeight,
            },
            mic: MicConfig {
                audio_device: Some("Shure MV7".to_owned()),
                sample_rate: 48000,
                gain: 2.25,
                hold_ms: 750,
                always_on: false,
            },
            deepgram: DeepgramConfig {
                keyterms: vec!["macOS".to_owned(), "GitHub".to_owned()],
                api_key: Some("dg-key".to_owned()),
                language: "de-DE".to_owned(),
                model: "nova-2-meeting".to_owned(),
                endpointing_ms: 450,
                utterance_end_ms: 1500,
            },
            transformation: TransformationConfig {
                hotkey: "F8".to_owned(),
                auto: false,
                provider: Some("openai".to_owned()),
                api_key: Some("llm-key".to_owned()),
                ollama_base_url: Some("http://ollama.lan:11434".to_owned()),
                model: "gpt-test".to_owned(),
                system_prompt: "Dictation prompt\nwith two lines".to_owned(),
                correction_system_prompt: "  Correction prompt keeps whitespace  ".to_owned(),
            },
        }
    }

    #[test]
    fn default_config_round_trips_through_form() {
        let config = Config::default();

        assert_eq!(SettingsForm::from_config(&config).to_config(), Ok(config));
    }

    #[test]
    fn customized_config_round_trips_through_form() {
        let config = customized_config();

        assert_eq!(SettingsForm::from_config(&config).to_config(), Ok(config));
    }

    #[test]
    fn from_config_maps_every_edited_field_to_its_pane() {
        let form = SettingsForm::from_config(&customized_config());

        assert_eq!(
            form.general,
            GeneralForm {
                hotkey: "Cmd+F9".to_owned(),
                hold_ms: Some(750),
                correction_key: "Alt+Cmd".to_owned(),
                font_name_title: Some("Menlo".to_owned()),
                font_size: Some(15.5),
                footer_font_size: Some(10.25),
                meter_style_title: Some("animated-height".to_owned()),
                auto_check_updates: false,
                start_on_login: true,
            }
        );
        assert_eq!(
            form.microphone,
            MicrophoneForm {
                audio_device: Some("Shure MV7".to_owned()),
                sample_rate: Some(48000),
                gain: 2.25,
                always_on: false,
            }
        );
        assert_eq!(
            form.deepgram,
            DeepgramForm {
                api_key: "dg-key".to_owned(),
                language: "de-DE".to_owned(),
                keyterms: "macOS, GitHub".to_owned(),
                model_title: Some("nova-2-meeting".to_owned()),
                endpointing_ms: Some(450),
                utterance_end_ms: Some(1500),
            }
        );
        assert_eq!(
            form.transformation,
            TransformationForm {
                hotkey: "F8".to_owned(),
                auto: false,
                provider_title: Some("openai".to_owned()),
                api_key: "llm-key".to_owned(),
                ollama_base_url: "http://ollama.lan:11434".to_owned(),
                model: "gpt-test".to_owned(),
            }
        );
        assert_eq!(
            form.prompts,
            PromptsForm {
                system_prompt: "Dictation prompt\nwith two lines".to_owned(),
                correction_system_prompt: "  Correction prompt keeps whitespace  ".to_owned(),
                resets: DefaultResets::default(),
            }
        );
    }

    #[test]
    fn unset_font_and_provider_select_their_sentinel_popup_titles() {
        let form = SettingsForm::from_config(&Config::default());

        assert_eq!(
            form.general.font_name_title.as_deref(),
            Some("System monospaced")
        );
        assert_eq!(
            form.transformation.provider_title.as_deref(),
            Some("Disabled")
        );
        assert_eq!(form.general.footer_font_size, None);
        assert_eq!(form.deepgram.api_key, "");
        assert_eq!(form.transformation.api_key, "");
    }

    #[test]
    fn sentinel_and_blank_popup_titles_read_back_as_unset() {
        for title in [None, Some(""), Some("  "), Some("System monospaced")] {
            let mut form = SettingsForm::from_config(&customized_config());
            form.general.font_name_title = title.map(str::to_owned);

            assert_eq!(form.to_config().unwrap().ui.font_name, None, "{title:?}");
        }

        for title in [None, Some(""), Some("  "), Some("Disabled")] {
            let mut form = SettingsForm::from_config(&customized_config());
            form.transformation.provider_title = title.map(str::to_owned);

            assert_eq!(
                form.to_config().unwrap().transformation.provider,
                None,
                "{title:?}"
            );
        }
    }

    #[test]
    fn text_values_are_trimmed_and_blank_optional_values_are_unset() {
        let mut form = SettingsForm::from_config(&customized_config());
        form.general.hotkey = "  F9 ".to_owned();
        form.general.font_name_title = Some(" Menlo ".to_owned());
        form.deepgram.api_key = "   ".to_owned();
        form.deepgram.language = " en-GB ".to_owned();
        form.transformation.api_key = "".to_owned();
        form.transformation.provider_title = Some(" Anthropic ".to_owned());
        form.transformation.model = " claude-test ".to_owned();

        let config = form.to_config().unwrap();

        assert_eq!(config.ui.hotkey, "F9");
        assert_eq!(config.ui.font_name.as_deref(), Some("Menlo"));
        assert_eq!(config.deepgram.api_key, None);
        assert_eq!(config.deepgram.language, "en-GB");
        assert_eq!(config.transformation.api_key, None);
        assert_eq!(config.transformation.provider.as_deref(), Some("anthropic"));
        assert_eq!(config.transformation.model, "claude-test");
    }

    #[test]
    fn prompts_are_saved_verbatim() {
        let mut form = SettingsForm::from_config(&customized_config());
        form.prompts.system_prompt = "".to_owned();
        form.prompts.correction_system_prompt = "\n  indented\n".to_owned();

        let config = form.to_config().unwrap();

        assert_eq!(config.transformation.system_prompt, "");
        assert_eq!(
            config.transformation.correction_system_prompt,
            "\n  indented\n"
        );
    }

    #[test]
    fn resetting_the_dictation_prompt_restores_its_default_and_records_the_reset() {
        let mut form = SettingsForm::from_config(&customized_config());

        form.prompts.reset_to_default(Prompt::Dictation);

        assert!(form.prompts.text(Prompt::Dictation).contains("\n\n"));
        assert_eq!(
            form.prompts,
            PromptsForm {
                system_prompt: default_transformation_system_prompt(),
                correction_system_prompt: "  Correction prompt keeps whitespace  ".to_owned(),
                resets: DefaultResets::from_iter(["transformation.system_prompt"]),
            }
        );
        assert_eq!(
            form.to_config().unwrap().transformation.system_prompt,
            default_transformation_system_prompt()
        );
    }

    #[test]
    fn resetting_the_correction_prompt_restores_its_default_and_records_the_reset() {
        let mut form = SettingsForm::from_config(&customized_config());

        form.prompts.reset_to_default(Prompt::Correction);

        assert!(form.prompts.text(Prompt::Correction).contains("\n\n"));
        assert_eq!(
            form.prompts,
            PromptsForm {
                system_prompt: "Dictation prompt\nwith two lines".to_owned(),
                correction_system_prompt: default_transformation_correction_system_prompt(),
                resets: DefaultResets::from_iter(["transformation.correction_system_prompt"]),
            }
        );
        assert_eq!(
            form.prompts.text(Prompt::Correction),
            default_transformation_correction_system_prompt()
        );
    }

    #[test]
    fn editing_a_prompt_after_its_reset_keeps_the_reset_and_the_edit() {
        let mut form = SettingsForm::from_config(&customized_config());
        form.prompts.reset_to_default(Prompt::Dictation);
        form.prompts.system_prompt = "Edited after reset".to_owned();

        assert!(form.prompts.resets.contains("transformation.system_prompt"));
        assert_eq!(
            form.to_config().unwrap().transformation.system_prompt,
            "Edited after reset"
        );
    }

    #[test]
    fn a_loaded_form_has_no_prompt_resets() {
        for config in [Config::default(), customized_config()] {
            assert_eq!(
                SettingsForm::from_config(&config).prompts.resets,
                DefaultResets::default()
            );
        }
    }

    #[test]
    fn keyterms_are_split_on_commas_and_blank_entries_dropped() {
        let mut form = SettingsForm::from_config(&customized_config());
        form.deepgram.keyterms = " macOS ,, GitHub , ,Rust".to_owned();
        assert_eq!(
            form.to_config().unwrap().deepgram.keyterms,
            vec!["macOS", "GitHub", "Rust"]
        );

        form.deepgram.keyterms = "  ".to_owned();
        assert!(form.to_config().unwrap().deepgram.keyterms.is_empty());
    }

    #[test]
    fn an_unselected_popup_is_rejected_with_its_field_name() {
        let cases: [FormEditCase; 2] = [
            ("General > Meter style: required", |form| {
                form.general.meter_style_title = None
            }),
            ("Deepgram > Model: required", |form| {
                form.deepgram.model_title = Some(" ".to_owned())
            }),
        ];

        for (expected_error, edit) in cases {
            let mut form = SettingsForm::from_config(&customized_config());
            edit(&mut form);

            assert_eq!(form.to_config(), Err(expected_error.to_owned()));
        }
    }

    #[test]
    fn a_blank_field_takes_its_default_and_follows_it() {
        let mut form = SettingsForm::from_config(&customized_config());
        form.general.hotkey = " ".to_owned();
        form.general.hold_ms = None;
        form.general.font_size = None;
        form.microphone.sample_rate = None;
        form.deepgram.language.clear();
        form.deepgram.endpointing_ms = None;
        form.transformation.hotkey.clear();

        let config = form.to_config().unwrap();
        let defaults = Config::default();
        assert_eq!(config.ui.hotkey, defaults.ui.hotkey);
        assert_eq!(config.mic.hold_ms, defaults.mic.hold_ms);
        assert_eq!(config.ui.font_size, defaults.ui.font_size);
        assert_eq!(config.mic.sample_rate, defaults.mic.sample_rate);
        assert_eq!(config.deepgram.language, defaults.deepgram.language);
        assert_eq!(
            config.deepgram.endpointing_ms,
            defaults.deepgram.endpointing_ms
        );
        assert_eq!(config.transformation.hotkey, defaults.transformation.hotkey);
        assert_eq!(config.ui.correction_key, "Alt+Cmd");

        let resets = form.resets();
        for key in [
            "ui.hotkey",
            "mic.hold_ms",
            "ui.font_size",
            "mic.sample_rate",
            "deepgram.language",
            "deepgram.endpointing_ms",
            "transformation.hotkey",
        ] {
            assert!(resets.contains(key), "{key}");
        }
        assert!(!resets.contains("ui.correction_key"));
        assert!(!resets.contains("deepgram.utterance_end_ms"));
    }

    #[test]
    fn fields_whose_key_the_file_lacks_are_blank_and_the_rest_keep_their_values() {
        let written = BTreeSet::from(["ui.hotkey", "deepgram.endpointing_ms"]);

        let form = SettingsForm::from_config(&customized_config()).follow_defaults_not_in(&written);

        assert_eq!(form.general.hotkey, "Cmd+F9");
        assert_eq!(form.deepgram.endpointing_ms, Some(450));
        assert_eq!(form.general.correction_key, "");
        assert_eq!(form.general.hold_ms, None);
        assert_eq!(form.general.font_size, None);
        assert_eq!(form.microphone.sample_rate, None);
        assert_eq!(form.deepgram.language, "");
        assert_eq!(form.deepgram.utterance_end_ms, None);
        assert_eq!(form.transformation.hotkey, "");
        // Controls that cannot be blank keep showing the value.
        assert_eq!(
            form.general.meter_style_title.as_deref(),
            Some("animated-height")
        );
        assert!(form.general.start_on_login);
        assert_eq!(form.microphone.gain, 2.25);
        assert_eq!(form.deepgram.model_title.as_deref(), Some("nova-2-meeting"));
    }

    #[test]
    fn resetting_a_row_restores_its_default_and_records_the_reset() {
        let defaults = Config::default();
        let mut form = SettingsForm::from_config(&customized_config());

        for row in DefaultedRow::ALL {
            form.reset_row(row);
        }

        assert_eq!(form.general.hotkey, "");
        assert_eq!(form.general.hold_ms, None);
        assert_eq!(form.deepgram.utterance_end_ms, None);
        assert_eq!(
            form.general.meter_style_title.as_deref(),
            Some(super::meter_style_title(defaults.ui.meter_style))
        );
        assert_eq!(form.general.start_on_login, defaults.ui.start_on_login);
        assert_eq!(
            form.general.auto_check_updates,
            defaults.ui.auto_check_updates
        );
        assert_eq!(form.microphone.gain, defaults.mic.gain);
        assert_eq!(form.microphone.always_on, defaults.mic.always_on);
        assert_eq!(
            form.deepgram.model_title.as_deref(),
            Some(defaults.deepgram.model.as_str())
        );
        assert_eq!(form.transformation.auto, defaults.transformation.auto);

        let config = form.to_config().unwrap();
        assert_eq!(
            config.ui,
            UiConfig {
                font_name: Some("Menlo".to_owned()),
                footer_font_size: Some(10.25),
                ..defaults.ui.clone()
            }
        );
        let resets = form.resets();
        for row in DefaultedRow::ALL {
            assert!(resets.contains(row.config_key()), "{row:?}");
        }
    }

    #[test]
    fn integers_outside_the_config_type_range_are_rejected() {
        let cases: [FormEditCase; 3] = [
            (
                "Microphone > Sample rate: must be an unsigned integer",
                |form| form.microphone.sample_rate = Some(u64::from(u32::MAX) + 1),
            ),
            (
                "Deepgram > Endpointing (ms): must be an unsigned integer",
                |form| form.deepgram.endpointing_ms = Some(u64::from(u16::MAX) + 1),
            ),
            (
                "Deepgram > Utterance end (ms): must be an unsigned integer",
                |form| form.deepgram.utterance_end_ms = Some(u64::from(u16::MAX) + 1),
            ),
        ];

        for (expected_prefix, edit) in cases {
            let mut form = SettingsForm::from_config(&customized_config());
            edit(&mut form);

            let error = form.to_config().unwrap_err();
            assert!(error.starts_with(expected_prefix), "{error}");
        }
    }

    #[test]
    fn integers_at_the_config_type_maximum_are_accepted() {
        let mut form = SettingsForm::from_config(&customized_config());
        form.microphone.sample_rate = Some(u64::from(u32::MAX));
        form.general.hold_ms = Some(u64::MAX);
        form.deepgram.endpointing_ms = Some(u64::from(u16::MAX));
        form.deepgram.utterance_end_ms = Some(0);

        let config = form.to_config().unwrap();

        assert_eq!(config.mic.sample_rate, u32::MAX);
        assert_eq!(config.mic.hold_ms, u64::MAX);
        assert_eq!(config.deepgram.endpointing_ms, u16::MAX);
        assert_eq!(config.deepgram.utterance_end_ms, 0);
    }

    #[test]
    fn empty_footer_font_size_is_unset() {
        let mut form = SettingsForm::from_config(&customized_config());
        form.general.footer_font_size = None;

        assert_eq!(form.to_config().unwrap().ui.footer_font_size, None);
    }

    #[test]
    fn unknown_meter_style_title_is_rejected() {
        let mut form = SettingsForm::from_config(&customized_config());
        form.general.meter_style_title = Some("sparkles".to_owned());

        let error = form.to_config().unwrap_err();

        assert!(
            error.starts_with("General > Meter style: must be one of: "),
            "{error}"
        );
        assert!(error.ends_with(" (got \"sparkles\")"), "{error}");
        for (_, title) in METER_STYLE_TITLES {
            assert!(error.contains(title), "{title} missing from {error}");
        }
    }

    #[test]
    fn every_meter_style_round_trips_through_its_popup_title() {
        for meter_style in [
            UiMeterStyle::Pills,
            UiMeterStyle::AnimatedColor,
            UiMeterStyle::AnimatedHeight,
            UiMeterStyle::None,
        ] {
            let mut config = customized_config();
            config.ui.meter_style = meter_style;

            assert_eq!(
                SettingsForm::from_config(&config)
                    .to_config()
                    .unwrap()
                    .ui
                    .meter_style,
                meter_style
            );
        }
    }

    #[test]
    fn mic_always_on_false_round_trips_through_form() {
        let mut config = Config::default();
        config.mic.always_on = false;

        assert_eq!(SettingsForm::from_config(&config).to_config(), Ok(config));
    }

    #[test]
    fn mic_always_on_true_round_trips_through_form() {
        let mut config = Config::default();
        config.mic.always_on = true;

        assert_eq!(SettingsForm::from_config(&config).to_config(), Ok(config));
    }

    #[test]
    fn mic_gain_label_shows_the_gain_in_db() {
        assert_eq!(mic_gain_label(0.0), "0.0 dB");
        assert_eq!(mic_gain_label(4.0), "4.0 dB");
        assert_eq!(mic_gain_label(4.5), "4.5 dB");
        assert_eq!(mic_gain_label(10.0), "10.0 dB");
    }

    #[test]
    fn a_blank_model_is_saved_blank_so_the_provider_default_applies() {
        let mut form = SettingsForm::from_config(&customized_config());
        form.transformation.model = "  ".to_owned();

        assert_eq!(form.to_config().unwrap().transformation.model, "");
    }

    #[test]
    fn the_model_placeholder_names_the_providers_default_or_requires_a_model() {
        use super::transformation_model_placeholder;

        assert_eq!(
            transformation_model_placeholder(Some("anthropic")),
            "Default: claude-haiku-5-5"
        );
        assert_eq!(
            transformation_model_placeholder(Some("ollama")),
            "Required for ollama"
        );
        assert_eq!(transformation_model_placeholder(Some("galadriel")), "");
        assert_eq!(transformation_model_placeholder(None), "");
    }
}
