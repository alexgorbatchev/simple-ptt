#![allow(dead_code)]

use std::path::PathBuf;

use super::{
    default_config, default_transformation_correction_system_prompt,
    default_transformation_model, default_transformation_system_prompt, Config,
    CONFIG_OVERRIDE_ENV_VAR,
};

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

pub fn resolve_transformation_api_key(
    configured_key: Option<&str>,
    provider: &str,
) -> Option<String> {
    let trimmed = configured_key.map(str::trim).unwrap_or("");
    if !trimmed.is_empty() {
        return Some(trimmed.to_owned());
    }

    let env_var = match provider {
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        "anthropic" => "ANTHROPIC_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "groq" => "GROQ_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        _ => return None,
    };

    std::env::var(env_var).ok().filter(|val| !val.trim().is_empty())
}

pub fn load_config() -> Config {
    let path = match super::config_path() {
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

fn non_empty_env_path(variable_name: &str) -> Option<PathBuf> {
    std::env::var_os(variable_name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn override_config_path() -> Option<PathBuf> {
    non_empty_env_path(CONFIG_OVERRIDE_ENV_VAR)
}
