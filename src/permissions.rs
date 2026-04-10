use std::path::{Path, PathBuf};
use std::process::Command;

use block2::RcBlock;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaType, AVMediaTypeAudio};
use objc2_core_graphics::{CGPreflightListenEventAccess, CGRequestListenEventAccess};

const APP_BUNDLE_IDENTIFIER: &str = "io.github.alexgorbatchev.simple-ptt";

extern "C-unwind" {
    fn AXIsProcessTrusted() -> bool;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlobalHotkeyPermissionState {
    Unknown,
    Missing,
    Requested,
    Granted,
    NeedsRelaunch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlobalHotkeyPermissions {
    pub accessibility_granted: bool,
    pub input_monitoring_granted: bool,
    pub microphone_granted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlobalHotkeyPermissionFlow {
    pub permissions: GlobalHotkeyPermissions,
    pub accessibility_state: GlobalHotkeyPermissionState,
    pub input_monitoring_state: GlobalHotkeyPermissionState,
    pub microphone_state: GlobalHotkeyPermissionState,
}

impl GlobalHotkeyPermissions {
    pub fn current() -> Self {
        Self {
            accessibility_granted: has_accessibility_access(),
            input_monitoring_granted: has_input_monitoring_access(),
            microphone_granted: has_microphone_access(),
        }
    }

    pub fn hotkey_permissions_granted(self) -> bool {
        self.accessibility_granted && self.input_monitoring_granted
    }

    pub fn all_granted(self) -> bool {
        self.hotkey_permissions_granted() && self.microphone_granted
    }
}

impl GlobalHotkeyPermissionFlow {
    pub fn unknown() -> Self {
        Self {
            permissions: GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
                microphone_granted: false,
            },
            accessibility_state: GlobalHotkeyPermissionState::Unknown,
            input_monitoring_state: GlobalHotkeyPermissionState::Unknown,
            microphone_state: GlobalHotkeyPermissionState::Unknown,
        }
    }

    pub fn all_granted(self) -> bool {
        self.permissions.all_granted()
    }

    pub fn relaunch_required(self) -> bool {
        matches!(
            self.accessibility_state,
            GlobalHotkeyPermissionState::NeedsRelaunch
        ) || matches!(
            self.input_monitoring_state,
            GlobalHotkeyPermissionState::NeedsRelaunch
        )
    }
}

pub fn resolve_global_hotkey_permission_flow(
    startup_permissions: GlobalHotkeyPermissions,
    accessibility_requested: bool,
    input_monitoring_requested: bool,
    microphone_requested: bool,
) -> GlobalHotkeyPermissionFlow {
    build_global_hotkey_permission_flow(
        startup_permissions,
        GlobalHotkeyPermissions::current(),
        accessibility_requested,
        input_monitoring_requested,
        microphone_requested,
    )
}

pub fn request_input_monitoring_access() -> Result<bool, String> {
    Ok(CGRequestListenEventAccess())
}

pub fn request_microphone_access() -> Result<(), String> {
    let media_type = audio_media_type()?;
    let completion = RcBlock::new(|_granted| {});
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &completion);
    }
    Ok(())
}

pub fn accessibility_settings_urls() -> [&'static str; 2] {
    [
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    ]
}

pub fn microphone_settings_urls() -> [&'static str; 2] {
    [
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Microphone",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
    ]
}

pub fn reset_application_permissions_and_relaunch() -> Result<(), String> {
    let executable_path = std::env::current_exe()
        .map_err(|error| format!("failed to resolve current executable path: {}", error))?;

    let app_bundle_path = app_bundle_path_from_executable(&executable_path);
    let target_path = app_bundle_path.as_deref().unwrap_or(&executable_path);

    // tccutil resets are cached by macOS tccd if the process is currently running.
    // We spawn a detached background script that outlives us, waits 1 second for us
    // to cleanly terminate, runs the TCC resets, and then launches a fresh instance.
    let script = format!(
        "sleep 1.5; \
         /usr/bin/tccutil reset Accessibility {0}; \
         /usr/bin/tccutil reset ListenEvent {0}; \
         /usr/bin/tccutil reset Microphone {0}; \
         /usr/bin/open -n \"{1}\"",
        APP_BUNDLE_IDENTIFIER,
        target_path.display()
    );

    std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .spawn()
        .map_err(|error| format!("failed to spawn background reset script: {}", error))?;

    Ok(())
}

