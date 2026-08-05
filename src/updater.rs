#![allow(dead_code)]

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel, MainThreadMarker};

#[derive(Debug)]
pub struct AppUpdater {
    controller: Retained<AnyObject>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GitHubReleaseResponse {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

pub fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let trimmed = v.trim().trim_start_matches('v');
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() < 3 {
        return None;
    }
    let major = parts[0].parse().ok()?;
    let minor = parts[1].parse().ok()?;
    let patch = parts[2].split('-').next()?.parse().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer_version(remote: &str, current: &str) -> bool {
    let Some(remote_ver) = parse_semver(remote) else {
        return false;
    };
    let Some(current_ver) = parse_semver(current) else {
        return false;
    };

    remote_ver > current_ver
}

pub fn check_for_github_release_update() {
    std::thread::spawn(move || {
        let current_version = env!("CARGO_PKG_VERSION");
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent(concat!("simple-ptt/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(c) => c,
            Err(error) => {
                log::error!("failed to create HTTP client for update check: {}", error);
                dispatch_update_alert(
                    "Unable to Check for Updates",
                    &format!("Could not initialize network client: {}", error),
                    None,
                    None,
                );
                return;
            }
        };

        let response = match client
            .get("https://api.github.com/repos/alexgorbatchev/simple-ptt/releases/latest")
            .header("Accept", "application/vnd.github.v3+json")
            .send()
        {
            Ok(resp) => resp,
            Err(error) => {
                log::error!("failed to check GitHub releases for update: {}", error);
                dispatch_update_alert(
                    "Unable to Check for Updates",
                    &format!(
                        "Could not connect to update server. Please check your internet connection.\n\nError: {}",
                        error
                    ),
                    None,
                    None,
                );
                return;
            }
        };

        let release: GitHubReleaseResponse = match response.json() {
            Ok(rel) => rel,
            Err(error) => {
                log::error!("failed to parse GitHub release response: {}", error);
                dispatch_update_alert(
                    "Unable to Check for Updates",
                    &format!("Failed to parse release information: {}", error),
                    None,
                    None,
                );
                return;
            }
        };

        let remote_ver_str = release.tag_name.trim_start_matches('v').to_owned();
        if is_newer_version(&release.tag_name, current_version) {
            let download_url = release
                .assets
                .iter()
                .find(|asset| asset.name.ends_with(".dmg"))
                .map(|asset| asset.browser_download_url.clone())
                .unwrap_or_else(|| {
                    format!(
                        "https://github.com/alexgorbatchev/simple-ptt/releases/download/{}/simple-ptt-{}-macos-arm64.dmg",
                        release.tag_name, release.tag_name
                    )
                });

            dispatch_update_alert(
                "A new version of simple-ptt is available!",
                &format!(
                    "simple-ptt {} is available (you have {}).\n\nWould you like to update now or open the release page?",
                    remote_ver_str, current_version
                ),
                Some(release.html_url),
                Some((download_url, remote_ver_str)),
            );
        } else {
            dispatch_update_alert(
                "You're up to date!",
                &format!(
                    "simple-ptt {} is currently the newest version available.",
                    current_version
                ),
                None,
                None,
            );
        }
    });
}

fn detect_target_app_installation_path() -> std::path::PathBuf {
    if let Ok(exe_path) = std::env::current_exe() {
        for ancestor in exe_path.ancestors() {
            if ancestor.extension().and_then(|ext| ext.to_str()) == Some("app") {
                return ancestor.to_path_buf();
            }
        }
    }

    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        let user_apps = home.join("Applications").join("simple-ptt.app");
        if user_apps.parent().map(|p| p.exists()).unwrap_or(false) {
            return user_apps;
        }
    }

    std::path::PathBuf::from("/Applications/simple-ptt.app")
}

fn show_update_error(title: &str, message: &str) {
    dispatch_update_alert(title, message, None, None);
}

pub fn perform_in_app_update(download_url: String, remote_version: String) {
    std::thread::spawn(move || {
        log::info!("downloading update from {}...", download_url);

        let temp_dir = std::env::temp_dir();
        let dmg_path = temp_dir.join(format!("simple-ptt-update-{}.dmg", remote_version));
        let mount_point = temp_dir.join(format!("simple_ptt_mount_{}", std::process::id()));

        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .user_agent(concat!("simple-ptt/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(c) => c,
            Err(err) => {
                show_update_error("Update Failed", &format!("Failed to create HTTP client: {}", err));
                return;
            }
        };

        let mut response = match client.get(&download_url).send() {
            Ok(resp) => resp,
            Err(err) => {
                show_update_error("Update Failed", &format!("Failed to download update package: {}", err));
                return;
            }
        };

        if !response.status().is_success() {
            show_update_error("Update Failed", &format!("Download server returned status {}", response.status()));
            return;
        }

        let mut file = match std::fs::File::create(&dmg_path) {
            Ok(f) => f,
            Err(err) => {
                show_update_error("Update Failed", &format!("Failed to create local update file: {}", err));
                return;
            }
        };

        if let Err(err) = std::io::copy(&mut response, &mut file) {
            show_update_error("Update Failed", &format!("Failed to write update file: {}", err));
            let _ = std::fs::remove_file(&dmg_path);
            return;
        }
        drop(file);

        let _ = std::fs::create_dir_all(&mount_point);
        let attach_status = std::process::Command::new("hdiutil")
            .arg("attach")
            .arg(&dmg_path)
            .arg("-nobrowse")
            .arg("-mountpoint")
            .arg(&mount_point)
            .status();

        if attach_status.as_ref().map(|s| !s.success()).unwrap_or(true) {
            show_update_error("Update Failed", "Failed to mount DMG update package.");
            let _ = std::fs::remove_file(&dmg_path);
            let _ = std::fs::remove_dir_all(&mount_point);
            return;
        }

        let app_source = mount_point.join("simple-ptt.app");
        if !app_source.exists() {
            show_update_error("Update Failed", "Update package did not contain simple-ptt.app.");
            let _ = std::process::Command::new("hdiutil").arg("detach").arg(&mount_point).arg("-force").status();
            let _ = std::fs::remove_file(&dmg_path);
            let _ = std::fs::remove_dir_all(&mount_point);
            return;
        }

        let target_app_path = detect_target_app_installation_path();

        if target_app_path.exists() {
            let _ = std::fs::remove_dir_all(&target_app_path);
        }

        if let Some(parent) = target_app_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let copy_status = std::process::Command::new("cp")
            .arg("-R")
            .arg(&app_source)
            .arg(&target_app_path)
            .status();

        let _ = std::process::Command::new("hdiutil")
            .arg("detach")
            .arg(&mount_point)
            .arg("-force")
            .status();
        let _ = std::fs::remove_file(&dmg_path);
        let _ = std::fs::remove_dir_all(&mount_point);

        if copy_status.as_ref().map(|s| !s.success()).unwrap_or(true) {
            show_update_error(
                "Update Failed",
                &format!("Failed to copy updated app bundle to {}.", target_app_path.display()),
            );
            return;
        }

        dispatch_to_main_thread(move || {
            let mtm = match MainThreadMarker::new() {
                Some(mtm) => mtm,
                None => return,
            };

            let alert = objc2_app_kit::NSAlert::new(mtm);
            alert.setMessageText(&objc2_foundation::NSString::from_str("Update Installed!"));
            alert.setInformativeText(&objc2_foundation::NSString::from_str(&format!(
                "simple-ptt has been updated to version {}.\n\nClick Relaunch to start the updated version now.",
                remote_version
            )));
            alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("Relaunch"));
            alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("Later"));

            let response = alert.runModal();
            if response == objc2_app_kit::NSAlertFirstButtonReturn {
                let _ = std::process::Command::new("open")
                    .arg("-n")
                    .arg(&target_app_path)
                    .spawn();
                std::process::exit(0);
            }
        });
    });
}

fn dispatch_to_main_thread(work: impl FnOnce() + Send + 'static) {
    struct MainThreadWork {
        work: Box<dyn FnOnce() + Send>,
    }

    extern "C" fn perform_main_thread_work(context: *mut std::ffi::c_void) {
        let work_box = unsafe { Box::from_raw(context as *mut MainThreadWork) };
        (work_box.work)();
    }

    let work_box = Box::new(MainThreadWork {
        work: Box::new(work),
    });

    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }

