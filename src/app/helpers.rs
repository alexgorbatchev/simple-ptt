use std::path::Path;

use objc2_app_kit::NSAlert;
use objc2_foundation::{MainThreadMarker, NSString};

use crate::config::Config;
use crate::hotkey_binding::{parse_hotkey_binding, parse_key};
use crate::overlay::OverlayStyle;

pub fn validate_settings_config(config: &Config) -> Result<(), String> {
    let record_binding = parse_hotkey_binding(&config.ui.hotkey)?;
    let correction_key = parse_key(&config.ui.correction_key)?;

    if record_binding.modifiers.is_empty() && record_binding.key == correction_key {
        return Err("record hotkey and correction key must be different".to_owned());
    }

    Ok(())
}

pub fn overlay_style_from_config(config: &Config) -> OverlayStyle {
    let record_shortcut = format_short_shortcut(&config.ui.hotkey);

    let correction_label = match parse_key(&config.ui.correction_key) {
        Ok(crate::hotkey_binding::Key::Option) => "Alt",
        Ok(crate::hotkey_binding::Key::Command) => "Cmd",
        Ok(crate::hotkey_binding::Key::Control) => "Ctrl",
        Ok(crate::hotkey_binding::Key::Shift) => "Shift",
        _ => config.ui.correction_key.as_str(),
    };

    let transformation_shortcut = config
        .transformation
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .and_then(|_| config.transformation.hotkey.as_deref())
        .map(str::trim)
        .filter(|shortcut| !shortcut.is_empty())
        .map(format_short_shortcut);

    let shortcut_hint = match transformation_shortcut {
        Some(transform_shortcut) => format!(
            "<Hold {}> correction <{}> transform <{}> paste <Cmd+V> insert <ESC> cancel",
            correction_label, transform_shortcut, record_shortcut
        ),
        None => format!(
            "<Hold {}> correction <{}> paste <Cmd+V> insert <ESC> cancel",
            correction_label, record_shortcut
        ),
    };

    OverlayStyle {
        font_name: config.ui.font_name.clone(),
        font_size: config.ui.font_size,
        footer_font_size: config.ui.footer_font_size.unwrap_or(11.0),
        meter_style: config.ui.meter_style,
        shortcut_hint: Some(shortcut_hint),
    }
}

fn format_short_shortcut(shortcut: &str) -> String {
    shortcut
        .replace("Command+", "Cmd+")
        .replace("Option+", "Alt+")
        .replace("Control+", "Ctrl+")
}

pub fn billing_menu_text(overlay_footer_text: &str) -> Option<&str> {
    let trimmed_overlay_footer_text = overlay_footer_text.trim();
    if trimmed_overlay_footer_text.starts_with("Deepgram (")
        && trimmed_overlay_footer_text.contains(": $")
    {
        Some(trimmed_overlay_footer_text)
    } else {
        None
    }
}

pub fn config_file_is_missing(config_path: &Path) -> bool {
    !config_path.exists()
}

pub fn show_modal_alert(message_text: &str, informative_text: &str) {
    let mtm = MainThreadMarker::new().expect("must be on main thread");
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(message_text));
    alert.setInformativeText(&NSString::from_str(informative_text));
    alert.runModal();
}

pub fn show_startup_error_dialog(message_text: &str, informative_text: &str) {
    show_modal_alert(message_text, informative_text);
}

#[cfg(test)]
mod tests {
    use super::{
        billing_menu_text, config_file_is_missing, overlay_style_from_config,
        validate_settings_config,
    };
    use crate::config::Config;

    #[test]
    fn config_file_is_missing_only_reports_not_found_paths() {
        let path = std::env::temp_dir().join(format!(
            "simple-ptt-missing-config-{}-{}.toml",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));

        let _ = std::fs::remove_file(&path);
        assert!(config_file_is_missing(&path));

        std::fs::write(&path, "[ui]\nhotkey = \"F5\"\n").unwrap();
        assert!(!config_file_is_missing(&path));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn overlay_style_includes_clipboard_shortcut_without_transformation() {
        let config = Config::default();

        let style = overlay_style_from_config(&config);

        assert_eq!(
            style.shortcut_hint.as_deref(),
            Some("<Hold Cmd> correction <F5> paste <Cmd+V> insert <ESC> cancel")
        );
    }

    #[test]
    fn overlay_style_includes_transform_shortcut_when_transformation_is_configured() {
        let mut config = Config::default();
        config.transformation.provider = Some("openai".to_owned());

        let style = overlay_style_from_config(&config);

        assert_eq!(
            style.shortcut_hint.as_deref(),
            Some("<Hold Cmd> correction <F6> transform <F5> paste <Cmd+V> insert <ESC> cancel")
        );
    }

    #[test]
    fn overlay_style_uses_configured_correction_key_label() {
        let mut config = Config::default();
        config.ui.correction_key = "RightAlt".to_owned();

        let style = overlay_style_from_config(&config);

        assert_eq!(
            style.shortcut_hint.as_deref(),
            Some("<Hold Alt> correction <F5> paste <Cmd+V> insert <ESC> cancel")
        );
    }

    #[test]
    fn validate_settings_rejects_correction_key_that_overlaps_record_hotkey() {
        let mut config = Config::default();
        config.ui.hotkey = "Cmd+F5".to_owned();

        assert_eq!(
            validate_settings_config(&config).unwrap_err(),
            "record hotkey and correction key must be different"
        );
    }

    #[test]
    fn billing_menu_text_accepts_deepgram_monthly_spend_label() {
        assert_eq!(
            billing_menu_text("Deepgram (Apr 2026): $12.34"),
            Some("Deepgram (Apr 2026): $12.34")
        );
        assert_eq!(billing_menu_text("Billing (Apr 2026): $12.34"), None);
    }
}
