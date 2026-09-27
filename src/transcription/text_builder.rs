use crate::transformation::TransformationRuntimeConfig;

pub fn transformation_correction_runtime_config(
    config: &TransformationRuntimeConfig,
) -> TransformationRuntimeConfig {
    let mut config = config.clone();
    config.system_prompt = config.correction_system_prompt.clone();
    config
}

pub fn build_correction_transform_input(working_text: &str, correction_text: &str) -> String {
    format!(
        "CURRENT ANNOTATION:\n{}\n\nCORRECTION REQUEST:\n{}",
        working_text.trim(),
        correction_text.trim()
    )
}

pub fn append_text_segment(output: &mut String, segment: &str) {
    let trimmed_segment = segment.trim();
    if trimmed_segment.is_empty() {
        return;
    }

    if !output.is_empty() {
        output.push(' ');
    }
    output.push_str(trimmed_segment);
}

/// Overlay text during a live transcription session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveOverlayText {
    pub text: String,
    /// Byte offset in `text` where the interim transcript begins. Deepgram may
    /// still revise everything from here to the end; `None` when all of `text`
    /// is final.
    pub provisional_start: Option<usize>,
}

pub fn build_overlay_text(
    recording_prefix: &str,
    transcript_parts: &[String],
    interim_transcript: Option<&str>,
) -> LiveOverlayText {
    let mut overlay_text = String::new();
    append_text_segment(&mut overlay_text, recording_prefix);

    for transcript in transcript_parts {
        append_text_segment(&mut overlay_text, transcript.as_str());
    }

    let final_text_len = overlay_text.len();
    if let Some(interim_transcript) = interim_transcript {
        append_text_segment(&mut overlay_text, interim_transcript);
    }
    let provisional_start = (overlay_text.len() > final_text_len)
        .then(|| overlay_text.len() - interim_transcript.map_or(0, |text| text.trim().len()));

    if !overlay_text.is_empty() && !overlay_text.ends_with(|c: char| c.is_whitespace()) {
        overlay_text.push(' ');
    }

    LiveOverlayText {
        text: overlay_text,
        provisional_start,
    }
}

pub fn join_transcript_parts(recording_prefix: &str, transcript_parts: &[String]) -> String {
    build_overlay_text(recording_prefix, transcript_parts, None).text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_text_segment_joins_non_empty_segments_with_single_spaces() {
        let mut buffer = String::new();
        append_text_segment(&mut buffer, "hello");
        append_text_segment(&mut buffer, "   world  ");
        append_text_segment(&mut buffer, "");

        assert_eq!(buffer, "hello world");
    }

    #[test]
    fn build_overlay_text_preserves_recording_prefix_before_live_interim_text() {
        let prefix = "Prefix text";
        let parts = vec!["final segment".to_owned()];

        assert_eq!(
            build_overlay_text(prefix, &parts, Some("interim")).text,
            "Prefix text final segment interim "
        );
    }

    #[test]
    fn build_overlay_text_marks_where_the_interim_transcript_begins() {
        let parts = vec!["final segment".to_owned()];
        let live_text = build_overlay_text("Prefix text", &parts, Some("  still talking "));

        assert_eq!(live_text.text, "Prefix text final segment still talking ");
        let provisional_start = live_text
            .provisional_start
            .expect("interim text should be marked provisional");
        assert_eq!(&live_text.text[..provisional_start], "Prefix text final segment ");
        assert_eq!(&live_text.text[provisional_start..], "still talking ");
    }

    #[test]
    fn build_overlay_text_marks_interim_text_that_is_the_only_text() {
        let live_text = build_overlay_text("", &[], Some("héllo"));

        assert_eq!(live_text.text, "héllo ");
        assert_eq!(live_text.provisional_start, Some(0));
    }

    #[test]
    fn build_overlay_text_has_no_provisional_text_without_an_interim_transcript() {
        let parts = vec!["final segment".to_owned()];

        assert_eq!(build_overlay_text("Prefix", &parts, None).provisional_start, None);
        assert_eq!(build_overlay_text("Prefix", &parts, Some("   ")).provisional_start, None);
    }

    #[test]
    fn build_correction_transform_input_wraps_annotation_and_correction_request() {
        assert_eq!(
            build_correction_transform_input("original text", "fix spelling"),
            "CURRENT ANNOTATION:\noriginal text\n\nCORRECTION REQUEST:\nfix spelling"
        );
    }

    #[test]
    fn correction_runtime_config_swaps_correction_prompt() {
        let base_config = TransformationRuntimeConfig {
            provider: "openai".to_owned(),
            api_key: Some("key".to_owned()),
            model: "gpt-4o-mini".to_owned(),
            system_prompt: "dictation prompt".to_owned(),
            correction_system_prompt: "correction prompt".to_owned(),
        };

        let correction_config = transformation_correction_runtime_config(&base_config);
        assert_eq!(correction_config.system_prompt, "correction prompt");
    }
}
