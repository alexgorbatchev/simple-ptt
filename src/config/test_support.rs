use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use super::{
    default_transformation_correction_system_prompt, default_transformation_system_prompt, Config,
    UiMeterStyle,
};

pub(super) fn env_lock() -> MutexGuard<'static, ()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

pub(super) struct EnvVarGuard {
    name: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    pub(super) fn set(name: &'static str, value: Option<&str>) -> Self {
        let guard = Self {
            name,
            previous: std::env::var_os(name),
        };
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
        guard
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.name, value),
            None => std::env::remove_var(self.name),
        }
    }
}

pub(super) const EXAMPLE_CONFIG: &str = include_str!("../../config.example.toml");

pub(super) fn assert_example_config_values(config: &Config) {
    assert!(!config.ui.start_on_login);
    assert!(config.ui.auto_check_updates);
    assert_eq!(config.ui.hotkey, "F5");
    assert_eq!(config.ui.correction_key, "Alt+Cmd");
    assert_eq!(config.ui.font_name, None);
    assert_eq!(config.ui.font_size, 12.0);
    assert_eq!(config.ui.footer_font_size, None);
    assert_eq!(config.ui.meter_style, UiMeterStyle::Pills);
    assert_eq!(config.mic.audio_device, None);
    assert_eq!(config.mic.sample_rate, 16000);
    assert_eq!(config.mic.gain, 4.5);
    assert_eq!(config.mic.hold_ms, 300);
    assert!(config.mic.always_on);
    assert_eq!(config.deepgram.api_key, None);
    assert_eq!(config.deepgram.language, "en-US");
    assert!(config.deepgram.keyterms.is_empty());
    assert_eq!(config.deepgram.model, "nova-3");
    assert_eq!(config.deepgram.endpointing_ms, 300);
    assert_eq!(config.deepgram.utterance_end_ms, 1000);
    assert_eq!(config.transformation.hotkey, "F6");
    assert!(config.transformation.auto);
    assert_eq!(config.transformation.provider, None);
    assert_eq!(config.transformation.api_key, None);
    assert_eq!(
        config.transformation.system_prompt,
        default_transformation_system_prompt()
    );
    assert_eq!(
        config.transformation.correction_system_prompt,
        default_transformation_correction_system_prompt()
    );
}

mod tests {
    use std::ffi::OsString;

    use super::{env_lock, EnvVarGuard};

    #[test]
    fn environment_is_restored_after_panic_and_lock_remains_usable() {
        const NAME: &str = "SIMPLE_PTT_ENV_GUARD_TEST";
        for previous in [None, Some("original")] {
            {
                let _lock = env_lock();
                let _original = EnvVarGuard::set(NAME, previous);
                let result = std::panic::catch_unwind(|| {
                    let _changed = EnvVarGuard::set(NAME, Some("temporary"));
                    panic!("exercise environment restoration");
                });
                assert!(result.is_err());
                assert_eq!(std::env::var_os(NAME), previous.map(OsString::from));
            }

            let result = std::panic::catch_unwind(|| {
                let _lock = env_lock();
                let _changed = EnvVarGuard::set(NAME, Some("temporary"));
                panic!("exercise lock poisoning");
            });
            assert!(result.is_err());
            let _lock = env_lock();
        }
    }
}
