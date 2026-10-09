use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_foundation::{kCFRunLoopCommonModes, CFMachPort, CFRunLoop};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType,
};

use super::HotkeyEvent;
use crate::hotkey_binding::HotkeyModifiers;
use crate::key::Key;

/// The side bits of the modifier flags, one per modifier key
/// (`NX_DEVICE*KEYMASK` in IOKit's `hidsystem/IOLLEvent.h`). Hardware
/// keyboards set them alongside the device-independent masks.
const LEFT_CONTROL_BIT: u64 = 0x0000_0001;
const LEFT_SHIFT_BIT: u64 = 0x0000_0002;
const RIGHT_SHIFT_BIT: u64 = 0x0000_0004;
const LEFT_COMMAND_BIT: u64 = 0x0000_0008;
const RIGHT_COMMAND_BIT: u64 = 0x0000_0010;
const LEFT_OPTION_BIT: u64 = 0x0000_0020;
const RIGHT_OPTION_BIT: u64 = 0x0000_0040;
const RIGHT_CONTROL_BIT: u64 = 0x0000_2000;

const BACKSPACE: u16 = 51;
const CAPS_LOCK: u16 = 57;
const COMMA: u16 = 0x2B; // kVK_ANSI_Comma in HIToolbox/Events.h.
const CONTROL_LEFT: u16 = 59;
const CONTROL_RIGHT: u16 = 62;
const DOWN_ARROW: u16 = 125;
const END: u16 = 119;
const ESCAPE: u16 = 53;
const F1: u16 = 122;
const F10: u16 = 109;
const F11: u16 = 103;
const F12: u16 = 111;
const F2: u16 = 120;
const F3: u16 = 99;
const F4: u16 = 118;
const F5: u16 = 96;
const F6: u16 = 97;
const F7: u16 = 98;
const F8: u16 = 100;
const F9: u16 = 101;
const FORWARD_DELETE: u16 = 117;
const HOME: u16 = 115;
const LEFT_ARROW: u16 = 123;
const META_LEFT: u16 = 55;
const META_RIGHT: u16 = 54;
const OPTION_LEFT: u16 = 58;
const OPTION_RIGHT: u16 = 61;
const PAGE_DOWN: u16 = 121;
const PAGE_UP: u16 = 116;
const RETURN: u16 = 36;
const RIGHT_ARROW: u16 = 124;
const SHIFT_LEFT: u16 = 56;
const SHIFT_RIGHT: u16 = 60;
const SPACE: u16 = 49;
const TAB: u16 = 48;
const UP_ARROW: u16 = 126;
const NUM1: u16 = 18;
const NUM2: u16 = 19;
const NUM3: u16 = 20;
const NUM4: u16 = 21;
const NUM5: u16 = 23;
const NUM6: u16 = 22;
const NUM7: u16 = 26;
const NUM8: u16 = 28;
const NUM9: u16 = 25;
const NUM0: u16 = 29;
const KEY_Q: u16 = 12;
const KEY_W: u16 = 13;
const KEY_E: u16 = 14;
const KEY_R: u16 = 15;
const KEY_T: u16 = 17;
const KEY_Y: u16 = 16;
const KEY_U: u16 = 32;
const KEY_I: u16 = 34;
const KEY_O: u16 = 31;
const KEY_P: u16 = 35;
const KEY_A: u16 = 0;
const KEY_S: u16 = 1;
const KEY_D: u16 = 2;
const KEY_F: u16 = 3;
const KEY_G: u16 = 5;
const KEY_H: u16 = 4;
const KEY_J: u16 = 38;
const KEY_K: u16 = 40;
const KEY_L: u16 = 37;
const KEY_Z: u16 = 6;
const KEY_X: u16 = 7;
const KEY_C: u16 = 8;
const KEY_V: u16 = 9;
const KEY_B: u16 = 11;
const KEY_N: u16 = 45;
const KEY_M: u16 = 46;

struct HotkeyTapState {
    callback: Box<dyn FnMut(HotkeyEvent) -> bool>,
    tap_port: Cell<*const CFMachPort>,
}

impl HotkeyTapState {
    fn new(callback: Box<dyn FnMut(HotkeyEvent) -> bool>) -> Self {
        Self {
            callback,
            tap_port: Cell::new(std::ptr::null()),
        }
    }
}

