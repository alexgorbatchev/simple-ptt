//! Plain model of the values shown in the settings window, one struct per
//! pane. The pane views only copy values between their controls and these
//! structs; every conversion to and from `Config` (trimming, required and
//! optional values, popup sentinel titles, integer ranges) lives here so it can
//! be tested without AppKit.
//!
//! Numbers are `None` when their field is empty. The fields' `NSNumberFormatter`
//! rejects anything that is not a number at entry, so only emptiness and the
//! config type's range remain to be checked on save.

use crate::config::{
    Config, DeepgramConfig, MicConfig, TransformationConfig, UiConfig, UiMeterStyle,
};

pub const SYSTEM_DEFAULT_FONT_LABEL: &str = "System default";
pub const TRANSFORMATION_PROVIDER_DISABLED_LABEL: &str = "Disabled";

/// Meter styles in popup order, with the popup title of each.
pub const METER_STYLE_TITLES: &[(UiMeterStyle, &str)] = &[
    (UiMeterStyle::AnimatedColor, "animated-color"),
    (UiMeterStyle::AnimatedHeight, "animated-height"),
    (UiMeterStyle::None, "none"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsForm {
    pub general: GeneralForm,
    pub microphone: MicrophoneForm,
    pub deepgram: DeepgramForm,
    pub transformation: TransformationForm,
    pub prompts: PromptsForm,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneralForm {
    pub hotkey: String,
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
    pub hold_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeepgramForm {
    pub api_key: String,
    pub project_id: String,
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
    pub model: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromptsForm {
    pub system_prompt: String,
    pub correction_system_prompt: String,
}

impl SettingsForm {
    pub fn from_config(config: &Config) -> Self {
        Self {
            general: GeneralForm {
                hotkey: config.ui.hotkey.clone(),
                correction_key: config.ui.correction_key.clone(),
                font_name_title: Some(
                    config
                        .ui
                        .font_name
                        .clone()
                        .unwrap_or_else(|| SYSTEM_DEFAULT_FONT_LABEL.to_owned()),
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
                hold_ms: Some(config.mic.hold_ms),
            },
            deepgram: DeepgramForm {
                api_key: config.deepgram.api_key.clone().unwrap_or_default(),
                project_id: config.deepgram.project_id.clone().unwrap_or_default(),
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
                model: config.transformation.model.clone(),
            },
            prompts: PromptsForm {
                system_prompt: config.transformation.system_prompt.clone(),
                correction_system_prompt: config.transformation.correction_system_prompt.clone(),
            },
        }
    }

    pub fn to_config(&self) -> Result<Config, String> {
        let general = &self.general;
        let microphone = &self.microphone;
        let deepgram = &self.deepgram;
        let transformation = &self.transformation;

        Ok(Config {
            ui: UiConfig {
                start_on_login: general.start_on_login,
                auto_check_updates: general.auto_check_updates,
                hotkey: required_text(&general.hotkey, "Record hotkey")?,
                correction_key: required_text(&general.correction_key, "Correction key")?,
                font_name: optional_popup_value(
                    general.font_name_title.as_deref(),
                    SYSTEM_DEFAULT_FONT_LABEL,
                ),
                font_size: required_number(general.font_size, "Font size")?,
                footer_font_size: general.footer_font_size,
                meter_style: parse_meter_style(&required_popup_title(
                    general.meter_style_title.as_deref(),
                    "Meter style",
                )?)?,
            },
            mic: MicConfig {
                audio_device: microphone.audio_device.clone(),
                sample_rate: required_integer(microphone.sample_rate, "Sample rate")?,
                gain: microphone.gain,
                hold_ms: required_integer(microphone.hold_ms, "Hold ms")?,
                // The window does not load or read its always-on checkbox yet
                // (#4), so saving always writes `true`.
                always_on: true,
            },
            deepgram: DeepgramConfig {
                api_key: optional_text(&deepgram.api_key),
                project_id: optional_text(&deepgram.project_id),
                language: required_text(&deepgram.language, "Deepgram language")?,
                keyterms: parse_keyterms(&deepgram.keyterms),
                model: required_popup_title(deepgram.model_title.as_deref(), "Deepgram model")?,
                endpointing_ms: required_integer(deepgram.endpointing_ms, "Endpointing ms")?,
                utterance_end_ms: required_integer(deepgram.utterance_end_ms, "Utterance end ms")?,
            },
            transformation: TransformationConfig {
                hotkey: required_text(&transformation.hotkey, "Transform hotkey")?,
                auto: transformation.auto,
                provider: optional_popup_value(
                    transformation.provider_title.as_deref(),
                    TRANSFORMATION_PROVIDER_DISABLED_LABEL,
                ),
                api_key: optional_text(&transformation.api_key),
                model: required_text(&transformation.model, "Transformation model")?,
                system_prompt: self.prompts.system_prompt.clone(),
                correction_system_prompt: self.prompts.correction_system_prompt.clone(),
            },
        })
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

/// Mic gain label text shown when the window loads a config.
pub fn loaded_mic_gain_label(gain: f32) -> String {
    format!("{:.1}", gain / 1.5)
}

/// Mic gain label text shown while the gain slider moves.
pub fn mic_gain_label(gain_db: f32) -> String {
    format!("{:.1} dB", gain_db)
}

fn required_text(value: &str, field_name: &str) -> Result<String, String> {
    optional_text(value).ok_or_else(|| format!("{} is required", field_name))
}

fn required_popup_title(selected_title: Option<&str>, field_name: &str) -> Result<String, String> {
    required_text(selected_title.unwrap_or_default(), field_name)
}

fn required_number(value: Option<f64>, field_name: &str) -> Result<f64, String> {
    value.ok_or_else(|| format!("{} is required", field_name))
}

fn required_integer<T>(value: Option<u64>, field_name: &str) -> Result<T, String>
where
    T: TryFrom<u64>,
    T::Error: std::fmt::Display,
{
    let value = value.ok_or_else(|| format!("{} is required", field_name))?;
    T::try_from(value)
        .map_err(|error| format!("{} must be an unsigned integer: {}", field_name, error))
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
                "Meter style must be one of: animated-color, animated-height, none (got '{}')",
                raw_value
            )
        })
}

#[cfg(test)]
mod tests {
    use super::{
        loaded_mic_gain_label, mic_gain_label, DeepgramForm, GeneralForm, MicrophoneForm,
        PromptsForm, SettingsForm, TransformationForm,
    };
    use crate::config::{
        Config, DeepgramConfig, MicConfig, TransformationConfig, UiConfig, UiMeterStyle,
    };

    fn customized_config() -> Config {
        Config {
            ui: UiConfig {
                start_on_login: true,
                auto_check_updates: false,
                hotkey: "Cmd+F9".to_owned(),
                correction_key: "RightAlt".to_owned(),
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
                always_on: true,
            },
            deepgram: DeepgramConfig {
                keyterms: vec!["macOS".to_owned(), "GitHub".to_owned()],
                api_key: Some("dg-key".to_owned()),
                project_id: Some("project-1".to_owned()),
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
                correction_key: "RightAlt".to_owned(),
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
                hold_ms: Some(750),
            }
        );
        assert_eq!(
            form.deepgram,
            DeepgramForm {
                api_key: "dg-key".to_owned(),
                project_id: "project-1".to_owned(),
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
                model: "gpt-test".to_owned(),
            }
        );
        assert_eq!(
            form.prompts,
            PromptsForm {
                system_prompt: "Dictation prompt\nwith two lines".to_owned(),
                correction_system_prompt: "  Correction prompt keeps whitespace  ".to_owned(),
            }
        );
    }

    #[test]
    fn unset_font_and_provider_select_their_sentinel_popup_titles() {
        let form = SettingsForm::from_config(&Config::default());

        assert_eq!(
            form.general.font_name_title.as_deref(),
            Some("System default")
        );
        assert_eq!(
            form.transformation.provider_title.as_deref(),
            Some("Disabled")
        );
        assert_eq!(form.general.footer_font_size, None);
        assert_eq!(form.deepgram.api_key, "");
        assert_eq!(form.deepgram.project_id, "");
        assert_eq!(form.transformation.api_key, "");
    }

    #[test]
    fn sentinel_and_blank_popup_titles_read_back_as_unset() {
        for title in [None, Some(""), Some("  "), Some("System default")] {
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
        form.deepgram.project_id = " project-2 ".to_owned();
        form.deepgram.language = " en-GB ".to_owned();
        form.transformation.api_key = "".to_owned();
        form.transformation.provider_title = Some(" anthropic ".to_owned());
        form.transformation.model = " claude-test ".to_owned();

        let config = form.to_config().unwrap();

        assert_eq!(config.ui.hotkey, "F9");
        assert_eq!(config.ui.font_name.as_deref(), Some("Menlo"));
        assert_eq!(config.deepgram.api_key, None);
        assert_eq!(config.deepgram.project_id.as_deref(), Some("project-2"));
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
    fn blank_required_text_values_are_rejected_with_field_names() {
        let cases: [(&str, fn(&mut SettingsForm)); 7] = [
            ("Record hotkey is required", |form| {
                form.general.hotkey = " ".to_owned()
            }),
            ("Correction key is required", |form| {
                form.general.correction_key = "".to_owned()
            }),
            ("Meter style is required", |form| {
                form.general.meter_style_title = None
            }),
            ("Deepgram language is required", |form| {
                form.deepgram.language = "\t".to_owned()
            }),
            ("Deepgram model is required", |form| {
                form.deepgram.model_title = Some(" ".to_owned())
            }),
            ("Transform hotkey is required", |form| {
                form.transformation.hotkey = "".to_owned()
            }),
            ("Transformation model is required", |form| {
                form.transformation.model = "  ".to_owned()
            }),
        ];

        for (expected_error, edit) in cases {
            let mut form = SettingsForm::from_config(&customized_config());
            edit(&mut form);

            assert_eq!(form.to_config(), Err(expected_error.to_owned()));
        }
    }

    #[test]
    fn empty_required_numbers_are_rejected_with_field_names() {
        let cases: [(&str, fn(&mut SettingsForm)); 5] = [
            ("Font size is required", |form| {
                form.general.font_size = None
            }),
            ("Sample rate is required", |form| {
                form.microphone.sample_rate = None
            }),
            ("Hold ms is required", |form| form.microphone.hold_ms = None),
            ("Endpointing ms is required", |form| {
                form.deepgram.endpointing_ms = None
            }),
            ("Utterance end ms is required", |form| {
                form.deepgram.utterance_end_ms = None
            }),
        ];

        for (expected_error, edit) in cases {
            let mut form = SettingsForm::from_config(&customized_config());
            edit(&mut form);

            assert_eq!(form.to_config(), Err(expected_error.to_owned()));
        }
    }

    #[test]
    fn integers_outside_the_config_type_range_are_rejected() {
        let cases: [(&str, fn(&mut SettingsForm)); 3] = [
            ("Sample rate must be an unsigned integer", |form| {
                form.microphone.sample_rate = Some(u64::from(u32::MAX) + 1)
            }),
            ("Endpointing ms must be an unsigned integer", |form| {
                form.deepgram.endpointing_ms = Some(u64::from(u16::MAX) + 1)
            }),
            ("Utterance end ms must be an unsigned integer", |form| {
                form.deepgram.utterance_end_ms = Some(u64::from(u16::MAX) + 1)
            }),
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
        form.microphone.hold_ms = Some(u64::MAX);
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

        assert_eq!(
            form.to_config(),
            Err(
                "Meter style must be one of: animated-color, animated-height, none (got 'sparkles')"
                    .to_owned()
            )
        );
    }

    #[test]
    fn every_meter_style_round_trips_through_its_popup_title() {
        for meter_style in [
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

    // The window has no working control for `mic.always_on` yet (#4): the
    // checkbox is neither loaded nor read, and saving writes `true`.
    #[test]
    fn saving_writes_always_on_true_regardless_of_loaded_value() {
        let mut config = customized_config();
        config.mic.always_on = false;

        assert!(
            SettingsForm::from_config(&config)
                .to_config()
                .unwrap()
                .mic
                .always_on
        );
    }

    // Current label semantics (#5): the loaded label divides the gain by 1.5
    // without a unit, while slider changes show the raw gain with a dB unit.
    #[test]
    fn mic_gain_labels_keep_current_formats() {
        assert_eq!(loaded_mic_gain_label(4.5), "3.0");
        assert_eq!(mic_gain_label(4.5), "4.5 dB");
    }
}
