use std::process::Command;

use objc2_core_graphics::CGPreflightListenEventAccess;

const APP_BUNDLE_IDENTIFIER: &str = "io.github.alexgorbatchev.simple-ptt";

extern "C-unwind" {
    fn AXIsProcessTrusted() -> bool;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlobalHotkeyPermissions {
    pub accessibility_granted: bool,
    pub input_monitoring_granted: bool,
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

pub fn request_input_monitoring_access() -> Result<(), String> {
    open_system_settings_url(
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ListenEvent",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
    )
}

pub fn request_accessibility_access() -> Result<(), String> {
    if has_accessibility_access() {
        return Ok(());
    }

    open_system_settings_url(
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    )
}

pub fn reset_global_hotkey_permissions() -> Result<(), String> {
    run_tccutil_reset("Accessibility")?;
    run_tccutil_reset("ListenEvent")?;
    Ok(())
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

fn open_system_settings_url(primary_url: &str, fallback_url: &str) -> Result<(), String> {
    if open_url(primary_url).is_ok() {
        return Ok(());
    }

    open_url(fallback_url)
}

fn open_url(url: &str) -> Result<(), String> {
    let output = Command::new("/usr/bin/open")
        .arg(url)
        .output()
        .map_err(|error| format!("failed to open {}: {}", url, error))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        return Err(format!("open {} failed with status {}", url, output.status));
    }

    Err(format!("open {} failed: {}", url, stderr))
}

fn has_input_monitoring_access() -> bool {
    CGPreflightListenEventAccess()
}

fn has_accessibility_access() -> bool {
    unsafe { AXIsProcessTrusted() }
}
