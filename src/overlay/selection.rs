//! Preserve the editor's selection while narration replaces its text.

use std::time::Duration;

use objc2_foundation::NSRange;

pub(super) struct SelectionUpdate {
    pub range: NSRange,
    pub follow_end: bool,
}

pub(super) fn selection_after_update(
    previous: NSRange,
    previous_length: usize,
    text: &str,
    since_keyboard_edit: Option<Duration>,
) -> SelectionUpdate {
    let idle = match since_keyboard_edit {
        Some(elapsed) => elapsed >= Duration::from_secs(2),
        None => true,
    };
    let follow_end = previous.length == 0 && previous.location == previous_length && idle;
    let range = if follow_end {
        NSRange::new(text.encode_utf16().count(), 0)
    } else {
        // A revision can shorten the text or put a surrogate pair across
        // an old offset. Keep the selection within valid UTF-16 boundaries.
        let start = utf16_boundary(text, previous.location);
        let end = utf16_boundary(text, previous.location.saturating_add(previous.length));
        NSRange::new(start, end - start)
    };
    SelectionUpdate { range, follow_end }
}

fn utf16_boundary(text: &str, offset: usize) -> usize {
    let mut boundary = 0;
    for character in text.chars() {
        let next = boundary + character.len_utf16();
        if next > offset {
            break;
        }
        boundary = next;
    }
    boundary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_caret_at_the_end_follows_narration() {
        for elapsed in [
            None,
            Some(Duration::from_secs(2)),
            Some(Duration::from_secs(3)),
        ] {
            let update = selection_after_update(NSRange::new(5, 0), 5, "hello world", elapsed);
            assert_eq!(update.range, NSRange::new(11, 0));
            assert!(update.follow_end);
        }
    }

    #[test]
    fn typing_keeps_the_caret_at_its_previous_offset_until_two_seconds() {
        for elapsed in [
            Duration::ZERO,
            Duration::from_millis(1000),
            Duration::from_millis(1999),
        ] {
            let update =
                selection_after_update(NSRange::new(5, 0), 5, "hello world", Some(elapsed));
            assert_eq!(update.range, NSRange::new(5, 0));
            assert!(!update.follow_end);
        }
    }

    #[test]
    fn idle_caret_inside_the_text_never_follows_narration() {
        let update = selection_after_update(
            NSRange::new(2, 0),
            5,
            "hello world",
            Some(Duration::from_secs(20)),
        );
        assert_eq!(update.range, NSRange::new(2, 0));
        assert!(!update.follow_end);
    }

    #[test]
    fn a_selection_ending_at_the_tail_is_preserved() {
        let update = selection_after_update(NSRange::new(2, 3), 5, "hello world", None);
        assert_eq!(update.range, NSRange::new(2, 3));
        assert!(!update.follow_end);
    }

    #[test]
    fn a_shortened_revision_clamps_the_selection_to_the_remaining_text() {
        let update = selection_after_update(NSRange::new(2, 6), 10, "hello", None);
        assert_eq!(update.range, NSRange::new(2, 3));
        assert!(!update.follow_end);
        let update = selection_after_update(NSRange::new(8, 0), 10, "hello", None);
        assert_eq!(update.range, NSRange::new(5, 0));
        assert!(!update.follow_end);
    }

    #[test]
    fn unicode_revisions_do_not_split_a_surrogate_pair() {
        let update = selection_after_update(NSRange::new(2, 2), 5, "a😀b", None);
        assert_eq!(update.range, NSRange::new(1, 3));
        let update = selection_after_update(NSRange::new(4, 0), 4, "a😀b😀", None);
        assert_eq!(update.range, NSRange::new(6, 0));
        assert!(update.follow_end);
    }

    #[test]
    fn empty_revisions_clear_only_the_invalid_selection() {
        let update = selection_after_update(NSRange::new(2, 3), 5, "", None);
        assert_eq!(update.range, NSRange::new(0, 0));
        assert!(!update.follow_end);
    }
}
