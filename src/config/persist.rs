use std::path::Path;
use toml_edit::{DocumentMut, Item};

use super::defaults::{
    default_always_on, default_auto_check_updates, default_correction_key,
    default_deepgram_language, default_deepgram_model, default_endpointing_ms, default_gain,
    default_hold_ms, default_hotkey, default_overlay_font_size, default_sample_rate,
    default_transformation_auto, default_transformation_hotkey, default_utterance_end_ms,
};
use super::file::{load_document, write_document_atomically};
use super::prompts::{
    default_transformation_correction_system_prompt, default_transformation_system_prompt,
};
use super::toml_keys::{
    ensure_named_table, root_table, set_defaulted_key, set_defaulted_string_key, set_float_key,
    set_optional_string_key, set_string_array_key,
};
use super::{
    Config, DeepgramConfig, DefaultResets, MicConfig, TransformationConfig, TransformationProvider,
    UiConfig, UiMeterStyle, DEFAULTED_KEYS,
};

/// The defaulted keys (`DEFAULTED_KEYS`) the config file at `path` has,
/// under their name or an alias. A missing file has none.
pub fn defaulted_keys_in_file(
    path: &Path,
) -> Result<std::collections::BTreeSet<&'static str>, String> {
    let document = load_document(path)?;
    Ok(DEFAULTED_KEYS
        .iter()
        .filter(|(path, aliases)| {
            let (section, key) = path
                .split_once('.')
                .expect("defaulted keys are section.key");
            document
                .get(section)
                .and_then(Item::as_table_like)
                .is_some_and(|table| {
                    table.contains_key(key) || aliases.iter().any(|alias| table.contains_key(alias))
                })
        })
        .map(|(path, _)| *path)
        .collect())
}

fn write_ui_table(
    document: &mut DocumentMut,
    ui: &UiConfig,
    resets: &DefaultResets,
) -> Result<(), String> {
    let table = ensure_named_table(document, "ui")?;
    set_defaulted_key(
        table,
        "start_on_login",
        &[],
        ui.start_on_login,
        false,
        resets.contains("ui.start_on_login"),
    );
    set_defaulted_key(
        table,
        "auto_check_updates",
        &[],
        ui.auto_check_updates,
        default_auto_check_updates(),
        resets.contains("ui.auto_check_updates"),
    );
    set_defaulted_key(
        table,
        "hotkey",
        &[],
        ui.hotkey.as_str(),
        &default_hotkey(),
        resets.contains("ui.hotkey"),
    );
    set_defaulted_key(
        table,
        "correction_key",
        &["instruction_key"],
        ui.correction_key.as_str(),
        &default_correction_key(),
        resets.contains("ui.correction_key"),
    );
    set_optional_string_key(
        table,
        "font_name",
        &["overlay_font_family"],
        ui.font_name.as_deref(),
    );
    set_defaulted_key(
        table,
        "font_size",
        &[],
        ui.font_size,
        default_overlay_font_size(),
        resets.contains("ui.font_size"),
    );
    match ui.footer_font_size {
        Some(footer_font_size) => set_float_key(table, "footer_font_size", footer_font_size),
        None => {
            table.remove("footer_font_size");
        }
    }
    set_defaulted_key(
        table,
        "meter_style",
        &[],
        meter_style_name(ui.meter_style),
        meter_style_name(UiMeterStyle::default()),
        resets.contains("ui.meter_style"),
    );
    Ok(())
}

