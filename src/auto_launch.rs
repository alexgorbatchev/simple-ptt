use auto_launch::{AutoLaunchBuilder, MacOSLaunchMode};

pub fn apply_auto_launch_config(enable: bool) {
    if let Ok(path) = std::env::current_exe() {
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
            .build();

        if let Ok(auto) = auto {
            if enable {
                let _ = auto.enable();
            } else {
                let _ = auto.disable();
            }
        }
    }
}
