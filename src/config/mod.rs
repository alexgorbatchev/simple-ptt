//! The app's `config.toml`: its types and built-in defaults, where the file
//! lives, how its values resolve at runtime, and how it is loaded and saved.

mod defaults;
mod file;
mod load;
mod paths;
mod persist;
mod problems;
mod prompts;
mod provider;
mod resolve;
mod toml_keys;
mod types;

pub use defaults::{DefaultResets, DEFAULTED_KEYS, MIC_GAIN_MAX_DB, MIC_GAIN_MIN_DB};
pub use file::ConfigFileSnapshot;
pub use load::load_config;
pub use paths::{cache_dir, config_path};
pub use persist::{defaulted_keys_in_file, save_config};
pub use problems::{
    mic_gain_problem, missing_model_problem, problems, unsupported_provider_problem, ConfigProblem,
};
pub(crate) use prompts::{
    default_transformation_correction_system_prompt, default_transformation_system_prompt,
};
pub use provider::TransformationProvider;
pub use resolve::{materialize_runtime_config, resolve_ollama_base_url, DEFAULT_OLLAMA_BASE_URL};
pub(crate) use resolve::{
    resolve_transformation_api_key_for_provider, supported_transformation_providers,
    transformation_api_key_env_var_in_use,
};
pub use types::{Config, DeepgramConfig, MicConfig, TransformationConfig, UiConfig, UiMeterStyle};

#[cfg(test)]
mod inline_tables_tests;
#[cfg(test)]
mod test_support;
