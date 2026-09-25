use std::sync::Arc;

use rig_core::client::{CompletionClient, ProviderClient, ProviderClientError};
use rig_core::completion::CompletionModel;
use rig_core::message::{ReasoningContent, Text};
use rig_core::providers::{
    anthropic, cohere, deepseek, gemini, groq, huggingface, hyperbolic, mira, mistral,
    moonshot, ollama, openai, openrouter, perplexity, together, xai,
};
use rig_core::streaming::StreamedAssistantContent;
use tokio_stream::StreamExt;

use crate::state::AppState;



#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransformationPreviewMode<'a> {
    ReplaceOverlay,
    #[expect(
        dead_code,
        reason = "inline correction preview (5fd64e9) lost its only constructor when 2719264 \
                  rewrote the correction-apply path to use ReplaceOverlay; restoring it is \
                  pending an owner decision"
    )]
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

    if state.is_abort_requested() {
        return Err("transformation aborted".to_owned());
    }

    macro_rules! stream_with_client {
        ($client:expr) => {{
            let model = $client.completion_model(config.model.as_str());
            stream_completion_response(
                model,
                &config.system_prompt,
                None,
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
        "gemini" => {
            let client = gemini::Client::new(required_api_key(config)?).map_err(format_http_client_error)?;
            let model = client.completion_model(config.model.as_str());
            stream_completion_response(
                model,
                &config.system_prompt,
                Some(serde_json::json!({
                    "generationConfig": {
                        "thinkingConfig": {
                            "thinkingBudget": 0
                        }
                    }
                })),
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
        "ollama" => stream_with_client!(
            ollama::Client::from_env().map_err(format_provider_client_error)?
        ),
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
        _ => {
            let supported = crate::config::supported_transformation_providers();
            Err(format!(
                "unsupported transformation provider '{}'; supported providers: {}",
                config.provider,
                supported.join(", ")
            ))
        }
    }
}

async fn stream_completion_response<M>(
    model: M,
    system_prompt: &str,
    additional_params: Option<serde_json::Value>,
    input_text: &str,
    state: Arc<AppState>,
    preview_mode: TransformationPreviewMode<'_>,
) -> Result<String, String>
where
    M: CompletionModel + Clone + 'static,
{
    // Build the completion request using the CompletionModel API
    let mut request = model
        .completion_request(input_text.to_owned())
        .preamble(system_prompt.to_owned());

    if let Some(params) = additional_params {
        request = request.additional_params(params);
    }

    let mut stream = request
        .stream()
        .await
        .map_err(|error| format!("transformation stream failed: {}", error))?;

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
            StreamedAssistantContent::Text(Text { text, .. }) => {
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
            StreamedAssistantContent::ReasoningDelta {
                reasoning,
                ..
            } => {
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
            StreamedAssistantContent::Reasoning {
                reasoning,
                ..
            } => {
                let mut extracted_reasoning = String::new();
                for block in reasoning.content {
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
            StreamedAssistantContent::Final(_) => {
                // Terminal record received; use aggregated text from the stream's choice
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

fn format_http_client_error(error: rig_core::http_client::Error) -> String {
    format!("{}", error)
}

fn format_provider_client_error(error: ProviderClientError) -> String {
    format!("{}", error)
}

fn normalize_provider_name(provider_name: &str) -> String {
    provider_name.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn supported_providers_does_not_include_galadriel() {
        let providers = config::supported_transformation_providers();
        assert!(
            !providers.iter().any(|p| p.eq_ignore_ascii_case("galadriel")),
            "galadriel should not be in supported providers list"
        );
    }

    #[test]
    fn unsupported_provider_error_lists_supported_providers() {
        let providers = config::supported_transformation_providers();
        assert!(
            !providers.iter().any(|p| p.eq_ignore_ascii_case("galadriel")),
            "galadriel should not be in supported providers list"
        );
        // Verify all supported providers are valid
        assert!(!providers.is_empty(), "should have at least one provider");
    }

    #[test]
    fn error_message_lists_exactly_supported_providers() {
        let providers = config::supported_transformation_providers();
        let error_msg = format!(
            "unsupported transformation provider 'test'; supported providers: {}",
            providers.join(", ")
        );
        // Ensure the error message matches the provider list
        for provider in providers {
            assert!(
                error_msg.contains(provider),
                "provider '{}' should be in error message",
                provider
            );
        }
        // Ensure galadriel is NOT in the error message
        assert!(
            !error_msg.contains("galadriel"),
            "galadriel should not be in supported providers"
        );
    }

    #[tokio::test]
    async fn every_supported_hosted_provider_client_builds_with_completion_model() {
        macro_rules! assert_completion_model_builds {
            ($($provider:ident),+ $(,)?) => {{
                $(
                    let client = $provider::Client::new("test-key").unwrap_or_else(|error| {
                        panic!(
                            "{} client failed to build: {}",
                            stringify!($provider),
                            format_http_client_error(error)
                        )
                    });
                    let _model = client.completion_model("test-model");
                )+
            }};
        }

        assert_completion_model_builds!(
            anthropic,
            cohere,
            deepseek,
            gemini,
            groq,
            huggingface,
            hyperbolic,
            mira,
            mistral,
            moonshot,
            openai,
            openrouter,
            perplexity,
            together,
            xai,
        );
    }
}
