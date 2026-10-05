use serde::Deserialize;
use std::path::{Path, PathBuf};
use toml_edit::{value, DocumentMut, Item, Table};

use crate::transformation::TransformationRuntimeConfig;

const CONFIG_OVERRIDE_ENV_VAR: &str = "SIMPLE_PTT_CONFIG";
const DEFAULT_CONFIG_FILE_NAME: &str = "config.toml";
const XDG_APP_NAME: &str = "simple-ptt";

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

    #[serde(default = "default_transformation_model")]
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
            model: default_transformation_model(),
            system_prompt: default_transformation_system_prompt(),
            correction_system_prompt: default_transformation_correction_system_prompt(),
        }
    }
}

/// Prompts the user reset to their built-in default in the settings window.
/// `save_config` removes a reset prompt's key while its value still equals the
/// default, even when the file already had the key, so the config follows
/// future changes to the default again.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PromptResets {
    pub system_prompt: bool,
    pub correction_system_prompt: bool,
}

fn default_auto_check_updates() -> bool {
    true
}

fn default_hotkey() -> String {
    "F5".into()
}

fn default_correction_key() -> String {
    "LeftMeta".into()
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
    4.5
}

/// Lowest `mic.gain`, in dB. The audio stream amplifies samples by
/// `10^(gain / 20)`, so 0 dB leaves the input unchanged and the range never
/// mutes it.
pub const MIC_GAIN_MIN_DB: f32 = 0.0;
/// Highest `mic.gain`, in dB (about 3.2 times the input amplitude).
pub const MIC_GAIN_MAX_DB: f32 = 10.0;

/// Checks that `gain_db` is within the range the settings gain slider covers.
/// A value outside it, including NaN and infinity, is rejected rather than
/// clamped by the slider.
pub fn validate_mic_gain(gain_db: f32) -> Result<(), String> {
    if (MIC_GAIN_MIN_DB..=MIC_GAIN_MAX_DB).contains(&gain_db) {
        Ok(())
    } else {
        Err(format!(
            "Gain must be between {} and {} dB",
            MIC_GAIN_MIN_DB, MIC_GAIN_MAX_DB
        ))
    }
}

fn default_hold_ms() -> u64 {
    300
}

fn default_always_on() -> bool {
    true
}

fn default_overlay_font_size() -> f64 {
    12.0
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

fn default_transformation_auto() -> bool {
    true
}

pub(crate) fn default_transformation_model() -> String {
    "gpt-5.4-mini".into()
}

pub(crate) fn default_transformation_system_prompt() -> String {
    concat!(
        "You are editing raw speech-to-text output that was dictated quickly as instructions for ",
        "an LLM agent. Edit the input into written instructions while preserving the speaker's ",
        "wording, meaning, and intent.\n\n",
        "Preserve all details, context, emphasis, qualifications, examples, and side notes. ",
        "Do not summarize, shorten, or omit content because it seems redundant, incidental, ",
        "or unimportant. Keep repetitions that express emphasis and words that express ",
        "uncertainty, urgency, or degree.\n\n",
        "Fix punctuation, capitalization, and obvious transcription mistakes. Remove only ",
        "non-content hesitations and accidental stutters. Resolve false starts, self-repairs, ",
        "and retractions only when the speaker explicitly replaces or withdraws earlier wording; ",
        "keep the final intended wording and all surrounding context. If it is unclear whether ",
        "something is intended content, keep it.\n\n",
        "Preserve technical jargon, product ",
        "names, API names, CLI flags, file paths, environment variable names, and programmer ",
        "vocabulary when clearly intended.\n\n",
        "If the speaker is clearly dictating structure such as ",
        "bullet points, numbered lists, headings, or short action items, format the output ",
        "accordingly.\n\n",
        "When the speaker is clearly dictating symbols or meta words in a technical ",
        "context, convert them to the intended characters, for example dash to -, underscore to ",
        "_, slash to /, backslash to \\, colon to :, dot to ., open paren to (, close paren to ",
        "), open bracket to [, close bracket to ], open brace to {{, and close brace to }}.\n\n",
        "Do not add new facts, commentary, or formatting beyond what is implied by the input. ",
        "Return only the transformed text."
    )
    .into()
}

pub(crate) fn default_transformation_correction_system_prompt() -> String {
    concat!(
        "You are editing an existing annotation using a spoken correction request. The user will ",
        "provide input with two labeled sections: CURRENT ANNOTATION and CORRECTION REQUEST.\n\n",
        "Rewrite the current annotation by applying the correction request exactly as intended. ",
        "Preserve all content that the correction request does not change. Make the smallest ",
        "coherent edits that satisfy the request.\n\n",
        "If the request asks for ",
        "rewording, insertion, deletion, restructuring, emphasis, or formatting changes, apply ",
        "those changes to the annotation itself rather than commenting on them.\n\n",
        "Preserve ",
        "technical jargon, product names, API names, CLI flags, file paths, environment variable ",
        "names, and programmer vocabulary when clearly intended.\n\n",
        "Do not add explanations, ",
        "analysis, surrounding quotes, or commentary. Return only the fully rewritten ",
        "annotation."
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

pub fn cache_dir() -> Result<PathBuf, String> {
    if let Some(xdg_cache_home) = non_empty_env_path("XDG_CACHE_HOME") {
        return Ok(xdg_cache_home.join(XDG_APP_NAME));
    }

    if let Some(home_path) = non_empty_env_path("HOME") {
        return Ok(home_path.join(".cache").join(XDG_APP_NAME));
    }

    Err(
        "Neither HOME nor XDG_CACHE_HOME is available, so no cache directory can be resolved."
            .to_owned(),
    )
}

impl Config {
    pub fn deepgram_api_key_env_var_in_use(&self) -> Option<&'static str> {
        if self
            .deepgram
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            return None;
        }

        env_var_is_present("DEEPGRAM_API_KEY").then_some("DEEPGRAM_API_KEY")
    }

    pub fn transformation_api_key_env_var_in_use(&self) -> Option<&'static str> {
        transformation_api_key_env_var_in_use(
            self.transformation.provider.as_deref(),
            self.transformation.api_key.as_deref(),
        )
    }

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
        let correction_system_prompt = self.transformation.correction_system_prompt.trim();

        Ok(TransformationRuntimeConfig {
            provider,
            api_key,
            model,
            system_prompt: if system_prompt.is_empty() {
                default_transformation_system_prompt()
            } else {
                system_prompt.to_owned()
            },
            correction_system_prompt: if correction_system_prompt.is_empty() {
                default_transformation_correction_system_prompt()
            } else {
                correction_system_prompt.to_owned()
            },
        })
    }
}

