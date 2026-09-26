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
const MISSING_API_KEY_MESSAGE: &str =
    "The selected provider requires an API key before models can be loaded.";
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

    /// Checks that this request's models can be fetched: its provider has a
    /// models endpoint and, when that endpoint needs one, an API key resolved
    /// from the API key field or the provider's environment variable.
    fn models_fetch_readiness(&self) -> Result<(), String> {
        let endpoint = models_endpoint(&self.provider)?;
        if endpoint.auth.requires_api_key() {
            required_api_key(self.resolved_api_key.as_deref())?;
        }
        Ok(())
    }

    /// Whether fetching this request's models sends its API key.
    fn sends_api_key(&self) -> bool {
        self.resolved_api_key.is_some()
            && models_endpoint(&self.provider).is_ok_and(|endpoint| endpoint.auth.sends_api_key())
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
        action: TransformationModelAction,
        message: String,
    },
}

impl TransformationModelUpdate {
    fn request(&self) -> &TransformationProviderRequest {
        match self {
            Self::ModelsRefreshed { request, .. }
            | Self::ConnectionChecked { request, .. }
            | Self::ActionFailed { request, .. } => request,
        }
    }

    /// The action this update reports the result of.
    fn action(&self) -> TransformationModelAction {
        match self {
            Self::ModelsRefreshed { .. } => TransformationModelAction::Refresh,
            Self::ConnectionChecked { .. } => TransformationModelAction::Check,
            Self::ActionFailed { action, .. } => *action,
        }
    }

    /// The result the Check button shows for this update: only a Check
    /// reports one, so a failed Fetch models (manual or after a provider
    /// change) leaves the button as it is.
    pub fn check_result(&self) -> Option<bool> {
        match self {
            Self::ConnectionChecked { .. } => Some(true),
            Self::ActionFailed {
                action: TransformationModelAction::Check,
                ..
            } => Some(false),
            Self::ModelsRefreshed { .. } | Self::ActionFailed { .. } => None,
        }
    }

    /// Whether this update belongs to `current`, the provider and account the
    /// Transformation pane shows now. An update for a provider or API key the
    /// user has since switched away from must not fill the model list or the
    /// status.
    pub fn applies_to(&self, current: Option<&TransformationProviderRequest>) -> bool {
        current.is_some_and(|current| current.same_source_as(self.request()))
    }
}

/// Whether an automatic fetch may send `request`'s API key to its provider.
///
/// The pane has one API key field for every provider, so after a provider
/// change the field can still hold the previous provider's key. A key from the
/// field counts as this provider's only when it is the key saved in the config
/// for this provider. With the field empty, the key comes from this provider's
/// own environment variable, and a request that sends no key is always safe.
pub fn api_key_belongs_to_provider(
    request: &TransformationProviderRequest,
    api_key_field: Option<&str>,
    saved_provider: Option<&str>,
    saved_api_key: Option<&str>,
) -> bool {
    let Some(field_key) = non_empty_trimmed(api_key_field) else {
        return true;
    };
    !request.sends_api_key()
        || (non_empty_trimmed(saved_provider)
            .is_some_and(|provider| provider.eq_ignore_ascii_case(&request.provider))
            && non_empty_trimmed(saved_api_key) == Some(field_key))
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Why the Transformation pane's model list is being synced with the selected
/// provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelListTrigger {
    /// The user chose another provider, so the list is fetched when none is
    /// cached.
    ProviderChanged,
    /// Settings opened or was saved with the provider it already had; only the
    /// cache is read.
    Resync,
}

/// Why the models of a provider with nothing cached wait for the user to
/// click Fetch models.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManualFetchReason {
    /// The list is synced without a provider change, which never fetches.
    NotCached,
    /// The API key field holds a key that is not saved for this provider, so
    /// it may be another provider's.
    UnsavedApiKey,
}

