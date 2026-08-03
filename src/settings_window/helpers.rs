use objc2_app_kit::{NSComboBox, NSPopUpButton, NSTextField};

use crate::audio::AvailableAudioInputDevices;
use crate::config::UiMeterStyle;

pub const SYSTEM_DEFAULT_FONT_LABEL: &str = "System default";
pub const SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL: &str = "System default";
pub const TRANSFORMATION_PROVIDER_DISABLED_LABEL: &str = "Disabled";

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

pub fn mic_audio_device_popup_state(
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

pub fn read_required_string(field: &NSTextField, field_name: &str) -> Result<String, String> {
    let value = field.stringValue().to_string();
    let trimmed_value = value.trim();
    if trimmed_value.is_empty() {
        return Err(format!("{} is required", field_name));
    }
    Ok(trimmed_value.to_owned())
}

pub fn read_optional_string(field: &NSTextField) -> Option<String> {
    let value = field.stringValue().to_string();
    let trimmed_value = value.trim();
    if trimmed_value.is_empty() {
        None
    } else {
        Some(trimmed_value.to_owned())
    }
}

pub fn read_optional_pop_up_button_string(popup_button: &NSPopUpButton) -> Option<String> {
    let selected_title = popup_button.titleOfSelectedItem()?.to_string();
    let trimmed_value = selected_title.trim();
    if trimmed_value.is_empty() || trimmed_value == SYSTEM_DEFAULT_FONT_LABEL {
        None
    } else {
        Some(trimmed_value.to_owned())
    }
}

pub fn read_optional_provider_pop_up_button_string(popup_button: &NSPopUpButton) -> Option<String> {
    let selected_title = popup_button.titleOfSelectedItem()?.to_string();
    let trimmed_value = selected_title.trim();
    if trimmed_value.is_empty() || trimmed_value == TRANSFORMATION_PROVIDER_DISABLED_LABEL {
        None
    } else {
        Some(trimmed_value.to_owned())
    }
}

pub fn read_required_combo_box_string(
    combo_box: &NSComboBox,
    field_name: &str,
) -> Result<String, String> {
    let value = combo_box.stringValue().to_string();
    let trimmed_value = value.trim();
    if trimmed_value.is_empty() {
        Err(format!("{} is required", field_name))
    } else {
        Ok(trimmed_value.to_owned())
    }
}

pub fn read_required_pop_up_button_string(
    popup_button: &NSPopUpButton,
    field_name: &str,
) -> Result<String, String> {
    let selected_title = popup_button
        .titleOfSelectedItem()
        .ok_or_else(|| format!("{} is required", field_name))?
        .to_string();
    let trimmed_value = selected_title.trim();
    if trimmed_value.is_empty() {
        Err(format!("{} is required", field_name))
    } else {
        Ok(trimmed_value.to_owned())
    }
}

pub fn read_required_f64(field: &NSTextField, field_name: &str) -> Result<f64, String> {
    read_required_string(field, field_name)?
        .parse::<f64>()
        .map_err(|error| format!("{} must be a number: {}", field_name, error))
}

pub fn read_optional_f64(field: &NSTextField, field_name: &str) -> Result<Option<f64>, String> {
    match read_optional_string(field) {
        Some(value) => value
            .parse::<f64>()
            .map(Some)
            .map_err(|error| format!("{} must be a number: {}", field_name, error)),
        None => Ok(None),
    }
}

pub fn read_required_u32(field: &NSTextField, field_name: &str) -> Result<u32, String> {
    read_required_string(field, field_name)?
        .parse::<u32>()
        .map_err(|error| format!("{} must be an unsigned integer: {}", field_name, error))
}

pub fn read_required_u64(field: &NSTextField, field_name: &str) -> Result<u64, String> {
    read_required_string(field, field_name)?
        .parse::<u64>()
        .map_err(|error| format!("{} must be an unsigned integer: {}", field_name, error))
}

pub fn read_required_u16(field: &NSTextField, field_name: &str) -> Result<u16, String> {
    read_required_string(field, field_name)?
        .parse::<u16>()
        .map_err(|error| format!("{} must be an unsigned integer: {}", field_name, error))
}

pub fn parse_meter_style(raw_value: &str) -> Result<UiMeterStyle, String> {
    match raw_value.trim() {
        "none" => Ok(UiMeterStyle::None),
        "animated-height" => Ok(UiMeterStyle::AnimatedHeight),
        "animated-color" => Ok(UiMeterStyle::AnimatedColor),
        other_value => Err(format!(
            "Meter style must be one of: animated-color, animated-height, none (got '{}')",
            other_value
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        default_audio_device_title, find_mic_audio_device_option_title,
        is_system_default_audio_device_value, mic_audio_device_popup_state, MicAudioDeviceOption,
        SYSTEM_DEFAULT_AUDIO_DEVICE_LABEL,
    };
    use crate::audio::{AudioInputDeviceChoice, AvailableAudioInputDevices};

    #[test]
    fn default_audio_device_title_includes_detected_default_name() {
        assert_eq!(
            default_audio_device_title(Some("MacBook Pro Microphone")),
            "System default (MacBook Pro Microphone)"
        );
    }

    #[test]
    fn mic_audio_device_popup_state_selects_default_when_unconfigured() {
        let (options, selected_title) = mic_audio_device_popup_state(
            AvailableAudioInputDevices {
                default_device_name: Some("MacBook Pro Microphone".to_owned()),
                choices: vec![AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                }],
            },
            None,
        );

        assert_eq!(
            options,
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
        assert_eq!(selected_title, "System default (MacBook Pro Microphone)");
    }

    #[test]
    fn mic_audio_device_popup_state_matches_configured_name_case_insensitively() {
        let (options, selected_title) = mic_audio_device_popup_state(
            AvailableAudioInputDevices {
                default_device_name: None,
                choices: vec![AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                }],
            },
            Some("shure mv7"),
        );

        assert_eq!(
            options,
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
        assert_eq!(selected_title, "Shure MV7");
    }

    #[test]
    fn mic_audio_device_popup_state_inserts_missing_configured_value() {
        let (options, selected_title) = mic_audio_device_popup_state(
            AvailableAudioInputDevices {
                default_device_name: None,
                choices: vec![AudioInputDeviceChoice {
                    label: "Shure MV7".to_owned(),
                    value: "Shure MV7".to_owned(),
                }],
            },
            Some("Missing Mic"),
        );

        assert_eq!(
            options,
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
        assert_eq!(selected_title, "Missing Mic");
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
