use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const CACHE_FILE_NAME: &str = "transformation-models.toml";
const HTTP_TIMEOUT_SECS: u64 = 20;
const MAX_MODEL_COUNT: usize = 50;
const CACHE_VERSION: u8 = 2;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const APPLICATION_USER_AGENT: &str = concat!("simple-ptt/", env!("CARGO_PKG_VERSION"));
const OBVIOUSLY_NON_CHAT_MODEL_FRAGMENTS: &[&str] = &[
    "embedding",
    "embed",
    "moderation",
    "omni-moderation",
    "whisper",
    "transcribe",
    "transcription",
    "tts",
    "text-to-speech",
    "speech-to-text",
    "rerank",
    "ranker",
    "stable-diffusion",
    "sdxl",
    "dall",
    "imagen",
    "recraft",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransformationProviderRequest {
    pub provider: String,
    pub resolved_api_key: Option<String>,
    pub selected_model: String,
}

impl TransformationProviderRequest {
    pub fn new(provider: String, resolved_api_key: Option<String>, selected_model: String) -> Self {
        Self {
            provider: provider.trim().to_ascii_lowercase(),
            resolved_api_key: resolved_api_key
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            selected_model: selected_model.trim().to_owned(),
        }
    }

    pub fn same_source_as(&self, other: &Self) -> bool {
        self.provider == other.provider && self.account_fingerprint() == other.account_fingerprint()
    }

    pub fn account_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.provider.as_bytes());
        hasher.update([0]);
        if let Some(api_key) = self.resolved_api_key.as_deref() {
            hasher.update(api_key.as_bytes());
        } else {
            hasher.update(b"no-api-key");
        }
        let digest = hasher.finalize();
        digest[..8]
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub enum TransformationModelUpdate {
    CachedModelsLoaded {
        request: TransformationProviderRequest,
        models: Vec<String>,
        message: String,
    },
    ModelsRefreshed {
        request: TransformationProviderRequest,
        models: Vec<String>,
        message: String,
    },
    ConnectionChecked {
        request: TransformationProviderRequest,
        models: Vec<String>,
        message: String,
    },
    ActionFailed {
        request: TransformationProviderRequest,
        message: String,
    },
}

#[derive(Clone, Default)]
pub struct TransformationModelsController {
    state: Arc<Mutex<TransformationModelsState>>,
    cache_lock: Arc<Mutex<()>>,
}

#[derive(Default)]
struct TransformationModelsState {
    pending_updates: VecDeque<TransformationModelUpdate>,
}

#[derive(Clone, Copy, Debug)]
pub enum TransformationModelAction {
    Refresh,
    Check,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TransformationModelsCache {
    #[serde(default = "default_cache_version")]
    version: u8,
    #[serde(default)]
    entries: Vec<TransformationModelsCacheEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TransformationModelsCacheEntry {
    provider: String,
    account_fingerprint: String,
    updated_at_unix_seconds: u64,
    #[serde(default)]
    models: Vec<String>,
}

fn default_cache_version() -> u8 {
    CACHE_VERSION
}

impl Default for TransformationModelsCache {
    fn default() -> Self {
        Self {
            version: default_cache_version(),
            entries: Vec::new(),
        }
    }
}

impl TransformationModelsController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_pending_ui_update(&self) -> bool {
        self.state
            .lock()
            .map(|state| !state.pending_updates.is_empty())
            .unwrap_or(false)
    }

    pub fn take_update(&self) -> Option<TransformationModelUpdate> {
        self.state
            .lock()
            .ok()
            .and_then(|mut state| state.pending_updates.pop_front())
    }

    pub fn load_cached_models_now(
        &self,
        request: TransformationProviderRequest,
    ) -> TransformationModelUpdate {
        self.load_cached_models(request)
    }

    pub fn start_action(
        &self,
        action: TransformationModelAction,
        request: TransformationProviderRequest,
    ) {
        let controller = self.clone();
        std::thread::Builder::new()
            .name(format!("transformation-models-{}", request.provider))
            .spawn(move || {
                let update = match action {
                    TransformationModelAction::Refresh => controller.refresh_models(request),
                    TransformationModelAction::Check => controller.check_connection(request),
                };
                controller.push_update(update);
            })
            .expect("failed to spawn transformation models worker thread");
    }

    fn load_cached_models(
        &self,
        request: TransformationProviderRequest,
    ) -> TransformationModelUpdate {
        let _cache_guard = self.cache_lock.lock().ok();
        match read_cache_file().and_then(|cache| {
            cache
                .entries
                .into_iter()
                .find(|entry| {
                    entry.provider == request.provider
                        && entry.account_fingerprint == request.account_fingerprint()
                })
                .map(|entry| normalize_model_names(entry.models))
                .ok_or_else(|| {
                    format!(
                        "No cached models for {}. Click Refresh to load them.",
                        request.provider
                    )
                })
        }) {
            Ok(models) => TransformationModelUpdate::CachedModelsLoaded {
                request: request.clone(),
                message: format!(
                    "Loaded {} cached models for {}.",
                    models.len(),
                    request.provider
                ),
                models,
            },
            Err(error) => TransformationModelUpdate::ActionFailed {
                request,
                message: error,
            },
        }
    }

    fn refresh_models(&self, request: TransformationProviderRequest) -> TransformationModelUpdate {
        match fetch_provider_models(&request) {
            Ok(models) => {
                let normalized_models = normalize_model_names(models);
                let _cache_guard = self.cache_lock.lock().ok();
                if let Err(error) = write_models_to_cache(&request, &normalized_models) {
                    return TransformationModelUpdate::ActionFailed {
                        request,
                        message: format!("Loaded models but failed to update the cache: {}", error),
                    };
                }

                TransformationModelUpdate::ModelsRefreshed {
                    request: request.clone(),
                    message: format!(
                        "Refreshed {} models for {} and updated the cache.",
                        normalized_models.len(),
                        request.provider
                    ),
                    models: normalized_models,
                }
            }
            Err(error) => TransformationModelUpdate::ActionFailed {
                request,
                message: error,
            },
        }
    }

    fn check_connection(
        &self,
        request: TransformationProviderRequest,
    ) -> TransformationModelUpdate {
        match fetch_provider_models(&request) {
            Ok(models) => {
                let normalized_models = normalize_model_names(models);
                let _cache_guard = self.cache_lock.lock().ok();
                let _ = write_models_to_cache(&request, &normalized_models);
                let selected_model = request.selected_model.trim();
                let model_message = if selected_model.is_empty() {
                    format!(
                        "Connected to {}. Found {} models.",
                        request.provider,
                        normalized_models.len()
                    )
                } else if normalized_models
                    .iter()
                    .any(|model| model == selected_model)
                {
                    format!(
                        "Connected to {}. Selected model '{}' is available.",
                        request.provider, selected_model
                    )
                } else {
                    format!(
                        "Connected to {} and found {} models, but '{}' was not listed.",
                        request.provider,
                        normalized_models.len(),
                        selected_model
                    )
                };
                TransformationModelUpdate::ConnectionChecked {
                    request,
                    models: normalized_models,
                    message: model_message,
                }
            }
            Err(error) => TransformationModelUpdate::ActionFailed {
                request,
                message: error,
            },
        }
    }

    fn push_update(&self, update: TransformationModelUpdate) {
        if let Ok(mut state) = self.state.lock() {
            state.pending_updates.push_back(update);
        }
    }
}

fn fetch_provider_models(request: &TransformationProviderRequest) -> Result<Vec<String>, String> {
    let http_client = Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|error| format!("failed to build HTTP client: {}", error))?;

    let provider = request.provider.as_str();
    let models = match provider {
        "anthropic" => fetch_json_models(
            with_anthropic_headers(
                http_client.get("https://api.anthropic.com/v1/models"),
                request.resolved_api_key.as_deref(),
            )?,
            provider,
        )?,
        "cohere" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.cohere.ai/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "deepseek" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.deepseek.com/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "galadriel" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.galadriel.com/v1/verified/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "gemini" => fetch_json_models(
            with_google_api_key_query(
                http_client.get("https://generativelanguage.googleapis.com/v1beta/models"),
                request.resolved_api_key.as_deref(),
            )?,
            provider,
        )?,
        "groq" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.groq.com/openai/v1/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "huggingface" => fetch_json_models(
            with_optional_bearer_auth(
                http_client.get(
                    "https://huggingface.co/api/models?inference_provider=hf-inference&pipeline_tag=text-generation&limit=200",
                ),
                request.resolved_api_key.as_deref(),
            )?,
            provider,
        )?,
        "hyperbolic" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.hyperbolic.xyz/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "mira" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.mira.network/v1/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "mistral" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.mistral.ai/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "moonshot" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.moonshot.cn/v1/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "ollama" => fetch_json_models(http_client.get("http://localhost:11434/api/tags"), provider)?,
        "openai" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.openai.com/v1/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "openrouter" => fetch_json_models(
            with_bearer_auth(http_client.get("https://openrouter.ai/api/v1/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "perplexity" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.perplexity.ai/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "together" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.together.xyz/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        "xai" => fetch_json_models(
            with_bearer_auth(http_client.get("https://api.x.ai/v1/models"), request.resolved_api_key.as_deref())?,
            provider,
        )?,
        _ => {
            return Err(format!(
                "Model refresh is not implemented for provider '{}'.",
                request.provider
            ))
        }
    };

    let filtered_models = filter_completion_model_names(models);

    if filtered_models.is_empty() {
        return Err(format!(
            "{} returned models, but none looked like chat/completion-capable models after filtering.",
            request.provider
        ));
    }

    Ok(filtered_models)
}

fn with_bearer_auth(
    request_builder: RequestBuilder,
    api_key: Option<&str>,
) -> Result<RequestBuilder, String> {
    let api_key = required_api_key(api_key)?;
    with_optional_bearer_auth(request_builder, Some(api_key))
}

fn with_optional_bearer_auth(
    request_builder: RequestBuilder,
    api_key: Option<&str>,
) -> Result<RequestBuilder, String> {
    let mut headers = default_headers();
    if let Some(api_key) = api_key.filter(|value| !value.trim().is_empty()) {
        let header_value = HeaderValue::from_str(&format!("Bearer {}", api_key.trim()))
            .map_err(|error| format!("invalid Authorization header value: {}", error))?;
        headers.insert(AUTHORIZATION, header_value);
    }
    Ok(request_builder.headers(headers))
}

fn with_anthropic_headers(
    request_builder: RequestBuilder,
    api_key: Option<&str>,
) -> Result<RequestBuilder, String> {
    let api_key = required_api_key(api_key)?;
    let mut headers = default_headers();
    headers.insert(
        "x-api-key",
        HeaderValue::from_str(api_key)
            .map_err(|error| format!("invalid Anthropic API key header: {}", error))?,
    );
    headers.insert(
        "anthropic-version",
        HeaderValue::from_static(ANTHROPIC_VERSION),
    );
    Ok(request_builder.headers(headers))
}

fn with_google_api_key_query(
    request_builder: RequestBuilder,
    api_key: Option<&str>,
) -> Result<RequestBuilder, String> {
    let api_key = required_api_key(api_key)?;
    Ok(request_builder
        .query(&[("key", api_key)])
        .headers(default_headers()))
}

fn default_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(APPLICATION_USER_AGENT));
    headers
}

fn required_api_key(api_key: Option<&str>) -> Result<&str, String> {
    api_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "The selected provider requires an API key before models can be loaded.".to_owned()
        })
}

