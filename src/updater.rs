use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel, MainThreadMarker};

#[derive(Debug)]
pub struct AppUpdater {
    controller: Retained<AnyObject>,
}

fn load_sparkle_framework() {
    if AnyClass::get(c"SPUStandardUpdaterController").is_some() {
        return;
    }

    let mut candidate_paths = Vec::new();

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidate_paths.push(exe_dir.join("../Frameworks/Sparkle.framework/Sparkle"));
            candidate_paths.push(exe_dir.join("Frameworks/Sparkle.framework/Sparkle"));
            candidate_paths.push(exe_dir.join("../../../vendor/Sparkle.framework/Sparkle"));
            candidate_paths.push(exe_dir.join("../../../.tmp/sparkle/Sparkle.framework/Sparkle"));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidate_paths.push(cwd.join("vendor/Sparkle.framework/Sparkle"));
        candidate_paths.push(cwd.join(".tmp/sparkle/Sparkle.framework/Sparkle"));
        candidate_paths.push(cwd.join("dist/simple-ptt.app/Contents/Frameworks/Sparkle.framework/Sparkle"));
    }

    for path in candidate_paths {
        if path.exists() {
            if let Some(path_str) = path.to_str() {
                if let Ok(c_path) = std::ffi::CString::new(path_str) {
                    unsafe {
                        let handle = libc::dlopen(c_path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_GLOBAL);
                        if !handle.is_null() {
                            log::info!("successfully loaded Sparkle.framework from {}", path_str);
                            return;
                        }
                    }
                }
            }
        }
    }
}

impl AppUpdater {
    pub fn init(_mtm: MainThreadMarker) -> Option<Self> {
        load_sparkle_framework();

        let cls = AnyClass::get(c"SPUStandardUpdaterController")?;

        let controller: Option<Retained<AnyObject>> = unsafe {
            let alloc: *mut AnyObject = msg_send![cls, alloc];
            if alloc.is_null() {
                return None;
            }
            let instance_ptr: *mut AnyObject = msg_send![
                alloc,
                initWithStartingUpdater: true,
                updaterDelegate: std::ptr::null::<AnyObject>(),
                userDriverDelegate: std::ptr::null::<AnyObject>()
            ];
            Retained::from_raw(instance_ptr)
        };

        let controller = controller?;
        log::info!("Sparkle updater initialized successfully");
        Some(Self { controller })
    }

    pub fn controller(&self) -> &AnyObject {
        &self.controller
    }

    pub fn set_automatically_checks_for_updates(&self, enabled: bool) {
        unsafe {
            let updater: Option<Retained<AnyObject>> = msg_send![&self.controller, updater];
            if let Some(updater) = updater {
                let _: () = msg_send![&updater, setAutomaticallyChecksForUpdates: enabled];
                let _: () = msg_send![&updater, setUpdateCheckInterval: 604800.0f64]; // 1 week interval
            }
        }
    }

    pub fn check_for_updates_selector() -> Sel {
        sel!(checkForUpdates:)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_updater_initializes_when_sparkle_framework_is_available() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let updater = AppUpdater::init(mtm);
        assert!(updater.is_some(), "Sparkle updater should initialize when Sparkle.framework is available");
    }
}
