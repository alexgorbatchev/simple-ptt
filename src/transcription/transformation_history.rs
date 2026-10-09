use std::future::Future;

use crate::state::AppState;

use super::{finish_transformation, settle_session_finish};

/// Successful manual output for the current annotation. The worker owns it,
/// so keyboard edits and final speech are compared only after capture settles.
#[derive(Default)]
pub(super) struct TransformationHistory {
    manual_output: Option<String>,
}

impl TransformationHistory {
    pub(super) fn begin_dictation(&mut self, recording_prefix: &str) {
        if recording_prefix.trim().is_empty() {
            self.manual_output = None;
        }
    }

    pub(super) async fn finish_manual(
        &mut self,
        state: &AppState,
        transformation: impl Future<Output = Result<String, String>>,
    ) -> Result<Option<String>, String> {
        let result = finish_transformation(state, transformation).await;
        self.manual_output = match &result {
            Ok(Some(text)) => Some(text.trim().to_owned()),
            _ => None,
        };
        result
    }

    pub(super) async fn finish_automatic(
        &mut self,
        state: &AppState,
        input_text: &str,
        transformation: impl Future<Output = Result<String, String>>,
    ) -> Result<Option<String>, String> {
        // Live transcription trims segment edges and adds a trailing separator.
        // Those separators do not make an otherwise unchanged result dirty.
        let result = if self.manual_output.as_deref() == Some(input_text.trim()) {
            log::info!("reusing unchanged manual transformation for paste");
            settle_session_finish(Ok(input_text.to_owned()), state)
        } else {
            finish_transformation(state, transformation).await
        };
        if matches!(result, Ok(None)) {
            self.manual_output = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::TransformationHistory;
    use crate::state::{AppState, STATE_IDLE, STATE_PROCESSING};
    use crate::transcription::join_transcript_parts;

    #[test]
    fn speech_and_keyboard_edits_after_a_manual_transform_require_another_request() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for (edited, speech) in [
                ("Corrected transcript. More speech. ", true),
                ("Keyboard edit.", false),
                ("corrected transcript.", false),
            ] {
                let state = AppState::new();
                let mut history = TransformationHistory::default();
                let original = history
                    .finish_manual(&state, async { Ok("Corrected transcript.".to_owned()) })
                    .await
                    .unwrap()
                    .unwrap();
                history.begin_dictation(&original);
                state.set_overlay_text(join_transcript_parts(&original, &[]));
                let rendered = state.overlay_text();
                if speech {
                    state.merge_live_overlay_text(&rendered, edited.to_owned(), None);
                } else {
                    state.apply_overlay_edit(&rendered, edited);
                }
                let final_text = state.overlay_text();
                let requests = Cell::new(0);
                let result = history
                    .finish_automatic(&state, &final_text, async {
                        requests.set(requests.get() + 1);
                        Ok("New transformation.".to_owned())
                    })
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(requests.get(), 1, "{edited}");
                assert_eq!(result, "New transformation.");
            }
        });
    }

    #[test]
    fn an_explicit_manual_transform_runs_again_even_when_text_is_unchanged() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let state = AppState::new();
            let mut history = TransformationHistory::default();
            let requests = Cell::new(0);
            for _ in 0..2 {
                assert_eq!(
                    history
                        .finish_manual(&state, async {
                            requests.set(requests.get() + 1);
                            Ok("Same output.".to_owned())
                        })
                        .await
                        .unwrap()
                        .unwrap(),
                    "Same output."
                );
            }
            let final_text = join_transcript_parts("Same output.", &[]);
            history
                .finish_automatic(&state, &final_text, async {
                    requests.set(requests.get() + 1);
                    Ok("Unwanted transformation.".to_owned())
                })
                .await
                .unwrap();
            assert_eq!(requests.get(), 2);
        });
    }

    #[test]
    fn a_fresh_dictation_does_not_reuse_a_previous_annotations_transformation() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let state = AppState::new();
            let mut history = TransformationHistory::default();
            let text = history
                .finish_manual(&state, async { Ok("Same words.".to_owned()) })
                .await
                .unwrap()
                .unwrap();
            history.begin_dictation(&text);
            let requests = Cell::new(0);
            history
                .finish_automatic(&state, &text, async {
                    requests.set(requests.get() + 1);
                    Ok(text.clone())
                })
                .await
                .unwrap();
            assert_eq!(requests.get(), 0, "resuming retains the manual result");

            history.begin_dictation("");
            let result = history
                .finish_automatic(&state, &text, async {
                    requests.set(requests.get() + 1);
                    Ok("Fresh transformation.".to_owned())
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(requests.get(), 1);
            assert_eq!(result, "Fresh transformation.");
        });
    }

    #[test]
    fn failed_manual_transformation_cannot_suppress_the_next_automatic_request() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let state = AppState::new();
            let mut history = TransformationHistory::default();
            let text = history
                .finish_manual(&state, async { Ok("Previous result.".to_owned()) })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                history
                    .finish_manual(&state, async {
                        state.set_overlay_text("Partial preview.");
                        Err("Connection lost".to_owned())
                    })
                    .await
                    .unwrap_err(),
                "Connection lost"
            );
            let requests = Cell::new(0);
            history
                .finish_automatic(&state, &text, async {
                    requests.set(requests.get() + 1);
                    Ok(text.clone())
                })
                .await
                .unwrap();
            assert_eq!(requests.get(), 1);
        });
    }

    #[test]
    fn cancellation_wins_a_reused_result_and_clears_its_history() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let state = AppState::new();
            let mut history = TransformationHistory::default();
            let text = history
                .finish_manual(&state, async { Ok("Do not paste this.".to_owned()) })
                .await
                .unwrap()
                .unwrap();
            state.set_overlay_text(text.clone());
            state.set_state(STATE_PROCESSING);
            state.request_abort();
            let requests = Cell::new(0);
            assert_eq!(
                history
                    .finish_automatic(&state, &text, async {
                        requests.set(requests.get() + 1);
                        Ok(text.clone())
                    })
                    .await
                    .unwrap(),
                None
            );
            assert_eq!(requests.get(), 0);
            assert_eq!(state.get_state(), STATE_IDLE);
            assert!(state.overlay_text().is_empty());
            assert!(!state.is_abort_requested());

            history
                .finish_automatic(&state, &text, async {
                    requests.set(requests.get() + 1);
                    Ok(text.clone())
                })
                .await
                .unwrap();
            assert_eq!(requests.get(), 1);
        });
    }
}
