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

pub fn build_overlay_text(
    recording_prefix: &str,
    transcript_parts: &[String],
    interim_transcript: Option<&str>,
) -> String {
    let mut overlay_text = String::new();
    append_text_segment(&mut overlay_text, recording_prefix);

    for transcript in transcript_parts {
        append_text_segment(&mut overlay_text, transcript.as_str());
    }

    if let Some(interim_transcript) = interim_transcript {
        append_text_segment(&mut overlay_text, interim_transcript);
    }

    if !overlay_text.is_empty() && !overlay_text.ends_with(|c: char| c.is_whitespace()) {
        overlay_text.push(' ');
    }

    overlay_text
}

pub fn join_transcript_parts(recording_prefix: &str, transcript_parts: &[String]) -> String {
    build_overlay_text(recording_prefix, transcript_parts, None)
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
            build_overlay_text(prefix, &parts, Some("interim")),
            "Prefix text final segment interim "
        );
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
