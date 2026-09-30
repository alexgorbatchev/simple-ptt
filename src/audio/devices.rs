use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{FromSample, Host};

use super::input_choice::{
    built_in_microphone_uids, choose_input, device_uid, InputAvailability, InputChoice,
    NO_MICROPHONE_MESSAGE,
};
use crate::state::MIC_SILENCE_DB;

const UNKNOWN_AUDIO_INPUT_DEVICE_LABEL: &str = "<unknown>";
/// The fixed meters (the Settings microphone meter and the overlay meter
/// styles other than the pills) map this range of RMS levels, in dBFS, from
/// empty to full: soft speech (about -45 to -35 dBFS) shows, and a quiet room
/// (about -58) barely lifts them. The Settings meter is for setting the gain,
/// so it keeps this absolute scale rather than adapting.
const METER_MIN_DB: f32 = -60.0;
const METER_MAX_DB: f32 = -6.0;
const CLIP_DETECTION_THRESHOLD: f32 = 0.99;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioInputDeviceChoice {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableAudioInputDevices {
    pub default_device_name: Option<String>,
    pub choices: Vec<AudioInputDeviceChoice>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputDeviceDescriptor {
    pub index: usize,
    pub name: Option<String>,
}

/// Human-readable device name. On CoreAudio this is `kAudioDevicePropertyDeviceNameCFString`,
/// the same value `mic.audio_device` has always been matched against.
pub fn device_name(device: &cpal::Device) -> Option<String> {
    device
        .description()
        .ok()
        .map(|description| description.name().to_owned())
}

pub fn available_audio_input_devices() -> Result<AvailableAudioInputDevices, String> {
    let host = cpal::default_host();
    let default_device_name = host
        .default_input_device()
        .and_then(|device| device_name(&device));
    let devices = enumerate_input_devices(&host)?;

    Ok(AvailableAudioInputDevices {
        default_device_name,
        choices: build_audio_input_device_choices(&devices),
    })
}

pub fn print_input_devices() -> Result<(), String> {
    let host = cpal::default_host();
    let default_device_name = host
        .default_input_device()
        .and_then(|device| device_name(&device));
    let devices = enumerate_input_devices(&host)?;

    match default_device_name {
        Some(default_device_name) => println!("Default input device: {}", default_device_name),
        None => println!("Default input device: <none>"),
    }

    if devices.is_empty() {
        println!("Available input devices: <none>");
        return Ok(());
    }

    println!("Available input devices:");
    for device in devices {
        println!(
            "  {}: {}",
            device.index + 1,
            device.name.as_deref().unwrap_or(UNKNOWN_AUDIO_INPUT_DEVICE_LABEL)
        );
    }
    Ok(())
}

pub fn enumerate_input_devices(host: &Host) -> Result<Vec<InputDeviceDescriptor>, String> {
    let devices = host
        .input_devices()
        .map_err(|error| format!("failed to enumerate audio input devices: {}", error))?;

    Ok(devices
        .enumerate()
        .map(|(index, device)| InputDeviceDescriptor {
            index,
            name: device_name(&device),
        })
        .collect())
}

pub fn build_audio_input_device_choices(
    devices: &[InputDeviceDescriptor],
) -> Vec<AudioInputDeviceChoice> {
    let mut name_counts = std::collections::HashMap::new();
    for device in devices {
        if let Some(name) = normalized_configured_audio_device(device.name.as_deref()) {
            *name_counts.entry(name.to_owned()).or_insert(0usize) += 1;
        }
    }

    devices
        .iter()
        .map(|device| {
            let label = match device.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(name) => {
                    if name_counts.get(name).copied().unwrap_or(0) > 1 {
                        format!("{} (#{})", name, device.index + 1)
                    } else {
                        name.to_owned()
                    }
                }
                None => format!("{} (#{})", UNKNOWN_AUDIO_INPUT_DEVICE_LABEL, device.index + 1),
            };

            let value = device.index.to_string();

            AudioInputDeviceChoice { label, value }
        })
        .collect()
}

/// The input device to record from (see `input_choice::choose_input`): the
/// device `configured_value` names while it is connected, else the built-in
/// microphone or the system default; with `prefer_built_in` (the device in
/// use disappeared) the built-in microphone before the system default.
pub fn resolve_input_device(
    host: &Host,
    configured_value: Option<&str>,
    prefer_built_in: bool,
) -> Result<cpal::Device, String> {
    let devices = host
        .input_devices()
        .map_err(|error| format!("failed to enumerate audio input devices: {}", error))?
        .collect::<Vec<_>>();
    let configured = normalized_configured_audio_device(configured_value)
        .map(|configured| find_configured_device(&devices, configured));
    let built_in_uids = built_in_microphone_uids();
    let built_in = devices
        .iter()
        .find(|device| device_uid(device).is_some_and(|uid| built_in_uids.contains(&uid)))
        .cloned();
    let system_default = host.default_input_device();

    let available = InputAvailability {
        configured: configured.as_ref().map(Option::is_some),
        built_in: built_in.is_some(),
        system_default: system_default.is_some(),
    };
    let choice = choose_input(available, prefer_built_in);
    log::info!("choosing audio input {:?} from {:?}", choice, available);
    match choice {
        Some(InputChoice::Configured) => configured.flatten(),
        Some(InputChoice::BuiltIn) => built_in,
        Some(InputChoice::SystemDefault) => system_default,
        None => None,
    }
    .ok_or_else(|| NO_MICROPHONE_MESSAGE.to_owned())
}

/// The device `configured` names: by its index in `devices`, or by its name.
fn find_configured_device(devices: &[cpal::Device], configured: &str) -> Option<cpal::Device> {
    if let Some(device) = configured.parse::<usize>().ok().and_then(|index| devices.get(index)) {
        return Some(device.clone());
    }
    devices
        .iter()
        .find(|device| {
            device_name(device)
                .map(|name| name.eq_ignore_ascii_case(configured))
                .unwrap_or(false)
        })
        .cloned()
}

pub fn is_system_default_audio_device_value(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty()
        || trimmed == "System default"
        || trimmed.starts_with("System default (")
}

pub fn normalized_configured_audio_device(raw_value: Option<&str>) -> Option<&str> {
    let trimmed = raw_value.map(str::trim).unwrap_or("");
    if is_system_default_audio_device_value(trimmed) {
        None
    } else {
        Some(trimmed)
    }
}

pub fn encode_pcm_mono<T>(data: &[T], channels: usize, gain_linear: f32) -> (bytes::Bytes, usize, f32, f32)
where
    T: cpal::Sample + cpal::SizedSample,
    f32: FromSample<T>,
{
    let frame_count = data.len() / channels;
    let mut pcm_bytes = bytes::BytesMut::with_capacity(frame_count * 2);
    let mut clipped_sample_count = 0usize;
    let mut max_abs = 0.0f32;
    let mut sum_sq = 0.0f32;

    for frame in data.chunks_exact(channels) {
        let sample_f32 = f32::from_sample_(frame[0]) * gain_linear;
        let abs_sample = sample_f32.abs();
        if abs_sample > max_abs {
            max_abs = abs_sample;
        }
        sum_sq += abs_sample * abs_sample;

        if abs_sample >= CLIP_DETECTION_THRESHOLD {
            clipped_sample_count += 1;
        }

        let clamped = sample_f32.clamp(-1.0, 1.0);
        let sample_i16 = (clamped * 32767.0) as i16;
        pcm_bytes.extend_from_slice(&sample_i16.to_le_bytes());
    }

    let rms = if frame_count > 0 {
        (sum_sq / frame_count as f32).sqrt()
    } else {
        0.0
    };

    let level_db = if rms > 1e-6 {
        20.0 * rms.log10()
    } else {
        MIC_SILENCE_DB
    };

    let peak_db = if max_abs > 1e-6 {
        20.0 * max_abs.log10()
    } else {
        MIC_SILENCE_DB
    };

    (pcm_bytes.freeze(), clipped_sample_count, level_db, peak_db)
}

pub fn normalize_meter_amplitude(db: f32) -> f32 {
    if db <= METER_MIN_DB {
        0.0
    } else if db >= METER_MAX_DB {
        1.0
    } else {
        (db - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB)
    }
}

/// The next smoothed level in dBFS for `level_db`: the level itself when
/// metering starts (`previous` is `None`), so a meter that adapts to the
/// room never sees a ramp up from silence, then `smooth_meter_value`.
pub fn smooth_meter_level_db(previous: Option<f32>, level_db: f32) -> f32 {
    previous.map_or(level_db, |current| smooth_meter_value(current, level_db))
}

pub fn smooth_meter_value(current: f32, target: f32) -> f32 {
    if target > current {
        current + (target - current) * 0.4
    } else {
        current + (target - current) * 0.15
    }
}
