pub(crate) fn default_transformation_system_prompt() -> String {
    concat!(
        "You are editing raw speech-to-text output that was dictated quickly as instructions for ",
        "an LLM agent. Edit the input into written instructions while preserving the speaker's ",
        "wording, meaning, and intent.\n\n",
        "Preserve all details, context, emphasis, qualifications, examples, and side notes. ",
        "Do not summarize, shorten, or omit content because it seems redundant, incidental, ",
        "or unimportant. Keep repetitions that express emphasis and words that express ",
        "uncertainty, urgency, or degree.\n\n",
        "Fix punctuation, capitalization, and obvious transcription mistakes. Remove only ",
        "non-content hesitations and accidental stutters. Resolve false starts, self-repairs, ",
        "and retractions only when the speaker explicitly replaces or withdraws earlier wording; ",
        "keep the final intended wording and all surrounding context. If it is unclear whether ",
        "something is intended content, keep it.\n\n",
        "Preserve technical jargon, product ",
        "names, API names, CLI flags, file paths, environment variable names, and programmer ",
        "vocabulary when clearly intended.\n\n",
        "If the speaker is clearly dictating structure such as ",
        "bullet points, numbered lists, headings, or short action items, format the output ",
        "accordingly.\n\n",
        "When the speaker is clearly dictating symbols or meta words in a technical ",
        "context, convert them to the intended characters, for example dash to -, underscore to ",
        "_, slash to /, backslash to \\, colon to :, dot to ., open paren to (, close paren to ",
        "), open bracket to [, close bracket to ], open brace to {, and close brace to }.\n\n",
        "Do not add new facts, commentary, or formatting beyond what is implied by the input. ",
        "Return only the transformed text."
    )
    .into()
}

pub(crate) fn default_transformation_correction_system_prompt() -> String {
    concat!(
        "You are editing an existing annotation using a spoken correction request. The user will ",
        "provide input with two labeled sections: CURRENT ANNOTATION and CORRECTION REQUEST.\n\n",
        "Rewrite the current annotation by applying the correction request exactly as intended. ",
        "Preserve all content that the correction request does not change. Make the smallest ",
        "coherent edits that satisfy the request.\n\n",
        "If the request asks for ",
        "rewording, insertion, deletion, restructuring, emphasis, or formatting changes, apply ",
        "those changes to the annotation itself rather than commenting on them.\n\n",
        "Preserve ",
        "technical jargon, product names, API names, CLI flags, file paths, environment variable ",
        "names, and programmer vocabulary when clearly intended.\n\n",
        "Do not add explanations, ",
        "analysis, surrounding quotes, or commentary. Return only the fully rewritten ",
        "annotation."
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::{
        default_transformation_correction_system_prompt, default_transformation_system_prompt,
    };

    #[test]
    fn default_transformation_system_prompt_uses_single_braces() {
        let prompt = default_transformation_system_prompt();

        assert!(prompt.contains("open brace to {,"));
        assert!(prompt.contains("close brace to }."));
        assert!(!prompt.contains("open brace to {{"));
        assert!(!prompt.contains("close brace to }}"));
    }

    #[test]
    fn default_transformation_prompt_limits_cleanup_and_preserves_context() {
        let prompt = default_transformation_system_prompt();

        assert!(prompt.contains("self-repairs"));
        assert!(prompt.contains("retractions"));
        assert!(prompt.contains("final intended wording"));
        assert!(prompt.contains("Do not summarize, shorten, or omit content"));
        assert!(
            prompt.contains("details, context, emphasis, qualifications, examples, and side notes")
        );
        assert!(prompt.contains("If it is unclear whether something is intended content, keep it"));
    }

    #[test]
    fn default_correction_transformation_prompt_mentions_correction_sections() {
        let prompt = default_transformation_correction_system_prompt();

        assert!(prompt.contains("CURRENT ANNOTATION"));
        assert!(prompt.contains("CORRECTION REQUEST"));
        assert!(prompt.contains("Return only the fully rewritten annotation"));
    }
}
