use rdev::Key;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HotkeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HotkeyBinding {
    pub modifiers: HotkeyModifiers,
    pub key: Key,
}

impl HotkeyBinding {
    pub fn matches_press(&self, key: Key, active_modifiers: HotkeyModifiers) -> bool {
        self.key == key && self.modifiers == active_modifiers
    }

    pub fn matches_release(&self, key: Key) -> bool {
        self.key == key
    }
}

impl HotkeyModifiers {
    pub fn with_key_pressed(mut self, key: Key) -> Self {
        match modifier_group_for_key(key) {
            Some(ModifierGroup::Shift) => self.shift = true,
            Some(ModifierGroup::Control) => self.control = true,
            Some(ModifierGroup::Alt) => self.alt = true,
            Some(ModifierGroup::Meta) => self.meta = true,
            None => {}
        }
        self
    }

    pub fn with_key_released(mut self, key: Key) -> Self {
        match modifier_group_for_key(key) {
            Some(ModifierGroup::Shift) => self.shift = false,
            Some(ModifierGroup::Control) => self.control = false,
            Some(ModifierGroup::Alt) => self.alt = false,
            Some(ModifierGroup::Meta) => self.meta = false,
            None => {}
        }
        self
    }

    pub fn any(&self) -> bool {
        self.shift || self.control || self.alt || self.meta
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModifierGroup {
    Shift,
    Control,
    Alt,
    Meta,
}

pub fn is_modifier_key(key: Key) -> bool {
    modifier_group_for_key(key).is_some()
}

fn modifier_group_for_key(key: Key) -> Option<ModifierGroup> {
    match key {
        Key::ShiftLeft | Key::ShiftRight => Some(ModifierGroup::Shift),
        Key::ControlLeft | Key::ControlRight => Some(ModifierGroup::Control),
        Key::Alt | Key::AltGr => Some(ModifierGroup::Alt),
        Key::MetaLeft | Key::MetaRight => Some(ModifierGroup::Meta),
        _ => None,
    }
}

pub fn format_hotkey_binding(binding: HotkeyBinding) -> Option<String> {
    let mut tokens = Vec::new();
    if binding.modifiers.control {
        tokens.push("Ctrl");
    }
    if binding.modifiers.alt {
        tokens.push("Alt");
    }
    if binding.modifiers.shift {
        tokens.push("Shift");
    }
    if binding.modifiers.meta {
        tokens.push("Cmd");
    }
    tokens.push(key_name(binding.key)?);
    Some(tokens.join("+"))
}

pub fn parse_hotkey_binding(raw: &str) -> Result<HotkeyBinding, String> {
    let tokens = raw
        .split('+')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err("hotkey is required".to_owned());
    }

    if tokens.len() == 1 {
        let token = tokens[0];
        if let Some(key) = parse_key(token) {
            return Ok(HotkeyBinding {
                modifiers: HotkeyModifiers::default(),
                key,
            });
        }
        if parse_modifier_token(token).is_some() {
            return Err(format!(
                "generic modifier '{}' must be combined with a non-modifier key; use a specific key such as LeftShift for a single-key binding",
                token
            ));
        }
        return Err(format!("hotkey token '{}' is not supported", token));
    }

    let mut modifiers = HotkeyModifiers::default();
    let mut key = None;

    for token in tokens {
        if let Some(modifier_group) = parse_modifier_token(token) {
            match modifier_group {
                ModifierGroup::Shift => modifiers.shift = true,
                ModifierGroup::Control => modifiers.control = true,
                ModifierGroup::Alt => modifiers.alt = true,
                ModifierGroup::Meta => modifiers.meta = true,
            }
            continue;
        }

        let Some(parsed_key) = parse_key(token) else {
            return Err(format!("hotkey token '{}' is not supported", token));
        };
        if is_modifier_key(parsed_key) {
            return Err(format!(
                "modifier '{}' must be written as Shift, Ctrl, Alt, or Cmd inside a multi-key chord",
                token
            ));
        }
        if key.replace(parsed_key).is_some() {
            return Err("hotkey chords must contain exactly one non-modifier key".to_owned());
        }
    }

    let Some(key) = key else {
        return Err("hotkey chords must include a non-modifier key".to_owned());
    };

    Ok(HotkeyBinding { modifiers, key })
}

fn parse_modifier_token(token: &str) -> Option<ModifierGroup> {
    if token.eq_ignore_ascii_case("shift")
        || token.eq_ignore_ascii_case("leftshift")
        || token.eq_ignore_ascii_case("rightshift")
        || token.eq_ignore_ascii_case("lshift")
        || token.eq_ignore_ascii_case("rshift")
    {
        return Some(ModifierGroup::Shift);
    }
    if token.eq_ignore_ascii_case("control")
        || token.eq_ignore_ascii_case("ctrl")
        || token.eq_ignore_ascii_case("leftcontrol")
        || token.eq_ignore_ascii_case("rightcontrol")
        || token.eq_ignore_ascii_case("lctrl")
        || token.eq_ignore_ascii_case("rctrl")
    {
        return Some(ModifierGroup::Control);
    }
    if token.eq_ignore_ascii_case("alt")
        || token.eq_ignore_ascii_case("option")
        || token.eq_ignore_ascii_case("leftalt")
        || token.eq_ignore_ascii_case("rightalt")
        || token.eq_ignore_ascii_case("leftoption")
        || token.eq_ignore_ascii_case("rightoption")
        || token.eq_ignore_ascii_case("lalt")
        || token.eq_ignore_ascii_case("ralt")
    {
        return Some(ModifierGroup::Alt);
    }
    if token.eq_ignore_ascii_case("cmd")
        || token.eq_ignore_ascii_case("command")
        || token.eq_ignore_ascii_case("meta")
        || token.eq_ignore_ascii_case("leftmeta")
        || token.eq_ignore_ascii_case("rightmeta")
        || token.eq_ignore_ascii_case("leftcommand")
        || token.eq_ignore_ascii_case("rightcommand")
        || token.eq_ignore_ascii_case("lcmd")
        || token.eq_ignore_ascii_case("rcmd")
    {
        return Some(ModifierGroup::Meta);
    }
    None
}

pub fn key_name(key: Key) -> Option<&'static str> {
    match key {
        Key::KeyA => Some("A"),
        Key::KeyB => Some("B"),
        Key::KeyC => Some("C"),
        Key::KeyD => Some("D"),
        Key::KeyE => Some("E"),
        Key::KeyF => Some("F"),
        Key::KeyG => Some("G"),
        Key::KeyH => Some("H"),
        Key::KeyI => Some("I"),
        Key::KeyJ => Some("J"),
        Key::KeyK => Some("K"),
        Key::KeyL => Some("L"),
        Key::KeyM => Some("M"),
        Key::KeyN => Some("N"),
        Key::KeyO => Some("O"),
        Key::KeyP => Some("P"),
        Key::KeyQ => Some("Q"),
        Key::KeyR => Some("R"),
        Key::KeyS => Some("S"),
        Key::KeyT => Some("T"),
        Key::KeyU => Some("U"),
        Key::KeyV => Some("V"),
        Key::KeyW => Some("W"),
        Key::KeyX => Some("X"),
        Key::KeyY => Some("Y"),
        Key::KeyZ => Some("Z"),
        Key::Num0 => Some("0"),
        Key::Num1 => Some("1"),
        Key::Num2 => Some("2"),
        Key::Num3 => Some("3"),
        Key::Num4 => Some("4"),
        Key::Num5 => Some("5"),
        Key::Num6 => Some("6"),
        Key::Num7 => Some("7"),
        Key::Num8 => Some("8"),
        Key::Num9 => Some("9"),
        Key::F1 => Some("F1"),
        Key::F2 => Some("F2"),
        Key::F3 => Some("F3"),
        Key::F4 => Some("F4"),
        Key::F5 => Some("F5"),
        Key::F6 => Some("F6"),
        Key::F7 => Some("F7"),
        Key::F8 => Some("F8"),
        Key::F9 => Some("F9"),
        Key::F10 => Some("F10"),
        Key::F11 => Some("F11"),
        Key::F12 => Some("F12"),
        Key::Escape => Some("Escape"),
        Key::Space => Some("Space"),
        Key::Tab => Some("Tab"),
        Key::CapsLock => Some("CapsLock"),
        Key::ShiftLeft => Some("LeftShift"),
        Key::ShiftRight => Some("RightShift"),
        Key::ControlLeft => Some("LeftControl"),
        Key::ControlRight => Some("RightControl"),
        Key::Alt => Some("LeftAlt"),
        Key::AltGr => Some("RightAlt"),
        Key::MetaLeft => Some("LeftMeta"),
        Key::MetaRight => Some("RightMeta"),
        Key::Return => Some("Return"),
        Key::Backspace => Some("Backspace"),
        Key::Delete => Some("ForwardDelete"),
        Key::Home => Some("Home"),
        Key::End => Some("End"),
        Key::PageUp => Some("PageUp"),
        Key::PageDown => Some("PageDown"),
        Key::UpArrow => Some("UpArrow"),
        Key::DownArrow => Some("DownArrow"),
        Key::LeftArrow => Some("LeftArrow"),
        Key::RightArrow => Some("RightArrow"),
        _ => None,
    }
}

