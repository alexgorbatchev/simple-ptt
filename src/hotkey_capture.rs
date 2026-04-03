use std::sync::{Arc, Mutex};

use rdev::Key;

use crate::hotkey_binding::{
    format_hotkey_binding, is_modifier_key, HotkeyBinding, HotkeyModifiers,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HotkeyCaptureTarget {
    Record,
    Transform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HotkeyCaptureOutcome {
    Captured {
        target: HotkeyCaptureTarget,
        binding: HotkeyBinding,
    },
    Cancelled {
        target: HotkeyCaptureTarget,
    },
}

#[derive(Clone, Default)]
pub struct HotkeyCaptureController {
    state: Arc<Mutex<HotkeyCaptureState>>,
}

#[derive(Default)]
struct HotkeyCaptureState {
    active_target: Option<HotkeyCaptureTarget>,
    active_modifiers: HotkeyModifiers,
    pending_outcome: Option<HotkeyCaptureOutcome>,
    pending_single_modifier_key: Option<Key>,
}

impl HotkeyCaptureController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_capture(&self, target: HotkeyCaptureTarget) {
        if let Ok(mut state) = self.state.lock() {
            state.active_target = Some(target);
            state.active_modifiers = HotkeyModifiers::default();
            state.pending_outcome = None;
            state.pending_single_modifier_key = None;
        }
    }

    pub fn cancel(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.active_target = None;
            state.active_modifiers = HotkeyModifiers::default();
            state.pending_outcome = None;
            state.pending_single_modifier_key = None;
        }
    }

    pub fn has_pending_outcome(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.pending_outcome.is_some())
            .unwrap_or(false)
    }

    pub fn take_outcome(&self) -> Option<HotkeyCaptureOutcome> {
        self.state
            .lock()
            .ok()
            .and_then(|mut state| state.pending_outcome.take())
    }

    pub fn handle_key_press(&self, key: Key, active_modifiers: HotkeyModifiers) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let Some(target) = state.active_target else {
            return false;
        };

        if key == Key::Escape && !active_modifiers.any() {
            state.active_target = None;
            state.active_modifiers = HotkeyModifiers::default();
            state.pending_single_modifier_key = None;
            state.pending_outcome = Some(HotkeyCaptureOutcome::Cancelled { target });
            return true;
        }

        if is_modifier_key(key) {
            state.active_modifiers = active_modifiers.with_key_pressed(key);
            state.pending_single_modifier_key = Some(key);
            return true;
        }

        state.active_target = None;
        state.active_modifiers = HotkeyModifiers::default();
        state.pending_single_modifier_key = None;
        state.pending_outcome = Some(HotkeyCaptureOutcome::Captured {
            target,
            binding: HotkeyBinding {
                modifiers: active_modifiers,
                key,
            },
        });
        true
    }

    pub fn handle_key_release(&self, key: Key) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let Some(target) = state.active_target else {
            return false;
        };

        if is_modifier_key(key) {
            let modifier_before_release = state.active_modifiers;
            state.active_modifiers = state.active_modifiers.with_key_released(key);
            if state.pending_single_modifier_key == Some(key) && !state.active_modifiers.any() {
                state.active_target = None;
                state.pending_single_modifier_key = None;
                state.pending_outcome = Some(HotkeyCaptureOutcome::Captured {
                    target,
                    binding: HotkeyBinding {
                        modifiers: HotkeyModifiers::default(),
                        key,
                    },
                });
            } else if modifier_before_release != state.active_modifiers {
                state.pending_single_modifier_key = None;
            }
            return true;
        }

        true
    }
}

pub fn capture_outcome_message(outcome: HotkeyCaptureOutcome) -> Option<String> {
    match outcome {
        HotkeyCaptureOutcome::Cancelled { .. } => Some("Hotkey capture canceled.".to_owned()),
        HotkeyCaptureOutcome::Captured { target, binding } => {
            let target_label = match target {
                HotkeyCaptureTarget::Record => "record",
                HotkeyCaptureTarget::Transform => "transform",
            };
            Some(format!(
                "Captured {} hotkey: {}.",
                target_label,
                format_hotkey_binding(binding)?
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HotkeyCaptureController, HotkeyCaptureOutcome, HotkeyCaptureTarget};
    use crate::hotkey_binding::{HotkeyBinding, HotkeyModifiers};
    use rdev::Key;

    #[test]
    fn capture_consumes_modifier_chords() {
        let controller = HotkeyCaptureController::new();
        controller.begin_capture(HotkeyCaptureTarget::Record);

        assert!(controller.handle_key_press(Key::ShiftLeft, HotkeyModifiers::default()));
        assert!(controller.handle_key_press(
            Key::MetaLeft,
            HotkeyModifiers {
                shift: true,
                ..HotkeyModifiers::default()
            }
        ));
        assert!(controller.handle_key_press(
            Key::KeyZ,
            HotkeyModifiers {
                shift: true,
                meta: true,
                ..HotkeyModifiers::default()
            }
        ));
        assert_eq!(
            controller.take_outcome(),
            Some(HotkeyCaptureOutcome::Captured {
                target: HotkeyCaptureTarget::Record,
                binding: HotkeyBinding {
                    modifiers: HotkeyModifiers {
                        shift: true,
                        meta: true,
                        ..HotkeyModifiers::default()
                    },
                    key: Key::KeyZ,
                },
            })
        );
    }

    #[test]
    fn escape_cancels_capture_without_modifiers() {
        let controller = HotkeyCaptureController::new();
        controller.begin_capture(HotkeyCaptureTarget::Transform);

        assert!(controller.handle_key_press(Key::Escape, HotkeyModifiers::default()));
        assert_eq!(
            controller.take_outcome(),
            Some(HotkeyCaptureOutcome::Cancelled {
                target: HotkeyCaptureTarget::Transform,
            })
        );
    }

    #[test]
    fn releasing_a_single_modifier_captures_it() {
        let controller = HotkeyCaptureController::new();
        controller.begin_capture(HotkeyCaptureTarget::Transform);

        assert!(controller.handle_key_press(Key::ShiftLeft, HotkeyModifiers::default()));
        assert!(controller.handle_key_release(Key::ShiftLeft));
        assert_eq!(
            controller.take_outcome(),
            Some(HotkeyCaptureOutcome::Captured {
                target: HotkeyCaptureTarget::Transform,
                binding: HotkeyBinding {
                    modifiers: HotkeyModifiers::default(),
                    key: Key::ShiftLeft,
                },
            })
        );
    }
}