fn fetch_json_models(
    request_builder: RequestBuilder,
    provider: &str,
) -> Result<Vec<String>, String> {
    let response = request_builder
        .send()
        .map_err(|error| format!("failed to contact {}: {}", provider, error))?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| String::from("<response body unavailable>"));
        return Err(format!(
            "{} returned {} while loading models: {}",
            provider,
            status,
            truncate_error_body(&body)
        ));
    }

    let value = response
        .json::<Value>()
        .map_err(|error| format!("{} returned invalid JSON: {}", provider, error))?;
    extract_model_names(provider, &value)
}

fn extract_model_names(provider: &str, value: &Value) -> Result<Vec<String>, String> {
    let models = match value {
        Value::Object(map) => {
            if provider == "gemini" {
                extract_gemini_model_names(map.get("models").unwrap_or(&Value::Null))
            } else if provider == "ollama" {
                extract_array_model_names(map.get("models").unwrap_or(&Value::Null))
            } else if let Some(data) = map.get("data") {
                extract_array_model_names(data)
            } else if let Some(models) = map.get("models") {
                extract_array_model_names(models)
            } else if let Some(result) = map.get("result") {
                extract_array_model_names(result)
            } else {
                Vec::new()
            }
        }
        Value::Array(_) => extract_array_model_names(value),
        _ => Vec::new(),
    };

    if models.is_empty() {
        Err(format!(
            "{} returned a payload shape that does not expose model identifiers in the expected places.",
            provider
        ))
    } else {
        Ok(models)
    }
}

