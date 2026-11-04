use toml_edit::{value, DocumentMut, Item, Table, TableLike};

pub(super) fn root_table(document: &mut DocumentMut) -> &mut toml_edit::Table {
    document.as_table_mut()
}

pub(super) fn ensure_named_table<'a>(
    document: &'a mut DocumentMut,
    table_name: &str,
) -> Result<&'a mut dyn TableLike, String> {
    let root = root_table(document);
    if !root.contains_key(table_name) {
        root.insert(table_name, Item::Table(Table::new()));
    }

    root[table_name]
        .as_table_like_mut()
        .ok_or_else(|| format!("config section '{table_name}' must be a table"))
}

fn existing_key_name(table: &dyn TableLike, primary_key: &str, aliases: &[&str]) -> String {
    if table.contains_key(primary_key) {
        return primary_key.to_owned();
    }

    aliases
        .iter()
        .copied()
        .find(|alias| table.contains_key(alias))
        .unwrap_or(primary_key)
        .to_owned()
}

fn set_required_string_key(
    table: &mut dyn TableLike,
    key: &str,
    aliases: &[&str],
    field_value: &str,
) {
    write_key_value(table, key, aliases, field_value.into());
}

/// Writes `new_value` under `key`, or under the alias the file already uses,
/// and removes the other aliases. An existing value is replaced in place, so
/// the comment after it stays.
fn write_key_value(
    table: &mut dyn TableLike,
    key: &str,
    aliases: &[&str],
    mut new_value: toml_edit::Value,
) {
    let selected_key = existing_key_name(table, key, aliases);
    match table.get_mut(&selected_key).and_then(Item::as_value_mut) {
        Some(existing_value) => {
            *new_value.decor_mut() = existing_value.decor().clone();
            *existing_value = new_value;
        }
        None => {
            *table.entry(&selected_key).or_insert(Item::None) = Item::Value(new_value);
        }
    }
    for alias in aliases {
        if *alias != selected_key {
            table.remove(alias);
        }
    }
}

pub(super) fn set_optional_string_key(
    table: &mut dyn TableLike,
    key: &str,
    aliases: &[&str],
    field_value: Option<&str>,
) {
    match field_value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(non_empty_value) => write_key_value(table, key, aliases, non_empty_value.into()),
        None => {
            table.remove(key);
            for alias in aliases {
                table.remove(alias);
            }
        }
    }
}

/// Writes a string whose built-in default can change between releases, so a
/// config that leaves the key out follows the default. Resolution trims the
/// value and treats a blank one as the default, so a blank value or one equal
/// to the default after trimming is a default value. A default value removes
/// the key, unless the file already has it (under `key` or an alias) and
/// `reset` is false: a key the user wrote stays, while an explicit reset in
/// the settings window hands the key back to the default.
pub(super) fn set_defaulted_string_key(
    table: &mut dyn TableLike,
    key: &str,
    aliases: &[&str],
    field_value: &str,
    default_value: &str,
    reset: bool,
) {
    let trimmed_value = field_value.trim();
    let is_default = trimmed_value.is_empty() || trimmed_value == default_value.trim();
    let in_file = table.contains_key(key) || aliases.iter().any(|alias| table.contains_key(alias));
    if is_default && (trimmed_value.is_empty() || reset || !in_file) {
        table.remove(key);
        for alias in aliases {
            table.remove(alias);
        }
    } else {
        set_required_string_key(table, key, aliases, field_value);
    }
}

/// Writes a key that has a built-in default only when `field_value` differs
/// from `default_value` or the file already has the key (under `key` or an
/// alias). A config that leaves a default out keeps following the default of
/// later releases, and a key the user wrote stays, keeping its comments,
/// unless it was `reset` and still equals the default.
pub(super) fn set_defaulted_key<V>(
    table: &mut dyn TableLike,
    key: &str,
    aliases: &[&str],
    field_value: V,
    default_value: V,
    reset: bool,
) where
    V: PartialEq + Into<toml_edit::Value>,
{
    let in_file = table.contains_key(key) || aliases.iter().any(|alias| table.contains_key(alias));
    if field_value == default_value && (reset || !in_file) {
        table.remove(key);
        for alias in aliases {
            table.remove(alias);
        }
        return;
    }

    write_key_value(table, key, aliases, field_value.into());
}

pub(super) fn set_float_key(table: &mut dyn TableLike, key: &str, field_value: f64) {
    *table.entry(key).or_insert(Item::None) = value(field_value);
}

pub(super) fn set_string_array_key(table: &mut dyn TableLike, key: &str, values: &[String]) {
    if values.is_empty() {
        table.remove(key);
    } else {
        let mut array = toml_edit::Array::new();
        for v in values {
            array.push(v);
        }
        *table.entry(key).or_insert(Item::None) = value(array);
    }
}
