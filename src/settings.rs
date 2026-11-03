use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use crate::config::Config;

#[derive(Clone)]
pub struct LiveConfigStore {
    file_config: Arc<RwLock<Config>>,
    runtime_config: Arc<RwLock<Config>>,
    mic_gain: Arc<AtomicU32>,
    path: PathBuf,
}

impl LiveConfigStore {
    pub fn new(file_config: Config, runtime_config: Config, path: PathBuf) -> Self {
        Self {
            mic_gain: Arc::new(AtomicU32::new(runtime_config.mic.gain.to_bits())),
            file_config: Arc::new(RwLock::new(file_config)),
            runtime_config: Arc::new(RwLock::new(runtime_config)),
            path,
        }
    }

    pub fn current(&self) -> Config {
        self.runtime_config
            .read()
            .map(|config| config.clone())
            .unwrap_or_default()
    }

    pub fn current_file(&self) -> Config {
        self.file_config
            .read()
            .map(|config| config.clone())
            .unwrap_or_default()
    }

    pub fn replace(&self, file_config: Config, runtime_config: Config) {
        if let Ok(mut current_file_config) = self.file_config.write() {
            *current_file_config = file_config;
        }
        if let Ok(mut current_runtime_config) = self.runtime_config.write() {
            *current_runtime_config = runtime_config;
            self.mic_gain
                .store(current_runtime_config.mic.gain.to_bits(), Ordering::Relaxed);
        }
    }

    /// Saved runtime gain, readable by the audio callback without locking or cloning config.
    pub fn mic_gain(&self) -> f32 {
        f32::from_bits(self.mic_gain.load(Ordering::Relaxed))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::LiveConfigStore;
    use crate::config::Config;

    #[test]
    fn callback_gain_tracks_runtime_replacements_across_clones_without_config_lock() {
        let mut runtime_config = Config::default();
        runtime_config.mic.gain = 6.0;
        let store = LiveConfigStore::new(
            Config::default(),
            runtime_config.clone(),
            "/tmp/config.toml".into(),
        );
        let callback_store = store.clone();
        assert_eq!(callback_store.mic_gain(), 6.0);

        runtime_config.mic.gain = -3.0;
        store.replace(Config::default(), runtime_config);
        let config_lock = store.runtime_config.write().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            sender.send(callback_store.mic_gain()).unwrap();
        });
        let gain = receiver.recv_timeout(std::time::Duration::from_secs(1));
        drop(config_lock);
        reader.join().unwrap();
        assert_eq!(gain.unwrap(), -3.0);
    }

    #[test]
    fn keeps_file_config_separate_from_runtime_config() {
        let mut file_config = Config::default();
        file_config.deepgram.api_key = None;

        let mut runtime_config = file_config.clone();
        runtime_config.deepgram.api_key = Some("env-key".to_owned());

        let store = LiveConfigStore::new(
            file_config.clone(),
            runtime_config.clone(),
            std::path::PathBuf::from("/tmp/config.toml"),
        );

        assert_eq!(store.current_file().deepgram.api_key, None);
        assert_eq!(store.current().deepgram.api_key, Some("env-key".to_owned()));
    }
}