    unsafe {
        dispatch_async_f(
            &_dispatch_main_q,
            Box::into_raw(work_box) as *mut std::ffi::c_void,
            perform_main_thread_work,
        );
    }
}

fn dispatch_update_alert(
    title: &str,
    message: &str,
    open_url: Option<String>,
    download_info: Option<(String, String)>,
) {
    let title = title.to_owned();
    let message = message.to_owned();

    dispatch_to_main_thread(move || {
        let mtm = match MainThreadMarker::new() {
            Some(mtm) => mtm,
            None => return,
        };

        let alert = objc2_app_kit::NSAlert::new(mtm);
        alert.setMessageText(&objc2_foundation::NSString::from_str(&title));
        alert.setInformativeText(&objc2_foundation::NSString::from_str(&message));

        if let Some((download_url, remote_version)) = download_info {
            alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("Update Now"));
            if open_url.is_some() {
                alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("Open Release Page"));
            }
            alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("Cancel"));

            let response = alert.runModal();
            if response == objc2_app_kit::NSAlertFirstButtonReturn {
                perform_in_app_update(download_url, remote_version);
            } else if response == objc2_app_kit::NSAlertSecondButtonReturn && open_url.is_some() {
                if let Some(ref url) = open_url {
                    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
                    if let Some(ns_url) =
                        objc2_foundation::NSURL::URLWithString(&objc2_foundation::NSString::from_str(url))
                    {
                        workspace.openURL(&ns_url);
                    }
                }
            }
        } else if let Some(ref url) = open_url {
            alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("Open Release Page"));
            alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("Cancel"));
            let response = alert.runModal();
            if response == objc2_app_kit::NSAlertFirstButtonReturn {
                let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
                if let Some(ns_url) =
                    objc2_foundation::NSURL::URLWithString(&objc2_foundation::NSString::from_str(url))
                {
                    workspace.openURL(&ns_url);
                }
            }
        } else {
            alert.addButtonWithTitle(&objc2_foundation::NSString::from_str("OK"));
            alert.runModal();
        }
    });
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

