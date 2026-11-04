use super::paths::config_location_hint;
use super::prompts::{
    default_transformation_correction_system_prompt, default_transformation_system_prompt,
};
use super::{missing_model_problem, Config, TransformationProvider};
use crate::transformation::TransformationRuntimeConfig;

/// The local Ollama server, used when `transformation.ollama_base_url` is
/// unset.
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";

/// The Ollama server URL in effect for `configured`, without a trailing
/// slash.
pub fn resolve_ollama_base_url(configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or(DEFAULT_OLLAMA_BASE_URL)
        .trim_end_matches('/')
        .to_owned()
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

        Err(format!(
            "Deepgram API key is missing. Set deepgram.api_key in {} or export DEEPGRAM_API_KEY.",
            config_location_hint()
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
            .parse::<TransformationProvider>()?;

        let model = provider
            .resolve_model(&self.transformation.model)
            .ok_or_else(|| missing_model_problem(provider).to_string())?;

        let api_key =
            resolve_transformation_api_key(self.transformation.api_key.as_deref(), provider);

        Ok(TransformationRuntimeConfig {
            provider,
            api_key,
            ollama_base_url: resolve_ollama_base_url(
                self.transformation.ollama_base_url.as_deref(),
            ),
            model,
            system_prompt: prompt_or_default(
                self.transformation.system_prompt.trim(),
                default_transformation_system_prompt,
            ),
            correction_system_prompt: prompt_or_default(
                self.transformation.correction_system_prompt.trim(),
                default_transformation_correction_system_prompt,
            ),
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

    let provider = provider?.parse::<TransformationProvider>().ok()?;

    provider
        .api_key_env_vars()
        .iter()
        .copied()
        .find(|variable_name| env_var_is_present(variable_name))
}

pub(crate) fn resolve_transformation_api_key_for_provider(
    provider: Option<&str>,
    configured_api_key: Option<&str>,
) -> Option<String> {
    let provider = provider?.parse::<TransformationProvider>().ok()?;
    resolve_transformation_api_key(configured_api_key, provider)
}

fn resolve_transformation_api_key(
    configured_api_key: Option<&str>,
    provider: TransformationProvider,
) -> Option<String> {
    if let Some(api_key) = configured_api_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(api_key.to_owned());
    }

    provider
        .api_key_env_vars()
        .iter()
        .find_map(|variable_name| {
            std::env::var(variable_name)
                .ok()
                .map(|api_key| api_key.trim().to_owned())
                .filter(|api_key| !api_key.is_empty())
        })
}

pub(crate) fn supported_transformation_providers() -> impl Iterator<Item = &'static str> {
    TransformationProvider::ALL
        .into_iter()
        .map(TransformationProvider::as_str)
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
        runtime_config.transformation.api_key = resolve_transformation_api_key_for_provider(
            runtime_config.transformation.provider.as_deref(),
            config.transformation.api_key.as_deref(),
        );
    }
    if let Some(model) = runtime_config
        .transformation
        .provider
        .as_deref()
        .unwrap_or_default()
        .parse::<TransformationProvider>()
        .ok()
        .and_then(|provider| provider.resolve_model(&config.transformation.model))
    {
        runtime_config.transformation.model = model;
    }
    runtime_config.transformation.system_prompt = prompt_or_default(
        &config.transformation.system_prompt,
        default_transformation_system_prompt,
    );
    runtime_config.transformation.correction_system_prompt = prompt_or_default(
        &config.transformation.correction_system_prompt,
        default_transformation_correction_system_prompt,
    );
    runtime_config
}

/// `configured` as given, or the built-in `default` when it is blank. Callers
/// that send the prompt to a model pass it trimmed.
fn prompt_or_default(configured: &str, default: fn() -> String) -> String {
    if configured.trim().is_empty() {
        default()
    } else {
        configured.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{materialize_runtime_config, Config};
    use crate::config::test_support::{env_lock, EnvVarGuard, EXAMPLE_CONFIG};

    /// Resolves the example config's Deepgram key with `DEEPGRAM_API_KEY` set
    /// to `env_value` (or unset), restoring the previous value before
    /// returning so a failed assertion cannot leak it into other tests.
    fn resolve_example_deepgram_api_key_with_env(
        env_value: Option<&str>,
    ) -> Result<String, String> {
        let config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();

        let _lock = env_lock();
        let _key = EnvVarGuard::set("DEEPGRAM_API_KEY", env_value);
        config.resolve_deepgram_api_key()
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
    fn an_unset_model_resolves_per_provider_or_is_required() {
        let mut config = Config::default();
        config.transformation.provider = Some("anthropic".to_owned());
        assert_eq!(
            config.resolve_transformation_config().unwrap().model,
            "claude-haiku-5-5"
        );
        assert_eq!(
            materialize_runtime_config(&config).transformation.model,
            "claude-haiku-5-5"
        );

        config.transformation.provider = Some("ollama".to_owned());
        let error = config.resolve_transformation_config().unwrap_err();
        assert_eq!(
            error,
            "transformation.model: required for provider \"ollama\", which has no built-in default model"
        );
        assert_eq!(materialize_runtime_config(&config).transformation.model, "");
    }

    #[test]
    fn materialize_runtime_config_keeps_deepgram_key_when_present() {
        let mut config = Config::default();
        config.deepgram.api_key = Some("dg-key".to_owned());

        let runtime_config = materialize_runtime_config(&config);
        assert_eq!(runtime_config.deepgram.api_key.as_deref(), Some("dg-key"));
    }

    #[test]
    fn reports_env_backed_deepgram_api_key_without_file_value() {
        let _lock = env_lock();
        let _key = EnvVarGuard::set("DEEPGRAM_API_KEY", Some("env-deepgram-key"));

        let config = Config::default();
        assert_eq!(
            config.deepgram_api_key_env_var_in_use(),
            Some("DEEPGRAM_API_KEY")
        );
    }

    #[test]
    fn reports_env_backed_transformation_api_key_for_selected_provider() {
        let _lock = env_lock();
        let _key = EnvVarGuard::set("OPENAI_API_KEY", Some("env-openai-key"));

        let mut config = Config::default();
        config.transformation.provider = Some(" OpenAI ".to_owned());
        config.transformation.api_key = None;

        assert_eq!(
            config.transformation_api_key_env_var_in_use(),
            Some("OPENAI_API_KEY")
        );
        let resolved = config.resolve_transformation_config().unwrap();
        assert_eq!(resolved.provider, super::TransformationProvider::Openai);
        assert_eq!(resolved.api_key.as_deref(), Some("env-openai-key"));
    }

    #[test]
    fn transformation_provider_names_resolve_to_canonical_names() {
        for provider in ["openai", " OpenAI ", "ANTHROPIC"] {
            let mut config = Config::default();
            config.transformation.provider = Some(provider.to_owned());
            config.transformation.api_key = Some("test-key".to_owned());

            let resolved = config.resolve_transformation_config().unwrap();
            assert_eq!(
                resolved.provider.to_string(),
                provider.trim().to_ascii_lowercase()
            );
            assert_eq!(resolved.api_key.as_deref(), Some("test-key"));
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
            "galadriel"
                .parse::<super::TransformationProvider>()
                .unwrap_err()
        );
    }
}
