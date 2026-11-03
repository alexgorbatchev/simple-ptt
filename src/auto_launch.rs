use auto_launch::{AutoLaunchBuilder, MacOSLaunchMode};

/// Registers the app as a login item, or unregisters it, and reports why
/// that failed.
pub fn apply_auto_launch_config(enable: bool) -> Result<(), String> {
    let path = std::env::current_exe()
        .map_err(|error| format!("couldn't find the app to start on login: {error}"))?;
    let mut is_app_bundle = false;
    let mut app_path = path.clone();

    if let Some(parent1) = app_path.parent() {
        if let Some(parent2) = parent1.parent() {
            if let Some(parent3) = parent2.parent() {
                if parent3.extension().and_then(|e| e.to_str()) == Some("app") {
                    app_path = parent3.to_path_buf();
                    is_app_bundle = true;
                }
            }
        }
    }

    let mode = if is_app_bundle {
        MacOSLaunchMode::SMAppService
    } else {
        MacOSLaunchMode::LaunchAgent
    };

    let auto = AutoLaunchBuilder::new()
        .set_app_name("simple-ptt")
        .set_app_path(app_path.to_str().unwrap_or_default())
        .set_macos_launch_mode(mode)
        .build()
        .map_err(|error| format!("couldn't set up starting on login: {error}"))?;

    if enable {
        auto.enable()
            .map_err(|error| format!("couldn't turn on starting on login: {error}"))
    } else {
        auto.disable()
            .map_err(|error| format!("couldn't turn off starting on login: {error}"))
    }
}
