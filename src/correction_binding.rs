use crate::hotkey_binding::{
    format_modifiers, is_modifier_key, key_name, parse_key, parse_modifier_chord, HotkeyBinding,
    HotkeyModifiers,
};
use crate::key::Key;

/// A correction is held with one physical key or a chord of modifier groups.
/// Modifier chords have no primary key and work in either press/release order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrectionBinding {
    Key(Key),
    Modifiers(HotkeyModifiers),
}

impl CorrectionBinding {
    pub fn parse(raw: &str) -> Result<Self, String> {
        if let Some(key) = parse_key(raw.trim()) {
            return Ok(Self::Key(key));
        }
        parse_modifier_chord(raw).map(Self::Modifiers).map_err(|error| {
            format!("correction key must be a supported key such as LeftAlt or F7, or a modifier chord such as Alt+Cmd: {error}")
        })
    }

    pub fn matches_press(self, key: Key, modifiers_before: HotkeyModifiers) -> bool {
        match self {
            Self::Key(bound) => bound == key && (!is_modifier_key(key) || !modifiers_before.any()),
            Self::Modifiers(required) => {
                required.contains_key(key) && modifiers_before.with_key_pressed(key) == required
            }
        }
    }

    pub fn matches_release(self, key: Key, modifiers_after: HotkeyModifiers) -> bool {
        match self {
            Self::Key(bound) => bound == key,
            Self::Modifiers(required) => {
                required.contains_key(key) && !modifiers_after.contains(required)
            }
        }
    }

    pub fn overlaps(self, binding: HotkeyBinding) -> bool {
        match self {
            Self::Key(key) => binding.key == key || binding.modifiers.contains_key(key),
            Self::Modifiers(required) => {
                required.contains_key(binding.key) || binding.modifiers.contains(required)
            }
        }
    }

    pub fn hint_label(self) -> String {
        match self {
            Self::Modifiers(modifiers) => format_modifiers(modifiers),
            Self::Key(key) if is_modifier_key(key) => {
                format_modifiers(HotkeyModifiers::default().with_key_pressed(key))
            }
            Self::Key(Key::Escape) => "Esc".to_owned(),
            Self::Key(key) => key_name(key).to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CorrectionBinding;
    use crate::hotkey_binding::{parse_hotkey_binding, HotkeyModifiers};
    use crate::key::Key;

    #[test]
    fn modifier_chord_aliases_round_trip_and_accept_either_side_and_order() {
        for text in ["Alt+Cmd", "Command+Option", " rightalt + leftcommand "] {
            let binding = CorrectionBinding::parse(text).unwrap();
            assert_eq!(binding.hint_label(), "Alt+Cmd");
            assert_eq!(CorrectionBinding::parse(&binding.hint_label()), Ok(binding));
            for alt in [Key::AltLeft, Key::AltRight] {
                for command in [Key::MetaLeft, Key::MetaRight] {
                    assert!(!binding.matches_press(alt, HotkeyModifiers::default()));
                    assert!(!binding.matches_press(command, HotkeyModifiers::default()));
                    assert!(binding.matches_press(
                        command,
                        HotkeyModifiers {
                            alt: true,
                            ..Default::default()
                        }
                    ));
                    assert!(binding.matches_press(
                        alt,
                        HotkeyModifiers {
                            meta: true,
                            ..Default::default()
                        }
                    ));
                }
            }
        }
    }

    #[test]
    fn correction_release_waits_until_a_required_modifier_group_is_released() {
        let binding = CorrectionBinding::parse("Alt+Cmd").unwrap();
        let both = HotkeyModifiers {
            alt: true,
            meta: true,
            ..Default::default()
        };
        // Releasing right Option while left Option remains held keeps the chord down.
        assert!(!binding.matches_release(Key::AltRight, both));
        assert!(!binding.matches_release(Key::ShiftLeft, both));
        assert!(!binding.matches_release(Key::KeyC, both));
        assert!(binding.matches_release(
            Key::AltLeft,
            HotkeyModifiers {
                meta: true,
                shift: true,
                ..Default::default()
            }
        ));
        assert!(binding.matches_release(
            Key::MetaRight,
            HotkeyModifiers {
                alt: true,
                ..Default::default()
            }
        ));
        assert!(!binding.matches_press(
            Key::MetaRight,
            HotkeyModifiers {
                alt: true,
                shift: true,
                ..Default::default()
            }
        ));
    }

    #[test]
    fn correction_chord_conflicts_only_with_triggers_that_can_fire_during_its_hold() {
        let binding = CorrectionBinding::parse("Alt+Cmd").unwrap();
        for other in ["Alt+Cmd+F5", "Ctrl+Alt+Cmd+F6", "LeftAlt", "RightMeta"] {
            assert!(
                binding.overlaps(parse_hotkey_binding(other).unwrap()),
                "{other}"
            );
        }
        for other in ["F5", "Cmd+F6", "Alt+F5", "LeftShift"] {
            assert!(
                !binding.overlaps(parse_hotkey_binding(other).unwrap()),
                "{other}"
            );
        }
    }

    #[test]
    fn specific_single_keys_keep_their_press_release_and_conflict_behavior() {
        let binding = CorrectionBinding::parse("LeftAlt").unwrap();
        assert!(binding.matches_press(Key::AltLeft, HotkeyModifiers::default()));
        assert!(!binding.matches_press(Key::AltRight, HotkeyModifiers::default()));
        assert!(!binding.matches_press(
            Key::AltLeft,
            HotkeyModifiers {
                meta: true,
                ..Default::default()
            }
        ));
        assert!(binding.matches_release(Key::AltLeft, HotkeyModifiers::default()));
        assert!(binding.overlaps(parse_hotkey_binding("Alt+F5").unwrap()));
        assert_eq!(binding.hint_label(), "Alt");
    }

    #[test]
    fn invalid_or_ambiguous_correction_chords_are_rejected() {
        for text in [
            "",
            "Cmd",
            "Option",
            "Alt+Option",
            "Cmd+F7",
            "Alt+unknown",
            "F5+F6",
        ] {
            assert!(CorrectionBinding::parse(text).is_err(), "{text}");
        }
    }
}