fn env_var_is_present(variable_name: &str) -> bool {
    std::env::var(variable_name)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

pub(crate) fn transformation_api_key_env_var_in_use(
    provider: Option<&str>,
    configured_api_key: Option<&str>,
) -> Option<&'static str> {
    if configured_api_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
    {
        return None;
    }

    let provider = provider.map(str::trim).filter(|value| !value.is_empty())?;

    transformation_api_key_env_vars(provider)
        .iter()
        .copied()
        .find(|variable_name| env_var_is_present(variable_name))
}

pub(crate) fn resolve_transformation_api_key_for_provider(
    provider: Option<&str>,
    configured_api_key: Option<&str>,
) -> Option<String> {
    let provider = provider.map(str::trim).filter(|value| !value.is_empty())?;
    resolve_transformation_api_key(configured_api_key, provider)
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

pub(crate) fn supported_transformation_providers() -> &'static [&'static str] {
    &[
        "anthropic",
        "cohere",
        "deepseek",
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

fn load_document(path: &Path) -> Result<DocumentMut, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => contents
            .parse::<DocumentMut>()
            .map_err(|error| format!("failed to parse {} for editing: {}", path.display(), error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(error) => Err(format!("failed to read {}: {}", path.display(), error)),
    }
}

fn write_document_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let parent_directory = path.parent().ok_or_else(|| {
        format!(
            "failed to determine parent directory for {}",
            path.display()
        )
    })?;
    std::fs::create_dir_all(parent_directory).map_err(|error| {
        format!(
            "failed to create config directory {}: {}",
            parent_directory.display(),
            error
        )
    })?;

    let temporary_path = parent_directory.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|file_name| file_name.to_str())
            .unwrap_or("config.toml"),
        std::process::id()
    ));

    std::fs::write(&temporary_path, contents).map_err(|error| {
        format!(
            "failed to write temporary config file {}: {}",
            temporary_path.display(),
            error
        )
    })?;

    std::fs::rename(&temporary_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary_path);
        format!(
            "failed to replace config file {}: {}",
            path.display(),
            error
        )
    })
}

fn root_table(document: &mut DocumentMut) -> &mut toml_edit::Table {
    document.as_table_mut()
}

fn ensure_named_table<'a>(document: &'a mut DocumentMut, table_name: &str) -> &'a mut Table {
    let root = root_table(document);
    if !root.contains_key(table_name) {
        root.insert(table_name, Item::Table(Table::new()));
    }

    root[table_name]
        .as_table_mut()
        .expect("managed config section must be a table")
}

fn existing_key_name(table: &Table, primary_key: &str, aliases: &[&str]) -> String {
    if table.contains_key(primary_key) {
        return primary_key.to_owned();
    }

    aliases
        .iter()
        .copied()
        .find(|alias| table.contains_key(alias))
        .unwrap_or(primary_key)
        .to_owned()
}