fn write_mic_table(
    document: &mut DocumentMut,
    mic: &MicConfig,
    resets: &DefaultResets,
) -> Result<(), String> {
    let table = ensure_named_table(document, "mic")?;
    set_optional_string_key(table, "audio_device", &[], mic.audio_device.as_deref());
    set_defaulted_key(
        table,
        "sample_rate",
        &[],
        i64::from(mic.sample_rate),
        i64::from(default_sample_rate()),
        resets.contains("mic.sample_rate"),
    );
    set_defaulted_key(
        table,
        "gain",
        &[],
        f64::from(mic.gain),
        f64::from(default_gain()),
        resets.contains("mic.gain"),
    );
    set_defaulted_key(
        table,
        "hold_ms",
        &[],
        i64::try_from(mic.hold_ms).unwrap_or(i64::MAX),
        i64::try_from(default_hold_ms()).unwrap_or(i64::MAX),
        resets.contains("mic.hold_ms"),
    );
    set_defaulted_key(
        table,
        "always_on",
        &[],
        mic.always_on,
        default_always_on(),
        resets.contains("mic.always_on"),
    );
    Ok(())
}

fn write_deepgram_table(
    document: &mut DocumentMut,
    deepgram: &DeepgramConfig,
    resets: &DefaultResets,
) -> Result<(), String> {
    let table = ensure_named_table(document, "deepgram")?;
    set_optional_string_key(table, "api_key", &[], deepgram.api_key.as_deref());
    set_defaulted_key(
        table,
        "language",
        &[],
        deepgram.language.as_str(),
        &default_deepgram_language(),
        resets.contains("deepgram.language"),
    );
    set_defaulted_key(
        table,
        "model",
        &[],
        deepgram.model.as_str(),
        &default_deepgram_model(),
        resets.contains("deepgram.model"),
    );
    set_defaulted_key(
        table,
        "endpointing_ms",
        &[],
        i64::from(deepgram.endpointing_ms),
        i64::from(default_endpointing_ms()),
        resets.contains("deepgram.endpointing_ms"),
    );
    set_defaulted_key(
        table,
        "utterance_end_ms",
        &[],
        i64::from(deepgram.utterance_end_ms),
        i64::from(default_utterance_end_ms()),
        resets.contains("deepgram.utterance_end_ms"),
    );
    set_string_array_key(table, "keyterms", &deepgram.keyterms);
    Ok(())
}

fn write_transformation_table(
    document: &mut DocumentMut,
    transformation: &TransformationConfig,
    resets: &DefaultResets,
) -> Result<(), String> {
    let table = ensure_named_table(document, "transformation")?;
    set_defaulted_key(
        table,
        "hotkey",
        &[],
        transformation.hotkey.as_str(),
        &default_transformation_hotkey(),
        resets.contains("transformation.hotkey"),
    );
    set_defaulted_key(
        table,
        "auto",
        &[],
        transformation.auto,
        default_transformation_auto(),
        resets.contains("transformation.auto"),
    );
    set_optional_string_key(table, "provider", &[], transformation.provider.as_deref());
    set_optional_string_key(table, "api_key", &[], transformation.api_key.as_deref());
    set_optional_string_key(
        table,
        "ollama_base_url",
        &[],
        transformation.ollama_base_url.as_deref(),
    );
    let default_model = transformation
        .provider
        .as_deref()
        .and_then(|provider| provider.parse::<TransformationProvider>().ok())
        .and_then(TransformationProvider::default_model)
        .unwrap_or_default();
    set_defaulted_string_key(
        table,
        "model",
        &[],
        &transformation.model,
        default_model,
        resets.contains("transformation.model"),
    );
    set_defaulted_string_key(
        table,
        "system_prompt",
        &[],
        &transformation.system_prompt,
        &default_transformation_system_prompt(),
        resets.contains("transformation.system_prompt"),
    );
    set_defaulted_string_key(
        table,
        "correction_system_prompt",
        &["instruction_system_prompt"],
        &transformation.correction_system_prompt,
        &default_transformation_correction_system_prompt(),
        resets.contains("transformation.correction_system_prompt"),
    );
    Ok(())
}

const CONFIG_SECTIONS: [&str; 4] = ["ui", "mic", "deepgram", "transformation"];

fn meter_style_name(meter_style: UiMeterStyle) -> &'static str {
    match meter_style {
        UiMeterStyle::None => "none",
        UiMeterStyle::AnimatedHeight => "animated-height",
        UiMeterStyle::AnimatedColor => "animated-color",
        UiMeterStyle::Pills => "pills",
    }
}

