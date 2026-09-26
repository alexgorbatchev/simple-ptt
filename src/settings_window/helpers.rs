use crate::audio::AvailableAudioInputDevices;
use crate::settings_window::FETCH_MODELS_BUTTON_TITLE;
use crate::transformation_models::ManualFetchReason;

pub const SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL: &str = "System default";

// Source: Deepgram model docs and live streaming docs.
// - https://developers.deepgram.com/docs/model
// - https://developers.deepgram.com/docs/live-streaming-audio
pub const DEEPGRAM_MODEL_OPTIONS: &[&str] = &[
    "flux",
    "nova-3",
    "nova-3-general",
    "nova-3-medical",
    "nova-2-general",
    "nova-2-meeting",
    "nova-2-phonecall",
    "nova-2-voicemail",
    "nova-2-finance",
    "nova-2-conversationalai",
    "nova-2-video",
    "nova-2-medical",
    "nova-2-drivethru",
    "nova-2-automotive",
    "nova-2-atc",
    "nova-general",
    "nova-phonecall",
    "enhanced-general",
    "enhanced-meeting",
    "enhanced-phonecall",
    "enhanced-finance",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MicAudioDeviceOption {
    pub title: String,
    pub value: Option<String>,
}

pub fn default_audio_device_title(default_device_name: Option<&str>) -> String {
    match default_device_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(default_device_name) => {
            format!(
                "{} ({})",
                SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL, default_device_name
            )
        }
        None => SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL.to_owned(),
    }
}

pub fn is_system_default_audio_device_value(value: &str) -> bool {
    let trimmed_value = value.trim();
    trimmed_value == SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL
        || trimmed_value
            .strip_prefix(SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL)
            .map(|suffix| suffix.starts_with(" (") && suffix.ends_with(')'))
            .unwrap_or(false)
}

/// Items and selection of the audio device popup, with the status message to
/// show when the audio input devices could not be listed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MicAudioDevicePopupState {
    pub options: Vec<MicAudioDeviceOption>,
    pub selected_title: String,
    pub load_problem: Option<String>,
}

/// Popup state for the listed audio input devices and the configured device.
/// When the devices cannot be listed (#10), the popup offers only the system
/// default and the configured device. The configured device stays selected,
/// so saving keeps `mic.audio_device`, and the error becomes `load_problem`.
pub fn mic_audio_device_popup_state(
    available_audio_input_devices: Result<AvailableAudioInputDevices, String>,
    configured_audio_device: Option<&str>,
) -> MicAudioDevicePopupState {
    let (available_audio_input_devices, load_problem) = match available_audio_input_devices {
        Ok(available_audio_input_devices) => (available_audio_input_devices, None),
        Err(error) => (
            AvailableAudioInputDevices {
                default_device_name: None,
                choices: Vec::new(),
            },
            Some(format!("Microphone: {}", error)),
        ),
    };
    let (options, selected_title) =
        audio_device_options(available_audio_input_devices, configured_audio_device);
    MicAudioDevicePopupState {
        options,
        selected_title,
        load_problem,
    }
}

fn audio_device_options(
    available_audio_input_devices: AvailableAudioInputDevices,
    configured_audio_device: Option<&str>,
) -> (Vec<MicAudioDeviceOption>, String) {
    let default_audio_device_title =
        default_audio_device_title(available_audio_input_devices.default_device_name.as_deref());
    let mut audio_device_options = vec![MicAudioDeviceOption {
        title: default_audio_device_title.clone(),
        value: None,
    }];
    audio_device_options.extend(
        available_audio_input_devices
            .choices
            .into_iter()
            .map(|choice| MicAudioDeviceOption {
                title: choice.label,
                value: Some(choice.value),
            }),
    );

    let Some(configured_audio_device) = configured_audio_device
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return (audio_device_options, default_audio_device_title);
    };

    if let Some(selected_audio_device_title) =
        find_mic_audio_device_option_title(&audio_device_options, configured_audio_device)
            .map(str::to_owned)
    {
        return (audio_device_options, selected_audio_device_title);
    }

    audio_device_options.insert(
        1,
        MicAudioDeviceOption {
            title: configured_audio_device.to_owned(),
            value: Some(configured_audio_device.to_owned()),
        },
    );
    (audio_device_options, configured_audio_device.to_owned())
}