fn set_required_string_key(table: &mut Table, key: &str, aliases: &[&str], field_value: &str) {
    let selected_key = existing_key_name(table, key, aliases);
    table[&selected_key] = value(field_value);
    for alias in aliases {
        if *alias != selected_key {
            table.remove(alias);
        }
    }
}

fn set_optional_string_key(
    table: &mut Table,
    key: &str,
    aliases: &[&str],
    field_value: Option<&str>,
) {
    let selected_key = existing_key_name(table, key, aliases);
    match field_value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(non_empty_value) => {
            table[&selected_key] = value(non_empty_value);
        }
        None => {
            table.remove(&selected_key);
        }
    }

    for alias in aliases {
        if *alias != selected_key {
            table.remove(alias);
        }
    }
}

/// Writes a string whose built-in default can change between releases, so a
/// config that leaves the key out follows the default. Resolution trims the
/// value and treats a blank one as the default, so a blank value or one equal
/// to the default after trimming is a default value. A default value removes
/// the key, unless the file already has it (under `key` or an alias) and
/// `reset` is false: a key the user wrote stays, while an explicit reset in
/// the settings window hands the key back to the default.
fn set_defaulted_string_key(
    table: &mut Table,
    key: &str,
    aliases: &[&str],
    field_value: &str,
    default_value: &str,
    reset: bool,
) {
    let trimmed_value = field_value.trim();
    let is_default = trimmed_value.is_empty() || trimmed_value == default_value.trim();
    let in_file = table.contains_key(key) || aliases.iter().any(|alias| table.contains_key(alias));
    if is_default && (trimmed_value.is_empty() || reset || !in_file) {
        table.remove(key);
        for alias in aliases {
            table.remove(alias);
        }
    } else {
        set_required_string_key(table, key, aliases, field_value);
    }
}

fn set_float_key(table: &mut Table, key: &str, field_value: f64) {
    table[key] = value(field_value);
}

fn set_float32_key(table: &mut Table, key: &str, field_value: f32) {
    table[key] = value(f64::from(field_value));
}

fn set_string_array_key(table: &mut Table, key: &str, values: &[String]) {
    if values.is_empty() {
        table.remove(key);
    } else {
        let mut array = toml_edit::Array::new();
        for v in values {
            array.push(v);
        }
        table[key] = toml_edit::value(array);
    }
}

fn set_unsigned_key(table: &mut Table, key: &str, field_value: impl Into<i64>) {
    table[key] = value(field_value.into());
}

fn write_ui_table(document: &mut DocumentMut, ui: &UiConfig) {
    let table = ensure_named_table(document, "ui");
    if ui.start_on_login {
        table["start_on_login"] = value(true);
    } else {
        table.remove("start_on_login");
    }
    if !ui.auto_check_updates {
        table["auto_check_updates"] = value(false);
    } else {
        table.remove("auto_check_updates");
    }
    set_required_string_key(table, "hotkey", &[], &ui.hotkey);
    set_required_string_key(
        table,
        "correction_key",
        &["instruction_key"],
        &ui.correction_key,
    );
    set_optional_string_key(
        table,
        "font_name",
        &["overlay_font_family"],
        ui.font_name.as_deref(),
    );
    set_float_key(table, "font_size", ui.font_size);
    match ui.footer_font_size {
        Some(footer_font_size) => set_float_key(table, "footer_font_size", footer_font_size),
        None => {
            table.remove("footer_font_size");
        }
    }
    table["meter_style"] = value(match ui.meter_style {
        UiMeterStyle::None => "none",
        UiMeterStyle::AnimatedHeight => "animated-height",
        UiMeterStyle::AnimatedColor => "animated-color",
        UiMeterStyle::Pills => "pills",
    });
}

fn write_mic_table(document: &mut DocumentMut, mic: &MicConfig) {
    let table = ensure_named_table(document, "mic");
    set_optional_string_key(table, "audio_device", &[], mic.audio_device.as_deref());
    set_unsigned_key(table, "sample_rate", i64::from(mic.sample_rate));
    set_float32_key(table, "gain", mic.gain);
    set_unsigned_key(
        table,
        "hold_ms",
        i64::try_from(mic.hold_ms).unwrap_or(i64::MAX),
    );
    table["always_on"] = value(mic.always_on);
}

fn write_deepgram_table(document: &mut DocumentMut, deepgram: &DeepgramConfig) {
    let table = ensure_named_table(document, "deepgram");
    set_optional_string_key(table, "api_key", &[], deepgram.api_key.as_deref());
    set_required_string_key(table, "language", &[], &deepgram.language);
    set_required_string_key(table, "model", &[], &deepgram.model);
    set_unsigned_key(table, "endpointing_ms", i64::from(deepgram.endpointing_ms));
    set_unsigned_key(
        table,
        "utterance_end_ms",
        i64::from(deepgram.utterance_end_ms),
    );
    set_string_array_key(table, "keyterms", &deepgram.keyterms);
}

