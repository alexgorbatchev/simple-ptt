#![allow(dead_code)]

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlinePreviewRender {
    pub text: String,
    pub underlined_byte_ranges: Vec<std::ops::Range<usize>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenSpan {
    pub byte_range: std::ops::Range<usize>,
}

pub fn build_inline_correction_preview(
    original_text: &str,
    preview_text: &str,
) -> Option<InlinePreviewRender> {
    let original_tokens = non_whitespace_tokens(original_text);
    let preview_tokens = non_whitespace_tokens(preview_text);

    if original_tokens.is_empty() && preview_tokens.is_empty() {
        return None;
    }

    let shared_prefix_len = shared_prefix_len(
        original_text,
        &original_tokens,
        preview_text,
        &preview_tokens,
    );
    let shared_suffix_len = shared_suffix_len(
        original_text,
        &original_tokens,
        preview_text,
        &preview_tokens,
        shared_prefix_len,
    );
    let original_has_divergence = shared_prefix_len + shared_suffix_len < original_tokens.len();
    let preview_has_divergence = shared_prefix_len + shared_suffix_len < preview_tokens.len();

    let prefix_end = original_tokens
        .get(shared_prefix_len)
        .map(|token| token.byte_range.start)
        .unwrap_or(original_text.len());
    let mut rendered_text = original_text[..prefix_end].to_owned();
    let mut underlined_byte_ranges = Vec::new();

    if preview_has_divergence {
        let preview_segment_start =
            if shared_prefix_len < original_tokens.len() || shared_prefix_len == 0 {
                preview_tokens[shared_prefix_len].byte_range.start
            } else {
                preview_tokens[shared_prefix_len - 1].byte_range.end
            };
        let preview_segment_end = if shared_suffix_len > 0 {
            preview_tokens[preview_tokens.len() - shared_suffix_len]
                .byte_range
                .start
        } else {
            preview_text.len()
        };
        let preview_insert_offset = rendered_text.len();
        rendered_text.push_str(&preview_text[preview_segment_start..preview_segment_end]);

        let preview_changed_tokens =
            &preview_tokens[shared_prefix_len..(preview_tokens.len() - shared_suffix_len)];
        for token in preview_changed_tokens {
            underlined_byte_ranges.push(
                (preview_insert_offset + (token.byte_range.start - preview_segment_start))
                    ..(preview_insert_offset + (token.byte_range.end - preview_segment_start)),
            );
        }
    }

    if shared_suffix_len > 0 {
        let suffix_start = original_tokens[original_tokens.len() - shared_suffix_len]
            .byte_range
            .start;
        rendered_text.push_str(&original_text[suffix_start..]);
    } else if original_has_divergence {
        let preserved_tail_start = original_tokens[shared_prefix_len].byte_range.end;
        rendered_text.push_str(&original_text[preserved_tail_start..]);
    }

    Some(InlinePreviewRender {
        text: rendered_text,
        underlined_byte_ranges,
    })
}

pub fn non_whitespace_tokens(text: &str) -> Vec<TokenSpan> {
    let mut tokens = Vec::new();
    let mut current_start = None;

    for (index, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = current_start.take() {
                tokens.push(TokenSpan {
                    byte_range: start..index,
                });
            }
        } else if current_start.is_none() {
            current_start = Some(index);
        }
    }

    if let Some(start) = current_start {
        tokens.push(TokenSpan {
            byte_range: start..text.len(),
        });
    }

    tokens
}

pub fn shared_prefix_len(
    original_text: &str,
    original_tokens: &[TokenSpan],
    preview_text: &str,
    preview_tokens: &[TokenSpan],
) -> usize {
    original_tokens
        .iter()
        .zip(preview_tokens)
        .take_while(|(original, preview)| {
            original_text[original.byte_range.clone()] == preview_text[preview.byte_range.clone()]
        })
        .count()
}

pub fn shared_suffix_len(
    original_text: &str,
    original_tokens: &[TokenSpan],
    preview_text: &str,
    preview_tokens: &[TokenSpan],
    shared_prefix_len: usize,
) -> usize {
    let max_original_suffix = original_tokens.len().saturating_sub(shared_prefix_len);
    let max_preview_suffix = preview_tokens.len().saturating_sub(shared_prefix_len);
    let max_suffix_len = max_original_suffix.min(max_preview_suffix);
    let mut shared_suffix_len = 0;

    while shared_suffix_len < max_suffix_len {
        let original = &original_tokens[original_tokens.len() - shared_suffix_len - 1];
        let preview = &preview_tokens[preview_tokens.len() - shared_suffix_len - 1];
        if original_text[original.byte_range.clone()] != preview_text[preview.byte_range.clone()] {
            break;
        }
        shared_suffix_len += 1;
    }

    shared_suffix_len
}

pub fn utf16_offset(text: &str, byte_offset: usize) -> usize {
    text[..byte_offset].encode_utf16().count()
}

pub fn working_text_update_is_semantically_unchanged(current_text: &str, next_text: &str) -> bool {
    current_text == next_text || current_text.trim_end() == next_text.trim_end()
}

#[cfg(test)]
mod tests {
    use super::{build_inline_correction_preview, working_text_update_is_semantically_unchanged};

    #[test]
    fn renders_inline_replacement_with_suffix_anchor() {
        let preview =
            build_inline_correction_preview("the quick brown fox jumps", "the quick red fox jumps")
                .expect("preview should render");

        assert_eq!(preview.text, "the quick red fox jumps");
        assert_eq!(
            underlined_segments(&preview.text, &preview.underlined_byte_ranges),
            vec!["red"]
        );
    }

    #[test]
    fn preserves_tail_while_stream_has_not_reanchored() {
        let preview = build_inline_correction_preview("the quick brown fox jumps", "the quick red")
            .expect("preview should render");

        assert_eq!(preview.text, "the quick red fox jumps");
        assert_eq!(
            underlined_segments(&preview.text, &preview.underlined_byte_ranges),
            vec!["red"]
        );
    }

    #[test]
    fn preserves_spacing_for_insertions_at_end() {
        let preview =
            build_inline_correction_preview("hello", "hello world").expect("preview should render");

        assert_eq!(preview.text, "hello world");
        assert_eq!(
            underlined_segments(&preview.text, &preview.underlined_byte_ranges),
            vec!["world"]
        );
    }

    #[test]
    fn handles_pure_deletions() {
        let preview = build_inline_correction_preview("hello old world", "hello world")
            .expect("preview should render");

        assert_eq!(preview.text, "hello world");
        assert!(preview.underlined_byte_ranges.is_empty());
    }

    #[test]
    fn treats_trailing_whitespace_only_updates_as_unchanged() {
        assert!(working_text_update_is_semantically_unchanged(
            "hello world",
            "hello world "
        ));
        assert!(working_text_update_is_semantically_unchanged(
            "hello world  ",
            "hello world"
        ));
        assert!(!working_text_update_is_semantically_unchanged(
            "hello world",
            "hello there"
        ));
    }

    fn underlined_segments<'a>(text: &'a str, ranges: &[std::ops::Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|range| &text[range.clone()]).collect()
    }
}
