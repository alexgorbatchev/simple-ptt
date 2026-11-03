use auto_launch::{AutoLaunch, AutoLaunchBuilder, MacOSLaunchMode};

/// Makes the login item match `enable`, changing it only when its actual
/// registration differs, and returns whether it was registered before, so a
/// failed Save can put that state back.
pub fn set_start_on_login(enable: bool) -> Result<bool, String> {
    let login_item = login_item()?;
    let was_enabled = login_item
        .is_enabled()
        .map_err(|error| format!("couldn't check whether the app starts on login: {error}"))?;
    if was_enabled != enable {
        if enable {
            login_item
                .enable()
                .map_err(|error| format!("couldn't turn on starting on login: {error}"))?;
        } else {
            login_item
                .disable()
                .map_err(|error| format!("couldn't turn off starting on login: {error}"))?;
        }
    }
    Ok(was_enabled)
}

fn login_item() -> Result<AutoLaunch, String> {
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

    AutoLaunchBuilder::new()
        .set_app_name("simple-ptt")
        .set_app_path(app_path.to_str().unwrap_or_default())
        .set_macos_launch_mode(mode)
        .build()
        .map_err(|error| format!("couldn't set up starting on login: {error}"))
}
