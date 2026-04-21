use std::sync::Arc;
use std::time::Duration;

use rig::agent::{Agent, MultiTurnStreamItem};
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{CompletionModel, GetTokenUsage};
use rig::message::Text;
use rig::providers::{
    anthropic, cohere, deepseek, galadriel, gemini, groq, huggingface, hyperbolic, mira, mistral,
    moonshot, ollama, openai, openrouter, perplexity, together, xai,
};
use rig::streaming::{StreamedAssistantContent, StreamingPrompt};
use tokio_stream::StreamExt;

use crate::state::AppState;

const TRANSFORMATION_FADE_SETTLE_DELAY_MS: u64 = 150;

#[derive(Clone, Debug)]
pub struct TransformationRuntimeConfig {
    pub provider: String,
    pub api_key: Option<String>,
    pub model: String,
    pub system_prompt: String,
    pub correction_system_prompt: String,
}

pub async fn transform_text(
    state: Arc<AppState>,
    config: &TransformationRuntimeConfig,
    input_text: &str,
) -> Result<String, String> {
    let normalized_provider = normalize_provider_name(&config.provider);

    tokio::time::sleep(Duration::from_millis(TRANSFORMATION_FADE_SETTLE_DELAY_MS)).await;
    if state.is_abort_requested() {
        return Err("transformation aborted".to_owned());
    }

    macro_rules! stream_with_client {
        ($client:expr) => {{
            stream_agent_response(
                $client
                    .agent(config.model.as_str())
                    .preamble(config.system_prompt.as_str())
                    .build(),
                input_text,
                Arc::clone(&state),
            )
            .await
        }};
    }

    match normalized_provider.as_str() {
        "anthropic" => stream_with_client!(
            anthropic::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "cohere" => stream_with_client!(
            cohere::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "deepseek" => stream_with_client!(
            deepseek::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "galadriel" => stream_with_client!(
            galadriel::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "gemini" => stream_with_client!(
            gemini::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "groq" => stream_with_client!(
            groq::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "huggingface" => stream_with_client!(
            huggingface::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "hyperbolic" => stream_with_client!(
            hyperbolic::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "mira" => stream_with_client!(
            mira::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "mistral" => stream_with_client!(
            mistral::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "moonshot" => stream_with_client!(
            moonshot::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "ollama" => stream_with_client!(ollama::Client::from_env()),
        "openai" => stream_with_client!(
            openai::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "openrouter" => stream_with_client!(
            openrouter::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "perplexity" => stream_with_client!(
            perplexity::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "together" => stream_with_client!(
            together::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "xai" => stream_with_client!(
            xai::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        _ => Err(format!(
            "unsupported transformation provider '{}'; supported providers: anthropic, cohere, deepseek, galadriel, gemini, groq, huggingface, hyperbolic, mira, mistral, moonshot, ollama, openai, openrouter, perplexity, together, xai",
            config.provider
        )),
    }
}

async fn stream_agent_response<M>(
    agent: Agent<M>,
    input_text: &str,
    state: Arc<AppState>,
) -> Result<String, String>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Clone + GetTokenUsage + Send + Unpin + 'static,
{
    let mut stream = agent.stream_prompt(input_text).await;
    let mut transformed_text = String::new();
    let mut saw_text = false;

    while let Some(chunk_result) = stream.next().await {
        if state.is_abort_requested() {
            return Err("transformation aborted".to_owned());
        }

        match chunk_result.map_err(|error| format!("transformation stream failed: {}", error))? {
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(Text {
                text,
                ..
            })) => {
                if text.is_empty() {
                    continue;
                }

                if !saw_text {
                    transformed_text.clear();
                    state.set_overlay_text(String::new());
                    state.set_overlay_text_opacity(1.0);
                    saw_text = true;
                }

                transformed_text.push_str(&text);
                state.set_overlay_text(transformed_text.clone());
            }
            MultiTurnStreamItem::FinalResponse(final_response) => {
                if transformed_text.trim().is_empty() {
                    let response_text = final_response.response().trim();
                    if !response_text.is_empty() {
                        transformed_text = response_text.to_owned();
                        state.set_overlay_text(transformed_text.clone());
                        state.set_overlay_text_opacity(1.0);
                    }
                }
                break;
            }
            _ => {}
        }
    }

    let finalized_text = transformed_text.trim().to_owned();
    if finalized_text.is_empty() {
        return Err("transformation completed without returning any text".to_owned());
    }

    Ok(finalized_text)
}

fn required_api_key<'a>(config: &'a TransformationRuntimeConfig) -> Result<&'a str, String> {
    config
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "transformation.api_key is required for provider '{}'",
                config.provider
            )
        })
}

fn format_http_client_error(error: rig::http_client::Error) -> String {
    format!("{}", error)
}

fn normalize_provider_name(provider_name: &str) -> String {
    provider_name.trim().to_ascii_lowercase()
}
