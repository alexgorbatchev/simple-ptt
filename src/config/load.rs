use super::paths::{config_path, override_config_path, CONFIG_OVERRIDE_ENV_VAR};
use super::Config;

pub fn load_config() -> Config {
    let path = match config_path() {
        Ok(path) => path,
        Err(error) => {
            log::warn!("failed to resolve config path: {}, using defaults", error);
            return Config::default();
        }
    };

    if override_config_path().is_some() {
        let contents = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "{} is set but {} could not be read: {}",
                CONFIG_OVERRIDE_ENV_VAR,
                path.display(),
                error
            )
        });

        return toml::from_str(&contents).unwrap_or_else(|error| {
            panic!(
                "{} is set but {} could not be parsed: {}",
                CONFIG_OVERRIDE_ENV_VAR,
                path.display(),
                error
            )
        });
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_else(|error| {
            log::warn!(
                "failed to parse {}: {}, using defaults",
                path.display(),
                error
            );
            Config::default()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            log::info!("no config at {}, using defaults", path.display());
            Config::default()
        }
        Err(error) => {
            log::warn!(
                "failed to read {}: {}, using defaults",
                path.display(),
                error
            );
            Config::default()
        }
    }
}
