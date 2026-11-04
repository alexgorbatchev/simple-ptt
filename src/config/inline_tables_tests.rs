use super::test_support::TestDirectory;
use super::{save_config, Config, DefaultResets};

fn save_inline_section(section: &str, fields: &str) {
    let contents = format!(
        concat!(
            "# config header\n# managed section\n",
            "{section} = {{ {fields}, future = {{ nested = \"keep\" }} }} # section comment\n",
            "\n# custom table\n[custom]\n# custom key\nkeep_me = true\n"
        ),
        section = section,
        fields = fields,
    );
    let directory = TestDirectory::with_config(&contents);
    let path = directory.config();
    // The same deserializer used by load_config accepts the inline section.
    let mut config: Config = toml::from_str(&contents).unwrap();
    config.ui.hotkey = "F7".to_owned();
    config.mic.gain = 5.0;
    config.deepgram.language = "fr".to_owned();
    config.transformation.auto = !config.transformation.auto;

    save_config(&path, &config, &DefaultResets::default()).unwrap();

    let saved = std::fs::read_to_string(&path).unwrap();
    assert_eq!(toml::from_str::<Config>(&saved).unwrap(), config);
    let document = saved.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(document[section]["future"]["nested"].as_str(), Some("keep"));
    assert_eq!(document["custom"]["keep_me"].as_bool(), Some(true));
    for comment in [
        "# config header",
        "# managed section",
        "# section comment",
        "# custom table",
        "# custom key",
    ] {
        assert!(saved.contains(comment), "lost {comment}:\n{saved}");
    }
}

#[test]
fn save_updates_inline_ui_and_preserves_unknown_content() {
    save_inline_section("ui", "hotkey = \"F4\"");
}

#[test]
fn save_updates_inline_mic_and_preserves_unknown_content() {
    save_inline_section("mic", "gain = 3.0");
}

#[test]
fn save_updates_inline_deepgram_and_preserves_unknown_content() {
    save_inline_section("deepgram", "language = \"en\"");
}

#[test]
fn save_updates_inline_transformation_and_preserves_unknown_content() {
    save_inline_section("transformation", "auto = false");
}

#[test]
fn non_table_managed_sections_return_errors_without_changing_the_file() {
    for section in ["ui", "mic", "deepgram", "transformation"] {
        for value in ["\"not a table\"", "42", "true", "[]"] {
            let contents = format!("{section} = {value}\n");
            let directory = TestDirectory::with_config(&contents);
            let path = directory.config();
            let error =
                save_config(&path, &Config::default(), &DefaultResets::default()).unwrap_err();
            assert!(error.contains(section), "{section}: {error}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        }
    }
}

#[test]
fn managed_arrays_of_tables_return_errors_without_changing_the_file() {
    for section in ["ui", "mic", "deepgram", "transformation"] {
        let contents = format!("[[{section}]]\ncustom = true\n");
        let directory = TestDirectory::with_config(&contents);
        let path = directory.config();
        let error = save_config(&path, &Config::default(), &DefaultResets::default()).unwrap_err();
        assert!(error.contains(section), "{section}: {error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }
}
