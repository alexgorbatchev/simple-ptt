//! Settings rows whose config key has a built-in default. Each has a Reset
//! button, shown while `config.toml` has the key, whose tag is the row's
//! `tag`; text, number, and shortcut fields stay blank while they follow the
//! default, which they show as a placeholder.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultedRow {
    DictationShortcut,
    HoldThreshold,
    CorrectionKey,
    FontSize,
    MeterStyle,
    AutoCheckUpdates,
    StartOnLogin,
    SampleRate,
    MicGain,
    AlwaysOn,
    DeepgramLanguage,
    DeepgramModel,
    Endpointing,
    UtteranceEnd,
    TransformShortcut,
    AutoTransform,
}

impl DefaultedRow {
    pub const ALL: [Self; 16] = [
        Self::DictationShortcut,
        Self::HoldThreshold,
        Self::CorrectionKey,
        Self::FontSize,
        Self::MeterStyle,
        Self::AutoCheckUpdates,
        Self::StartOnLogin,
        Self::SampleRate,
        Self::MicGain,
        Self::AlwaysOn,
        Self::DeepgramLanguage,
        Self::DeepgramModel,
        Self::Endpointing,
        Self::UtteranceEnd,
        Self::TransformShortcut,
        Self::AutoTransform,
    ];

    /// The `config::DEFAULTED_KEYS` entry the row edits.
    pub const fn config_key(self) -> &'static str {
        match self {
            Self::DictationShortcut => "ui.hotkey",
            Self::HoldThreshold => "mic.hold_ms",
            Self::CorrectionKey => "ui.correction_key",
            Self::FontSize => "ui.font_size",
            Self::MeterStyle => "ui.meter_style",
            Self::AutoCheckUpdates => "ui.auto_check_updates",
            Self::StartOnLogin => "ui.start_on_login",
            Self::SampleRate => "mic.sample_rate",
            Self::MicGain => "mic.gain",
            Self::AlwaysOn => "mic.always_on",
            Self::DeepgramLanguage => "deepgram.language",
            Self::DeepgramModel => "deepgram.model",
            Self::Endpointing => "deepgram.endpointing_ms",
            Self::UtteranceEnd => "deepgram.utterance_end_ms",
            Self::TransformShortcut => "transformation.hotkey",
            Self::AutoTransform => "transformation.auto",
        }
    }

    /// The Reset button tag that identifies the row.
    pub fn tag(self) -> isize {
        Self::ALL
            .iter()
            .position(|row| *row == self)
            .expect("every row is in ALL") as isize
    }

    pub fn from_tag(tag: isize) -> Option<Self> {
        usize::try_from(tag).ok().and_then(|index| Self::ALL.get(index).copied())
    }
}

#[cfg(test)]
mod tests {
    use super::DefaultedRow;
    use crate::config::DEFAULTED_KEYS;

    #[test]
    fn every_row_edits_a_defaulted_key_and_round_trips_through_its_tag() {
        for row in DefaultedRow::ALL {
            assert!(
                DEFAULTED_KEYS.iter().any(|(key, _)| *key == row.config_key()),
                "{row:?}"
            );
            assert_eq!(DefaultedRow::from_tag(row.tag()), Some(row));
        }
        assert_eq!(DefaultedRow::from_tag(-1), None);
        assert_eq!(DefaultedRow::from_tag(DefaultedRow::ALL.len() as isize), None);
    }
}
