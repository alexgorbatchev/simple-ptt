use serde::Deserialize;
use std::path::{Path, PathBuf};
use toml_edit::{value, DocumentMut, Item, Table};

use crate::billing::deepgram_project_id_env_var;
use crate::transformation::TransformationRuntimeConfig;

const CONFIG_OVERRIDE_ENV_VAR: &str = "SIMPLE_PTT_CONFIG";
const DEFAULT_CONFIG_FILE_NAME: &str = "config.toml";
const XDG_APP_NAME: &str = "simple-ptt";

#[derive(Clone, Debug, Default, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
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

fn default_transformation_auto() -> bool {
    true
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

    pub fn deepgram_project_id_env_var_in_use(&self) -> Option<&'static str> {
        if self
            .deepgram
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            return None;
        }

        env_var_is_present(deepgram_project_id_env_var()).then_some(deepgram_project_id_env_var())
    }

    pub fn transformation_api_key_env_var_in_use(&self) -> Option<&'static str> {
        if self
            .transformation
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            return None;
        }

        let provider = self
            .transformation
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())?;

        transformation_api_key_env_vars(provider)
            .iter()
            .copied()
            .find(|variable_name| env_var_is_present(variable_name))
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

fn env_var_is_present(variable_name: &str) -> bool {
    std::env::var(variable_name)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
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

fn set_float_key(table: &mut Table, key: &str, field_value: f64) {
    table[key] = value(field_value);
}

fn set_float32_key(table: &mut Table, key: &str, field_value: f32) {
    table[key] = value(f64::from(field_value));
}

fn set_unsigned_key(table: &mut Table, key: &str, field_value: impl Into<i64>) {
    table[key] = value(field_value.into());
}

fn write_ui_table(document: &mut DocumentMut, ui: &UiConfig) {
    let table = ensure_named_table(document, "ui");
    set_required_string_key(table, "hotkey", &[], &ui.hotkey);
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
}

fn write_deepgram_table(document: &mut DocumentMut, deepgram: &DeepgramConfig) {
    let table = ensure_named_table(document, "deepgram");
    set_optional_string_key(table, "api_key", &[], deepgram.api_key.as_deref());
    set_optional_string_key(table, "project_id", &[], deepgram.project_id.as_deref());
    set_required_string_key(table, "language", &[], &deepgram.language);
    set_required_string_key(table, "model", &[], &deepgram.model);
    set_unsigned_key(table, "endpointing_ms", i64::from(deepgram.endpointing_ms));
    set_unsigned_key(
        table,
        "utterance_end_ms",
        i64::from(deepgram.utterance_end_ms),
    );
}

fn write_transformation_table(document: &mut DocumentMut, transformation: &TransformationConfig) {
    let table = ensure_named_table(document, "transformation");
    set_required_string_key(table, "hotkey", &[], &transformation.hotkey);
    table["auto"] = value(transformation.auto);
    set_optional_string_key(table, "provider", &[], transformation.provider.as_deref());
    set_optional_string_key(table, "api_key", &[], transformation.api_key.as_deref());
    set_required_string_key(table, "model", &[], &transformation.model);
    if transformation.system_prompt.trim().is_empty() {
        table.remove("system_prompt");
    } else {
        set_required_string_key(table, "system_prompt", &[], &transformation.system_prompt);
    }
}

pub fn save_config(path: &Path, config: &Config) -> Result<(), String> {
    let mut document = load_document(path)?;
    write_ui_table(&mut document, &config.ui);
    write_mic_table(&mut document, &config.mic);
    write_deepgram_table(&mut document, &config.deepgram);
    write_transformation_table(&mut document, &config.transformation);
    write_document_atomically(path, &document.to_string())
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
    if runtime_config.deepgram.project_id.is_none() {
        runtime_config.deepgram.project_id = config.resolve_deepgram_project_id();
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

    use super::{materialize_runtime_config, save_config, Config, UiMeterStyle};

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
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
        config.ui.font_name = Some("SF Mono".to_owned());
        config.ui.font_size = 14.0;
        config.ui.footer_font_size = Some(11.0);
        config.ui.meter_style = UiMeterStyle::AnimatedHeight;

        save_config(&path, &config).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();

        assert!(updated_contents.contains("# top comment"));
        assert!(updated_contents.contains("# keep this comment"));
        assert!(updated_contents.contains("[custom]"));
        assert!(updated_contents.contains("keep_me = true"));
        assert!(updated_contents.contains("overlay_font_family = \"SF Mono\""));
        assert!(updated_contents.contains("hotkey = \"F5\""));
    }

    #[test]
    fn materialize_runtime_config_keeps_deepgram_key_when_present() {
        let mut config = Config::default();
        config.deepgram.api_key = Some("dg-key".to_owned());

        let runtime_config = materialize_runtime_config(&config);
        assert_eq!(runtime_config.deepgram.api_key.as_deref(), Some("dg-key"));
    }

    #[test]
    fn reports_env_backed_deepgram_values_without_file_values() {
        let _guard = env_lock().lock().unwrap();
        let previous_deepgram_api_key = std::env::var("DEEPGRAM_API_KEY").ok();
        let previous_deepgram_project_id = std::env::var(super::deepgram_project_id_env_var()).ok();

        std::env::set_var("DEEPGRAM_API_KEY", "env-deepgram-key");
        std::env::set_var(super::deepgram_project_id_env_var(), "env-project-id");

        let config = Config::default();
        assert_eq!(
            config.deepgram_api_key_env_var_in_use(),
            Some("DEEPGRAM_API_KEY")
        );
        assert_eq!(
            config.deepgram_project_id_env_var_in_use(),
            Some(super::deepgram_project_id_env_var())
        );

        match previous_deepgram_api_key {
            Some(value) => std::env::set_var("DEEPGRAM_API_KEY", value),
            None => std::env::remove_var("DEEPGRAM_API_KEY"),
        }
        match previous_deepgram_project_id {
            Some(value) => std::env::set_var(super::deepgram_project_id_env_var(), value),
            None => std::env::remove_var(super::deepgram_project_id_env_var()),
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
}
