//! Merge keyboard edits with revisions to the same live transcription.
//! Unicode word differences preserve manual replacements; user edits take precedence
//! where both writers change the same source range.

use std::ops::Range;

use similar::{ChangeTag, TextDiff};

#[derive(Debug)]
struct Edit {
    range: Range<usize>,
    replacement: String,
}

/// A speech revision can replace a word and add more narration in one
/// hunk. Keep the extra tokens separate so an edited word does not also
/// suppress the words spoken after it.
fn push_edit(result: &mut Vec<Edit>, before: &str, mut edit: Edit) {
    if !edit.range.is_empty() && !edit.replacement.is_empty() {
        let removed = TextDiff::from_unicode_words("", &before[edit.range.clone()]).new_len();
        let inserted = TextDiff::from_unicode_words("", edit.replacement.as_str());
        if inserted.new_len() > removed {
            let split: usize = inserted
                .iter_all_changes()
                .take(removed)
                .map(|change| change.value().len())
                .sum();
            let tail = edit.replacement[split..].to_owned();
            edit.replacement.truncate(split);
            let end = edit.range.end;
            result.push(edit);
            result.push(Edit {
                range: end..end,
                replacement: tail,
            });
            return;
        }
    }
    result.push(edit);
}

/// Keep words atomic so conflicting revisions cannot splice pieces of two
/// different words together. Punctuation remains independently editable.
fn edits(before: &str, after: &str) -> Vec<Edit> {
    let mut result = Vec::new();
    let mut pending: Option<Edit> = None;
    let mut byte = 0;
    for change in TextDiff::from_unicode_words(before, after).iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                if let Some(edit) = pending.take() {
                    push_edit(&mut result, before, edit);
                }
                byte += change.value().len();
            }
            ChangeTag::Delete => {
                let edit = pending.get_or_insert_with(|| Edit {
                    range: byte..byte,
                    replacement: String::new(),
                });
                byte += change.value().len();
                edit.range.end = byte;
            }
            ChangeTag::Insert => {
                pending
                    .get_or_insert_with(|| Edit {
                        range: byte..byte,
                        replacement: String::new(),
                    })
                    .replacement
                    .push_str(change.value());
            }
        }
    }
    if let Some(edit) = pending {
        push_edit(&mut result, before, edit);
    }
    result
}

fn conflict(left: &Edit, right: &Edit) -> bool {
    if left.range.is_empty() && right.range.is_empty() {
        false
    } else if left.range.is_empty() {
        right.range.start < left.range.start && left.range.start < right.range.end
    } else if right.range.is_empty() {
        left.range.start < right.range.start && right.range.start < left.range.end
    } else {
        left.range.start < right.range.end && right.range.start < left.range.end
    }
}

pub fn merge_text(before: &str, edited: &str, incoming: &str) -> String {
    if edited == before || edited == incoming {
        return incoming.to_owned();
    }
    if incoming == before {
        return edited.to_owned();
    }
    let user_edits = edits(before, edited);
    let mut incoming_edits = edits(before, incoming);
    incoming_edits.retain(|incoming| !user_edits.iter().any(|user| conflict(user, incoming)));
    let mut combined = user_edits;
    combined.extend(incoming_edits);
    combined.sort_by_key(|edit| (edit.range.start, !edit.range.is_empty()));
    let mut text = String::new();
    let mut cursor = 0;
    for edit in combined {
        text.push_str(&before[cursor..edit.range.start]);
        text.push_str(&edit.replacement);
        cursor = edit.range.end;
    }
    text.push_str(&before[cursor..]);
    text
}

/// Moves a byte boundary with keyboard changes, without splitting Unicode.
pub fn edited_offset(before: &str, edited: &str, byte_offset: usize) -> usize {
    if before == edited {
        return byte_offset;
    }
    let offset = byte_offset;
    let mut mapped = offset;
    for edit in edits(before, edited) {
        if edit.range.start > offset {
            break;
        }
        mapped = mapped - edit.range.len().min(offset - edit.range.start) + edit.replacement.len();
    }
    mapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_deleting_do_not_replay_narration() {
        assert_eq!(
            merge_text(
                "hello still coming in ",
                "Hi still coming in ",
                "hello still coming in and working "
            ),
            "Hi still coming in and working "
        );
        assert_eq!(
            merge_text("meet Thursday ", "meet Friday ", "meet Thursday afternoon "),
            "meet Friday afternoon "
        );
        assert_eq!(
            merge_text("hello world ", "hello ", "hello world again "),
            "hello again "
        );
        assert_eq!(merge_text("hello ", "", "hello world "), "world ");
        assert_eq!(
            merge_text("hello ", "hello typed ", "hello spoken "),
            "hello typed spoken "
        );
        assert_eq!(
            merge_text(
                "hello still coming in ",
                "",
                "hello still coming in and working "
            ),
            "and working "
        );
        assert_eq!(
            merge_text("hello world ", "dear hello world ", "Hello world "),
            "dear Hello world "
        );
        assert_eq!(
            merge_text("hello world ", "Hi world ", "dear hello world "),
            "dear Hi world "
        );
    }

    #[test]
    fn revisions_preserve_manual_replacements_and_accept_unrelated_punctuation() {
        assert_eq!(
            merge_text(
                "meet Thursday afternoon ",
                "meet Friday afternoon ",
                "Meet Thursday afternoon. "
            ),
            "Meet Friday afternoon. "
        );
        assert_eq!(
            merge_text("meet Thursday ", "meet Friday ", "meet Tuesday afternoon "),
            "meet Friday afternoon "
        );
    }

    #[test]
    fn unicode_edits_move_the_provisional_boundary() {
        let before = "hello still talking ";
        let edited = "Hé🙂 still talking ";
        assert_eq!(
            merge_text(before, edited, "hello still talking today "),
            "Hé🙂 still talking today "
        );
        assert_eq!(edited_offset(before, edited, "hello ".len()), "Hé🙂 ".len());
    }
}