pub fn relaunch_current_application() -> Result<(), String> {
    let executable_path = std::env::current_exe()
        .map_err(|error| format!("failed to resolve current executable path: {}", error))?;

    if let Some(app_bundle_path) = app_bundle_path_from_executable(&executable_path) {
        let output = Command::new("/usr/bin/open")
            .args(["-n"])
            .arg(&app_bundle_path)
            .output()
            .map_err(|error| format!("failed to relaunch app bundle: {}", error))?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            return Err(format!("app relaunch failed with status {}", output.status));
        }

        return Err(format!("app relaunch failed: {}", stderr));
    }

    Command::new(&executable_path)
        .spawn()
        .map_err(|error| format!("failed to spawn executable for relaunch: {}", error))?;
    Ok(())
}

fn build_global_hotkey_permission_flow(
    startup_permissions: GlobalHotkeyPermissions,
    current_permissions: GlobalHotkeyPermissions,
    accessibility_requested: bool,
    input_monitoring_requested: bool,
    microphone_requested: bool,
) -> GlobalHotkeyPermissionFlow {
    GlobalHotkeyPermissionFlow {
        permissions: current_permissions,
        accessibility_state: resolve_hotkey_permission_state(
            startup_permissions.accessibility_granted,
            current_permissions.accessibility_granted,
            accessibility_requested,
        ),
        input_monitoring_state: resolve_hotkey_permission_state(
            startup_permissions.input_monitoring_granted,
            current_permissions.input_monitoring_granted,
            input_monitoring_requested,
        ),
        microphone_state: resolve_microphone_permission_state(
            current_permissions.microphone_granted,
            microphone_authorization_status(),
            microphone_requested,
        ),
    }
}

fn resolve_hotkey_permission_state(
    granted_at_startup: bool,
    granted_now: bool,
    requested: bool,
) -> GlobalHotkeyPermissionState {
    if granted_now {
        return if granted_at_startup {
            GlobalHotkeyPermissionState::Granted
        } else {
            GlobalHotkeyPermissionState::NeedsRelaunch
        };
    }

    if requested {
        return GlobalHotkeyPermissionState::Requested;
    }

    GlobalHotkeyPermissionState::Missing
}

fn resolve_microphone_permission_state(
    granted_now: bool,
    authorization_status: AVAuthorizationStatus,
    requested: bool,
) -> GlobalHotkeyPermissionState {
    if granted_now {
        return GlobalHotkeyPermissionState::Granted;
    }

    if requested
        || matches!(
            authorization_status,
            AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted
        )
    {
        return GlobalHotkeyPermissionState::Requested;
    }

    GlobalHotkeyPermissionState::Missing
}

fn app_bundle_path_from_executable(executable_path: &Path) -> Option<PathBuf> {
    let contents_path = executable_path.parent()?.parent()?;
    if contents_path.file_name()? != "Contents" {
        return None;
    }

    let app_bundle_path = contents_path.parent()?;
    if app_bundle_path.extension()? != "app" {
        return None;
    }

    Some(app_bundle_path.to_path_buf())
}

fn has_input_monitoring_access() -> bool {
    CGPreflightListenEventAccess()
}

fn has_accessibility_access() -> bool {
    unsafe { AXIsProcessTrusted() }
}

fn has_microphone_access() -> bool {
    matches!(
        microphone_authorization_status(),
        AVAuthorizationStatus::Authorized
    )
}

fn microphone_authorization_status() -> AVAuthorizationStatus {
    match audio_media_type() {
        Ok(media_type) => unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) },
        Err(_) => AVAuthorizationStatus::NotDetermined,
    }
}

