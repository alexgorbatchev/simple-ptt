use std::sync::Arc;
use std::time::Duration;

use rig::agent::{Agent, MultiTurnStreamItem};
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{CompletionModel, GetTokenUsage};
use rig::message::{ReasoningContent, Text};
use rig::providers::{
    anthropic, cohere, deepseek, galadriel, gemini, groq, huggingface, hyperbolic, mira, mistral,
    moonshot, ollama, openai, openrouter, perplexity, together, xai,
};
use rig::streaming::{StreamedAssistantContent, StreamingPrompt};
use tokio_stream::StreamExt;

use crate::state::AppState;

const TRANSFORMATION_FADE_SETTLE_DELAY_MS: u64 = 150;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransformationPreviewMode<'a> {
    ReplaceOverlay,
    InlineCorrection { original_text: &'a str },
}

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
    preview_mode: TransformationPreviewMode<'_>,
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
                preview_mode,
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
        "gemini" => {
            let client = gemini::Client::new(required_api_key(config)?).map_err(format_http_client_error)?;
            let agent = client
                .agent(config.model.as_str())
                .preamble(config.system_prompt.as_str())
                .additional_params(serde_json::json!({
                    "generationConfig": {
                        "thinkingConfig": {
                            "thinkingBudget": 0
                        }
                    }
                }))
                .build();
            stream_agent_response(
                agent,
                input_text,
                Arc::clone(&state),
                preview_mode,
            )
            .await
        }
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
    preview_mode: TransformationPreviewMode<'_>,
) -> Result<String, String>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Clone + GetTokenUsage + Send + Unpin + 'static,
{
    let mut stream = agent.stream_prompt(input_text).await;
    let mut transformed_text = String::new();
    let mut thinking_text = String::new();
    let mut saw_text = false;
    let mut saw_thinking = false;

    while let Some(chunk_result) = stream.next().await {
        if state.is_abort_requested() {
            return Err("transformation aborted".to_owned());
        }

        let chunk =
            chunk_result.map_err(|error| format!("transformation stream failed: {}", error))?;
        log::debug!("Received stream chunk");

        match chunk {
            MultiTurnStreamItem::StreamAssistantItem(content) => {
                match content {
                    StreamedAssistantContent::Text(Text { text }) => {
                        if text.is_empty() {
                            continue;
                        }

                        if !saw_text {
                            transformed_text.clear();
                            match preview_mode {
                                TransformationPreviewMode::ReplaceOverlay => {
                                    state.set_overlay_text(String::new());
                                }
                                TransformationPreviewMode::InlineCorrection { original_text } => {
                                    state.set_overlay_text(original_text.to_owned());
                                    state.clear_overlay_correction_text();
                                }
                            }
                            state.set_overlay_text_opacity(1.0);
                            saw_text = true;
                        }

                        transformed_text.push_str(&text);
                        match preview_mode {
                            TransformationPreviewMode::ReplaceOverlay => {
                                state.set_overlay_text(transformed_text.clone());
                            }
                            TransformationPreviewMode::InlineCorrection { .. } => {
                                state.set_overlay_correction_text(transformed_text.clone());
                            }
                        }
                    }
                    StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                        if reasoning.is_empty() {
                            continue;
                        }

                        if !saw_thinking && !saw_text {
                            thinking_text.clear();
                            match preview_mode {
                                TransformationPreviewMode::ReplaceOverlay => {
                                    state.set_overlay_text(String::new());
                                }
                                TransformationPreviewMode::InlineCorrection { original_text } => {
                                    state.set_overlay_text(original_text.to_owned());
                                    state.clear_overlay_correction_text();
                                }
                            }
                            state.set_overlay_text_opacity(0.6); // Slightly dim for thinking
                            saw_thinking = true;
                        }

                        thinking_text.push_str(&reasoning);

                        // Only show thinking if the actual answer hasn't started yet
                        if !saw_text {
                            let display_thinking = format!("Thinking: {}", thinking_text);
                            match preview_mode {
                                TransformationPreviewMode::ReplaceOverlay => {
                                    state.set_overlay_text(display_thinking);
                                }
                                TransformationPreviewMode::InlineCorrection { .. } => {
                                    state.set_overlay_correction_text(display_thinking);
                                }
                            }
                        }
                    }
                    StreamedAssistantContent::Reasoning(reasoning_block) => {
                        let mut extracted_reasoning = String::new();
                        for block in reasoning_block.content {
                            match block {
                                ReasoningContent::Text { text, .. } => {
                                    extracted_reasoning.push_str(&text);
                                }
                                ReasoningContent::Summary(text) => {
                                    extracted_reasoning.push_str(&text);
                                }
                                _ => {}
                            }
                        }

                        if extracted_reasoning.is_empty() {
                            continue;
                        }

                        if !saw_thinking && !saw_text {
                            thinking_text.clear();
                            match preview_mode {
                                TransformationPreviewMode::ReplaceOverlay => {
                                    state.set_overlay_text(String::new());
                                }
                                TransformationPreviewMode::InlineCorrection { original_text } => {
                                    state.set_overlay_text(original_text.to_owned());
                                    state.clear_overlay_correction_text();
                                }
                            }
                            state.set_overlay_text_opacity(0.6); // Slightly dim for thinking
                            saw_thinking = true;
                        }

                        thinking_text.push_str(&extracted_reasoning);

                        // Only show thinking if the actual answer hasn't started yet
                        if !saw_text {
                            let display_thinking = format!("Thinking: {}", thinking_text);
                            match preview_mode {
                                TransformationPreviewMode::ReplaceOverlay => {
                                    state.set_overlay_text(display_thinking);
                                }
                                TransformationPreviewMode::InlineCorrection { .. } => {
                                    state.set_overlay_correction_text(display_thinking);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            MultiTurnStreamItem::FinalResponse(final_response) => {
                if transformed_text.trim().is_empty() {
                    let response_text = final_response.response().trim();
                    if !response_text.is_empty() {
                        transformed_text = response_text.to_owned();
                        match preview_mode {
                            TransformationPreviewMode::ReplaceOverlay => {
                                state.set_overlay_text(transformed_text.clone());
                            }
                            TransformationPreviewMode::InlineCorrection { .. } => {
                                state.set_overlay_correction_text(transformed_text.clone());
                            }
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn test_gemini_streaming_items() {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .unwrap_or_default();
        if api_key.is_empty() {
            println!("Skipping test because GEMINI_API_KEY/GOOGLE_API_KEY is not set.");
            return;
        }

        let client = gemini::Client::new(&api_key).expect("failed to create client");
        let agent = client
            .agent("gemini-2.5-flash")
            .preamble("You are a helpful assistant.")
            .additional_params(serde_json::json!({
                "generationConfig": {
                    "thinkingConfig": {
                        "thinkingBudget": 0
                    }
                }
            }))
            .build();

        let mut stream = agent
            .stream_prompt("Write a 300 word essay about Apple.")
            .await;
        println!("Starting stream...");
        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(item) => {
                    println!("Chunk item: {:?}", item);
                }
                Err(e) => {
                    println!("Error: {:?}", e);
                }
            }
        }
        println!("Stream finished.");
    }
}
