use objc2::runtime::Sel;
use objc2::sel;

/// Generates `SettingsAction`, its `selector()` mapping, and the test-only
/// `ALL` list from one table, so a variant cannot be missing from `ALL`.
macro_rules! settings_actions {
    ($($variant:ident => $selector:ident),+ $(,)?) => {
        /// Every target/action message the settings window sends to its target
        /// (the `AppDelegate`). This is the single source of the settings action
        /// selectors: the window builders take a `SettingsAction`, and a test
        /// asserts that the delegate class implements each selector, because
        /// AppKit silently drops an action whose target does not respond to it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum SettingsAction {
            $($variant,)+
        }

        impl SettingsAction {
            // Only the selector-contract tests enumerate the actions.
            #[cfg(test)]
            pub const ALL: &'static [SettingsAction] = &[$(SettingsAction::$variant,)+];

            pub fn selector(self) -> Sel {
                match self {
                    $(SettingsAction::$variant => sel!($selector:),)+
                }
            }
        }
    };
}

settings_actions! {
    CaptureRecordHotkey => captureRecordHotkey,
    CaptureCorrectionKey => captureCorrectionKey,
    CaptureTransformHotkey => captureTransformHotkey,
    MicAudioDeviceChanged => micAudioDeviceChanged,
    MicGainSliderChanged => micGainSliderChanged,
    CheckDeepgramConnection => checkDeepgramConnection,
    TransformationProviderChanged => transformationProviderChanged,
    RefreshTransformationModels => refreshTransformationModels,
    CheckTransformationProvider => checkTransformationProvider,
    ResetDictationPrompt => resetDictationPrompt,
    ResetCorrectionPrompt => resetCorrectionPrompt,
    SaveSettings => applySettingsPressed,
    CancelSettings => cancelSettingsPressed,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::SettingsAction;

    #[test]
    fn settings_actions_have_distinct_selectors() {
        let selectors: HashSet<_> = SettingsAction::ALL
            .iter()
            .map(|action| action.selector())
            .collect();

        assert_eq!(selectors.len(), SettingsAction::ALL.len());
    }
}