fn write_transformation_table(
    document: &mut DocumentMut,
    transformation: &TransformationConfig,
    prompt_resets: PromptResets,
) {
    let table = ensure_named_table(document, "transformation");
    set_required_string_key(table, "hotkey", &[], &transformation.hotkey);
    table["auto"] = value(transformation.auto);
    set_optional_string_key(table, "provider", &[], transformation.provider.as_deref());
    set_optional_string_key(table, "api_key", &[], transformation.api_key.as_deref());
    set_defaulted_string_key(
        table,
        "model",
        &[],
        &transformation.model,
        &default_transformation_model(),
        false,
    );
    set_defaulted_string_key(
        table,
        "system_prompt",
        &[],
        &transformation.system_prompt,
        &default_transformation_system_prompt(),
        prompt_resets.system_prompt,
    );
    set_defaulted_string_key(
        table,
        "correction_system_prompt",
        &["instruction_system_prompt"],
        &transformation.correction_system_prompt,
        &default_transformation_correction_system_prompt(),
        prompt_resets.correction_system_prompt,
    );
}

pub fn save_config(
    path: &Path,
    config: &Config,
    prompt_resets: PromptResets,
) -> Result<(), String> {
    let mut document = load_document(path)?;
    write_ui_table(&mut document, &config.ui);
    write_mic_table(&mut document, &config.mic);
    write_deepgram_table(&mut document, &config.deepgram);
    write_transformation_table(&mut document, &config.transformation, prompt_resets);
    write_document_atomically(path, &document.to_string())
}

/// The config file exactly as it was on disk, taken before a save so a failed
/// apply can put the file back. Re-saving the previous `Config` instead would
/// not restore it: the resolved `Config` no longer records which keys the file
/// left out, so the default prompts and model would be written back.
#[derive(Debug)]
pub struct ConfigFileSnapshot {
    path: PathBuf,
    /// `None` when there was no file.
    contents: Option<String>,
}

impl ConfigFileSnapshot {
    pub fn take(path: &Path) -> Result<Self, String> {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("failed to read {}: {}", path.display(), error)),
        };
        Ok(Self {
            path: path.to_owned(),
            contents,
        })
    }

    /// Writes the snapshot back, or removes the file when there was none.
    pub fn restore(&self) -> Result<(), String> {
        match &self.contents {
            Some(contents) => write_document_atomically(&self.path, contents),
            None => match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!(
                    "failed to remove {}: {}",
                    self.path.display(),
                    error
                )),
            },
        }
    }
}

