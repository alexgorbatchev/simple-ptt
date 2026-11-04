/// Keys with a built-in default, as `section.key`, with the aliases a file
/// may spell them with. `save_config` writes each only when its value differs
/// from the default or the file already has it.
pub const DEFAULTED_KEYS: &[(&str, &[&str])] = &[
    ("ui.start_on_login", &[]),
    ("ui.auto_check_updates", &[]),
    ("ui.hotkey", &[]),
    ("ui.correction_key", &["instruction_key"]),
    ("ui.font_size", &[]),
    ("ui.meter_style", &[]),
    ("mic.sample_rate", &[]),
    ("mic.gain", &[]),
    ("mic.hold_ms", &[]),
    ("mic.always_on", &[]),
    ("deepgram.language", &[]),
    ("deepgram.model", &[]),
    ("deepgram.endpointing_ms", &[]),
    ("deepgram.utterance_end_ms", &[]),
    ("transformation.hotkey", &[]),
    ("transformation.auto", &[]),
    ("transformation.model", &[]),
    ("transformation.system_prompt", &[]),
    (
        "transformation.correction_system_prompt",
        &["instruction_system_prompt"],
    ),
];

/// Defaulted keys (`DEFAULTED_KEYS`) the user reset to their built-in default
/// in the settings window. `save_config` removes a reset key while its value
/// still equals the default, even when the file already had it, so the
/// config follows future changes to the default again.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DefaultResets(std::collections::BTreeSet<&'static str>);

impl DefaultResets {
    pub fn insert(&mut self, key: &'static str) {
        self.0.insert(key);
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains(key)
    }

    pub fn extend(&mut self, other: &DefaultResets) {
        self.0.extend(other.0.iter().copied());
    }
}

impl FromIterator<&'static str> for DefaultResets {
    fn from_iter<I: IntoIterator<Item = &'static str>>(keys: I) -> Self {
        Self(keys.into_iter().collect())
    }
}

pub(super) fn default_start_on_login() -> bool {
    false
}

pub(super) fn default_auto_check_updates() -> bool {
    true
}

pub(super) fn default_hotkey() -> String {
    "F5".into()
}

pub(super) fn default_correction_key() -> String {
    "Alt+Cmd".into()
}

pub(super) fn default_deepgram_language() -> String {
    "en-US".into()
}

pub(super) fn default_deepgram_model() -> String {
    "nova-3".into()
}

pub(super) fn default_sample_rate() -> u32 {
    16000
}

pub(super) fn default_gain() -> f32 {
    4.5
}

/// Lowest `mic.gain`, in dB. The audio stream amplifies samples by
/// `10^(gain / 20)`, so 0 dB leaves the input unchanged and the range never
/// mutes it.
pub const MIC_GAIN_MIN_DB: f32 = 0.0;
/// Highest `mic.gain`, in dB (about 3.2 times the input amplitude).
pub const MIC_GAIN_MAX_DB: f32 = 10.0;

pub(super) fn default_hold_ms() -> u64 {
    300
}

pub(super) fn default_always_on() -> bool {
    true
}

pub(super) fn default_overlay_font_size() -> f64 {
    12.0
}

pub(super) fn default_endpointing_ms() -> u16 {
    300
}

pub(super) fn default_utterance_end_ms() -> u16 {
    1000
}

pub(super) fn default_transformation_hotkey() -> String {
    "F6".into()
}

pub(super) fn default_transformation_auto() -> bool {
    true
}
