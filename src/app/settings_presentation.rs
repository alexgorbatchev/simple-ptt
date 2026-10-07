//! When the Settings window may be put on screen.
//!
//! `NSApplication::activate` is only a request: "You shouldn't assume the app
//! will be active immediately after sending this message. The framework also
//! does not guarantee that the app will be activated at all"
//! (`NSApplication.h`). A key window in an inactive app does not open
//! `NSPopUpButton` menus (#16), so the Settings window is ordered front only
//! once the app is active.
//! A key nonactivating overlay can make `NSApplication::isActive` true while
//! another process is frontmost. Check `NSRunningApplication::isActive` too,
//! and request activation through that process object before showing Settings.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum SettingsPresentation {
    #[default]
    Idle,
    /// A default-mode run loop turn is scheduled. The run loop leaves the
    /// default mode while a menu is tracked, so the turn runs only after the
    /// status item menu that sent "Settings…" has been dismissed.
    AwaitingDefaultModeTurn,
    /// Activation was requested; the window waits for
    /// `applicationDidBecomeActive:`.
    AwaitingActivation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsPresentationStep {
    Wait,
    ScheduleDefaultModeTurn,
    RequestActivation,
    Present,
}

impl SettingsPresentation {
    /// Records a request to show the window. A request made while an earlier
    /// activation request is still unanswered schedules a new one.
    pub(super) fn request(&mut self) -> SettingsPresentationStep {
        match self {
            Self::AwaitingDefaultModeTurn => SettingsPresentationStep::Wait,
            Self::Idle | Self::AwaitingActivation => {
                *self = Self::AwaitingDefaultModeTurn;
                SettingsPresentationStep::ScheduleDefaultModeTurn
            }
        }
    }

    /// Runs on the scheduled default-mode turn: an active, frontmost app
    /// presents at once; otherwise request activation and keep the window hidden.
    pub(super) fn default_mode_turn(
        &mut self,
        app_is_active: bool,
        app_is_frontmost: bool,
    ) -> SettingsPresentationStep {
        match self {
            Self::AwaitingDefaultModeTurn if app_is_active && app_is_frontmost => {
                *self = Self::Idle;
                SettingsPresentationStep::Present
            }
            Self::AwaitingDefaultModeTurn => {
                *self = Self::AwaitingActivation;
                SettingsPresentationStep::RequestActivation
            }
            Self::Idle | Self::AwaitingActivation => SettingsPresentationStep::Wait,
        }
    }

    /// Runs on `applicationDidBecomeActive:`. Before the scheduled turn has
    /// run, the turn itself sees the active app and presents.
    pub(super) fn app_did_become_active(&mut self) -> SettingsPresentationStep {
        match self {
            Self::AwaitingActivation => {
                *self = Self::Idle;
                SettingsPresentationStep::Present
            }
            Self::Idle | Self::AwaitingDefaultModeTurn => SettingsPresentationStep::Wait,
        }
    }

    /// Drops a pending request once Settings is closed, so a later activation
    /// does not reopen it.
    pub(super) fn cancel(&mut self) {
        *self = Self::Idle;
    }

    pub(super) fn is_pending(self) -> bool {
        self != Self::Idle
    }

    pub(super) fn is_awaiting_activation(self) -> bool {
        self == Self::AwaitingActivation
    }
}

#[cfg(test)]
mod tests {
    use super::{SettingsPresentation, SettingsPresentationStep};

    #[test]
    fn request_defers_to_a_default_mode_turn_instead_of_presenting() {
        let mut presentation = SettingsPresentation::default();

        assert_eq!(
            presentation.request(),
            SettingsPresentationStep::ScheduleDefaultModeTurn
        );
        assert!(presentation.is_pending());
        assert!(!presentation.is_awaiting_activation());
    }

    #[test]
    fn repeated_request_before_the_turn_schedules_only_once() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();

        assert_eq!(presentation.request(), SettingsPresentationStep::Wait);
        assert_eq!(presentation, SettingsPresentation::AwaitingDefaultModeTurn);
    }

    #[test]
    fn inactive_app_requests_activation_and_does_not_present() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();

        assert_eq!(
            presentation.default_mode_turn(false, false),
            SettingsPresentationStep::RequestActivation
        );
        assert!(presentation.is_awaiting_activation());
    }

    #[test]
    fn inactive_app_presents_only_after_it_becomes_active() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();
        presentation.default_mode_turn(false, false);

        assert_eq!(
            presentation.app_did_become_active(),
            SettingsPresentationStep::Present
        );
        assert!(!presentation.is_pending());
    }

    #[test]
    fn already_active_app_presents_on_the_turn_without_requesting_activation() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();

        assert_eq!(
            presentation.default_mode_turn(true, true),
            SettingsPresentationStep::Present
        );
        assert!(!presentation.is_pending());
    }

    #[test]
    fn key_nonactivating_panel_waits_for_application_activation() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();

        assert_eq!(
            presentation.default_mode_turn(true, false),
            SettingsPresentationStep::RequestActivation
        );
        assert!(presentation.is_awaiting_activation());
        assert_eq!(
            presentation.default_mode_turn(true, false),
            SettingsPresentationStep::Wait
        );
        assert_eq!(
            presentation.app_did_become_active(),
            SettingsPresentationStep::Present
        );
        assert!(!presentation.is_pending());
    }

    #[test]
    fn activation_without_a_pending_request_presents_nothing() {
        let mut presentation = SettingsPresentation::default();

        assert_eq!(
            presentation.app_did_become_active(),
            SettingsPresentationStep::Wait
        );
        assert!(!presentation.is_pending());
    }

    #[test]
    fn activation_before_the_turn_leaves_presentation_to_the_turn() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();

        assert_eq!(
            presentation.app_did_become_active(),
            SettingsPresentationStep::Wait
        );
        assert_eq!(
            presentation.default_mode_turn(true, true),
            SettingsPresentationStep::Present
        );
    }

    #[test]
    fn turn_without_a_pending_request_does_nothing() {
        let mut presentation = SettingsPresentation::default();

        assert_eq!(
            presentation.default_mode_turn(true, true),
            SettingsPresentationStep::Wait
        );
        assert_eq!(
            presentation.default_mode_turn(false, false),
            SettingsPresentationStep::Wait
        );
        assert!(!presentation.is_pending());
    }

    #[test]
    fn presentation_happens_once_per_request() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();
        presentation.default_mode_turn(false, false);
        presentation.app_did_become_active();

        assert_eq!(
            presentation.app_did_become_active(),
            SettingsPresentationStep::Wait
        );
    }

    #[test]
    fn closing_settings_while_awaiting_activation_drops_the_request() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();
        presentation.default_mode_turn(false, false);

        presentation.cancel();

        assert!(!presentation.is_pending());
        assert_eq!(
            presentation.app_did_become_active(),
            SettingsPresentationStep::Wait
        );
    }

    #[test]
    fn closing_settings_before_the_turn_makes_the_turn_do_nothing() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();

        presentation.cancel();

        assert!(!presentation.is_pending());
        assert_eq!(
            presentation.default_mode_turn(false, false),
            SettingsPresentationStep::Wait
        );
    }

    #[test]
    fn request_while_awaiting_activation_schedules_a_new_activation_request() {
        let mut presentation = SettingsPresentation::default();
        presentation.request();
        presentation.default_mode_turn(false, false);

        assert_eq!(
            presentation.request(),
            SettingsPresentationStep::ScheduleDefaultModeTurn
        );
        assert_eq!(
            presentation.default_mode_turn(false, false),
            SettingsPresentationStep::RequestActivation
        );
    }
}