pub fn materialize_runtime_config(config: &Config) -> Config {
    let mut runtime_config = config.clone();
    if runtime_config
        .deepgram
        .api_key
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        runtime_config.deepgram.api_key = config.resolve_deepgram_api_key().ok();
    }
    if runtime_config
        .transformation
        .provider
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        runtime_config.transformation.provider = None;
    }
    if runtime_config
        .transformation
        .api_key
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
        && runtime_config.transformation.provider.is_some()
    {
        runtime_config.transformation.api_key = resolve_transformation_api_key(
            config.transformation.api_key.as_deref(),
            runtime_config
                .transformation
                .provider
                .as_deref()
                .unwrap_or_default(),
        );
    }
    if runtime_config.transformation.model.trim().is_empty() {
        runtime_config.transformation.model = default_transformation_model();
    }
    if runtime_config
        .transformation
        .system_prompt
        .trim()
        .is_empty()
    {
        runtime_config.transformation.system_prompt = default_transformation_system_prompt();
    }
    if runtime_config
        .transformation
        .correction_system_prompt
        .trim()
        .is_empty()
    {
        runtime_config.transformation.correction_system_prompt =
            default_transformation_correction_system_prompt();
    }
    runtime_config
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

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::{
        default_transformation_correction_system_prompt, default_transformation_system_prompt,
        materialize_runtime_config, save_config, validate_mic_gain, Config, ConfigFileSnapshot,
        PromptResets, UiMeterStyle,
    };

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn a_config_without_a_meter_style_uses_pills() {
        let config: Config = toml::from_str("[ui]\nfont_size = 14.0\n").unwrap();

        assert_eq!(config.ui.meter_style, UiMeterStyle::Pills);
        assert_eq!(Config::default().ui.meter_style, UiMeterStyle::Pills);
    }

    #[test]
    fn pills_meter_style_is_saved_and_read_as_pills() {
        let temp_directory = std::env::temp_dir()
            .join(format!("simple-ptt-config-pills-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        std::fs::write(&path, "[ui]\nmeter_style = \"animated-height\"\n").unwrap();

        let mut config = Config::default();
        config.ui.meter_style = UiMeterStyle::Pills;
        save_config(&path, &config, PromptResets::default()).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert!(contents.contains("meter_style = \"pills\""), "{contents}");
        let read: Config = toml::from_str(&contents).unwrap();
        assert_eq!(read.ui.meter_style, UiMeterStyle::Pills);
    }

    #[test]
    fn save_config_preserves_unknown_sections_and_comments() {
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        std::fs::write(
            &path,
            concat!(
                "# top comment\n",
                "[ui]\n",
                "# keep this comment\n",
                "hotkey = \"F4\"\n",
                "overlay_font_family = \"Menlo\"\n",
                "\n",
                "[custom]\n",
                "keep_me = true\n"
            ),
        )
        .unwrap();

        let mut config = Config::default();
        config.ui.hotkey = "F5".to_owned();
        config.ui.correction_key = "RightMeta".to_owned();
        config.ui.font_name = Some("SF Mono".to_owned());
        config.ui.font_size = 14.0;
        config.ui.footer_font_size = Some(11.0);
        config.ui.meter_style = UiMeterStyle::AnimatedHeight;
        config.transformation.correction_system_prompt = "Apply the spoken correction.".to_owned();

        save_config(&path, &config, PromptResets::default()).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();

        assert!(updated_contents.contains("# top comment"));
        assert!(updated_contents.contains("# keep this comment"));
        assert!(updated_contents.contains("[custom]"));
        assert!(updated_contents.contains("keep_me = true"));
        assert!(updated_contents.contains("overlay_font_family = \"SF Mono\""));
        assert!(updated_contents.contains("hotkey = \"F5\""));
        assert!(updated_contents.contains("correction_key = \"RightMeta\""));
        assert!(updated_contents
            .contains("correction_system_prompt = \"Apply the spoken correction.\""));
    }

    const EXAMPLE_CONFIG: &str = include_str!("../../config.example.toml");

    fn assert_example_config_values(config: &Config) {
        assert!(!config.ui.start_on_login);
        assert!(config.ui.auto_check_updates);
        assert_eq!(config.ui.hotkey, "F5");
        assert_eq!(config.ui.correction_key, "LeftMeta");
        assert_eq!(config.ui.font_name, None);
        assert_eq!(config.ui.font_size, 12.0);
        assert_eq!(config.ui.footer_font_size, None);
        assert_eq!(config.ui.meter_style, UiMeterStyle::Pills);
        assert_eq!(config.mic.audio_device, None);
        assert_eq!(config.mic.sample_rate, 16000);
        assert_eq!(config.mic.gain, 4.0);
        assert_eq!(config.mic.hold_ms, 300);
        assert!(config.mic.always_on);
        assert_eq!(config.deepgram.api_key, None);
        assert_eq!(config.deepgram.language, "en-US");
        assert!(config.deepgram.keyterms.is_empty());
        assert_eq!(config.deepgram.model, "nova-3");
        assert_eq!(config.deepgram.endpointing_ms, 300);
        assert_eq!(config.deepgram.utterance_end_ms, 1000);
        assert_eq!(config.transformation.hotkey, "F6");
        assert!(config.transformation.auto);
        assert_eq!(config.transformation.provider, None);
        assert_eq!(config.transformation.api_key, None);
        assert_eq!(
            config.transformation.system_prompt,
            default_transformation_system_prompt()
        );
        assert_eq!(
            config.transformation.correction_system_prompt,
            default_transformation_correction_system_prompt()
        );
    }

    #[test]
    fn example_config_parses_with_expected_values() {
        let config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();

        assert_example_config_values(&config);
    }

    /// Resolves the example config's Deepgram key with `DEEPGRAM_API_KEY` set
    /// to `env_value` (or unset), restoring the previous value before
    /// returning so a failed assertion cannot leak it into other tests.
    fn resolve_example_deepgram_api_key_with_env(
        env_value: Option<&str>,
    ) -> Result<String, String> {
        let config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();

        let _guard = env_lock().lock().unwrap();
        let previous_deepgram_api_key = std::env::var("DEEPGRAM_API_KEY").ok();
        match env_value {
            Some(value) => std::env::set_var("DEEPGRAM_API_KEY", value),
            None => std::env::remove_var("DEEPGRAM_API_KEY"),
        }

        let resolved = config.resolve_deepgram_api_key();

        match previous_deepgram_api_key {
            Some(value) => std::env::set_var("DEEPGRAM_API_KEY", value),
            None => std::env::remove_var("DEEPGRAM_API_KEY"),
        }
        resolved
    }

    #[test]
    fn example_config_uses_deepgram_api_key_from_environment() {
        assert_eq!(
            resolve_example_deepgram_api_key_with_env(Some("env-deepgram-key")),
            Ok("env-deepgram-key".to_owned())
        );
    }

    #[test]
    fn example_config_without_environment_key_reports_missing_deepgram_api_key() {
        let error = resolve_example_deepgram_api_key_with_env(None).unwrap_err();

        assert!(
            error.starts_with("Deepgram API key is missing."),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn save_config_keeps_every_example_config_line_in_order_and_reparses() {
        let temp_directory = std::env::temp_dir().join(format!(
            "simple-ptt-example-config-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        std::fs::write(&path, EXAMPLE_CONFIG).unwrap();
        let config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();

        save_config(&path, &config, PromptResets::default()).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        // Every original line (comments, blank lines, headers, and unchanged
        // key/value lines) must survive in its original order.
        let mut updated_lines = updated_contents.lines();
        for original_line in EXAMPLE_CONFIG.lines() {
            assert!(
                updated_lines.any(|updated_line| updated_line == original_line),
                "line {:?} was lost or reordered in:\n{}",
                original_line,
                updated_contents
            );
        }

        let reparsed: Config = toml::from_str(&updated_contents).unwrap();
        assert_example_config_values(&reparsed);
    }

    #[test]
    fn save_config_writes_keyterms_as_inline_string_array() {
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-keyterms-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");

        let mut config = Config::default();
        config.deepgram.keyterms = vec!["macOS".to_owned(), "GitHub".to_owned()];
        save_config(&path, &config, PromptResets::default()).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert!(
            updated_contents.contains("keyterms = [\"macOS\", \"GitHub\"]\n"),
            "unexpected keyterms rendering in:\n{}",
            updated_contents
        );
        let reparsed: Config = toml::from_str(&updated_contents).unwrap();
        assert_eq!(reparsed.deepgram.keyterms, config.deepgram.keyterms);
    }

    /// Saves `config` over a file holding `initial_contents` (no file when
    /// `None`) and returns the `[transformation]` table that was written.
    fn saved_transformation_table(
        test_name: &str,
        initial_contents: Option<&str>,
        config: &Config,
        prompt_resets: PromptResets,
    ) -> toml_edit::Table {
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-{}-{}", test_name, std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        if let Some(initial_contents) = initial_contents {
            std::fs::write(&path, initial_contents).unwrap();
        }

        save_config(&path, config, prompt_resets).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        toml::from_str::<Config>(&updated_contents).unwrap();
        updated_contents.parse::<toml_edit::DocumentMut>().unwrap()["transformation"]
            .as_table()
            .expect("save_config writes a [transformation] table")
            .clone()
    }

    #[test]
    fn save_config_leaves_default_prompts_and_model_out_of_a_new_file() {
        let table = saved_transformation_table(
            "defaults-new-file",
            None,
            &Config::default(),
            PromptResets::default(),
        );

        for key in [
            "model",
            "system_prompt",
            "correction_system_prompt",
            "instruction_system_prompt",
        ] {
            assert!(!table.contains_key(key), "{key} was written:\n{table}");
        }
    }

    #[test]
    fn save_config_leaves_default_prompts_and_model_out_of_the_example_config() {
        let config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();

        let table = saved_transformation_table(
            "defaults-example",
            Some(EXAMPLE_CONFIG),
            &config,
            PromptResets::default(),
        );

        for key in ["model", "system_prompt", "correction_system_prompt"] {
            assert!(!table.contains_key(key), "{key} was written:\n{table}");
        }
    }

    #[test]
    fn save_config_writes_customized_prompts_and_model() {
        let mut config = Config::default();
        config.transformation.model = "claude-test".to_owned();
        config.transformation.system_prompt = "Custom dictation prompt.".to_owned();
        config.transformation.correction_system_prompt = "Custom correction prompt.".to_owned();

        let table =
            saved_transformation_table("customized", None, &config, PromptResets::default());

        assert_eq!(table["model"].as_str(), Some("claude-test"));
        assert_eq!(
            table["system_prompt"].as_str(),
            Some("Custom dictation prompt.")
        );
        assert_eq!(
            table["correction_system_prompt"].as_str(),
            Some("Custom correction prompt.")
        );
    }

    /// A key the file already has stays, even at its default value: the user
    /// (or an earlier Save) put it there, and dropping it would silently
    /// discard a line of their config.
    #[test]
    fn save_config_keeps_default_valued_keys_the_file_already_has() {
        let initial_contents = format!(
            "[transformation]\nmodel = {:?}\nsystem_prompt = {:?}\ninstruction_system_prompt = {:?}\n",
            super::default_transformation_model(),
            default_transformation_system_prompt(),
            default_transformation_correction_system_prompt(),
        );

        let table = saved_transformation_table(
            "defaults-present",
            Some(&initial_contents),
            &Config::default(),
            PromptResets::default(),
        );

        assert_eq!(
            table["model"].as_str(),
            Some(super::default_transformation_model().as_str())
        );
        assert_eq!(
            table["system_prompt"].as_str(),
            Some(default_transformation_system_prompt().as_str())
        );
        assert_eq!(
            table["instruction_system_prompt"].as_str(),
            Some(default_transformation_correction_system_prompt().as_str())
        );
        assert!(!table.contains_key("correction_system_prompt"));
    }

    #[test]
    fn save_config_removes_reset_prompts_the_file_already_has() {
        let initial_contents = concat!(
            "[transformation]\n",
            "system_prompt = \"Old custom dictation prompt.\"\n",
            "instruction_system_prompt = \"Old custom correction prompt.\"\n",
        );

        let table = saved_transformation_table(
            "reset-present",
            Some(initial_contents),
            &Config::default(),
            PromptResets {
                system_prompt: true,
                correction_system_prompt: true,
            },
        );

        for key in [
            "system_prompt",
            "correction_system_prompt",
            "instruction_system_prompt",
        ] {
            assert!(!table.contains_key(key), "{key} was kept:\n{table}");
        }
    }

    /// A reset only drops the key while the editor still holds the default;
    /// text edited after the reset is a customization and is saved.
    #[test]
    fn save_config_writes_prompts_edited_after_a_reset() {
        let mut config = Config::default();
        config.transformation.system_prompt = "Edited after reset.".to_owned();

        let table = saved_transformation_table(
            "reset-edited",
            Some("[transformation]\nsystem_prompt = \"Old custom prompt.\"\n"),
            &config,
            PromptResets {
                system_prompt: true,
                correction_system_prompt: true,
            },
        );

        assert_eq!(table["system_prompt"].as_str(), Some("Edited after reset."));
        assert!(!table.contains_key("correction_system_prompt"));
    }

    /// The runtime trims prompts and treats a blank one as the default, so a
    /// prompt that differs from the default only in surrounding whitespace is
    /// the default.
    #[test]
    fn save_config_treats_whitespace_padded_default_prompt_as_default() {
        let mut config = Config::default();
        config.transformation.system_prompt =
            format!("\n{}\n", default_transformation_system_prompt());
        config.transformation.correction_system_prompt = "  ".to_owned();

        let table =
            saved_transformation_table("defaults-padded", None, &config, PromptResets::default());

        assert!(!table.contains_key("system_prompt"), "{table}");
        assert!(!table.contains_key("correction_system_prompt"), "{table}");
    }

    #[test]
    fn save_config_removes_blank_prompts_the_file_already_has() {
        let mut config = Config::default();
        config.transformation.system_prompt = "".to_owned();
        config.transformation.correction_system_prompt = " \n ".to_owned();

        let table = saved_transformation_table(
            "blank-present",
            Some(concat!(
                "[transformation]\n",
                "system_prompt = \"Old custom dictation prompt.\"\n",
                "instruction_system_prompt = \"Old custom correction prompt.\"\n",
            )),
            &config,
            PromptResets::default(),
        );

        for key in [
            "system_prompt",
            "correction_system_prompt",
            "instruction_system_prompt",
        ] {
            assert!(!table.contains_key(key), "{key} was kept:\n{table}");
        }
    }

    /// A failed apply restores the file byte for byte, so keys the file left
    /// out stay out instead of coming back as defaults.
    #[test]
    fn config_file_snapshot_restores_the_file_saved_over() {
        let temp_directory = std::env::temp_dir().join(format!(
            "simple-ptt-snapshot-restore-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        std::fs::write(&path, EXAMPLE_CONFIG).unwrap();

        let snapshot = ConfigFileSnapshot::take(&path).unwrap();
        let mut config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();
        config.transformation.model = "claude-test".to_owned();
        config.transformation.system_prompt = "Custom dictation prompt.".to_owned();
        save_config(&path, &config, PromptResets::default()).unwrap();
        assert_ne!(std::fs::read_to_string(&path).unwrap(), EXAMPLE_CONFIG);

        snapshot.restore().unwrap();
        let restored_contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert_eq!(restored_contents, EXAMPLE_CONFIG);
    }

    #[test]
    fn config_file_snapshot_of_a_missing_file_removes_the_saved_file() {
        let temp_directory = std::env::temp_dir().join(format!(
            "simple-ptt-snapshot-missing-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");

        let snapshot = ConfigFileSnapshot::take(&path).unwrap();
        save_config(&path, &Config::default(), PromptResets::default()).unwrap();
        assert!(path.exists());

        snapshot.restore().unwrap();
        let file_exists = path.exists();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert!(!file_exists);
    }

    #[test]
    fn materialize_runtime_config_keeps_deepgram_key_when_present() {
        let mut config = Config::default();
        config.deepgram.api_key = Some("dg-key".to_owned());

        let runtime_config = materialize_runtime_config(&config);
        assert_eq!(runtime_config.deepgram.api_key.as_deref(), Some("dg-key"));
    }

    #[test]
    fn config_files_that_still_set_a_deepgram_project_id_load() {
        let config: Config = toml::from_str(
            "[deepgram]\napi_key = \"dg-key\"\nproject_id = \"00000000-0000-0000-0000-000000000000\"\n",
        )
        .expect("a leftover project_id is ignored");
        assert_eq!(config.deepgram.api_key.as_deref(), Some("dg-key"));
    }

    #[test]
    fn reports_env_backed_deepgram_api_key_without_file_value() {
        let _guard = env_lock().lock().unwrap();
        let previous_deepgram_api_key = std::env::var("DEEPGRAM_API_KEY").ok();

        std::env::set_var("DEEPGRAM_API_KEY", "env-deepgram-key");

        let config = Config::default();
        assert_eq!(
            config.deepgram_api_key_env_var_in_use(),
            Some("DEEPGRAM_API_KEY")
        );

        match previous_deepgram_api_key {
            Some(value) => std::env::set_var("DEEPGRAM_API_KEY", value),
            None => std::env::remove_var("DEEPGRAM_API_KEY"),
        }
    }

    #[test]
    fn reports_env_backed_transformation_api_key_for_selected_provider() {
        let _guard = env_lock().lock().unwrap();
        let previous_openai_api_key = std::env::var("OPENAI_API_KEY").ok();
        std::env::set_var("OPENAI_API_KEY", "env-openai-key");

        let mut config = Config::default();
        config.transformation.provider = Some("openai".to_owned());
        config.transformation.api_key = None;

        assert_eq!(
            config.transformation_api_key_env_var_in_use(),
            Some("OPENAI_API_KEY")
        );

        match previous_openai_api_key {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }

    #[test]
    fn default_transformation_prompt_limits_cleanup_and_preserves_context() {
        let prompt = default_transformation_system_prompt();

        assert!(prompt.contains("self-repairs"));
        assert!(prompt.contains("retractions"));
        assert!(prompt.contains("final intended wording"));
        assert!(prompt.contains("Do not summarize, shorten, or omit content"));
        assert!(
            prompt.contains("details, context, emphasis, qualifications, examples, and side notes")
        );
        assert!(prompt.contains("If it is unclear whether something is intended content, keep it"));
    }

    #[test]
    fn default_correction_transformation_prompt_mentions_correction_sections() {
        let prompt = default_transformation_correction_system_prompt();

        assert!(prompt.contains("CURRENT ANNOTATION"));
        assert!(prompt.contains("CORRECTION REQUEST"));
        assert!(prompt.contains("Return only the fully rewritten annotation"));
    }

    #[test]
    fn auto_check_updates_defaults_to_true_and_roundtrips() {
        let mut config = Config::default();
        assert!(config.ui.auto_check_updates);

        config.ui.auto_check_updates = false;
        let temp_dir =
            std::env::temp_dir().join(format!("simple-ptt-autocheck-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("config.toml");

        save_config(&path, &config, PromptResets::default()).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("auto_check_updates = false"));
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn always_on_defaults_to_true_and_roundtrips() {
        let mut config = Config::default();
        assert!(config.mic.always_on);

        config.mic.always_on = false;
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-always-on-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");

        save_config(&path, &config, PromptResets::default()).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();
        assert!(updated_contents.contains("always_on = false"));
        std::fs::remove_dir_all(&temp_directory).unwrap();
    }

    #[test]
    fn validate_mic_gain_accepts_the_slider_range() {
        for gain_db in [0.0, Config::default().mic.gain, 10.0] {
            assert_eq!(validate_mic_gain(gain_db), Ok(()));
        }
    }

    #[test]
    fn validate_mic_gain_rejects_values_outside_the_slider_range() {
        for gain_db in [-0.1, 10.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                validate_mic_gain(gain_db),
                Err("Gain must be between 0 and 10 dB".to_owned())
            );
        }
    }

    #[test]
    fn galadriel_provider_is_rejected_as_unsupported() {
        let toml_content = r#"
[deepgram]
api_key = "test-key"

[transformation]
provider = "galadriel"
api_key = "test-key"
"#;

        let config: Config = toml::from_str(toml_content).expect("should parse TOML");
        let error = config
            .resolve_transformation_config()
            .expect_err("should reject galadriel provider");

        assert_eq!(
            error,
            "unsupported transformation.provider 'galadriel'. Supported values: \
             anthropic, cohere, deepseek, gemini, groq, huggingface, hyperbolic, mira, mistral, \
             moonshot, ollama, openai, openrouter, perplexity, together, xai."
        );
    }
}
