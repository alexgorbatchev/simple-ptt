use std::path::PathBuf;

pub(super) const CONFIG_OVERRIDE_ENV_VAR: &str = "SIMPLE_PTT_CONFIG";
const DEFAULT_CONFIG_FILE_NAME: &str = "config.toml";
const XDG_APP_NAME: &str = "simple-ptt";

pub fn config_path() -> Result<PathBuf, String> {
    if let Some(override_path) = override_config_path() {
        return Ok(override_path);
    }

    if let Some(xdg_config_home) = non_empty_env_path("XDG_CONFIG_HOME") {
        return Ok(xdg_config_home
            .join(XDG_APP_NAME)
            .join(DEFAULT_CONFIG_FILE_NAME));
    }

    if let Some(home_path) = non_empty_env_path("HOME") {
        return Ok(home_path
            .join(".config")
            .join(XDG_APP_NAME)
            .join(DEFAULT_CONFIG_FILE_NAME));
    }

    Err(format!(
        "Neither {} nor HOME/XDG_CONFIG_HOME is available, so no config path can be resolved.",
        CONFIG_OVERRIDE_ENV_VAR
    ))
}

pub fn cache_dir() -> Result<PathBuf, String> {
    if let Some(xdg_cache_home) = non_empty_env_path("XDG_CACHE_HOME") {
        return Ok(xdg_cache_home.join(XDG_APP_NAME));
    }

    if let Some(home_path) = non_empty_env_path("HOME") {
        return Ok(home_path.join(".cache").join(XDG_APP_NAME));
    }

    Err(
        "Neither HOME nor XDG_CACHE_HOME is available, so no cache directory can be resolved."
            .to_owned(),
    )
}

pub(super) fn config_location_hint() -> String {
    match config_path() {
        Ok(path) => path.display().to_string(),
        Err(error) => error,
    }
}

fn non_empty_env_path(variable_name: &str) -> Option<PathBuf> {
    std::env::var_os(variable_name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

pub(super) fn override_config_path() -> Option<PathBuf> {
    non_empty_env_path(CONFIG_OVERRIDE_ENV_VAR)
}