pub fn parse_key(name: &str) -> Option<Key> {
    if name.len() == 1 {
        let ch = name.chars().next()?.to_ascii_uppercase();
        return match ch {
            'A' => Some(Key::KeyA),
            'B' => Some(Key::KeyB),
            'C' => Some(Key::KeyC),
            'D' => Some(Key::KeyD),
            'E' => Some(Key::KeyE),
            'F' => Some(Key::KeyF),
            'G' => Some(Key::KeyG),
            'H' => Some(Key::KeyH),
            'I' => Some(Key::KeyI),
            'J' => Some(Key::KeyJ),
            'K' => Some(Key::KeyK),
            'L' => Some(Key::KeyL),
            'M' => Some(Key::KeyM),
            'N' => Some(Key::KeyN),
            'O' => Some(Key::KeyO),
            'P' => Some(Key::KeyP),
            'Q' => Some(Key::KeyQ),
            'R' => Some(Key::KeyR),
            'S' => Some(Key::KeyS),
            'T' => Some(Key::KeyT),
            'U' => Some(Key::KeyU),
            'V' => Some(Key::KeyV),
            'W' => Some(Key::KeyW),
            'X' => Some(Key::KeyX),
            'Y' => Some(Key::KeyY),
            'Z' => Some(Key::KeyZ),
            '0' => Some(Key::Num0),
            '1' => Some(Key::Num1),
            '2' => Some(Key::Num2),
            '3' => Some(Key::Num3),
            '4' => Some(Key::Num4),
            '5' => Some(Key::Num5),
            '6' => Some(Key::Num6),
            '7' => Some(Key::Num7),
            '8' => Some(Key::Num8),
            '9' => Some(Key::Num9),
            _ => None,
        };
    }

    if name.eq_ignore_ascii_case("f1") {
        return Some(Key::F1);
    }
    if name.eq_ignore_ascii_case("f2") {
        return Some(Key::F2);
    }
    if name.eq_ignore_ascii_case("f3") {
        return Some(Key::F3);
    }
    if name.eq_ignore_ascii_case("f4") {
        return Some(Key::F4);
    }
    if name.eq_ignore_ascii_case("f5") {
        return Some(Key::F5);
    }
    if name.eq_ignore_ascii_case("f6") {
        return Some(Key::F6);
    }
    if name.eq_ignore_ascii_case("f7") {
        return Some(Key::F7);
    }
    if name.eq_ignore_ascii_case("f8") {
        return Some(Key::F8);
    }
    if name.eq_ignore_ascii_case("f9") {
        return Some(Key::F9);
    }
    if name.eq_ignore_ascii_case("f10") {
        return Some(Key::F10);
    }
    if name.eq_ignore_ascii_case("f11") {
        return Some(Key::F11);
    }
    if name.eq_ignore_ascii_case("f12") {
        return Some(Key::F12);
    }
    if name.eq_ignore_ascii_case("escape") || name.eq_ignore_ascii_case("esc") {
        return Some(Key::Escape);
    }
    if name.eq_ignore_ascii_case("space") {
        return Some(Key::Space);
    }
    if name.eq_ignore_ascii_case("tab") {
        return Some(Key::Tab);
    }
    if name.eq_ignore_ascii_case("capslock") {
        return Some(Key::CapsLock);
    }
    if name.eq_ignore_ascii_case("leftshift") || name.eq_ignore_ascii_case("lshift") {
        return Some(Key::ShiftLeft);
    }
    if name.eq_ignore_ascii_case("rightshift") || name.eq_ignore_ascii_case("rshift") {
        return Some(Key::ShiftRight);
    }
    if name.eq_ignore_ascii_case("leftcontrol") || name.eq_ignore_ascii_case("lctrl") {
        return Some(Key::ControlLeft);
    }
    if name.eq_ignore_ascii_case("rightcontrol") || name.eq_ignore_ascii_case("rctrl") {
        return Some(Key::ControlRight);
    }
    if name.eq_ignore_ascii_case("leftalt")
        || name.eq_ignore_ascii_case("lalt")
        || name.eq_ignore_ascii_case("leftoption")
    {
        return Some(Key::Alt);
    }
    if name.eq_ignore_ascii_case("rightalt")
        || name.eq_ignore_ascii_case("ralt")
        || name.eq_ignore_ascii_case("rightoption")
    {
        return Some(Key::AltGr);
    }
    if name.eq_ignore_ascii_case("leftmeta")
        || name.eq_ignore_ascii_case("leftcommand")
        || name.eq_ignore_ascii_case("lcmd")
    {
        return Some(Key::MetaLeft);
    }
    if name.eq_ignore_ascii_case("rightmeta")
        || name.eq_ignore_ascii_case("rightcommand")
        || name.eq_ignore_ascii_case("rcmd")
    {
        return Some(Key::MetaRight);
    }
    if name.eq_ignore_ascii_case("return") || name.eq_ignore_ascii_case("enter") {
        return Some(Key::Return);
    }
    if name.eq_ignore_ascii_case("backspace") || name.eq_ignore_ascii_case("delete") {
        return Some(Key::Backspace);
    }
    if name.eq_ignore_ascii_case("forwarddelete") {
        return Some(Key::Delete);
    }
    if name.eq_ignore_ascii_case("home") {
        return Some(Key::Home);
    }
    if name.eq_ignore_ascii_case("end") {
        return Some(Key::End);
    }
    if name.eq_ignore_ascii_case("pageup") {
        return Some(Key::PageUp);
    }
    if name.eq_ignore_ascii_case("pagedown") {
        return Some(Key::PageDown);
    }
    if name.eq_ignore_ascii_case("uparrow") || name.eq_ignore_ascii_case("up") {
        return Some(Key::UpArrow);
    }
    if name.eq_ignore_ascii_case("downarrow") || name.eq_ignore_ascii_case("down") {
        return Some(Key::DownArrow);
    }
    if name.eq_ignore_ascii_case("leftarrow") || name.eq_ignore_ascii_case("left") {
        return Some(Key::LeftArrow);
    }
    if name.eq_ignore_ascii_case("rightarrow") || name.eq_ignore_ascii_case("right") {
        return Some(Key::RightArrow);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        format_hotkey_binding, parse_hotkey_binding, parse_key, HotkeyBinding, HotkeyModifiers,
    };
    use rdev::Key;

    #[test]
    fn parses_single_key_bindings() {
        assert_eq!(
            parse_hotkey_binding("F5").unwrap(),
            HotkeyBinding {
                modifiers: HotkeyModifiers::default(),
                key: Key::F5,
            }
        );
        assert_eq!(
            parse_hotkey_binding("z").unwrap(),
            HotkeyBinding {
                modifiers: HotkeyModifiers::default(),
                key: Key::KeyZ,
            }
        );
    }

    #[test]
    fn parses_modifier_chords() {
        assert_eq!(
            parse_hotkey_binding("Shift+Cmd+Z").unwrap(),
            HotkeyBinding {
                modifiers: HotkeyModifiers {
                    shift: true,
                    control: false,
                    alt: false,
                    meta: true,
                },
                key: Key::KeyZ,
            }
        );
    }

    #[test]
    fn formats_chords_canonically() {
        assert_eq!(
            format_hotkey_binding(HotkeyBinding {
                modifiers: HotkeyModifiers {
                    shift: true,
                    control: true,
                    alt: false,
                    meta: true,
                },
                key: Key::KeyZ,
            })
            .as_deref(),
            Some("Ctrl+Shift+Cmd+Z")
        );
    }

    #[test]
    fn rejects_modifier_only_generic_single_token() {
        assert!(parse_hotkey_binding("Shift").is_err());
    }

    #[test]
    fn parse_key_supports_letters_and_digits() {
        assert_eq!(parse_key("Z"), Some(Key::KeyZ));
        assert_eq!(parse_key("7"), Some(Key::Num7));
    }
}
