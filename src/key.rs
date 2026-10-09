/// Generates `Key` and its test-only `Key::ALL` from a single variant list so the two cannot
/// drift apart.
macro_rules! define_keys {
    ($($variant:ident),+ $(,)?) => {
        /// A physical key the global hotkey event tap can report.
        ///
        /// The variant set is exactly what `key_from_code` in `src/hotkey_macos.rs` decodes from
        /// macOS virtual keycodes; config strings for each variant live in
        /// `src/hotkey_binding.rs` (`key_name` / `parse_key`).
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum Key {
            $($variant),+
        }

        #[cfg(test)]
        impl Key {
            pub const ALL: &'static [Key] = &[$(Key::$variant),+];
        }
    };
}

define_keys! {
    KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
    KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    Escape,
    Comma,
    Space,
    Tab,
    CapsLock,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    MetaLeft,
    MetaRight,
    Return,
    Backspace,
    ForwardDelete,
    Home,
    End,
    PageUp,
    PageDown,
    UpArrow,
    DownArrow,
    LeftArrow,
    RightArrow,
}