pub(super) fn run_hotkey_event_loop<F>(callback: F) -> Result<(), String>
where
    F: FnMut(HotkeyEvent) -> bool + 'static,
{
    let state = Box::new(HotkeyTapState::new(Box::new(callback)));
    let state_ptr = Box::into_raw(state);

    let tap = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::HIDEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            keyboard_event_mask(),
            Some(raw_event_callback),
            state_ptr.cast::<c_void>(),
        )
    }
    .ok_or_else(|| {
        unsafe {
            drop(Box::from_raw(state_ptr));
        }
        "failed to create macOS event tap for global hotkeys".to_owned()
    })?;

    let run_loop_source =
        CFMachPort::new_run_loop_source(None, Some(&tap), 0).ok_or_else(|| {
            unsafe {
                drop(Box::from_raw(state_ptr));
            }
            "failed to create run loop source for global hotkey tap".to_owned()
        })?;

    let current_run_loop = CFRunLoop::current()
        .ok_or_else(|| "failed to get current run loop for hotkey tap".to_owned())?;
    current_run_loop.add_source(Some(&run_loop_source), unsafe { kCFRunLoopCommonModes });

    unsafe {
        (*state_ptr).tap_port.set((&*tap) as *const CFMachPort);
    }
    CGEvent::tap_enable(&tap, true);
    CFRunLoop::run();

    unsafe {
        drop(Box::from_raw(state_ptr));
    }
    Ok(())
}

unsafe extern "C-unwind" fn raw_event_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    cg_event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    let state = unsafe { &mut *(user_info.cast::<HotkeyTapState>()) };
    let cg_event_ref = unsafe { cg_event.as_ref() };

    if matches!(
        event_type,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        if let Some(tap_port) = NonNull::new(state.tap_port.get().cast_mut()) {
            CGEvent::tap_enable(unsafe { tap_port.as_ref() }, true);
        }
        return cg_event.as_ptr();
    }

    if let Some(event) = hotkey_event_from_cg_event(event_type, cg_event_ref) {
        if (state.callback)(event) {
            CGEvent::set_type(Some(cg_event_ref), CGEventType::Null);
        }
    }

    cg_event.as_ptr()
}

fn hotkey_event_from_cg_event(event_type: CGEventType, cg_event: &CGEvent) -> Option<HotkeyEvent> {
    hotkey_event(event_type, event_keycode(cg_event)?, CGEvent::flags(Some(cg_event)))
}

/// The hotkey event for a keyboard event of `event_type` for key `code`,
/// whose modifier flags are `flags`. Modifiers are read from the flags every
/// event carries, never counted from earlier events, so an event the tap
/// missed (a keyboard unplugged while a modifier was held) cannot leave a
/// modifier held.
fn hotkey_event(event_type: CGEventType, code: u16, flags: CGEventFlags) -> Option<HotkeyEvent> {
    let key = key_from_code(code)?;
    match event_type {
        CGEventType::KeyDown => Some(HotkeyEvent::KeyPress(key, modifiers_held(flags, None))),
        CGEventType::KeyUp => Some(HotkeyEvent::KeyRelease(key)),
        CGEventType::FlagsChanged if is_modifier_key(key) => Some(if modifier_is_down(key, flags) {
            HotkeyEvent::KeyPress(key, modifiers_held(flags, Some(key)))
        } else {
            HotkeyEvent::KeyRelease(key)
        }),
        _ => None,
    }
}

/// A modifier key's device-independent mask, its own side bit, and the other
/// side's bit. Caps Lock has no sides.
fn modifier_bits(key: Key) -> Option<(CGEventFlags, u64, u64)> {
    match key {
        Key::ShiftLeft => Some((CGEventFlags::MaskShift, LEFT_SHIFT_BIT, RIGHT_SHIFT_BIT)),
        Key::ShiftRight => Some((CGEventFlags::MaskShift, RIGHT_SHIFT_BIT, LEFT_SHIFT_BIT)),
        Key::ControlLeft => Some((CGEventFlags::MaskControl, LEFT_CONTROL_BIT, RIGHT_CONTROL_BIT)),
        Key::ControlRight => Some((CGEventFlags::MaskControl, RIGHT_CONTROL_BIT, LEFT_CONTROL_BIT)),
        Key::AltLeft => Some((CGEventFlags::MaskAlternate, LEFT_OPTION_BIT, RIGHT_OPTION_BIT)),
        Key::AltRight => Some((CGEventFlags::MaskAlternate, RIGHT_OPTION_BIT, LEFT_OPTION_BIT)),
        Key::MetaLeft => Some((CGEventFlags::MaskCommand, LEFT_COMMAND_BIT, RIGHT_COMMAND_BIT)),
        Key::MetaRight => Some((CGEventFlags::MaskCommand, RIGHT_COMMAND_BIT, LEFT_COMMAND_BIT)),
        _ => None,
    }
}

