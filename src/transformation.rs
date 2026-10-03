use std::sync::Arc;

use rig_core::client::{CompletionClient, ProviderClient, ProviderClientError};
use rig_core::completion::CompletionModel;
use rig_core::message::{Reasoning, ReasoningContent, Text};
use rig_core::providers::{
    anthropic, cohere, deepseek, gemini, groq, huggingface, hyperbolic, mira, mistral, moonshot,
    ollama, openai, openrouter, perplexity, together, xai,
};
use rig_core::streaming::{StreamedAssistantContent, StreamingCompletionResponse};
use tokio_stream::StreamExt;

use crate::state::AppState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransformationPreviewMode<'a> {
    ReplaceOverlay,
    /// Collect the result for pasting while the narrated text stays visible.
    PreserveOverlay,
    #[expect(
        dead_code,
        reason = "inline correction preview (5fd64e9) lost its only constructor when 2719264 \
                  rewrote the correction-apply path to use ReplaceOverlay; restoring it is \
                  pending an owner decision"
    )]
    InlineCorrection {
        original_text: &'a str,
    },
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
                input_text,
                Arc::clone(&state),
                preview_mode,
            )
            .await
        }};
    }

    match normalized_provider.as_str() {
        "anthropic" => {
            stream_with_client!(anthropic::Client::new(required_api_key(config)?)
                .map_err(format_http_client_error)?)
        }
        "cohere" => stream_with_client!(
            cohere::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "deepseek" => {
            stream_with_client!(deepseek::Client::new(required_api_key(config)?)
                .map_err(format_http_client_error)?)
        }
        "gemini" => stream_with_client!(
            gemini::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "groq" => stream_with_client!(
            groq::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "huggingface" => stream_with_client!(huggingface::Client::new(required_api_key(config)?)
            .map_err(format_http_client_error)?),
        "hyperbolic" => {
            stream_with_client!(hyperbolic::Client::new(required_api_key(config)?)
                .map_err(format_http_client_error)?)
        }
        "mira" => stream_with_client!(
            mira::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "mistral" => {
            stream_with_client!(mistral::Client::new(required_api_key(config)?)
                .map_err(format_http_client_error)?)
        }
        "moonshot" => {
            stream_with_client!(moonshot::Client::new(required_api_key(config)?)
                .map_err(format_http_client_error)?)
        }
        "ollama" => {
            stream_with_client!(ollama::Client::from_env().map_err(format_provider_client_error)?)
        }
        "openai" => stream_with_client!(
            openai::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        "openrouter" => {
            stream_with_client!(openrouter::Client::new(required_api_key(config)?)
                .map_err(format_http_client_error)?)
        }
        "perplexity" => {
            stream_with_client!(perplexity::Client::new(required_api_key(config)?)
                .map_err(format_http_client_error)?)
        }
        "together" => {
            stream_with_client!(together::Client::new(required_api_key(config)?)
                .map_err(format_http_client_error)?)
        }
        "xai" => stream_with_client!(
            xai::Client::new(required_api_key(config)?).map_err(format_http_client_error)?
        ),
        _ => Err(unsupported_provider_error(&config.provider)),
    }
}

fn unsupported_provider_error(provider: &str) -> String {
    format!(
        "unsupported transformation provider '{}'; supported providers: {}",
        provider,
        crate::config::supported_transformation_providers().join(", ")
    )
}

async fn stream_completion_response<M>(
    model: M,
    system_prompt: &str,
    input_text: &str,
    state: Arc<AppState>,
    preview_mode: TransformationPreviewMode<'_>,
) -> Result<String, String>
where
    M: CompletionModel + Clone,
{
    let stream = model
        .completion_request(input_text.to_owned())
        .preamble(system_prompt.to_owned())
        .stream()
        .await
        .map_err(|error| format!("transformation stream failed: {}", error))?;

    drain_stream(stream, &state, preview_mode).await
}

async fn drain_stream(
    mut stream: StreamingCompletionResponse,
    state: &AppState,
    preview_mode: TransformationPreviewMode<'_>,
) -> Result<String, String> {
    let mut accumulator = StreamAccumulator::default();

    // Drained to the end rather than stopped at `Final`: rig passes provider
    // events through in order and does not end the stream at the terminal
    // record, so text can still follow it.
    while let Some(chunk_result) = stream.next().await {
        if state.is_abort_requested() {
            return Err("transformation aborted".to_owned());
        }

        let chunk =
            chunk_result.map_err(|error| format!("transformation stream failed: {}", error))?;
        log::debug!("Received stream chunk");

        if let Some(update) = accumulator.apply(chunk) {
            apply_overlay_update(state, preview_mode, update);
        }
    }

    accumulator.into_final_text()
}

const ANSWER_TEXT_OPACITY: f64 = 1.0;
const THINKING_TEXT_OPACITY: f64 = 0.6;

/// One change to the transformation preview, produced by [`StreamAccumulator`].
#[derive(Clone, Debug, PartialEq)]
struct OverlayUpdate {
    /// `Some` when this update starts a new display phase (thinking or answer):
    /// the preview is cleared and its text opacity set before `text` is shown.
    reset_with_opacity: Option<f64>,
    text: String,
}

/// Pure reducer over streamed assistant items.
///
/// A completed `Reasoning` item replaces the `ReasoningDelta` text accumulated
/// under the same rig correlator `id`, as the rig 0.42 streaming contract
/// requires. Reasoning parts are shown in the order their ids first appeared.
#[derive(Debug, Default)]
struct StreamAccumulator {
    transformed_text: String,
    reasoning_parts: Vec<(String, String)>,
    showing_thinking: bool,
}

impl StreamAccumulator {
    fn apply(&mut self, item: StreamedAssistantContent) -> Option<OverlayUpdate> {
        match item {
            StreamedAssistantContent::Text(Text { text, .. }) => {
                if text.is_empty() {
                    return None;
                }

                let reset_with_opacity = self
                    .transformed_text
                    .is_empty()
                    .then_some(ANSWER_TEXT_OPACITY);
                self.transformed_text.push_str(&text);
                Some(OverlayUpdate {
                    reset_with_opacity,
                    text: self.transformed_text.clone(),
                })
            }
            StreamedAssistantContent::ReasoningDelta { id, reasoning, .. } => {
                if reasoning.is_empty() {
                    return None;
                }

                self.reasoning_part_mut(id).push_str(&reasoning);
                self.thinking_update()
            }
            StreamedAssistantContent::Reasoning { reasoning, id } => {
                // A completion without displayable text (encrypted, redacted or
                // signature-only) must not blank thinking already shown.
                let completed = displayable_reasoning_text(&reasoning);
                if completed.is_empty() {
                    return None;
                }

                *self.reasoning_part_mut(id) = completed;
                self.thinking_update()
            }
            _ => None,
        }
    }

    fn into_final_text(self) -> Result<String, String> {
        let finalized_text = self.transformed_text.trim();
        if finalized_text.is_empty() {
            return Err("transformation completed without returning any text".to_owned());
        }

        Ok(finalized_text.to_owned())
    }

    fn reasoning_part_mut(&mut self, id: String) -> &mut String {
        let index = match self
            .reasoning_parts
            .iter()
            .position(|(part_id, _)| *part_id == id)
        {
            Some(index) => index,
            None => {
                self.reasoning_parts.push((id, String::new()));
                self.reasoning_parts.len() - 1
            }
        };
        &mut self.reasoning_parts[index].1
    }

    /// Thinking is shown only until the answer text starts.
    fn thinking_update(&mut self) -> Option<OverlayUpdate> {
        if !self.transformed_text.is_empty() {
            return None;
        }

        let thinking_text: String = self
            .reasoning_parts
            .iter()
            .map(|(_, text)| text.as_str())
            .collect();
        let reset_with_opacity = (!self.showing_thinking).then_some(THINKING_TEXT_OPACITY);
        self.showing_thinking = true;
        Some(OverlayUpdate {
            reset_with_opacity,
            text: format!("Thinking: {}", thinking_text),
        })
    }
}

/// Not `Reasoning::display_text`: that also renders opaque `Redacted` payloads
/// and joins blocks with newlines, which the concatenated deltas it replaces
/// never contain.
fn displayable_reasoning_text(reasoning: &Reasoning) -> String {
    reasoning
        .content
        .iter()
        .filter_map(|block| match block {
            ReasoningContent::Text { text, .. } | ReasoningContent::Summary(text) => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect()
}

fn apply_overlay_update(
    state: &AppState,
    preview_mode: TransformationPreviewMode<'_>,
    update: OverlayUpdate,
) {
    match preview_mode {
        TransformationPreviewMode::PreserveOverlay => {}
        TransformationPreviewMode::ReplaceOverlay => {
            if let Some(opacity) = update.reset_with_opacity {
                state.set_overlay_text(String::new());
                state.set_overlay_text_opacity(opacity);
            }
            state.set_overlay_text(update.text);
        }
        TransformationPreviewMode::InlineCorrection { original_text } => {
            if let Some(opacity) = update.reset_with_opacity {
                state.set_overlay_text(original_text.to_owned());
                state.clear_overlay_correction_text();
                state.set_overlay_text_opacity(opacity);
            }
            state.set_overlay_correction_text(update.text);
        }
    }
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
    use rig_core::completion::{CompletionError, Usage};
    use rig_core::streaming::{RawStreamingChoice, StreamFinal};

    fn update(reset_with_opacity: Option<f64>, text: &str) -> Option<OverlayUpdate> {
        Some(OverlayUpdate {
            reset_with_opacity,
            text: text.to_owned(),
        })
    }

    fn reasoning_delta(
        id: &str,
        provider_id: Option<&str>,
        reasoning: &str,
    ) -> StreamedAssistantContent {
        StreamedAssistantContent::ReasoningDelta {
            id: id.to_owned(),
            provider_id: provider_id.map(str::to_owned),
            reasoning: reasoning.to_owned(),
        }
    }

    fn completed_reasoning(id: &str, reasoning: Reasoning) -> StreamedAssistantContent {
        StreamedAssistantContent::Reasoning {
            reasoning,
            id: id.to_owned(),
        }
    }

    fn final_item() -> StreamedAssistantContent {
        StreamedAssistantContent::Final(StreamFinal::new("test-provider", Usage::new()))
    }

    #[test]
    fn unsupported_provider_error_names_provider_and_lists_supported_providers() {
        assert_eq!(
            unsupported_provider_error("galadriel"),
            "unsupported transformation provider 'galadriel'; supported providers: \
             anthropic, cohere, deepseek, gemini, groq, huggingface, hyperbolic, mira, mistral, \
             moonshot, ollama, openai, openrouter, perplexity, together, xai"
        );
    }

    #[test]
    fn text_deltas_accumulate_and_reset_the_overlay_once() {
        let mut accumulator = StreamAccumulator::default();

        assert_eq!(
            accumulator.apply(StreamedAssistantContent::text("Hel")),
            update(Some(1.0), "Hel")
        );
        assert_eq!(accumulator.apply(StreamedAssistantContent::text("")), None);
        assert_eq!(
            accumulator.apply(StreamedAssistantContent::text("lo ")),
            update(None, "Hello ")
        );
        assert_eq!(accumulator.apply(final_item()), None);
        assert_eq!(accumulator.into_final_text(), Ok("Hello".to_owned()));
    }

    #[test]
    fn reasoning_is_shown_dimmed_until_the_answer_starts() {
        let mut accumulator = StreamAccumulator::default();

        assert_eq!(
            accumulator.apply(reasoning_delta("X", None, "a")),
            update(Some(0.6), "Thinking: a")
        );
        assert_eq!(accumulator.apply(reasoning_delta("X", None, "")), None);
        assert_eq!(
            accumulator.apply(reasoning_delta("X", None, "b")),
            update(None, "Thinking: ab")
        );
        assert_eq!(
            accumulator.apply(StreamedAssistantContent::text("answer")),
            update(Some(1.0), "answer")
        );
        assert_eq!(accumulator.apply(reasoning_delta("X", None, "c")), None);
        assert_eq!(
            accumulator.apply(completed_reasoning("X", Reasoning::new("abc"))),
            None
        );
        assert_eq!(accumulator.into_final_text(), Ok("answer".to_owned()));
    }

    #[test]
    fn completed_reasoning_replaces_deltas_with_the_same_id() {
        let mut accumulator = StreamAccumulator::default();

        accumulator.apply(reasoning_delta("X", None, "a"));
        accumulator.apply(reasoning_delta("X", None, "b"));

        assert_eq!(
            accumulator.apply(completed_reasoning("X", Reasoning::new("ab"))),
            update(None, "Thinking: ab")
        );
    }

    #[test]
    fn completed_reasoning_keeps_each_part_in_first_seen_order() {
        let mut accumulator = StreamAccumulator::default();

        accumulator.apply(reasoning_delta("X", None, "a"));
        assert_eq!(
            accumulator.apply(completed_reasoning(
                "Y",
                Reasoning::summaries(vec!["z".to_owned()])
            )),
            update(None, "Thinking: az")
        );
        assert_eq!(
            accumulator.apply(completed_reasoning("X", Reasoning::new("A"))),
            update(None, "Thinking: Az")
        );
    }

    #[test]
    fn reasoning_is_keyed_by_the_rig_correlator_not_provider_ids() {
        let mut accumulator = StreamAccumulator::default();

        accumulator.apply(reasoning_delta("X", None, "a"));
        accumulator.apply(reasoning_delta("Y", None, "b"));
        assert_eq!(
            accumulator.apply(reasoning_delta("Z", Some("rs_1"), "c")),
            update(None, "Thinking: abc")
        );
        assert_eq!(
            accumulator.apply(completed_reasoning(
                "X",
                Reasoning::new("A").with_id("rs_1".to_owned())
            )),
            update(None, "Thinking: Abc")
        );
    }

    #[test]
    fn completed_reasoning_without_displayable_text_is_ignored_for_a_new_part() {
        let mut accumulator = StreamAccumulator::default();

        assert_eq!(
            accumulator.apply(completed_reasoning("X", Reasoning::encrypted("opaque"))),
            None
        );
        assert_eq!(
            accumulator.apply(completed_reasoning("Y", Reasoning::redacted("opaque"))),
            None
        );
        assert_eq!(
            accumulator.apply(reasoning_delta("Z", None, "a")),
            update(Some(0.6), "Thinking: a")
        );
    }

    #[test]
    fn completed_reasoning_without_displayable_text_keeps_existing_thinking() {
        let mut accumulator = StreamAccumulator::default();
        accumulator.apply(reasoning_delta("X", None, "a"));

        assert_eq!(
            accumulator.apply(completed_reasoning("X", Reasoning::encrypted("opaque"))),
            None
        );
        assert_eq!(
            accumulator.apply(completed_reasoning("X", Reasoning::redacted("opaque"))),
            None
        );
        assert_eq!(
            accumulator.apply(reasoning_delta("Y", None, "b")),
            update(None, "Thinking: ab")
        );
    }

    #[test]
    fn final_text_is_an_error_when_only_reasoning_arrives() {
        let mut accumulator = StreamAccumulator::default();
        accumulator.apply(reasoning_delta("X", None, "a"));
        accumulator.apply(completed_reasoning("X", Reasoning::new("a")));
        accumulator.apply(final_item());

        assert_eq!(
            accumulator.into_final_text(),
            Err("transformation completed without returning any text".to_owned())
        );
    }

    #[test]
    fn final_text_is_an_error_when_text_is_only_whitespace() {
        let mut accumulator = StreamAccumulator::default();
        accumulator.apply(StreamedAssistantContent::text(" \n"));

        assert_eq!(
            accumulator.into_final_text(),
            Err("transformation completed without returning any text".to_owned())
        );
    }

    #[test]
    fn final_text_is_an_error_for_an_empty_stream() {
        let accumulator = StreamAccumulator::default();

        assert_eq!(
            accumulator.into_final_text(),
            Err("transformation completed without returning any text".to_owned())
        );
    }

    fn raw_stream(
        items: Vec<Result<RawStreamingChoice, CompletionError>>,
    ) -> StreamingCompletionResponse {
        StreamingCompletionResponse::stream("test-provider", Box::pin(tokio_stream::iter(items)))
    }

    #[tokio::test]
    async fn finishing_preview_preserves_narration_through_reasoning_and_answer() {
        let state = AppState::new();
        state.set_overlay_text("The narrated text.");
        let original = state.overlay_text_snapshot().text;
        let stream = raw_stream(vec![
            Ok(RawStreamingChoice::Reasoning {
                id: rig_core::streaming::StreamPartId::wire("reasoning"),
                provider_id: None,
                content: ReasoningContent::Text { text: "Working on it".to_owned(), signature: None },
            }),
            Ok(RawStreamingChoice::Message("Transformed ".to_owned())),
            Ok(RawStreamingChoice::Message("result".to_owned())),
        ]);

        let result = drain_stream(stream, &state, TransformationPreviewMode::PreserveOverlay).await;

        assert_eq!(result, Ok("Transformed result".to_owned()));
        assert!(Arc::ptr_eq(&original, &state.overlay_text_snapshot().text));
        assert_eq!(state.overlay_text_opacity(), 1.0);
    }

    #[tokio::test]
    async fn drain_stream_collects_text_that_arrives_after_final() {
        let state = AppState::new();
        state.set_overlay_text_opacity(0.2);
        let stream = raw_stream(vec![
            Ok(RawStreamingChoice::Message("Hel".to_owned())),
            Ok(RawStreamingChoice::FinalResponse(StreamFinal::new(
                "test-provider",
                Usage::new(),
            ))),
            Ok(RawStreamingChoice::Message("lo".to_owned())),
        ]);

        let result = drain_stream(stream, &state, TransformationPreviewMode::ReplaceOverlay).await;

        assert_eq!(result, Ok("Hello".to_owned()));
        assert_eq!(&*state.overlay_text(), "Hello");
        assert_eq!(state.overlay_text_opacity(), 1.0);
    }

    #[tokio::test]
    async fn drain_stream_reports_stream_errors() {
        let state = AppState::new();
        let stream = raw_stream(vec![
            Ok(RawStreamingChoice::Message("partial".to_owned())),
            Err(CompletionError::ProviderError("boom".to_owned())),
        ]);

        let result = drain_stream(stream, &state, TransformationPreviewMode::ReplaceOverlay).await;

        assert_eq!(
            result,
            Err("transformation stream failed: ProviderError: boom".to_owned())
        );
    }

    #[tokio::test]
    async fn drain_stream_stops_when_abort_is_requested() {
        let state = AppState::new();
        state.request_abort();
        let stream = raw_stream(vec![Ok(RawStreamingChoice::Message("text".to_owned()))]);

        let result = drain_stream(stream, &state, TransformationPreviewMode::ReplaceOverlay).await;

        assert_eq!(result, Err("transformation aborted".to_owned()));
        assert_eq!(&*state.overlay_text(), "");
    }

    #[tokio::test]
    async fn drain_stream_without_text_is_an_error() {
        let state = AppState::new();
        let stream = raw_stream(vec![Ok(RawStreamingChoice::FinalResponse(
            StreamFinal::new("test-provider", Usage::new()),
        ))]);

        let result = drain_stream(stream, &state, TransformationPreviewMode::ReplaceOverlay).await;

        assert_eq!(
            result,
            Err("transformation completed without returning any text".to_owned())
        );
    }

    #[tokio::test]
    async fn every_supported_provider_is_dispatched() {
        let state = AppState::new();
        // `ollama` takes no API key: its client reads the environment and
        // would open a real connection to the local server.
        for provider in crate::config::supported_transformation_providers()
            .iter()
            .filter(|provider| **provider != "ollama")
        {
            let config = TransformationRuntimeConfig {
                provider: (*provider).to_owned(),
                api_key: None,
                model: "test-model".to_owned(),
                system_prompt: String::new(),
                correction_system_prompt: String::new(),
            };

            let result = transform_text(
                Arc::clone(&state),
                &config,
                "input",
                TransformationPreviewMode::ReplaceOverlay,
            )
            .await;

            assert_eq!(
                result,
                Err(format!(
                    "transformation.api_key is required for provider '{}'",
                    provider
                )),
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires GEMINI_API_KEY and calls the live Gemini API"]
    async fn gemini_flash_lite_streams_with_model_defaults() {
        let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
        let state = AppState::new();
        let config = TransformationRuntimeConfig {
            provider: "gemini".to_owned(),
            api_key: Some(api_key),
            model: "gemini-3.5-flash-lite".to_owned(),
            system_prompt: "Reply briefly.".to_owned(),
            correction_system_prompt: String::new(),
        };

        let result = transform_text(
            Arc::clone(&state),
            &config,
            "Reply with OK.",
            TransformationPreviewMode::ReplaceOverlay,
        )
        .await
        .expect("Gemini Flash-Lite must accept the transformation request");

        assert!(!result.trim().is_empty());
        assert_eq!(state.overlay_text().trim(), result);
        assert_eq!(state.overlay_text_opacity(), 1.0);
    }

    #[tokio::test]
    async fn test_gemini_streaming_items() {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .unwrap_or_default();
        if api_key.is_empty() {
            println!("Skipping test because GEMINI_API_KEY/GOOGLE_API_KEY is not set.");
            return;
        }

        let state = AppState::new();
        state.set_overlay_text_opacity(0.2);
        let config = TransformationRuntimeConfig {
            provider: "gemini".to_owned(),
            api_key: Some(api_key),
            model: "gemini-2.5-flash".to_owned(),
            system_prompt: "You are a helpful assistant.".to_owned(),
            correction_system_prompt: String::new(),
        };

        let text = transform_text(
            Arc::clone(&state),
            &config,
            "Write a 300 word essay about Apple.",
            TransformationPreviewMode::ReplaceOverlay,
        )
        .await
        .expect("gemini transformation failed");
        println!("Final text: {}", text);

        assert!(!text.is_empty());
        assert_eq!(state.overlay_text().trim(), text);
        assert_eq!(state.overlay_text_opacity(), 1.0);
    }

    #[test]
    fn overlay_updates_reset_the_preview_only_when_a_phase_starts() {
        let state = AppState::new();
        state.set_overlay_text_opacity(0.2);
        let assert_opacity = |expected: f64| {
            let actual = state.overlay_text_opacity();
            assert!(
                (actual - expected).abs() <= 1.0 / 255.0,
                "opacity {} != {}",
                actual,
                expected
            );
        };

        apply_overlay_update(
            &state,
            TransformationPreviewMode::ReplaceOverlay,
            OverlayUpdate {
                reset_with_opacity: Some(THINKING_TEXT_OPACITY),
                text: "Thinking: a".to_owned(),
            },
        );
        assert_eq!(&*state.overlay_text(), "Thinking: a");
        assert_opacity(0.6);

        state.set_overlay_text_opacity(0.2);
        apply_overlay_update(
            &state,
            TransformationPreviewMode::ReplaceOverlay,
            OverlayUpdate {
                reset_with_opacity: None,
                text: "Thinking: ab".to_owned(),
            },
        );
        assert_eq!(&*state.overlay_text(), "Thinking: ab");
        assert_opacity(0.2);

        apply_overlay_update(
            &state,
            TransformationPreviewMode::ReplaceOverlay,
            OverlayUpdate {
                reset_with_opacity: Some(ANSWER_TEXT_OPACITY),
                text: "answer".to_owned(),
            },
        );
        assert_eq!(&*state.overlay_text(), "answer");
        assert_opacity(1.0);
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

                let hosted_providers: Vec<&str> = crate::config::supported_transformation_providers()
                    .iter()
                    .copied()
                    .filter(|provider| *provider != "ollama")
                    .collect();
                assert_eq!(vec![$(stringify!($provider)),+], hosted_providers);
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