fn extract_gemini_model_names(value: &Value) -> Vec<String> {
    let Value::Array(items) = value else {
        return Vec::new();
    };

    items
        .iter()
        .filter(|item| {
            item.get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .map(|methods| {
                    methods.iter().any(|method| {
                        method
                            .as_str()
                            .map(|method| {
                                matches!(method, "generateContent" | "streamGenerateContent")
                            })
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .filter_map(extract_single_model_name)
        .map(|model| model.trim_start_matches("models/").to_owned())
        .collect()
}

fn extract_array_model_names(value: &Value) -> Vec<String> {
    let Value::Array(items) = value else {
        return Vec::new();
    };

    items.iter().filter_map(extract_single_model_name).collect()
}

fn extract_single_model_name(value: &Value) -> Option<String> {
    match value {
        Value::String(model_name) => Some(model_name.trim().to_owned()),
        Value::Object(map) => ["id", "name", "model", "modelId", "slug"]
            .into_iter()
            .find_map(|key| map.get(key).and_then(Value::as_str))
            .map(|value| value.trim().to_owned()),
        _ => None,
    }
    .filter(|value| !value.is_empty())
}

fn filter_completion_model_names(models: Vec<String>) -> Vec<String> {
    models
        .into_iter()
        .filter(|model| !is_obviously_non_chat_model(model))
        .collect()
}

fn is_obviously_non_chat_model(model: &str) -> bool {
    let normalized_model = model.trim().to_ascii_lowercase();
    OBVIOUSLY_NON_CHAT_MODEL_FRAGMENTS
        .iter()
        .any(|fragment| normalized_model.contains(fragment))
}

fn normalize_model_names(models: Vec<String>) -> Vec<String> {
    let mut seen_models = BTreeSet::new();
    let mut normalized_models = Vec::new();
    for model in models {
        let trimmed_model = model.trim();
        if trimmed_model.is_empty() {
            continue;
        }
        if seen_models.insert(trimmed_model.to_owned()) {
            normalized_models.push(trimmed_model.to_owned());
        }
        if normalized_models.len() >= MAX_MODEL_COUNT {
            break;
        }
    }
    normalized_models
}

fn truncate_error_body(body: &str) -> String {
    let trimmed_body = body.trim();
    if trimmed_body.len() <= 400 {
        trimmed_body.to_owned()
    } else {
        format!("{}…", &trimmed_body[..400])
    }
}

fn write_models_to_cache(
    request: &TransformationProviderRequest,
    models: &[String],
) -> Result<(), String> {
    let mut cache = read_cache_file().unwrap_or_default();
    cache.entries.retain(|entry| {
        !(entry.provider == request.provider
            && entry.account_fingerprint == request.account_fingerprint())
    });
    cache.entries.push(TransformationModelsCacheEntry {
        provider: request.provider.clone(),
        account_fingerprint: request.account_fingerprint(),
        updated_at_unix_seconds: current_unix_timestamp(),
        models: models.to_vec(),
    });

    let cache_path = cache_file_path()?;
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create model cache directory '{}': {}",
                parent.display(),
                error
            )
        })?;
    }

    let cache_contents = toml::to_string_pretty(&cache)
        .map_err(|error| format!("failed to serialize model cache: {}", error))?;
    std::fs::write(&cache_path, cache_contents).map_err(|error| {
        format!(
            "failed to write model cache '{}': {}",
            cache_path.display(),
            error
        )
    })
}

fn read_cache_file() -> Result<TransformationModelsCache, String> {
    let cache_path = cache_file_path()?;
    let cache_contents = match std::fs::read_to_string(&cache_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TransformationModelsCache::default())
        }
        Err(error) => {
            return Err(format!(
                "failed to read model cache '{}': {}",
                cache_path.display(),
                error
            ))
        }
    };

    let cache: TransformationModelsCache = toml::from_str(&cache_contents).map_err(|error| {
        format!(
            "failed to parse model cache '{}': {}",
            cache_path.display(),
            error
        )
    })?;

    if cache.version != CACHE_VERSION {
        return Ok(TransformationModelsCache::default());
    }

    Ok(cache)
}