/// Whether modifier `key` is down in `flags`: by its side bit, or, for an
/// event that carries only the device-independent mask (some synthetic
/// events do), by that mask.
fn modifier_is_down(key: Key, flags: CGEventFlags) -> bool {
    let Some((mask, own_side, other_side)) = modifier_bits(key) else {
        return flags.contains(CGEventFlags::MaskAlphaShift);
    };
    let bits = flags.bits();
    if bits & (own_side | other_side) != 0 {
        bits & own_side != 0
    } else {
        flags.contains(mask)
    }
}

/// The modifiers held in `flags`. For the press of modifier `except`, its own
/// side is left out, so the press reports the modifiers held before it, as a
/// binding like "Shift+Cmd" or a modifier-only binding expects.
fn modifiers_held(flags: CGEventFlags, except: Option<Key>) -> HotkeyModifiers {
    let held = |mask: CGEventFlags| {
        match except.and_then(modifier_bits).filter(|(except_mask, _, _)| *except_mask == mask) {
            Some((_, _, other_side)) => flags.bits() & other_side != 0,
            None => flags.contains(mask),
        }
    };
    HotkeyModifiers {
        shift: held(CGEventFlags::MaskShift),
        control: held(CGEventFlags::MaskControl),
        alt: held(CGEventFlags::MaskAlternate),
        meta: held(CGEventFlags::MaskCommand),
    }
}

fn event_keycode(cg_event: &CGEvent) -> Option<u16> {
    u16::try_from(CGEvent::integer_value_field(
        Some(cg_event),
        CGEventField::KeyboardEventKeycode,
    ))
    .ok()
}

fn keyboard_event_mask() -> CGEventMask {
    event_type_mask(CGEventType::KeyDown)
        | event_type_mask(CGEventType::KeyUp)
        | event_type_mask(CGEventType::FlagsChanged)
}

fn event_type_mask(event_type: CGEventType) -> CGEventMask {
    1u64 << (event_type.0 as u64)
}

fn is_modifier_key(key: Key) -> bool {
    matches!(
        key,
        Key::AltLeft
            | Key::AltRight
            | Key::CapsLock
            | Key::ControlLeft
            | Key::ControlRight
            | Key::MetaLeft
            | Key::MetaRight
            | Key::ShiftLeft
            | Key::ShiftRight
    )
}

