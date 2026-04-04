use std::path::{Path, PathBuf};
use std::process::Command;

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlobalHotkeyPermissionFlow {
    pub permissions: GlobalHotkeyPermissions,
    pub accessibility_state: GlobalHotkeyPermissionState,
    pub input_monitoring_state: GlobalHotkeyPermissionState,
}

impl GlobalHotkeyPermissions {
    pub fn current() -> Self {
        Self {
            accessibility_granted: has_accessibility_access(),
            input_monitoring_granted: has_input_monitoring_access(),
        }
    }

    pub fn all_granted(self) -> bool {
        self.accessibility_granted && self.input_monitoring_granted
    }
}

impl GlobalHotkeyPermissionFlow {
    pub fn unknown() -> Self {
        Self {
            permissions: GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
            },
            accessibility_state: GlobalHotkeyPermissionState::Unknown,
            input_monitoring_state: GlobalHotkeyPermissionState::Unknown,
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
) -> GlobalHotkeyPermissionFlow {
    build_global_hotkey_permission_flow(
        startup_permissions,
        GlobalHotkeyPermissions::current(),
        accessibility_requested,
        input_monitoring_requested,
    )
}

pub fn request_input_monitoring_access() -> Result<bool, String> {
    Ok(CGRequestListenEventAccess())
}

pub fn input_monitoring_settings_urls() -> [&'static str; 2] {
    [
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ListenEvent",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
    ]
}

pub fn accessibility_settings_urls() -> [&'static str; 2] {
    [
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    ]
}

pub fn reset_global_hotkey_permissions() -> Result<(), String> {
    run_tccutil_reset("Accessibility")?;
    run_tccutil_reset("ListenEvent")?;
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
) -> GlobalHotkeyPermissionFlow {
    GlobalHotkeyPermissionFlow {
        permissions: current_permissions,
        accessibility_state: resolve_permission_state(
            startup_permissions.accessibility_granted,
            current_permissions.accessibility_granted,
            accessibility_requested,
        ),
        input_monitoring_state: resolve_permission_state(
            startup_permissions.input_monitoring_granted,
            current_permissions.input_monitoring_granted,
            input_monitoring_requested,
        ),
    }
}

fn resolve_permission_state(
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

fn run_tccutil_reset(service: &str) -> Result<(), String> {
    let output = Command::new("/usr/bin/tccutil")
        .args(["reset", service, APP_BUNDLE_IDENTIFIER])
        .output()
        .map_err(|error| format!("failed to run tccutil reset {}: {}", service, error))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        return Err(format!(
            "tccutil reset {} failed with status {}",
            service, output.status
        ));
    }

    Err(format!("tccutil reset {} failed: {}", service, stderr))
}

fn has_input_monitoring_access() -> bool {
    CGPreflightListenEventAccess()
}

fn has_accessibility_access() -> bool {
    unsafe { AXIsProcessTrusted() }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        app_bundle_path_from_executable, build_global_hotkey_permission_flow,
        GlobalHotkeyPermissionFlow, GlobalHotkeyPermissionState, GlobalHotkeyPermissions,
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
        assert!(!flow.all_granted());
        assert!(!flow.relaunch_required());
    }

    #[test]
    fn flow_requires_relaunch_when_permissions_were_missing_at_startup() {
        let flow = build_global_hotkey_permission_flow(
            GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
            },
            GlobalHotkeyPermissions {
                accessibility_granted: true,
                input_monitoring_granted: true,
            },
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
        assert!(flow.relaunch_required());
    }

    #[test]
    fn flow_reports_requested_for_permissions_still_missing_after_request() {
        let flow = build_global_hotkey_permission_flow(
            GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
            },
            GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
            },
            true,
            false,
        );

        assert_eq!(
            flow.accessibility_state,
            GlobalHotkeyPermissionState::Requested
        );
        assert_eq!(
            flow.input_monitoring_state,
            GlobalHotkeyPermissionState::Missing
        );
        assert!(!flow.relaunch_required());
    }

    #[test]
    fn flow_keeps_granted_permissions_granted_when_present_at_startup() {
        let flow = build_global_hotkey_permission_flow(
            GlobalHotkeyPermissions {
                accessibility_granted: true,
                input_monitoring_granted: true,
            },
            GlobalHotkeyPermissions {
                accessibility_granted: true,
                input_monitoring_granted: true,
            },
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
        assert!(flow.all_granted());
        assert!(!flow.relaunch_required());
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
