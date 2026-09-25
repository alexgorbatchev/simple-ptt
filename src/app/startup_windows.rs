//! Which windows open at launch.
//!
//! The permissions dialog and the startup alerts open right away. Settings is
//! ordered front only once the app is active (see `settings_presentation`),
//! so when it opens after the permissions dialog, the dialog is brought back
//! in front of it, as it was when Settings opened first.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct StartupWindows {
    pub(super) settings: bool,
    pub(super) permissions_dialog: bool,
    pub(super) missing_config_alert: bool,
    pub(super) audio_failure_alert: bool,
}

impl StartupWindows {
    pub(super) fn plan(
        config_file_missing: bool,
        deepgram_api_key_missing: bool,
        audio_startup_failed: bool,
        hotkey_permissions_missing: bool,
    ) -> Self {
        Self {
            settings: config_file_missing || deepgram_api_key_missing || audio_startup_failed,
            permissions_dialog: hotkey_permissions_missing,
            missing_config_alert: config_file_missing && !deepgram_api_key_missing,
            audio_failure_alert: audio_startup_failed,
        }
    }

    /// Whether the permissions dialog is brought back in front once the
    /// startup Settings window has been presented.
    pub(super) fn permissions_dialog_returns_in_front_of_settings(self) -> bool {
        self.settings && self.permissions_dialog
    }
}

#[cfg(test)]
mod tests {
    use super::StartupWindows;

    #[test]
    fn complete_setup_opens_nothing() {
        let windows = StartupWindows::plan(false, false, false, false);

        assert_eq!(windows, StartupWindows::default());
        assert!(!windows.permissions_dialog_returns_in_front_of_settings());
    }

    #[test]
    fn missing_permissions_alone_open_only_the_permissions_dialog() {
        let windows = StartupWindows::plan(false, false, false, true);

        assert_eq!(
            windows,
            StartupWindows {
                permissions_dialog: true,
                ..StartupWindows::default()
            }
        );
        assert!(!windows.permissions_dialog_returns_in_front_of_settings());
    }

    #[test]
    fn first_launch_keeps_the_permissions_dialog_in_front_of_settings() {
        let windows = StartupWindows::plan(true, true, false, true);

        assert_eq!(
            windows,
            StartupWindows {
                settings: true,
                permissions_dialog: true,
                ..StartupWindows::default()
            }
        );
        assert!(windows.permissions_dialog_returns_in_front_of_settings());
    }

    #[test]
    fn missing_config_with_an_api_key_opens_settings_and_the_missing_config_alert() {
        let windows = StartupWindows::plan(true, false, false, false);

        assert_eq!(
            windows,
            StartupWindows {
                settings: true,
                missing_config_alert: true,
                ..StartupWindows::default()
            }
        );
        assert!(!windows.permissions_dialog_returns_in_front_of_settings());
    }

    #[test]
    fn missing_api_key_opens_settings_without_an_alert() {
        assert_eq!(
            StartupWindows::plan(false, true, false, false),
            StartupWindows {
                settings: true,
                ..StartupWindows::default()
            }
        );
    }

    #[test]
    fn audio_failure_opens_settings_and_the_audio_failure_alert() {
        assert_eq!(
            StartupWindows::plan(false, false, true, false),
            StartupWindows {
                settings: true,
                audio_failure_alert: true,
                ..StartupWindows::default()
            }
        );
    }

    #[test]
    fn every_startup_problem_opens_every_window() {
        let windows = StartupWindows::plan(true, false, true, true);

        assert_eq!(
            windows,
            StartupWindows {
                settings: true,
                permissions_dialog: true,
                missing_config_alert: true,
                audio_failure_alert: true,
            }
        );
        assert!(windows.permissions_dialog_returns_in_front_of_settings());
    }
}