/// What the Transformation pane does to show the selected provider's models.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelListPlan {
    /// Show the cached models, without a network call.
    UseCached {
        models: Vec<String>,
        message: String,
    },
    /// Fetch the models from the provider.
    StartFetch,
    /// This action is already running for this provider and account, and its
    /// result fills the list.
    AwaitFetch(TransformationModelAction),
    /// Nothing is cached, and the models are fetched only when the user asks.
    NeedsManualFetch(ManualFetchReason),
    /// The model cache could not be read (the error); nothing is fetched
    /// until the user clicks Fetch models, which rewrites the cache.
    CacheUnreadable(String),
    /// No models can be shown; the message says why.
    ShowError(String),
}

/// Decides how to show the models of `provider` from the cache lookup for its
/// account (`Ok(None)` when nothing is cached), the action already fetching
/// them, whether a fetch could start (an error when the provider needs an API
/// key that is not set), and whether the API key it would send is this
/// provider's (see `api_key_belongs_to_provider`).
fn plan_model_list(
    trigger: ModelListTrigger,
    provider: &str,
    cached_models: Result<Option<Vec<String>>, CacheReadError>,
    fetch_in_flight: Option<TransformationModelAction>,
    fetch_readiness: Result<(), String>,
    api_key_belongs_to_provider: bool,
) -> ModelListPlan {
    let cached_models = match cached_models {
        Ok(cached_models) => cached_models,
        Err(CacheReadError::Unavailable(error)) => return ModelListPlan::ShowError(error),
        Err(CacheReadError::Unreadable(error)) => return ModelListPlan::CacheUnreadable(error),
    };
    if let Some(models) = cached_models {
        return ModelListPlan::UseCached {
            message: format!("Loaded {} cached models for {}.", models.len(), provider),
            models,
        };
    }
    if let Some(action) = fetch_in_flight {
        return ModelListPlan::AwaitFetch(action);
    }
    match trigger {
        ModelListTrigger::Resync => match fetch_readiness {
            Err(error) => ModelListPlan::ShowError(error),
            Ok(()) => ModelListPlan::NeedsManualFetch(ManualFetchReason::NotCached),
        },
        ModelListTrigger::ProviderChanged => match fetch_readiness {
            Err(error) => ModelListPlan::ShowError(error),
            Ok(()) if !api_key_belongs_to_provider => {
                ModelListPlan::NeedsManualFetch(ManualFetchReason::UnsavedApiKey)
            }
            Ok(()) => ModelListPlan::StartFetch,
        },
    }
}

#[derive(Clone, Default)]
pub struct TransformationModelsController {
    state: Arc<Mutex<TransformationModelsState>>,
    cache_lock: Arc<Mutex<()>>,
}

#[derive(Default)]
struct TransformationModelsState {
    pending_updates: VecDeque<TransformationModelUpdate>,
    /// One entry per running Refresh or Check action; both fetch the model
    /// list and fill it on success.
    fetches_in_flight: Vec<(TransformationModelAction, TransformationProviderRequest)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransformationModelAction {
    Refresh,
    Check,
}

impl TransformationModelAction {
    /// The status shown while this action runs for `provider`.
    pub fn progress_message(self, provider: &str) -> String {
        match self {
            Self::Refresh => format!("Fetching models for {}…", provider),
            Self::Check => format!("Checking {} connection…", provider),
        }
    }

    /// The status when this action for `provider` stopped without a result.
    fn interrupted_message(self, provider: &str) -> String {
        match self {
            Self::Refresh => format!("Fetching models for {} stopped unexpectedly.", provider),
            Self::Check => format!("Checking {} connection stopped unexpectedly.", provider),
        }
    }
}

/// A running Refresh or Check, from its start on the main thread until its
/// worker reports the result. Dropped without `finish` (the worker panicked),
/// it reports the action as failed, so the action never stays in flight.
struct InFlightFetch {
    controller: TransformationModelsController,
    action: TransformationModelAction,
    request: Option<TransformationProviderRequest>,
}

impl InFlightFetch {
    fn start(
        controller: &TransformationModelsController,
        action: TransformationModelAction,
        request: &TransformationProviderRequest,
    ) -> Self {
        if let Ok(mut state) = controller.state.lock() {
            state.fetches_in_flight.push((action, request.clone()));
        }
        Self {
            controller: controller.clone(),
            action,
            request: Some(request.clone()),
        }
    }

