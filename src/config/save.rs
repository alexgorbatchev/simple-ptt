#![allow(dead_code)]

use std::path::Path;
use toml_edit::{value, Array, DocumentMut, Item, Table};

use super::{
    Config, DeepgramConfig, MicConfig, TransformationConfig, UiConfig,
};

pub fn save_config(path: &Path, config: &Config) -> Result<(), String> {
    let mut document = load_document(path)?;
    write_ui_table(&mut document, &config.ui);
    write_mic_table(&mut document, &config.mic);
    write_deepgram_table(&mut document, &config.deepgram);
    write_transformation_table(&mut document, &config.transformation);
    write_document_atomically(path, &document.to_string())
}

fn load_document(path: &Path) -> Result<DocumentMut, String> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }

    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {}", path.display(), error))?;

    contents
        .parse::<DocumentMut>()
        .map_err(|error| format!("failed to parse {}: {}", path.display(), error))
}

fn write_document_atomically(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create directory {}: {}", parent.display(), error))?;
    }

    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, contents)
        .map_err(|error| format!("failed to write temporary file {}: {}", temp_path.display(), error))?;

    std::fs::rename(&temp_path, path)
        .map_err(|error| format!("failed to rename temporary file to {}: {}", path.display(), error))
}

fn ensure_named_table<'a>(document: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    if !document.contains_key(name) {
        document[name] = Item::Table(Table::new());
    }
    document[name].as_table_mut().expect("table must be valid")
}

fn set_required_string_key(
    table: &mut Table,
    primary_key: &str,
    legacy_keys: &[&str],
    value_str: &str,
) {
    for legacy_key in legacy_keys {
        table.remove(legacy_key);
    }
    table[primary_key] = value(value_str);
}

fn set_optional_string_key(
    table: &mut Table,
    primary_key: &str,
    legacy_keys: &[&str],
    value_opt: Option<&str>,
) {
    for legacy_key in legacy_keys {
        table.remove(legacy_key);
    }

    match value_opt.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value_str) => table[primary_key] = value(value_str),
        None => {
            table.remove(primary_key);
        }
    }
}

fn set_unsigned_key(table: &mut Table, key: &str, val: i64) {
    table[key] = value(val);
}

fn set_float_key(table: &mut Table, key: &str, val: f64) {
    table[key] = value(val);
}

fn set_string_array_key(table: &mut Table, key: &str, values: &[String]) {
    if values.is_empty() {
        table.remove(key);
        return;
    }

    let mut array = Array::new();
    for val in values {
        array.push(val.as_str());
    }
    table[key] = Item::Value(toml_edit::Value::Array(array));
}

fn write_ui_table(document: &mut DocumentMut, ui: &UiConfig) {
    let table = ensure_named_table(document, "ui");
    table["start_on_login"] = value(ui.start_on_login);
    set_required_string_key(table, "hotkey", &["record_hotkey"], &ui.hotkey);
    set_required_string_key(table, "correction_key", &[], &ui.correction_key);
    set_optional_string_key(
        table,
        "font_name",
        &["overlay_font_family"],
        ui.font_name.as_deref(),
    );
    set_float_key(table, "font_size", ui.font_size);

    match ui.footer_font_size {
        Some(font_size) => set_float_key(table, "footer_font_size", font_size),
        None => {
            table.remove("footer_font_size");
        }
    }

    let meter_style_str = match ui.meter_style {
        crate::config::UiMeterStyle::AnimatedColor => "animated-color",
        crate::config::UiMeterStyle::AnimatedHeight => "animated-height",
        crate::config::UiMeterStyle::None => "none",
    };
    table["meter_style"] = value(meter_style_str);
}

fn write_mic_table(document: &mut DocumentMut, mic: &MicConfig) {
    let table = ensure_named_table(document, "mic");
    set_optional_string_key(table, "audio_device", &[], mic.audio_device.as_deref());
    set_unsigned_key(table, "sample_rate", i64::from(mic.sample_rate));
    set_float_key(table, "gain", f64::from(mic.gain));
    set_unsigned_key(table, "hold_ms", mic.hold_ms as i64);
    table["always_on"] = value(mic.always_on);
}

fn write_deepgram_table(document: &mut DocumentMut, deepgram: &DeepgramConfig) {
    let table = ensure_named_table(document, "deepgram");
    set_optional_string_key(table, "api_key", &[], deepgram.api_key.as_deref());
    set_optional_string_key(table, "project_id", &[], deepgram.project_id.as_deref());
    set_required_string_key(table, "language", &[], &deepgram.language);
    set_required_string_key(table, "model", &[], &deepgram.model);
    set_unsigned_key(table, "endpointing_ms", i64::from(deepgram.endpointing_ms));
    set_unsigned_key(
        table,
        "utterance_end_ms",
        i64::from(deepgram.utterance_end_ms),
    );
    set_string_array_key(table, "keyterms", &deepgram.keyterms);
}

fn write_transformation_table(document: &mut DocumentMut, transformation: &TransformationConfig) {
    let table = ensure_named_table(document, "transformation");
    set_required_string_key(table, "hotkey", &[], &transformation.hotkey);
    table["auto"] = value(transformation.auto);
    set_optional_string_key(table, "provider", &[], transformation.provider.as_deref());
    set_optional_string_key(table, "api_key", &[], transformation.api_key.as_deref());
    set_required_string_key(table, "model", &[], &transformation.model);
    if transformation.system_prompt.trim().is_empty() {
        table.remove("system_prompt");
    } else {
        set_required_string_key(table, "system_prompt", &[], &transformation.system_prompt);
    }
    if transformation.correction_system_prompt.trim().is_empty() {
        table.remove("correction_system_prompt");
        table.remove("instruction_system_prompt");
    } else {
        set_required_string_key(
            table,
            "correction_system_prompt",
            &["instruction_system_prompt"],
            &transformation.correction_system_prompt,
        );
    }
}