pub fn save_config(path: &Path, config: &Config, resets: &DefaultResets) -> Result<(), String> {
    let mut document = load_document(path)?;
    let sections_in_file =
        CONFIG_SECTIONS.map(|section| root_table(&mut document).contains_key(section));
    write_ui_table(&mut document, &config.ui, resets)?;
    write_mic_table(&mut document, &config.mic, resets)?;
    write_deepgram_table(&mut document, &config.deepgram, resets)?;
    write_transformation_table(&mut document, &config.transformation, resets)?;
    // A section Save created only to hold keys that all stayed at their
    // defaults is left out too.
    let root = root_table(&mut document);
    for (section, in_file) in CONFIG_SECTIONS.into_iter().zip(sections_in_file) {
        let empty = root
            .get(section)
            .and_then(Item::as_table_like)
            .is_some_and(|table| table.is_empty());
        if !in_file && empty {
            root.remove(section);
        }
    }
    write_document_atomically(path, &document.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        default_transformation_correction_system_prompt, default_transformation_system_prompt,
        save_config, Config, DefaultResets, UiMeterStyle,
    };
    use crate::config::test_support::{assert_example_config_values, EXAMPLE_CONFIG};

    #[test]
    fn pills_meter_style_is_saved_and_read_as_pills() {
        let temp_directory = std::env::temp_dir().join(format!(
            "simple-ptt-config-pills-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        std::fs::write(&path, "[ui]\nmeter_style = \"animated-height\"\n").unwrap();

        let mut config = Config::default();
        config.ui.meter_style = UiMeterStyle::Pills;
        save_config(&path, &config, &DefaultResets::default()).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert!(contents.contains("meter_style = \"pills\""), "{contents}");
        let read: Config = toml::from_str(&contents).unwrap();
        assert_eq!(read.ui.meter_style, UiMeterStyle::Pills);
    }

    #[test]
    fn save_config_preserves_unknown_sections_and_comments() {
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        std::fs::write(
            &path,
            concat!(
                "# top comment\n",
                "[ui]\n",
                "# keep this comment\n",
                "hotkey = \"F4\"\n",
                "overlay_font_family = \"Menlo\"\n",
                "\n",
                "[custom]\n",
                "keep_me = true\n"
            ),
        )
        .unwrap();

        let mut config = Config::default();
        config.ui.hotkey = "F5".to_owned();
        config.ui.correction_key = "Alt+Cmd".to_owned();
        config.ui.font_name = Some("SF Mono".to_owned());
        config.ui.font_size = 14.0;
        config.ui.footer_font_size = Some(11.0);
        config.ui.meter_style = UiMeterStyle::AnimatedHeight;
        config.transformation.correction_system_prompt = "Apply the spoken correction.".to_owned();

        save_config(&path, &config, &DefaultResets::default()).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();

        assert!(updated_contents.contains("# top comment"));
        assert!(updated_contents.contains("# keep this comment"));
        assert!(updated_contents.contains("[custom]"));
        assert!(updated_contents.contains("keep_me = true"));
        assert!(updated_contents.contains("overlay_font_family = \"SF Mono\""));
        assert!(updated_contents.contains("hotkey = \"F5\""));
        // Alt+Cmd is the default correction key and the file had none.
        assert!(!updated_contents.contains("correction_key"));
        assert!(updated_contents
            .contains("correction_system_prompt = \"Apply the spoken correction.\""));
    }

    #[test]
    fn save_config_keeps_every_example_config_line_in_order_and_reparses() {
        let temp_directory = std::env::temp_dir().join(format!(
            "simple-ptt-example-config-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        std::fs::write(&path, EXAMPLE_CONFIG).unwrap();
        let config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();

        save_config(&path, &config, &DefaultResets::default()).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        // Every original line (comments, blank lines, headers, and unchanged
        // key/value lines) must survive in its original order.
        let mut updated_lines = updated_contents.lines();
        for original_line in EXAMPLE_CONFIG.lines() {
            assert!(
                updated_lines.any(|updated_line| updated_line == original_line),
                "line {:?} was lost or reordered in:\n{}",
                original_line,
                updated_contents
            );
        }

        let reparsed: Config = toml::from_str(&updated_contents).unwrap();
        assert_example_config_values(&reparsed);
    }

    #[test]
    fn save_config_writes_keyterms_as_inline_string_array() {
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-keyterms-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");

        let mut config = Config::default();
        config.deepgram.keyterms = vec!["macOS".to_owned(), "GitHub".to_owned()];
        save_config(&path, &config, &DefaultResets::default()).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert!(
            updated_contents.contains("keyterms = [\"macOS\", \"GitHub\"]\n"),
            "unexpected keyterms rendering in:\n{}",
            updated_contents
        );
        let reparsed: Config = toml::from_str(&updated_contents).unwrap();
        assert_eq!(reparsed.deepgram.keyterms, config.deepgram.keyterms);
    }

    /// Saves `config` over a file holding `initial_contents` (no file when
    /// `None`) and returns the `[transformation]` table that was written.
    fn saved_transformation_table(
        test_name: &str,
        initial_contents: Option<&str>,
        config: &Config,
        resets: DefaultResets,
    ) -> toml_edit::Table {
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-{}-{}", test_name, std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        if let Some(initial_contents) = initial_contents {
            std::fs::write(&path, initial_contents).unwrap();
        }

        save_config(&path, config, &resets).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        toml::from_str::<Config>(&updated_contents).unwrap();
        // A [transformation] table whose keys all stay at their defaults is
        // left out of a new file.
        updated_contents
            .parse::<toml_edit::DocumentMut>()
            .unwrap()
            .get("transformation")
            .map(|item| {
                item.as_table()
                    .expect("[transformation] is a table")
                    .clone()
            })
            .unwrap_or_default()
    }

    #[test]
    fn saving_the_default_config_to_a_new_file_writes_no_defaulted_key() {
        let temp_directory = std::env::temp_dir().join(format!(
            "simple-ptt-defaults-everywhere-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");

        save_config(&path, &Config::default(), &DefaultResets::default()).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert_eq!(contents.trim(), "", "a default config wrote:\n{contents}");
    }

    #[test]
    fn a_reset_key_the_file_has_is_removed_while_it_equals_its_default() {
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-reset-keys-{}", std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        std::fs::write(
            &path,
            "[ui]\nhotkey = \"F5\"\nstart_on_login = false\n[mic]\nhold_ms = 300 # mine\n",
        )
        .unwrap();
        let mut config = Config::default();
        config.mic.hold_ms = 450;

        assert_eq!(
            super::defaulted_keys_in_file(&path).unwrap(),
            std::collections::BTreeSet::from(["ui.hotkey", "ui.start_on_login", "mic.hold_ms"])
        );
        let resets = DefaultResets::from_iter(["ui.hotkey", "ui.start_on_login", "mic.hold_ms"]);
        save_config(&path, &config, &resets).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        // The two defaults go; the reset value the user then changed stays.
        assert_eq!(contents, "[ui]\n[mic]\nhold_ms = 450 # mine\n");
    }

    /// Every key in `DEFAULTED_KEYS` is one `save_config` writes and, once
    /// reset, removes again.
    #[test]
    fn every_defaulted_key_is_written_when_changed_and_removed_when_reset() {
        let temp_directory = std::env::temp_dir().join(format!(
            "simple-ptt-every-defaulted-key-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        let mut changed = Config::default();
        changed.ui.start_on_login = true;
        changed.ui.auto_check_updates = false;
        changed.ui.hotkey = "F9".to_owned();
        changed.ui.correction_key = "F7".to_owned();
        changed.ui.font_size = 15.0;
        changed.ui.meter_style = UiMeterStyle::None;
        changed.mic.sample_rate = 48000;
        changed.mic.gain = 6.0;
        changed.mic.hold_ms = 450;
        changed.mic.always_on = false;
        changed.deepgram.language = "de-DE".to_owned();
        changed.deepgram.model = "nova-2".to_owned();
        changed.deepgram.endpointing_ms = 450;
        changed.deepgram.utterance_end_ms = 1500;
        changed.transformation.hotkey = "F8".to_owned();
        changed.transformation.auto = false;
        changed.transformation.model = "custom-model".to_owned();
        changed.transformation.system_prompt = "Mine.".to_owned();
        changed.transformation.correction_system_prompt = "Mine too.".to_owned();

        save_config(&path, &changed, &DefaultResets::default()).unwrap();
        let written = super::defaulted_keys_in_file(&path).unwrap();
        let every_key = super::DEFAULTED_KEYS
            .iter()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        assert_eq!(
            written.into_iter().collect::<Vec<_>>().len(),
            every_key.len()
        );

        let all_reset = DefaultResets::from_iter(every_key.iter().copied());
        save_config(&path, &Config::default(), &all_reset).unwrap();
        let remaining = super::defaulted_keys_in_file(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert!(remaining.is_empty(), "{remaining:?}");
    }

    #[test]
    fn a_missing_file_has_no_defaulted_keys() {
        let path = std::env::temp_dir()
            .join(format!("simple-ptt-missing-{}", std::process::id()))
            .join("config.toml");

        assert!(super::defaulted_keys_in_file(&path).unwrap().is_empty());
    }

    #[test]
    fn save_config_writes_only_the_values_that_differ_from_their_defaults() {
        let temp_directory = std::env::temp_dir().join(format!(
            "simple-ptt-changed-defaults-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        let mut config = Config::default();
        config.mic.gain = 6.0;
        config.deepgram.endpointing_ms = 450;

        save_config(&path, &config, &DefaultResets::default()).unwrap();
        let document = std::fs::read_to_string(&path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert_eq!(document["mic"].as_table().unwrap().len(), 1, "{document}");
        assert_eq!(document["mic"]["gain"].as_float(), Some(6.0));
        assert_eq!(
            document["deepgram"].as_table().unwrap().len(),
            1,
            "{document}"
        );
        assert_eq!(
            document["deepgram"]["endpointing_ms"].as_integer(),
            Some(450)
        );
        assert!(!document.contains_key("ui"), "{document}");
        assert!(!document.contains_key("transformation"), "{document}");
    }

    /// Every defaulted key the user already wrote stays, at its default
    /// value too, with the comment after it.
    #[test]
    fn save_config_keeps_default_valued_keys_of_every_section_and_their_comments() {
        let mut defaults = Config::default();
        defaults.transformation.model = "gpt-x".to_owned();
        defaults.transformation.system_prompt = "Mine.".to_owned();
        let initial_contents = format!(
            concat!(
                "[ui]\nhotkey = {:?} # keep me\ninstruction_key = {:?}\nfont_size = {:?}\n",
                "meter_style = \"pills\"\nstart_on_login = false\nauto_check_updates = true\n",
                "[mic]\nsample_rate = {}\ngain = {:?}\nhold_ms = {}\nalways_on = {}\n",
                "[deepgram]\nlanguage = {:?}\nmodel = {:?}\nendpointing_ms = {}\nutterance_end_ms = {}\n",
                "[transformation]\nhotkey = {:?}\nauto = {}\nmodel = \"gpt-x\" # mine\n",
                "system_prompt = \"Mine.\" # prompt\n"
            ),
            defaults.ui.hotkey,
            defaults.ui.correction_key,
            defaults.ui.font_size,
            defaults.mic.sample_rate,
            f64::from(defaults.mic.gain),
            defaults.mic.hold_ms,
            defaults.mic.always_on,
            defaults.deepgram.language,
            defaults.deepgram.model,
            defaults.deepgram.endpointing_ms,
            defaults.deepgram.utterance_end_ms,
            defaults.transformation.hotkey,
            defaults.transformation.auto,
        );
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-defaults-kept-{}", std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");
        std::fs::write(&path, &initial_contents).unwrap();

        save_config(&path, &defaults, &DefaultResets::default()).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&temp_directory).unwrap();

        assert_eq!(contents, initial_contents);
    }

    #[test]
    fn save_config_leaves_default_prompts_and_model_out_of_a_new_file() {
        let table = saved_transformation_table(
            "defaults-new-file",
            None,
            &Config::default(),
            DefaultResets::default(),
        );

        for key in [
            "model",
            "system_prompt",
            "correction_system_prompt",
            "instruction_system_prompt",
        ] {
            assert!(!table.contains_key(key), "{key} was written:\n{table}");
        }
    }

    #[test]
    fn save_config_leaves_default_prompts_and_model_out_of_the_example_config() {
        let config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();

        let table = saved_transformation_table(
            "defaults-example",
            Some(EXAMPLE_CONFIG),
            &config,
            DefaultResets::default(),
        );

        for key in ["model", "system_prompt", "correction_system_prompt"] {
            assert!(!table.contains_key(key), "{key} was written:\n{table}");
        }
    }

    #[test]
    fn save_config_compares_the_model_with_the_configured_providers_default() {
        let mut config = Config::default();
        config.transformation.provider = Some("anthropic".to_owned());
        config.transformation.model = "claude-haiku-5-5".to_owned();
        let table =
            saved_transformation_table("provider-default", None, &config, DefaultResets::default());
        assert!(!table.contains_key("model"), "model was written:\n{table}");

        config.transformation.model = "gpt-5.4-mini".to_owned();
        let table =
            saved_transformation_table("other-default", None, &config, DefaultResets::default());
        assert_eq!(table["model"].as_str(), Some("gpt-5.4-mini"));
    }

    #[test]
    fn save_config_writes_customized_prompts_and_model() {
        let mut config = Config::default();
        config.transformation.model = "claude-test".to_owned();
        config.transformation.system_prompt = "Custom dictation prompt.".to_owned();
        config.transformation.correction_system_prompt = "Custom correction prompt.".to_owned();

        let table =
            saved_transformation_table("customized", None, &config, DefaultResets::default());

        assert_eq!(table["model"].as_str(), Some("claude-test"));
        assert_eq!(
            table["system_prompt"].as_str(),
            Some("Custom dictation prompt.")
        );
        assert_eq!(
            table["correction_system_prompt"].as_str(),
            Some("Custom correction prompt.")
        );
    }

    /// A key the file already has stays, even at its default value: the user
    /// (or an earlier Save) put it there, and dropping it would silently
    /// discard a line of their config.
    #[test]
    fn save_config_keeps_default_valued_keys_the_file_already_has() {
        let initial_contents = format!(
            "[transformation]\nprovider = \"openai\"\nmodel = \"gpt-5.4-mini\"\nsystem_prompt = {:?}\ninstruction_system_prompt = {:?}\n",
            default_transformation_system_prompt(),
            default_transformation_correction_system_prompt(),
        );
        let mut config = Config::default();
        config.transformation.provider = Some("openai".to_owned());
        config.transformation.model = "gpt-5.4-mini".to_owned();

        let table = saved_transformation_table(
            "defaults-present",
            Some(&initial_contents),
            &config,
            DefaultResets::default(),
        );

        assert_eq!(table["model"].as_str(), Some("gpt-5.4-mini"));
        assert_eq!(
            table["system_prompt"].as_str(),
            Some(default_transformation_system_prompt().as_str())
        );
        assert_eq!(
            table["instruction_system_prompt"].as_str(),
            Some(default_transformation_correction_system_prompt().as_str())
        );
        assert!(!table.contains_key("correction_system_prompt"));
    }

    #[test]
    fn save_config_removes_reset_prompts_the_file_already_has() {
        let initial_contents = concat!(
            "[transformation]\n",
            "system_prompt = \"Old custom dictation prompt.\"\n",
            "instruction_system_prompt = \"Old custom correction prompt.\"\n",
        );

        let table = saved_transformation_table(
            "reset-present",
            Some(initial_contents),
            &Config::default(),
            DefaultResets::from_iter([
                "transformation.system_prompt",
                "transformation.correction_system_prompt",
            ]),
        );

        for key in [
            "system_prompt",
            "correction_system_prompt",
            "instruction_system_prompt",
        ] {
            assert!(!table.contains_key(key), "{key} was kept:\n{table}");
        }
    }

    /// A reset only drops the key while the editor still holds the default;
    /// text edited after the reset is a customization and is saved.
    #[test]
    fn save_config_writes_prompts_edited_after_a_reset() {
        let mut config = Config::default();
        config.transformation.system_prompt = "Edited after reset.".to_owned();

        let table = saved_transformation_table(
            "reset-edited",
            Some("[transformation]\nsystem_prompt = \"Old custom prompt.\"\n"),
            &config,
            DefaultResets::from_iter([
                "transformation.system_prompt",
                "transformation.correction_system_prompt",
            ]),
        );

        assert_eq!(table["system_prompt"].as_str(), Some("Edited after reset."));
        assert!(!table.contains_key("correction_system_prompt"));
    }

    /// The runtime trims prompts and treats a blank one as the default, so a
    /// prompt that differs from the default only in surrounding whitespace is
    /// the default.
    #[test]
    fn save_config_treats_whitespace_padded_default_prompt_as_default() {
        let mut config = Config::default();
        config.transformation.system_prompt =
            format!("\n{}\n", default_transformation_system_prompt());
        config.transformation.correction_system_prompt = "  ".to_owned();

        let table =
            saved_transformation_table("defaults-padded", None, &config, DefaultResets::default());

        assert!(!table.contains_key("system_prompt"), "{table}");
        assert!(!table.contains_key("correction_system_prompt"), "{table}");
    }

    #[test]
    fn save_config_removes_blank_prompts_the_file_already_has() {
        let mut config = Config::default();
        config.transformation.system_prompt = "".to_owned();
        config.transformation.correction_system_prompt = " \n ".to_owned();

        let table = saved_transformation_table(
            "blank-present",
            Some(concat!(
                "[transformation]\n",
                "system_prompt = \"Old custom dictation prompt.\"\n",
                "instruction_system_prompt = \"Old custom correction prompt.\"\n",
            )),
            &config,
            DefaultResets::default(),
        );

        for key in [
            "system_prompt",
            "correction_system_prompt",
            "instruction_system_prompt",
        ] {
            assert!(!table.contains_key(key), "{key} was kept:\n{table}");
        }
    }

    #[test]
    fn auto_check_updates_defaults_to_true_and_roundtrips() {
        let mut config = Config::default();
        assert!(config.ui.auto_check_updates);

        config.ui.auto_check_updates = false;
        let temp_dir =
            std::env::temp_dir().join(format!("simple-ptt-autocheck-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("config.toml");

        save_config(&path, &config, &DefaultResets::default()).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("auto_check_updates = false"));
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn always_on_defaults_to_true_and_roundtrips() {
        let mut config = Config::default();
        assert!(config.mic.always_on);

        config.mic.always_on = false;
        let temp_directory =
            std::env::temp_dir().join(format!("simple-ptt-always-on-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_directory).unwrap();
        let path = temp_directory.join("config.toml");

        save_config(&path, &config, &DefaultResets::default()).unwrap();
        let updated_contents = std::fs::read_to_string(&path).unwrap();
        assert!(updated_contents.contains("always_on = false"));
        std::fs::remove_dir_all(&temp_directory).unwrap();
    }
}