fn cache_file_path() -> Result<PathBuf, String> {
    Ok(crate::config::cache_dir()?.join(CACHE_FILE_NAME))
}

fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        extract_model_names, filter_completion_model_names, normalize_model_names, MAX_MODEL_COUNT,
    };
    use serde_json::json;

    #[test]
    fn extracts_openai_style_model_ids() {
        let value = json!({
            "data": [
                { "id": "gpt-4.1" },
                { "id": "gpt-4o" }
            ]
        });

        assert_eq!(
            extract_model_names("openai", &value).unwrap(),
            vec!["gpt-4.1".to_owned(), "gpt-4o".to_owned()]
        );
    }

    #[test]
    fn extracts_gemini_model_ids_without_models_prefix() {
        let value = json!({
            "models": [
                {
                    "name": "models/gemini-2.5-flash",
                    "supportedGenerationMethods": ["generateContent"]
                },
                {
                    "name": "models/text-embedding-004",
                    "supportedGenerationMethods": ["embedContent"]
                }
            ]
        });

        assert_eq!(
            extract_model_names("gemini", &value).unwrap(),
            vec!["gemini-2.5-flash".to_owned()]
        );
    }

    #[test]
    fn filters_obviously_non_chat_models() {
        assert_eq!(
            filter_completion_model_names(vec![
                "gpt-4o".to_owned(),
                "text-embedding-3-large".to_owned(),
                "whisper-1".to_owned(),
                "claude-sonnet-4-5".to_owned(),
                "dall-e-3".to_owned(),
            ]),
            vec!["gpt-4o".to_owned(), "claude-sonnet-4-5".to_owned()]
        );
    }

    #[test]
    fn normalizes_duplicate_and_blank_model_names_preserving_provider_order() {
        assert_eq!(
            normalize_model_names(vec![
                "  ".to_owned(),
                "gpt-4o".to_owned(),
                "gpt-4o".to_owned(),
                " claude-sonnet ".to_owned(),
            ]),
            vec!["gpt-4o".to_owned(), "claude-sonnet".to_owned()]
        );
    }

    #[test]
    fn normalizes_model_names_caps_results_aggressively() {
        let models = (0..(MAX_MODEL_COUNT + 10))
            .map(|index| format!("model-{:03}", index))
            .collect::<Vec<_>>();

        let normalized_models = normalize_model_names(models);

        assert_eq!(normalized_models.len(), MAX_MODEL_COUNT);
        assert_eq!(
            normalized_models.first().map(String::as_str),
            Some("model-000")
        );
        assert_eq!(
            normalized_models.last().map(String::as_str),
            Some("model-049")
        );
    }
}