fn key_from_code(code: u16) -> Option<Key> {
    match code {
        BACKSPACE => Some(Key::Backspace),
        CAPS_LOCK => Some(Key::CapsLock),
        CONTROL_LEFT => Some(Key::ControlLeft),
        CONTROL_RIGHT => Some(Key::ControlRight),
        DOWN_ARROW => Some(Key::DownArrow),
        END => Some(Key::End),
        ESCAPE => Some(Key::Escape),
        F1 => Some(Key::F1),
        F10 => Some(Key::F10),
        F11 => Some(Key::F11),
        F12 => Some(Key::F12),
        F2 => Some(Key::F2),
        F3 => Some(Key::F3),
        F4 => Some(Key::F4),
        F5 => Some(Key::F5),
        F6 => Some(Key::F6),
        F7 => Some(Key::F7),
        F8 => Some(Key::F8),
        F9 => Some(Key::F9),
        NUM0 => Some(Key::Num0),
        NUM1 => Some(Key::Num1),
        NUM2 => Some(Key::Num2),
        NUM3 => Some(Key::Num3),
        NUM4 => Some(Key::Num4),
        NUM5 => Some(Key::Num5),
        NUM6 => Some(Key::Num6),
        NUM7 => Some(Key::Num7),
        NUM8 => Some(Key::Num8),
        NUM9 => Some(Key::Num9),
        KEY_A => Some(Key::KeyA),
        KEY_B => Some(Key::KeyB),
        KEY_C => Some(Key::KeyC),
        KEY_D => Some(Key::KeyD),
        KEY_E => Some(Key::KeyE),
        KEY_F => Some(Key::KeyF),
        KEY_G => Some(Key::KeyG),
        KEY_H => Some(Key::KeyH),
        KEY_I => Some(Key::KeyI),
        KEY_J => Some(Key::KeyJ),
        KEY_K => Some(Key::KeyK),
        KEY_L => Some(Key::KeyL),
        KEY_M => Some(Key::KeyM),
        KEY_N => Some(Key::KeyN),
        KEY_O => Some(Key::KeyO),
        KEY_P => Some(Key::KeyP),
        KEY_Q => Some(Key::KeyQ),
        KEY_R => Some(Key::KeyR),
        KEY_S => Some(Key::KeyS),
        KEY_T => Some(Key::KeyT),
        KEY_U => Some(Key::KeyU),
        KEY_V => Some(Key::KeyV),
        KEY_W => Some(Key::KeyW),
        KEY_X => Some(Key::KeyX),
        KEY_Y => Some(Key::KeyY),
        KEY_Z => Some(Key::KeyZ),
        FORWARD_DELETE => Some(Key::ForwardDelete),
        HOME => Some(Key::Home),
        LEFT_ARROW => Some(Key::LeftArrow),
        META_LEFT => Some(Key::MetaLeft),
        META_RIGHT => Some(Key::MetaRight),
        OPTION_LEFT => Some(Key::AltLeft),
        OPTION_RIGHT => Some(Key::AltRight),
        PAGE_DOWN => Some(Key::PageDown),
        PAGE_UP => Some(Key::PageUp),
        RETURN => Some(Key::Return),
        RIGHT_ARROW => Some(Key::RightArrow),
        SHIFT_LEFT => Some(Key::ShiftLeft),
        SHIFT_RIGHT => Some(Key::ShiftRight),
        SPACE => Some(Key::Space),
        TAB => Some(Key::Tab),
        COMMA => Some(Key::Comma),
        UP_ARROW => Some(Key::UpArrow),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::key_from_code;
    use crate::hotkey_binding::{
        format_hotkey_binding, parse_hotkey_binding, HotkeyBinding, HotkeyModifiers,
    };
    use crate::key::Key;

    fn reported_keys() -> HashSet<Key> {
        (0..=u16::MAX).filter_map(key_from_code).collect()
    }

    use objc2_core_graphics::{CGEventFlags, CGEventType};

    use super::{hotkey_event, META_LEFT, META_RIGHT, F5, SHIFT_LEFT};
    use crate::hotkey::HotkeyEvent;

    #[test]
    fn command_comma_is_reported_with_its_modifier_and_roundtrips() {
        let event = hotkey_event(CGEventType::KeyDown, 0x2B, CGEventFlags::MaskCommand);
        assert!(event.is_some(), "the event tap must report the comma key");
        let Some(HotkeyEvent::KeyPress(key, modifiers)) = event else {
            panic!("expected a key press");
        };
        let binding = HotkeyBinding { key, modifiers };
        assert_eq!(format_hotkey_binding(binding).as_deref(), Some("Cmd+,"));
        assert_eq!(parse_hotkey_binding("Cmd+,").unwrap(), binding);
        assert_eq!(
            hotkey_event(CGEventType::KeyUp, 0x2B, CGEventFlags::MaskCommand),
            Some(HotkeyEvent::KeyRelease(key)),
        );
    }

    /// Left Command held, as a hardware keyboard reports it: the Command mask
    /// and the left-Command device bit (`NX_DEVICELCMDKEYMASK`).
    fn left_command() -> CGEventFlags {
        CGEventFlags::MaskCommand | CGEventFlags::from_bits_retain(0x08)
    }

    fn right_command() -> CGEventFlags {
        CGEventFlags::MaskCommand | CGEventFlags::from_bits_retain(0x10)
    }

    fn no_modifiers() -> HotkeyModifiers {
        HotkeyModifiers::default()
    }

    #[test]
    fn a_key_press_carries_the_modifiers_its_event_reports() {
        assert_eq!(
            hotkey_event(CGEventType::KeyDown, F5, CGEventFlags::empty()),
            Some(HotkeyEvent::KeyPress(Key::F5, no_modifiers()))
        );
        assert_eq!(
            hotkey_event(CGEventType::KeyDown, F5, left_command()),
            Some(HotkeyEvent::KeyPress(Key::F5, HotkeyModifiers { meta: true, ..no_modifiers() }))
        );
    }

    #[test]
    fn a_missed_modifier_release_does_not_leave_the_modifier_held() {
        // Left Command goes down; its release is never seen (the keyboard
        // was unplugged while it was held).
        assert_eq!(
            hotkey_event(CGEventType::FlagsChanged, META_LEFT, left_command()),
            Some(HotkeyEvent::KeyPress(Key::MetaLeft, no_modifiers()))
        );
        // F5 on another keyboard reports no modifiers, so it is plain F5.
        assert_eq!(
            hotkey_event(CGEventType::KeyDown, F5, CGEventFlags::empty()),
            Some(HotkeyEvent::KeyPress(Key::F5, no_modifiers()))
        );
    }

    #[test]
    fn a_modifier_is_pressed_or_released_by_its_own_flag() {
        assert_eq!(
            hotkey_event(CGEventType::FlagsChanged, META_LEFT, left_command()),
            Some(HotkeyEvent::KeyPress(Key::MetaLeft, no_modifiers()))
        );
        assert_eq!(
            hotkey_event(CGEventType::FlagsChanged, META_LEFT, CGEventFlags::empty()),
            Some(HotkeyEvent::KeyRelease(Key::MetaLeft))
        );
        // Left Command let go while right Command stays held.
        assert_eq!(
            hotkey_event(CGEventType::FlagsChanged, META_LEFT, right_command()),
            Some(HotkeyEvent::KeyRelease(Key::MetaLeft))
        );
        // Right Command pressed while left Command is held: the modifiers
        // before it already include Command.
        assert_eq!(
            hotkey_event(CGEventType::FlagsChanged, META_RIGHT, left_command() | right_command()),
            Some(HotkeyEvent::KeyPress(Key::MetaRight, HotkeyModifiers { meta: true, ..no_modifiers() }))
        );
    }

    #[test]
    fn a_modifier_press_reports_the_other_modifiers_held() {
        let shift_left = CGEventFlags::MaskShift | CGEventFlags::from_bits_retain(0x02);
        assert_eq!(
            hotkey_event(CGEventType::FlagsChanged, META_LEFT, left_command() | shift_left),
            Some(HotkeyEvent::KeyPress(Key::MetaLeft, HotkeyModifiers { shift: true, ..no_modifiers() }))
        );
        assert_eq!(
            hotkey_event(CGEventType::FlagsChanged, SHIFT_LEFT, shift_left),
            Some(HotkeyEvent::KeyPress(Key::ShiftLeft, no_modifiers()))
        );
    }

    #[test]
    fn a_synthetic_event_without_side_bits_still_reads_its_modifier() {
        // Some synthetic events set only the device-independent mask.
        assert_eq!(
            hotkey_event(CGEventType::FlagsChanged, META_LEFT, CGEventFlags::MaskCommand),
            Some(HotkeyEvent::KeyPress(Key::MetaLeft, no_modifiers()))
        );
        assert_eq!(
            hotkey_event(CGEventType::KeyDown, F5, CGEventFlags::MaskCommand),
            Some(HotkeyEvent::KeyPress(Key::F5, HotkeyModifiers { meta: true, ..no_modifiers() }))
        );
    }

    #[test]
    fn maps_supported_navigation_and_modifier_keycodes() {
        assert_eq!(key_from_code(62), Some(Key::ControlRight));
        assert_eq!(key_from_code(117), Some(Key::ForwardDelete));
        assert_eq!(key_from_code(121), Some(Key::PageDown));
    }

    #[test]
    fn every_key_variant_is_reported_by_a_keycode() {
        let reported = reported_keys();
        let unreported = Key::ALL
            .iter()
            .copied()
            .filter(|key| !reported.contains(key))
            .collect::<Vec<_>>();
        assert!(
            unreported.is_empty(),
            "Key variants the event tap can never report: {unreported:?}"
        );
    }

    #[test]
    fn every_reported_key_round_trips_through_binding_strings() {
        for key in reported_keys() {
            let binding = HotkeyBinding {
                modifiers: HotkeyModifiers::default(),
                key,
            };
            let formatted = format_hotkey_binding(binding)
                .unwrap_or_else(|| panic!("{key:?} has no binding string"));
            assert_eq!(
                parse_hotkey_binding(&formatted),
                Ok(binding),
                "{key:?} formatted as {formatted:?} did not parse back"
            );
        }
    }
}