    fn finish(mut self, update: TransformationModelUpdate) {
        self.request = None;
        self.controller.finish_fetch(update);
    }
}

impl Drop for InFlightFetch {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            let message = self.action.interrupted_message(&request.provider);
            self.controller
                .finish_fetch(TransformationModelUpdate::ActionFailed {
                    request,
                    action: self.action,
                    message,
                });
        }
    }
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

    /// Plans how the Transformation pane shows the models of `request`'s
    /// provider and account. `api_key_belongs_to_provider` is the result of
    /// the function of that name.
    pub fn plan_model_list(
        &self,
        trigger: ModelListTrigger,
        request: &TransformationProviderRequest,
        api_key_belongs_to_provider: bool,
    ) -> ModelListPlan {
        // Only the main thread starts fetches, so reading the in-flight state
        // before the cache cannot miss a fetch started in between; a fetch
        // that finishes in between has written the cache that is read next.
        let fetch_in_flight = self.fetch_in_flight(request);
        plan_model_list(
            trigger,
            &request.provider,
            self.read_cached_models(request),
            fetch_in_flight,
            request.models_fetch_readiness(),
            api_key_belongs_to_provider,
        )
    }

    pub fn start_action(
        &self,
        action: TransformationModelAction,
        request: TransformationProviderRequest,
    ) {
        let in_flight = InFlightFetch::start(self, action, &request);
        let controller = self.clone();
        std::thread::Builder::new()
            .name(format!("transformation-models-{}", request.provider))
            .spawn(move || {
                let update = match action {
                    TransformationModelAction::Refresh => controller.refresh_models(request),
                    TransformationModelAction::Check => controller.check_connection(request),
                };
                in_flight.finish(update);
            })
            .expect("failed to spawn transformation models worker thread");
    }

    /// The action, if any, fetching the models of `request`'s provider and
    /// account; the earliest one when several are.
    fn fetch_in_flight(
        &self,
        request: &TransformationProviderRequest,
    ) -> Option<TransformationModelAction> {
        let state = self.state.lock().ok()?;
        state
            .fetches_in_flight
            .iter()
            .find(|(_, in_flight)| in_flight.same_source_as(request))
            .map(|(action, _)| *action)
    }

    /// Queues the result of a fetch and ends its in-flight entry under one
    /// lock, so the fetch is never seen as both finished and unreported.
    fn finish_fetch(&self, update: TransformationModelUpdate) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(index) = state
                .fetches_in_flight
                .iter()
                .position(|(action, in_flight)| {
                    *action == update.action() && in_flight.same_source_as(update.request())
                })
            {
                state.fetches_in_flight.remove(index);
            }
            state.pending_updates.push_back(update);
        }
    }

    fn read_cached_models(
        &self,
        request: &TransformationProviderRequest,
    ) -> Result<Option<Vec<String>>, CacheReadError> {
        let _cache_guard = self.cache_lock.lock().ok();
        cached_models_in(read_cache_file(), request)
    }

    fn refresh_models(&self, request: TransformationProviderRequest) -> TransformationModelUpdate {
        match fetch_provider_models(&request) {
            Ok(models) => {
                let normalized_models = normalize_model_names(models);
                let _cache_guard = self.cache_lock.lock().ok();
                if let Err(error) = write_models_to_cache(&request, &normalized_models) {
                    return TransformationModelUpdate::ActionFailed {
                        request,
                        action: TransformationModelAction::Refresh,
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
                action: TransformationModelAction::Refresh,
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
                action: TransformationModelAction::Check,
                message: error,
            },
        }
    }
}

/// The cached models of `request`'s provider and account: `Ok(None)` when the
/// cache has no entry for them (including a missing cache file), and the
/// error when the cache could not be read.
fn cached_models_in(
    cache: Result<TransformationModelsCache, CacheReadError>,
    request: &TransformationProviderRequest,
) -> Result<Option<Vec<String>>, CacheReadError> {
    let account_fingerprint = request.account_fingerprint();
    Ok(cache?
        .entries
        .into_iter()
        .find(|entry| {
            entry.provider == request.provider && entry.account_fingerprint == account_fingerprint
        })
        .map(|entry| normalize_model_names(entry.models)))
}

/// How a provider's models endpoint authenticates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelsAuth {
    /// `Authorization: Bearer <key>`; the key is required.
    Bearer,
    /// `Authorization: Bearer <key>` when a key is set; the endpoint is public.
    OptionalBearer,
    /// Anthropic's `x-api-key` and `anthropic-version` headers.
    Anthropic,
    /// Google's `key` query parameter.
    GoogleApiKeyQuery,
    /// No authentication (the local Ollama server).
    None,
}

