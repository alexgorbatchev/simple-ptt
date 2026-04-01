use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,

    pub deepgram_api_key: Option<String>,

    #[serde(default = "default_deepgram_language")]
    pub deepgram_language: String,

    #[serde(default = "default_deepgram_model")]
    pub deepgram_model: String,

    pub audio_device: Option<String>,

    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    #[serde(default = "default_gain")]
    pub gain: f32,

    #[serde(default = "default_hold_ms")]
    pub hold_ms: u64,

    #[serde(default = "default_endpointing_ms")]
    pub endpointing_ms: u16,

    #[serde(default = "default_utterance_end_ms")]
    pub utterance_end_ms: u16,
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

fn default_endpointing_ms() -> u16 {
    300
}

fn default_utterance_end_ms() -> u16 {
    1000
}

pub fn config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".config/jarvis/native.yaml"))
        .unwrap_or_else(|| PathBuf::from("/tmp/jarvis-native.yaml"))
}

impl Config {
    pub fn resolve_deepgram_api_key(&self) -> Result<String, String> {
        if let Some(api_key) = self
            .deepgram_api_key
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

        Err(format!(
            "Deepgram API key is missing. Set deepgram_api_key in {} or export DEEPGRAM_API_KEY.",
            config_path().display()
        ))
    }
}

pub fn load_config() -> Config {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_yaml::from_str(&contents).unwrap_or_else(|error| {
            log::warn!(
                "failed to parse {}: {}, using defaults",
                path.display(),
                error
            );
            serde_yaml::from_str("{}").expect("default config YAML must parse")
        }),
        Err(_) => {
            log::info!("no config at {}, using defaults", path.display());
            serde_yaml::from_str("{}").expect("default config YAML must parse")
        }
    }
}
