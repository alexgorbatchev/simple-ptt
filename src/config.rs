use serde::Deserialize;
use std::path::PathBuf;

use crate::billing::deepgram_project_id_env_var;
use crate::transformation::TransformationRuntimeConfig;

const CONFIG_OVERRIDE_ENV_VAR: &str = "SIMPLE_PTT_CONFIG";
const DEFAULT_CONFIG_FILE_NAME: &str = "config.toml";
const XDG_APP_NAME: &str = "simple-ptt";

#[derive(Debug, Default, Deserialize)]
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
    #[default]
    AnimatedColor,
}

#[derive(Debug, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,

    #[serde(alias = "overlay_font_family")]
    pub font_name: Option<String>,

    #[serde(default = "default_overlay_font_size")]
    pub font_size: f64,

    pub footer_font_size: Option<f64>,

    #[serde(default = "default_ui_meter_style")]
    pub meter_style: UiMeterStyle,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            font_name: None,
            font_size: default_overlay_font_size(),
            footer_font_size: None,
            meter_style: default_ui_meter_style(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MicConfig {
    pub audio_device: Option<String>,

    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    #[serde(default = "default_gain")]
    pub gain: f32,

    #[serde(default = "default_hold_ms")]
    pub hold_ms: u64,
}

impl Default for MicConfig {
    fn default() -> Self {
        Self {
            audio_device: None,
            sample_rate: default_sample_rate(),
            gain: default_gain(),
            hold_ms: default_hold_ms(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DeepgramConfig {
    pub api_key: Option<String>,

    pub project_id: Option<String>,

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
            project_id: None,
            language: default_deepgram_language(),
            model: default_deepgram_model(),
            endpointing_ms: default_endpointing_ms(),
            utterance_end_ms: default_utterance_end_ms(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TransformationConfig {
    #[serde(default = "default_transformation_hotkey")]
    pub hotkey: String,

    pub provider: Option<String>,

    pub api_key: Option<String>,

    #[serde(default = "default_transformation_model")]
    pub model: String,

    #[serde(default = "default_transformation_system_prompt")]
    pub system_prompt: String,
}

impl Default for TransformationConfig {
    fn default() -> Self {
        Self {
            hotkey: default_transformation_hotkey(),
            provider: None,
            api_key: None,
            model: default_transformation_model(),
            system_prompt: default_transformation_system_prompt(),
        }
    }
}

fn default_hotkey() -> String {
    "F5".into()
}

fn default_deepgram_language() -> String {
    "en-US".into()
}

fn default_deepgram_model() -> String {
    "nova-3".into()
}

fn default_sample_rate() -> u32 {
    16000
}

fn default_gain() -> f32 {
    4.0
}

fn default_hold_ms() -> u64 {
    300
}

fn default_overlay_font_size() -> f64 {
    12.0
}

fn default_ui_meter_style() -> UiMeterStyle {
    UiMeterStyle::AnimatedColor
}

fn default_endpointing_ms() -> u16 {
    300
}

fn default_utterance_end_ms() -> u16 {
    1000
}

fn default_transformation_hotkey() -> String {
    "F6".into()
}

fn default_transformation_model() -> String {
    "gpt-5.4-mini".into()
}

fn default_transformation_system_prompt() -> String {
    concat!(
        "You are editing raw speech-to-text output that was dictated quickly as instructions for ",
        "an LLM agent. Rewrite the input as clean, direct written instructions while preserving ",
        "the original meaning and intent. Do not blindly remove words just because they sound ",
        "like filler. Instead, infer the final intended wording. If the speaker starts a phrase, ",
        "revises it, or corrects themselves, keep only the semantically final version and omit ",
        "intermediate wording that was clearly discarded by the correction. Remove hesitations, ",
        "repair trails, and dictation noise only when they are not part of the intended content. ",
        "Fix punctuation, capitalization, and obvious transcription mistakes. Preserve technical ",
        "jargon, product names, API names, CLI flags, file paths, environment variable names, ",
        "and programmer vocabulary when clearly intended. If the speaker is clearly dictating ",
        "structure such as bullet points, numbered lists, headings, or short action items, ",
        "format the output accordingly. When the speaker is clearly dictating symbols or meta ",
        "words in a technical context, convert them to the intended characters, for example dash ",
        "to -, underscore to _, slash to /, backslash to \\, colon to :, dot to ., open paren ",
        "to (, close paren to ), open bracket to [, close bracket to ], open brace to {{, and ",
        "close brace to }}. Do not add new facts, commentary, or formatting beyond what is ",
        "implied by the input. Return only the transformed text."
    )
    .into()
}

pub fn config_path() -> Result<PathBuf, String> {
    if let Some(override_path) = override_config_path() {
        return Ok(override_path);
    }

    if let Some(xdg_config_home) = non_empty_env_path("XDG_CONFIG_HOME") {
        return Ok(xdg_config_home
            .join(XDG_APP_NAME)
            .join(DEFAULT_CONFIG_FILE_NAME));
    }

    if let Some(home_path) = non_empty_env_path("HOME") {
        return Ok(home_path
            .join(".config")
            .join(XDG_APP_NAME)
            .join(DEFAULT_CONFIG_FILE_NAME));
    }

    Err(format!(
        "Neither {} nor HOME/XDG_CONFIG_HOME is available, so no config path can be resolved.",
        CONFIG_OVERRIDE_ENV_VAR
    ))
}

impl Config {
    pub fn resolve_deepgram_api_key(&self) -> Result<String, String> {
        if let Some(api_key) = self
            .deepgram
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(api_key.to_owned());
        }

        if let Ok(api_key) = std::env::var("DEEPGRAM_API_KEY") {
            let trimmed_api_key = api_key.trim();
            if !trimmed_api_key.is_empty() {
                return Ok(trimmed_api_key.to_owned());
            }
        }

        let config_location_hint = match config_path() {
            Ok(path) => format!("{}", path.display()),
            Err(error) => error,
        };

        Err(format!(
            "Deepgram API key is missing. Set deepgram.api_key in {} or export DEEPGRAM_API_KEY.",
            config_location_hint
        ))
    }

    pub fn resolve_deepgram_project_id(&self) -> Option<String> {
        if let Some(project_id) = self
            .deepgram
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(project_id.to_owned());
        }

        std::env::var(deepgram_project_id_env_var())
            .ok()
            .map(|project_id| project_id.trim().to_owned())
            .filter(|project_id| !project_id.is_empty())
    }

    pub fn resolve_transformation_config(&self) -> Result<TransformationRuntimeConfig, String> {
        let provider = self
            .transformation
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!(
                    "transformation.provider is missing. Set it in {}.",
                    config_location_hint()
                )
            })?
            .to_owned();

        let model = match self.transformation.model.trim() {
            "" => default_transformation_model(),
            model => model.to_owned(),
        };

        if !supported_transformation_providers().contains(&provider.as_str()) {
            return Err(format!(
                "unsupported transformation.provider '{}'. Supported values: {}.",
                provider,
                supported_transformation_providers().join(", ")
            ));
        }

        let api_key = resolve_transformation_api_key(
            self.transformation.api_key.as_deref(),
            provider.as_str(),
        );

        let system_prompt = self.transformation.system_prompt.trim();

        Ok(TransformationRuntimeConfig {
            provider,
            api_key,
            model,
            system_prompt: if system_prompt.is_empty() {
                default_transformation_system_prompt()
            } else {
                system_prompt.to_owned()
            },
        })
    }
}

fn resolve_transformation_api_key(
    configured_api_key: Option<&str>,
    provider: &str,
) -> Option<String> {
    if let Some(api_key) = configured_api_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(api_key.to_owned());
    }

    transformation_api_key_env_vars(provider)
        .into_iter()
        .find_map(|variable_name| {
            std::env::var(variable_name)
                .ok()
                .map(|api_key| api_key.trim().to_owned())
                .filter(|api_key| !api_key.is_empty())
        })
}

fn transformation_api_key_env_vars(provider: &str) -> &'static [&'static str] {
    match provider.trim().to_ascii_lowercase().as_str() {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "cohere" => &["COHERE_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "galadriel" => &["GALADRIEL_API_KEY"],
        "gemini" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "groq" => &["GROQ_API_KEY"],
        "huggingface" => &["HUGGINGFACE_API_KEY", "HF_TOKEN"],
        "hyperbolic" => &["HYPERBOLIC_API_KEY"],
        "mira" => &["MIRA_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "moonshot" => &["MOONSHOT_API_KEY"],
        "ollama" => &[],
        "openai" => &["OPENAI_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "perplexity" => &["PERPLEXITY_API_KEY"],
        "together" => &["TOGETHER_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        _ => &[],
    }
}

fn supported_transformation_providers() -> &'static [&'static str] {
    &[
        "anthropic",
        "cohere",
        "deepseek",
        "galadriel",
        "gemini",
        "groq",
        "huggingface",
        "hyperbolic",
        "mira",
        "mistral",
        "moonshot",
        "ollama",
        "openai",
        "openrouter",
        "perplexity",
        "together",
        "xai",
    ]
}

fn config_location_hint() -> String {
    match config_path() {
        Ok(path) => path.display().to_string(),
        Err(error) => error,
    }
}

pub fn load_config() -> Config {
    let path = match config_path() {
        Ok(path) => path,
        Err(error) => {
            log::warn!("failed to resolve config path: {}, using defaults", error);
            return default_config();
        }
    };

    if override_config_path().is_some() {
        let contents = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "{} is set but {} could not be read: {}",
                CONFIG_OVERRIDE_ENV_VAR,
                path.display(),
                error
            )
        });

        return toml::from_str(&contents).unwrap_or_else(|error| {
            panic!(
                "{} is set but {} could not be parsed: {}",
                CONFIG_OVERRIDE_ENV_VAR,
                path.display(),
                error
            )
        });
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_else(|error| {
            log::warn!(
                "failed to parse {}: {}, using defaults",
                path.display(),
                error
            );
            default_config()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            log::info!("no config at {}, using defaults", path.display());
            default_config()
        }
        Err(error) => {
            log::warn!(
                "failed to read {}: {}, using defaults",
                path.display(),
                error
            );
            default_config()
        }
    }
}

fn default_config() -> Config {
    Config::default()
}

fn non_empty_env_path(variable_name: &str) -> Option<PathBuf> {
    std::env::var_os(variable_name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn override_config_path() -> Option<PathBuf> {
    non_empty_env_path(CONFIG_OVERRIDE_ENV_VAR)
}