impl ModelsAuth {
    fn requires_api_key(self) -> bool {
        match self {
            Self::Bearer | Self::Anthropic | Self::GoogleApiKeyQuery => true,
            Self::OptionalBearer | Self::None => false,
        }
    }

    fn sends_api_key(self) -> bool {
        self != Self::None
    }
}

#[derive(Clone, Copy, Debug)]
struct ModelsEndpoint {
    url: &'static str,
    auth: ModelsAuth,
}

fn models_endpoint(provider: &str) -> Result<ModelsEndpoint, String> {
    let (url, auth) = match provider {
        "anthropic" => ("https://api.anthropic.com/v1/models", ModelsAuth::Anthropic),
        "cohere" => ("https://api.cohere.ai/models", ModelsAuth::Bearer),
        "deepseek" => ("https://api.deepseek.com/models", ModelsAuth::Bearer),
        "gemini" => (
            "https://generativelanguage.googleapis.com/v1beta/models",
            ModelsAuth::GoogleApiKeyQuery,
        ),
        "groq" => ("https://api.groq.com/openai/v1/models", ModelsAuth::Bearer),
        "huggingface" => (
            "https://huggingface.co/api/models?inference_provider=hf-inference&pipeline_tag=text-generation&limit=200",
            ModelsAuth::OptionalBearer,
        ),
        "hyperbolic" => ("https://api.hyperbolic.xyz/models", ModelsAuth::Bearer),
        "mira" => ("https://api.mira.network/v1/models", ModelsAuth::Bearer),
        "mistral" => ("https://api.mistral.ai/models", ModelsAuth::Bearer),
        "moonshot" => ("https://api.moonshot.cn/v1/models", ModelsAuth::Bearer),
        "ollama" => ("http://localhost:11434/api/tags", ModelsAuth::None),
        "openai" => ("https://api.openai.com/v1/models", ModelsAuth::Bearer),
        "openrouter" => ("https://openrouter.ai/api/v1/models", ModelsAuth::Bearer),
        "perplexity" => ("https://api.perplexity.ai/models", ModelsAuth::Bearer),
        "together" => ("https://api.together.xyz/models", ModelsAuth::Bearer),
        "xai" => ("https://api.x.ai/v1/models", ModelsAuth::Bearer),
        _ => {
            return Err(format!(
                "Model refresh is not implemented for provider '{}'.",
                provider
            ))
        }
    };
    Ok(ModelsEndpoint { url, auth })
}

fn fetch_provider_models(request: &TransformationProviderRequest) -> Result<Vec<String>, String> {
    let endpoint = models_endpoint(&request.provider)?;
    let http_client = Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|error| format!("failed to build HTTP client: {}", error))?;

    let request_builder = http_client.get(endpoint.url);
    let api_key = request.resolved_api_key.as_deref();
    let request_builder = match endpoint.auth {
        ModelsAuth::Bearer => with_bearer_auth(request_builder, api_key)?,
        ModelsAuth::OptionalBearer => with_optional_bearer_auth(request_builder, api_key)?,
        ModelsAuth::Anthropic => with_anthropic_headers(request_builder, api_key)?,
        ModelsAuth::GoogleApiKeyQuery => with_google_api_key_query(request_builder, api_key)?,
        ModelsAuth::None => request_builder,
    };
    let models = fetch_json_models(request_builder, &request.provider)?;

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
    non_empty_trimmed(api_key).ok_or_else(|| MISSING_API_KEY_MESSAGE.to_owned())
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

