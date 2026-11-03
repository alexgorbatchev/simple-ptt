use objc2::runtime::Sel;
use objc2::sel;

/// Defines the application-menu and permission-dialog actions together with
/// their selector-contract test inventory.
macro_rules! application_actions {
    ($($variant:ident => $selector:ident),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum ApplicationAction {
            $($variant,)+
        }

        impl ApplicationAction {
            #[cfg(test)]
            pub(crate) const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub(crate) fn selector(self) -> Sel {
                match self {
                    $(Self::$variant => sel!($selector:),)+
                }
            }
        }
    };
}

application_actions! {
    OpenGitHubRepo => openGitHubRepo,
    OpenSettings => openSettings,
    OpenPermissions => openPermissions,
    RequestAccessibility => requestAccessibilityPermission,
    RequestMicrophone => requestMicrophonePermission,
    ResetPermissions => resetHotkeyPermissions,
    RecheckPermissions => recheckHotkeyPermissions,
    CompletePermissions => quitFromPermissionsDialog,
}