fn is_valid_sparkle_bundle() -> bool {
    let bundle = objc2_foundation::NSBundle::mainBundle();
    let has_bundle_id = bundle.bundleIdentifier().is_some();
    let feed_url_key = objc2_foundation::NSString::from_str("SUFeedURL");
    let has_feed_url = bundle.objectForInfoDictionaryKey(&feed_url_key).is_some();

    has_bundle_id && has_feed_url
}

impl AppUpdater {
    pub fn init(_mtm: MainThreadMarker) -> Option<Self> {
        if !is_valid_sparkle_bundle() {
            log::info!(
                "skipping Sparkle updater initialization: not running inside a packaged .app bundle with SUFeedURL"
            );
            return None;
        }

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

    #[allow(dead_code)]
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

    pub fn check_for_updates(&self, sender: Option<&AnyObject>) {
        unsafe {
            let _: () = msg_send![&self.controller, checkForUpdates: sender];
        }
    }

    #[allow(dead_code)]
    pub fn check_for_updates_selector() -> Sel {
        sel!(checkForUpdates:)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_updater_init_safely_handles_unbundled_execution() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let updater = AppUpdater::init(mtm);
        assert!(updater.is_none());
    }

    #[test]
    fn semver_comparison_correctly_identifies_newer_versions() {
        assert!(is_newer_version("v1.6.0", "1.5.9"));
        assert!(is_newer_version("v1.6.1", "1.6.0"));
        assert!(is_newer_version("1.7.0", "1.6.0"));
        assert!(is_newer_version("2.0.0", "1.6.0"));
        assert!(!is_newer_version("v1.6.0", "1.6.0"));
        assert!(!is_newer_version("v1.5.2", "1.6.0"));
    }
}