/// Why the model cache could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
enum CacheReadError {
    /// The cache directory cannot be resolved, so the cache can neither be
    /// read nor written.
    Unavailable(String),
    /// The cache file exists but cannot be read or parsed; fetching the
    /// models rewrites it.
    Unreadable(String),
}

fn read_cache_file() -> Result<TransformationModelsCache, CacheReadError> {
    let cache_path = cache_file_path().map_err(CacheReadError::Unavailable)?;
    let cache_contents = match std::fs::read_to_string(&cache_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TransformationModelsCache::default())
        }
        Err(error) => {
            return Err(CacheReadError::Unreadable(format!(
                "failed to read model cache '{}': {}",
                cache_path.display(),
                error
            )))
        }
    };

    let cache: TransformationModelsCache = toml::from_str(&cache_contents).map_err(|error| {
        CacheReadError::Unreadable(format!(
            "failed to parse model cache '{}': {}",
            cache_path.display(),
            error
        ))
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
        api_key_belongs_to_provider, cached_models_in, extract_model_names,
        filter_completion_model_names, models_endpoint, normalize_model_names, plan_model_list,
        CacheReadError, InFlightFetch, ManualFetchReason, ModelListPlan, ModelListTrigger,
        TransformationModelAction, TransformationModelUpdate, TransformationModelsCache,
        TransformationModelsCacheEntry, TransformationModelsController,
        TransformationProviderRequest, CACHE_VERSION, MAX_MODEL_COUNT, MISSING_API_KEY_MESSAGE,
    };
    use serde_json::json;

    #[test]
    fn models_cache_file_parses_and_roundtrips_through_toml() {
        let cache_contents = concat!(
            "version = 2\n",
            "\n",
            "[[entries]]\n",
            "provider = \"openai\"\n",
            "account_fingerprint = \"5d43f605b5bbc62f\"\n",
            "updated_at_unix_seconds = 1767225600\n",
            "models = [\n",
            "    \"gpt-4.1\",\n",
            "    \"gpt-4o\",\n",
            "]\n",
        );

        let cache: TransformationModelsCache = toml::from_str(cache_contents).unwrap();
        let serialized = toml::to_string_pretty(&cache).unwrap();
        let reparsed: TransformationModelsCache = toml::from_str(&serialized).unwrap();

        for parsed in [&cache, &reparsed] {
            assert_eq!(parsed.version, CACHE_VERSION);
            assert_eq!(parsed.entries.len(), 1);
            let entry = &parsed.entries[0];
            assert_eq!(entry.provider, "openai");
            assert_eq!(entry.account_fingerprint, "5d43f605b5bbc62f");
            assert_eq!(entry.updated_at_unix_seconds, 1_767_225_600);
            assert_eq!(
                entry.models,
                vec!["gpt-4.1".to_owned(), "gpt-4o".to_owned()]
            );
        }
    }

    #[test]
    fn account_fingerprint_is_stable_sha256_prefix() {
        // The fingerprint is persisted in the models cache, so it must stay the
        // first 8 bytes of SHA-256(provider || 0x00 || api key) across upgrades.
        let with_key = TransformationProviderRequest::new(
            "OpenAI".to_owned(),
            Some("sk-test".to_owned()),
            "gpt-4o".to_owned(),
        );
        let without_key =
            TransformationProviderRequest::new("openai".to_owned(), None, "gpt-4o".to_owned());

        assert_eq!(with_key.account_fingerprint(), "5d43f605b5bbc62f");
        assert_eq!(without_key.account_fingerprint(), "a5115c737f57d969");
    }

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

    fn request(provider: &str, api_key: Option<&str>) -> TransformationProviderRequest {
        TransformationProviderRequest::new(
            provider.to_owned(),
            api_key.map(str::to_owned),
            String::new(),
        )
    }

    fn cache_with_entry(
        provider: &str,
        api_key: Option<&str>,
        models: &[&str],
    ) -> TransformationModelsCache {
        TransformationModelsCache {
            version: CACHE_VERSION,
            entries: vec![TransformationModelsCacheEntry {
                provider: provider.to_owned(),
                account_fingerprint: request(provider, api_key).account_fingerprint(),
                updated_at_unix_seconds: 1_767_225_600,
                models: models.iter().map(|model| (*model).to_owned()).collect(),
            }],
        }
    }

    #[test]
    fn cached_models_are_found_for_the_same_provider_and_account() {
        let cache = cache_with_entry("openai", Some("sk-test"), &["gpt-4o", "gpt-4o"]);

        assert_eq!(
            cached_models_in(Ok(cache), &request("openai", Some("sk-test"))),
            Ok(Some(vec!["gpt-4o".to_owned()]))
        );
    }

    #[test]
    fn a_missing_cache_entry_is_a_miss_not_an_error() {
        let cache = cache_with_entry("openai", Some("sk-test"), &["gpt-4o"]);

        assert_eq!(
            cached_models_in(Ok(cache.clone()), &request("openai", Some("sk-other"))),
            Ok(None)
        );
        assert_eq!(
            cached_models_in(Ok(cache), &request("anthropic", Some("sk-test"))),
            Ok(None)
        );
        assert_eq!(
            cached_models_in(
                Ok(TransformationModelsCache::default()),
                &request("openai", Some("sk-test"))
            ),
            Ok(None)
        );
    }

    #[test]
    fn a_cache_read_error_is_reported_not_treated_as_a_miss() {
        assert_eq!(
            cached_models_in(
                Err(CacheReadError::Unreadable(
                    "failed to parse model cache 'x': bad".to_owned()
                )),
                &request("openai", Some("sk-test"))
            ),
            Err(CacheReadError::Unreadable(
                "failed to parse model cache 'x': bad".to_owned()
            ))
        );
    }

    #[test]
    fn every_supported_provider_has_a_models_endpoint() {
        for provider in crate::config::supported_transformation_providers() {
            assert!(
                models_endpoint(provider).is_ok(),
                "no models endpoint for {}",
                provider
            );
        }
    }

    #[test]
    fn a_hosted_provider_without_an_api_key_cannot_fetch_models() {
        assert_eq!(
            request("openai", None).models_fetch_readiness(),
            Err(MISSING_API_KEY_MESSAGE.to_owned())
        );
        assert_eq!(
            request("openai", Some("sk-test")).models_fetch_readiness(),
            Ok(())
        );
    }

    #[test]
    fn providers_that_list_models_without_a_key_can_fetch_without_one() {
        assert_eq!(request("ollama", None).models_fetch_readiness(), Ok(()));
        assert_eq!(
            request("huggingface", None).models_fetch_readiness(),
            Ok(())
        );
    }

    #[test]
    fn cached_models_are_used_without_a_fetch() {
        for trigger in [ModelListTrigger::ProviderChanged, ModelListTrigger::Resync] {
            assert_eq!(
                plan_model_list(
                    trigger,
                    "openai",
                    Ok(Some(vec!["gpt-4o".to_owned(), "gpt-4.1".to_owned()])),
                    None,
                    Ok(()),
                    true,
                ),
                ModelListPlan::UseCached {
                    models: vec!["gpt-4o".to_owned(), "gpt-4.1".to_owned()],
                    message: "Loaded 2 cached models for openai.".to_owned(),
                }
            );
        }
    }

    #[test]
    fn a_provider_change_without_cached_models_starts_a_fetch() {
        assert_eq!(
            plan_model_list(
                ModelListTrigger::ProviderChanged,
                "openai",
                Ok(None),
                None,
                Ok(()),
                true
            ),
            ModelListPlan::StartFetch
        );
    }

    #[test]
    fn a_provider_change_without_cached_models_or_api_key_shows_the_missing_key_message() {
        assert_eq!(
            plan_model_list(
                ModelListTrigger::ProviderChanged,
                "openai",
                Ok(None),
                None,
                Err(MISSING_API_KEY_MESSAGE.to_owned()),
                true,
            ),
            ModelListPlan::ShowError(MISSING_API_KEY_MESSAGE.to_owned())
        );
    }

    #[test]
    fn a_provider_change_does_not_send_a_key_that_may_be_another_providers() {
        assert_eq!(
            plan_model_list(
                ModelListTrigger::ProviderChanged,
                "anthropic",
                Ok(None),
                None,
                Ok(()),
                false,
            ),
            ModelListPlan::NeedsManualFetch(ManualFetchReason::UnsavedApiKey)
        );
    }

    #[test]
    fn no_second_fetch_starts_while_one_is_in_flight() {
        for trigger in [ModelListTrigger::ProviderChanged, ModelListTrigger::Resync] {
            for action in [
                TransformationModelAction::Refresh,
                TransformationModelAction::Check,
            ] {
                assert_eq!(
                    plan_model_list(trigger, "openai", Ok(None), Some(action), Ok(()), true),
                    ModelListPlan::AwaitFetch(action)
                );
            }
        }
    }

    #[test]
    fn a_cache_read_error_is_reported_instead_of_fetching() {
        for trigger in [ModelListTrigger::ProviderChanged, ModelListTrigger::Resync] {
            assert_eq!(
                plan_model_list(
                    trigger,
                    "openai",
                    Err(CacheReadError::Unreadable(
                        "failed to read model cache 'x': denied".to_owned()
                    )),
                    None,
                    Ok(()),
                    true,
                ),
                ModelListPlan::CacheUnreadable("failed to read model cache 'x': denied".to_owned())
            );
        }
    }

    #[test]
    fn an_unavailable_cache_directory_is_shown_without_promising_a_rebuild() {
        assert_eq!(
            plan_model_list(
                ModelListTrigger::ProviderChanged,
                "openai",
                Err(CacheReadError::Unavailable(
                    "Neither HOME nor XDG_CACHE_HOME is available".to_owned()
                )),
                None,
                Ok(()),
                true,
            ),
            ModelListPlan::ShowError("Neither HOME nor XDG_CACHE_HOME is available".to_owned())
        );
    }

    #[test]
    fn a_resync_without_cached_models_or_api_key_shows_the_missing_key_message() {
        assert_eq!(
            plan_model_list(
                ModelListTrigger::Resync,
                "openai",
                Ok(None),
                None,
                Err(MISSING_API_KEY_MESSAGE.to_owned()),
                true,
            ),
            ModelListPlan::ShowError(MISSING_API_KEY_MESSAGE.to_owned())
        );
    }

    #[test]
    fn a_resync_without_cached_models_does_not_fetch() {
        assert_eq!(
            plan_model_list(
                ModelListTrigger::Resync,
                "openai",
                Ok(None),
                None,
                Ok(()),
                true
            ),
            ModelListPlan::NeedsManualFetch(ManualFetchReason::NotCached)
        );
    }

    #[test]
    fn an_api_key_from_the_field_belongs_to_a_provider_only_when_saved_for_it() {
        let anthropic = request("anthropic", Some("sk-openai"));

        assert!(api_key_belongs_to_provider(
            &anthropic,
            Some("sk-openai"),
            Some("anthropic"),
            Some("sk-openai"),
        ));
        assert!(api_key_belongs_to_provider(
            &anthropic,
            Some(" sk-openai "),
            Some("Anthropic"),
            Some("sk-openai"),
        ));
        assert!(!api_key_belongs_to_provider(
            &anthropic,
            Some("sk-openai"),
            Some("openai"),
            Some("sk-openai"),
        ));
        assert!(!api_key_belongs_to_provider(
            &anthropic,
            Some("sk-openai"),
            Some("anthropic"),
            Some("sk-saved"),
        ));
        assert!(!api_key_belongs_to_provider(
            &anthropic,
            Some("sk-openai"),
            None,
            None
        ));
        // Hugging Face lists models without a key but sends one when set.
        assert!(!api_key_belongs_to_provider(
            &request("huggingface", Some("sk-openai")),
            Some("sk-openai"),
            Some("openai"),
            Some("sk-openai"),
        ));
    }

    #[test]
    fn an_environment_key_or_a_request_without_a_key_belongs_to_the_provider() {
        assert!(api_key_belongs_to_provider(
            &request("anthropic", Some("sk-env")),
            None,
            Some("openai"),
            Some("sk-openai"),
        ));
        assert!(api_key_belongs_to_provider(
            &request("anthropic", Some("sk-env")),
            Some("  "),
            None,
            None,
        ));
        // Ollama's models endpoint takes no key, so none is sent.
        assert!(api_key_belongs_to_provider(
            &request("ollama", Some("sk-openai")),
            Some("sk-openai"),
            Some("openai"),
            Some("sk-openai"),
        ));
    }

    #[test]
    fn an_update_applies_only_to_the_provider_and_account_it_was_requested_for() {
        let update = TransformationModelUpdate::ModelsRefreshed {
            request: request("openai", Some("sk-test")),
            models: vec!["gpt-4o".to_owned()],
            message: String::new(),
        };

        assert!(update.applies_to(Some(&request("openai", Some("sk-test")))));
        assert!(!update.applies_to(Some(&request("anthropic", Some("sk-test")))));
        assert!(!update.applies_to(Some(&request("openai", Some("sk-other")))));
        assert!(!update.applies_to(None));
    }

    #[test]
    fn only_a_check_shows_a_result_on_the_check_button() {
        let openai = request("openai", Some("sk-test"));
        let failed = |action| TransformationModelUpdate::ActionFailed {
            request: openai.clone(),
            action,
            message: String::new(),
        };

        assert_eq!(
            TransformationModelUpdate::ConnectionChecked {
                request: openai.clone(),
                models: Vec::new(),
                message: String::new(),
            }
            .check_result(),
            Some(true)
        );
        assert_eq!(
            failed(TransformationModelAction::Check).check_result(),
            Some(false)
        );
        assert_eq!(
            failed(TransformationModelAction::Refresh).check_result(),
            None
        );
        assert_eq!(
            TransformationModelUpdate::ModelsRefreshed {
                request: openai.clone(),
                models: Vec::new(),
                message: String::new(),
            }
            .check_result(),
            None
        );
    }

    #[test]
    fn fetches_in_flight_are_tracked_per_action_provider_and_account() {
        let controller = TransformationModelsController::new();
        let openai = request("openai", Some("sk-test"));

        let check = InFlightFetch::start(&controller, TransformationModelAction::Check, &openai);
        let refresh =
            InFlightFetch::start(&controller, TransformationModelAction::Refresh, &openai);
        assert_eq!(
            controller.fetch_in_flight(&openai),
            Some(TransformationModelAction::Check)
        );
        assert_eq!(
            controller.fetch_in_flight(&request("openai", Some("sk-other"))),
            None
        );
        assert_eq!(
            controller.fetch_in_flight(&request("anthropic", Some("sk-test"))),
            None
        );

        refresh.finish(TransformationModelUpdate::ActionFailed {
            request: openai.clone(),
            action: TransformationModelAction::Refresh,
            message: String::new(),
        });
        assert_eq!(
            controller.fetch_in_flight(&openai),
            Some(TransformationModelAction::Check)
        );

        check.finish(TransformationModelUpdate::ConnectionChecked {
            request: openai.clone(),
            models: Vec::new(),
            message: String::new(),
        });
        assert_eq!(controller.fetch_in_flight(&openai), None);
        assert!(controller.take_update().is_some());
        assert!(controller.take_update().is_some());
        assert!(controller.take_update().is_none());
    }

    #[test]
    fn a_fetch_dropped_without_a_result_is_reported_as_failed_and_leaves_flight() {
        let controller = TransformationModelsController::new();
        let openai = request("openai", Some("sk-test"));

        drop(InFlightFetch::start(
            &controller,
            TransformationModelAction::Refresh,
            &openai,
        ));

        assert_eq!(controller.fetch_in_flight(&openai), None);
        match controller.take_update() {
            Some(TransformationModelUpdate::ActionFailed {
                request,
                action,
                message,
            }) => {
                assert_eq!(request, openai);
                assert_eq!(action, TransformationModelAction::Refresh);
                assert_eq!(message, "Fetching models for openai stopped unexpectedly.");
            }
            other => panic!("expected ActionFailed, got {:?}", other),
        }
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