fn audio_media_type() -> Result<&'static AVMediaType, String> {
    unsafe { AVMediaTypeAudio }.ok_or_else(|| "AVMediaTypeAudio is unavailable".to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use objc2_av_foundation::AVAuthorizationStatus;

    use super::{
        app_bundle_path_from_executable, build_global_hotkey_permission_flow,
        resolve_microphone_permission_state, GlobalHotkeyPermissionFlow,
        GlobalHotkeyPermissionState, GlobalHotkeyPermissions,
    };

    #[test]
    fn unknown_flow_starts_in_unknown_state() {
        let flow = GlobalHotkeyPermissionFlow::unknown();

        assert_eq!(
            flow.accessibility_state,
            GlobalHotkeyPermissionState::Unknown
        );
        assert_eq!(
            flow.input_monitoring_state,
            GlobalHotkeyPermissionState::Unknown
        );
        assert_eq!(flow.microphone_state, GlobalHotkeyPermissionState::Unknown);
        assert!(!flow.all_granted());
        assert!(!flow.relaunch_required());
    }

    #[test]
    fn flow_requires_relaunch_when_hotkey_permissions_were_missing_at_startup() {
        let flow = build_global_hotkey_permission_flow(
            GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
                microphone_granted: false,
            },
            GlobalHotkeyPermissions {
                accessibility_granted: true,
                input_monitoring_granted: true,
                microphone_granted: true,
            },
            true,
            true,
            true,
        );

        assert_eq!(
            flow.accessibility_state,
            GlobalHotkeyPermissionState::NeedsRelaunch
        );
        assert_eq!(
            flow.input_monitoring_state,
            GlobalHotkeyPermissionState::NeedsRelaunch
        );
        assert_eq!(flow.microphone_state, GlobalHotkeyPermissionState::Granted);
        assert!(flow.relaunch_required());
    }

    #[test]
    fn flow_reports_requested_for_permissions_still_missing_after_request() {
        let flow = build_global_hotkey_permission_flow(
            GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
                microphone_granted: false,
            },
            GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
                microphone_granted: false,
            },
            true,
            false,
            true,
        );

        assert_eq!(
            flow.accessibility_state,
            GlobalHotkeyPermissionState::Requested
        );
        assert_eq!(
            flow.input_monitoring_state,
            GlobalHotkeyPermissionState::Missing
        );
        assert_eq!(
            flow.microphone_state,
            GlobalHotkeyPermissionState::Requested
        );
        assert!(!flow.relaunch_required());
    }

    #[test]
    fn flow_keeps_granted_permissions_granted_when_present_at_startup() {
        let flow = build_global_hotkey_permission_flow(
            GlobalHotkeyPermissions {
                accessibility_granted: true,
                input_monitoring_granted: true,
                microphone_granted: true,
            },
            GlobalHotkeyPermissions {
                accessibility_granted: true,
                input_monitoring_granted: true,
                microphone_granted: true,
            },
            false,
            false,
            false,
        );

        assert_eq!(
            flow.accessibility_state,
            GlobalHotkeyPermissionState::Granted
        );
        assert_eq!(
            flow.input_monitoring_state,
            GlobalHotkeyPermissionState::Granted
        );
        assert_eq!(flow.microphone_state, GlobalHotkeyPermissionState::Granted);
        assert!(flow.all_granted());
        assert!(!flow.relaunch_required());
    }

    #[test]
    fn microphone_denied_maps_to_requested_state() {
        assert_eq!(
            resolve_microphone_permission_state(false, AVAuthorizationStatus::Denied, false),
            GlobalHotkeyPermissionState::Requested
        );
    }

    #[test]
    fn app_bundle_path_is_derived_from_bundle_executable() {
        let bundle_path = app_bundle_path_from_executable(Path::new(
            "/Applications/simple-ptt.app/Contents/MacOS/simple-ptt",
        ));

        assert_eq!(
            bundle_path,
            Some(Path::new("/Applications/simple-ptt.app").to_path_buf())
        );
    }

    #[test]
    fn app_bundle_path_is_none_for_non_bundle_executable() {
        let bundle_path = app_bundle_path_from_executable(Path::new("/usr/local/bin/simple-ptt"));

        assert_eq!(bundle_path, None);
    }
}
