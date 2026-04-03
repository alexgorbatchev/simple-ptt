use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::config::Config;

#[derive(Clone)]
pub struct LiveConfigStore {
    file_config: Arc<RwLock<Config>>,
    runtime_config: Arc<RwLock<Config>>,
    path: PathBuf,
}

impl LiveConfigStore {
    pub fn new(file_config: Config, runtime_config: Config, path: PathBuf) -> Self {
        Self {
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
        }
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
