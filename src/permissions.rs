use std::ffi::c_void;

use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};
use objc2_core_graphics::{CGPreflightListenEventAccess, CGRequestListenEventAccess};

extern "C-unwind" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
}

pub fn ensure_global_hotkey_permissions() -> Result<(), String> {
    let mut missing_permissions = Vec::new();

    if !has_input_monitoring_access() {
        let _ = request_input_monitoring_access();
        if !has_input_monitoring_access() {
            missing_permissions.push("Input Monitoring");
        }
    }

    if !has_accessibility_access() {
        let _ = request_accessibility_access();
        if !has_accessibility_access() {
            missing_permissions.push("Accessibility");
        }
    }

    if missing_permissions.is_empty() {
        return Ok(());
    }

    Err(format!(
        "global hotkeys require {} access",
        format_permission_list(&missing_permissions)
    ))
}

fn has_input_monitoring_access() -> bool {
    CGPreflightListenEventAccess()
}

fn request_input_monitoring_access() -> bool {
    CGRequestListenEventAccess()
}

fn has_accessibility_access() -> bool {
    unsafe { AXIsProcessTrusted() }
}

fn request_accessibility_access() -> bool {
    let prompt_key = CFString::from_str("AXTrustedCheckOptionPrompt");
    let options = CFDictionary::from_slices(&[&*prompt_key], &[CFBoolean::new(true)]);

    unsafe { AXIsProcessTrustedWithOptions((&*options) as *const _ as *const c_void) }
}

fn format_permission_list(permissions: &[&str]) -> String {
    match permissions {
        [] => "".to_owned(),
        [only] => (*only).to_owned(),
        [first, second] => format!("{} and {}", first, second),
        [rest @ .., last] => format!("{}, and {}", rest.join(", "), last),
    }
}

#[cfg(test)]
mod tests {
    use super::format_permission_list;

    #[test]
    fn formats_one_permission_name() {
        assert_eq!(format_permission_list(&["Input Monitoring"]), "Input Monitoring");
    }

    #[test]
    fn formats_two_permission_names() {
        assert_eq!(
            format_permission_list(&["Input Monitoring", "Accessibility"]),
            "Input Monitoring and Accessibility"
        );
    }

    #[test]
    fn formats_three_permission_names() {
        assert_eq!(
            format_permission_list(&["Input Monitoring", "Accessibility", "Microphone"]),
            "Input Monitoring, Accessibility, and Microphone"
        );
    }
}
