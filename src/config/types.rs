use serde::Deserialize;

use super::defaults::{
    default_always_on, default_auto_check_updates, default_correction_key,
    default_deepgram_language, default_deepgram_model, default_endpointing_ms, default_gain,
    default_hold_ms, default_hotkey, default_overlay_font_size, default_sample_rate,
    default_transformation_auto, default_transformation_hotkey, default_utterance_end_ms,
};
use super::prompts::{
    default_transformation_correction_system_prompt, default_transformation_system_prompt,
};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub ui: UiConfig,

    #[serde(default)]
    pub mic: MicConfig,

    #[serde(default)]
    pub deepgram: DeepgramConfig,

    #[serde(default)]
    pub transformation: TransformationConfig,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum UiMeterStyle {
    None,
    AnimatedHeight,
    AnimatedColor,
    /// Rounded bars across the text column showing the level's recent
    /// history, flowing right to left.
    #[default]
    Pills,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct UiConfig {
    #[serde(default)]
    pub start_on_login: bool,

    #[serde(default = "default_auto_check_updates")]
    pub auto_check_updates: bool,

    #[serde(default = "default_hotkey")]
    pub hotkey: String,

    #[serde(default = "default_correction_key", alias = "instruction_key")]
    pub correction_key: String,

    #[serde(alias = "overlay_font_family")]
    pub font_name: Option<String>,

    #[serde(default = "default_overlay_font_size")]
    pub font_size: f64,

    pub footer_font_size: Option<f64>,

    #[serde(default)]
    pub meter_style: UiMeterStyle,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            start_on_login: false,
            auto_check_updates: true,
            hotkey: default_hotkey(),
            correction_key: default_correction_key(),
            font_name: None,
            font_size: default_overlay_font_size(),
            footer_font_size: None,
            meter_style: UiMeterStyle::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MicConfig {
    pub audio_device: Option<String>,

    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    #[serde(default = "default_gain")]
    pub gain: f32,

    #[serde(default = "default_hold_ms")]
    pub hold_ms: u64,

    #[serde(default = "default_always_on")]
    pub always_on: bool,
}

impl Default for MicConfig {
    fn default() -> Self {
        Self {
            audio_device: None,
            sample_rate: default_sample_rate(),
            gain: default_gain(),
            hold_ms: default_hold_ms(),
            always_on: default_always_on(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DeepgramConfig {
    #[serde(default)]
    pub keyterms: Vec<String>,
    pub api_key: Option<String>,

    #[serde(default = "default_deepgram_language")]
    pub language: String,

    #[serde(default = "default_deepgram_model")]
    pub model: String,

    #[serde(default = "default_endpointing_ms")]
    pub endpointing_ms: u16,

    #[serde(default = "default_utterance_end_ms")]
    pub utterance_end_ms: u16,
}

impl Default for DeepgramConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            keyterms: vec![],
            language: default_deepgram_language(),
            model: default_deepgram_model(),
            endpointing_ms: default_endpointing_ms(),
            utterance_end_ms: default_utterance_end_ms(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TransformationConfig {
    #[serde(default = "default_transformation_hotkey")]
    pub hotkey: String,

    #[serde(default = "default_transformation_auto")]
    pub auto: bool,

    pub provider: Option<String>,

    pub api_key: Option<String>,

    /// The Ollama server the `ollama` provider talks to;
    /// `DEFAULT_OLLAMA_BASE_URL` when unset.
    pub ollama_base_url: Option<String>,

    /// Blank when unset: the provider's `default_model` is used then.
    #[serde(default)]
    pub model: String,

    #[serde(default = "default_transformation_system_prompt")]
    pub system_prompt: String,

    #[serde(
        default = "default_transformation_correction_system_prompt",
        alias = "instruction_system_prompt"
    )]
    pub correction_system_prompt: String,
}

impl Default for TransformationConfig {
    fn default() -> Self {
        Self {
            hotkey: default_transformation_hotkey(),
            auto: default_transformation_auto(),
            provider: None,
            api_key: None,
            ollama_base_url: None,
            model: String::new(),
            system_prompt: default_transformation_system_prompt(),
            correction_system_prompt: default_transformation_correction_system_prompt(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, UiMeterStyle};
    use crate::config::test_support::{assert_example_config_values, EXAMPLE_CONFIG};

    #[test]
    fn a_config_without_a_meter_style_uses_pills() {
        let config: Config = toml::from_str("[ui]\nfont_size = 14.0\n").unwrap();

        assert_eq!(config.ui.meter_style, UiMeterStyle::Pills);
        assert_eq!(Config::default().ui.meter_style, UiMeterStyle::Pills);
    }

    #[test]
    fn example_config_parses_with_expected_values() {
        let config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();

        assert_example_config_values(&config);
    }

    #[test]
    fn config_files_that_still_set_a_deepgram_project_id_load() {
        let config: Config = toml::from_str(
            "[deepgram]\napi_key = \"dg-key\"\nproject_id = \"00000000-0000-0000-0000-000000000000\"\n",
        )
        .expect("a leftover project_id is ignored");
        assert_eq!(config.deepgram.api_key.as_deref(), Some("dg-key"));
    }
}