pub fn find_mic_audio_device_option_title<'a>(
    audio_device_options: &'a [MicAudioDeviceOption],
    configured_audio_device: &str,
) -> Option<&'a str> {
    if is_system_default_audio_device_value(configured_audio_device) {
        return audio_device_options
            .first()
            .map(|option| option.title.as_str());
    }

    audio_device_options
        .iter()
        .find(|option| {
            option.title == configured_audio_device
                || option.value.as_deref() == Some(configured_audio_device)
                || option
                    .value
                    .as_deref()
                    .map(|value| value.eq_ignore_ascii_case(configured_audio_device))
                    .unwrap_or(false)
        })
        .map(|option| option.title.as_str())
}

pub fn environment_hint_message(variable_name: &str) -> String {
    format!("Using ${} from environment.", variable_name)
}

/// Status text after the settings window loads a config, one message per
/// line: the caller's `status_message`, each problem the panes found while
/// loading, then `transformation_status` from syncing the transformation
/// model list. The status area shows the last message set, so every message
/// from opening the window goes through here at once.
pub fn settings_load_status(
    status_message: Option<&str>,
    load_problems: &[String],
    transformation_status: Option<&str>,
) -> String {
    status_message
        .into_iter()
        .chain(load_problems.iter().map(String::as_str))
        .chain(transformation_status)
        .filter(|message| !message.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Status for a transformation provider whose models are not cached and wait
/// for the user to click the Fetch models button.
pub fn manual_model_fetch_message(provider: &str, reason: ManualFetchReason) -> String {
    match reason {
        ManualFetchReason::NotCached => format!(
            "No cached models for {}. Click {} to load them.",
            provider, FETCH_MODELS_BUTTON_TITLE
        ),
        ManualFetchReason::UnsavedApiKey => format!(
            "No cached models for {0}. The API key field may hold another provider's key, so \
             they were not fetched. Enter the {0} API key or clear the field, then click {1}.",
            provider, FETCH_MODELS_BUTTON_TITLE
        ),
    }
}

/// Status for a model cache file that could not be read or parsed. Fetch
/// models rewrites the file, so the status leads with that; the error, which
/// can span several lines (a TOML parse error does), follows it and stays
/// readable in the status tooltip.
pub fn unreadable_model_cache_message(error: &str) -> String {
    format!(
        "The model cache could not be read. Click {} to rebuild it.\n{}",
        FETCH_MODELS_BUTTON_TITLE,
        error.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::{
        default_audio_device_title, find_mic_audio_device_option_title,
        is_system_default_audio_device_value, manual_model_fetch_message,
        mic_audio_device_popup_state, settings_load_status, unreadable_model_cache_message,
        MicAudioDeviceOption, SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL,
    };
    use crate::audio::{AudioInputDeviceChoice, AvailableAudioInputDevices};
    use crate::settings_window::FETCH_MODELS_BUTTON_TITLE;
    use crate::transformation_models::ManualFetchReason;

    #[test]
    fn manual_model_fetch_messages_name_the_fetch_models_button() {
        assert_eq!(
            manual_model_fetch_message("openai", ManualFetchReason::NotCached),
            format!("No cached models for openai. Click {FETCH_MODELS_BUTTON_TITLE} to load them.")
        );
        assert_eq!(
            manual_model_fetch_message("anthropic", ManualFetchReason::UnsavedApiKey),
            format!(
                "No cached models for anthropic. The API key field may hold another provider's \
                 key, so they were not fetched. Enter the anthropic API key or clear the field, \
                 then click {FETCH_MODELS_BUTTON_TITLE}."
            )
        );
        let parse_error = toml::from_str::<toml::Value>("version = [")
            .unwrap_err()
            .to_string();
        let message = unreadable_model_cache_message(&parse_error);
        assert_eq!(
            message.lines().next(),
            Some(
                format!(
                    "The model cache could not be read. Click {FETCH_MODELS_BUTTON_TITLE} to \
                     rebuild it."
                )
                .as_str()
            )
        );
        assert!(parse_error.lines().count() > 1);
        assert!(message.ends_with(parse_error.trim_end()));
        assert_eq!(FETCH_MODELS_BUTTON_TITLE, "Fetch models");
    }

    const ENUMERATION_ERROR: &str = "failed to enumerate audio input devices: no host";

    #[test]
    fn failed_device_listing_keeps_the_configured_device_selected_and_reports_the_error() {
        let state =
            mic_audio_device_popup_state(Err(ENUMERATION_ERROR.to_owned()), Some("Shure MV7"));

        assert_eq!(
            state.options,
            vec![
                MicAudioDeviceOption {
                    title: SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL.to_owned(),
                    value: None,
                },
                MicAudioDeviceOption {
                    title: "Shure MV7".to_owned(),
                    value: Some("Shure MV7".to_owned()),
                },
            ]
        );
        assert_eq!(state.selected_title, "Shure MV7");
        assert_eq!(
            state.load_problem.as_deref(),
            Some("Microphone: failed to enumerate audio input devices: no host")
        );
    }

    #[test]
    fn failed_device_listing_keeps_the_system_default_selected_when_unconfigured() {
        for configured_audio_device in [None, Some("System default (MacBook Pro Microphone)")] {
            let state = mic_audio_device_popup_state(
                Err(ENUMERATION_ERROR.to_owned()),
                configured_audio_device,
            );

            assert_eq!(
                state.options,
                vec![MicAudioDeviceOption {
                    title: SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL.to_owned(),
                    value: None,
                }]
            );
            assert_eq!(state.selected_title, SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL);
            assert_eq!(
                state.load_problem.as_deref(),
                Some("Microphone: failed to enumerate audio input devices: no host")
            );
        }
    }

    #[test]
    fn settings_load_status_is_empty_without_a_message_or_problems() {
        assert_eq!(settings_load_status(None, &[], None), "");
    }

    #[test]
    fn settings_load_status_keeps_the_callers_message() {
        assert_eq!(
            settings_load_status(Some("Audio failed to start."), &[], None),
            "Audio failed to start."
        );
    }

    #[test]
    fn settings_load_status_shows_every_load_problem() {
        let load_problems = [
            "Microphone: failed to enumerate audio input devices: no host".to_owned(),
            "Microphone: gain is out of range".to_owned(),
        ];

        assert_eq!(
            settings_load_status(None, &load_problems, None),
            "Microphone: failed to enumerate audio input devices: no host\n\
             Microphone: gain is out of range"
        );
    }

    #[test]
    fn settings_load_status_shows_the_callers_message_before_load_problems() {
        let load_problems =
            ["Microphone: failed to enumerate audio input devices: no host".to_owned()];

        assert_eq!(
            settings_load_status(Some("Audio failed to start."), &load_problems, None),
            "Audio failed to start.\n\
             Microphone: failed to enumerate audio input devices: no host"
        );
    }

    #[test]
    fn settings_load_status_shows_the_transformation_status_after_load_problems() {
        let load_problems =
            ["Microphone: failed to enumerate audio input devices: no host".to_owned()];

        assert_eq!(
            settings_load_status(
                Some("Audio failed to start."),
                &load_problems,
                Some("Loaded 3 cached models for openai."),
            ),
            "Audio failed to start.\n\
             Microphone: failed to enumerate audio input devices: no host\n\
             Loaded 3 cached models for openai."
        );
    }

    #[test]
    fn settings_load_status_skips_an_empty_transformation_status() {
        let load_problems =
            ["Microphone: failed to enumerate audio input devices: no host".to_owned()];

        assert_eq!(
            settings_load_status(None, &load_problems, Some("")),
            "Microphone: failed to enumerate audio input devices: no host"
        );
    }

    #[test]
    fn default_audio_device_title_includes_detected_default_name() {
        assert_eq!(
            default_audio_device_title(Some("MacBook Pro Microphone")),
            "System default (MacBook Pro Microphone)"
        );
    }

    #[test]
    fn mic_audio_device_popup_state_selects_default_when_unconfigured() {
        let state = mic_audio_device_popup_state(
            Ok(AvailableAudioInputDevices {
                default_device_name: Some("MacBook Pro Microphone".to_owned()),
                choices: vec![AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                }],
            }),
            None,
        );

        assert_eq!(
            state.options,
            vec![
                MicAudioDeviceOption {
                    title: "System default (MacBook Pro Microphone)".to_owned(),
                    value: None,
                },
                MicAudioDeviceOption {
                    title: "Shure MV7".to_owned(),
                    value: Some("Shure MV7".to_owned()),
                },
            ]
        );
        assert_eq!(state.load_problem, None);
        assert_eq!(
            state.selected_title,
            "System default (MacBook Pro Microphone)"
        );
    }

    #[test]
    fn mic_audio_device_popup_state_matches_configured_name_case_insensitively() {
        let state = mic_audio_device_popup_state(
            Ok(AvailableAudioInputDevices {
                default_device_name: None,
                choices: vec![AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                }],
            }),
            Some("shure mv7"),
        );

        assert_eq!(
            state.options,
            vec![
                MicAudioDeviceOption {
                    title: SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL.to_owned(),
                    value: None,
                },
                MicAudioDeviceOption {
                    title: "Shure MV7".to_owned(),
                    value: Some("Shure MV7".to_owned()),
                },
            ]
        );
        assert_eq!(state.load_problem, None);
        assert_eq!(state.selected_title, "Shure MV7");
    }

    #[test]
    fn mic_audio_device_popup_state_inserts_missing_configured_value() {
        let state = mic_audio_device_popup_state(
            Ok(AvailableAudioInputDevices {
                default_device_name: None,
                choices: vec![AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                }],
            }),
            Some("Missing Mic"),
        );

        assert_eq!(
            state.options,
            vec![
                MicAudioDeviceOption {
                    title: SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL.to_owned(),
                    value: None,
                },
                MicAudioDeviceOption {
                    title: "Missing Mic".to_owned(),
                    value: Some("Missing Mic".to_owned()),
                },
                MicAudioDeviceOption {
                    title: "Shure MV7".to_owned(),
                    value: Some("Shure MV7".to_owned()),
                },
            ]
        );
        assert_eq!(state.load_problem, None);
        assert_eq!(state.selected_title, "Missing Mic");
    }

    #[test]
    fn system_default_audio_device_value_is_recognized_from_decorated_title() {
        assert!(is_system_default_audio_device_value(
            "System default (MacBook Pro Microphone)"
        ));
        assert!(is_system_default_audio_device_value("System default"));
        assert!(!is_system_default_audio_device_value("Shure MV7"));
    }

    #[test]
    fn popup_state_maps_legacy_decorated_default_value_back_to_default_option() {
        let options = vec![
            MicAudioDeviceOption {
                title: "System default (MacBook Pro Microphone)".to_owned(),
                value: None,
            },
            MicAudioDeviceOption {
                title: "Shure MV7".to_owned(),
                value: Some("Shure MV7".to_owned()),
            },
        ];

        assert_eq!(
            find_mic_audio_device_option_title(&options, "System default (MacBook Pro Microphone)"),
            Some("System default (MacBook Pro Microphone)")
        );
    }
}
