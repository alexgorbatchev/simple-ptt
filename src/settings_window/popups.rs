//! Item lists and selection of the settings popups and combo box.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSComboBox, NSFontManager, NSPopUpButton};
use objc2_foundation::NSString;

use super::form::{
    METER_STYLE_TITLES, SYSTEM_MONOSPACED_FONT_LABEL, TRANSFORMATION_PROVIDER_DISABLED_LABEL,
};
use super::helpers::DEEPGRAM_MODEL_OPTIONS;

pub fn available_font_family_names(mtm: MainThreadMarker) -> Vec<String> {
    let font_manager = NSFontManager::sharedFontManager(mtm);
    let font_families = font_manager.availableFontFamilies();
    let mut names = (0..font_families.count())
        .map(|index| font_families.objectAtIndex(index).to_string())
        .collect::<Vec<_>>();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names.dedup();
    names
}

pub fn populate_font_name_popup(
    popup_button: &NSPopUpButton,
    available_font_family_names: &[String],
    selected_font_name: Option<&str>,
) {
    popup_button.removeAllItems();
    popup_button.addItemWithTitle(&NSString::from_str(SYSTEM_MONOSPACED_FONT_LABEL));
    for font_name in available_font_family_names {
        popup_button.addItemWithTitle(&NSString::from_str(font_name));
    }

    let Some(selected_font_name) = selected_font_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        popup_button.selectItemAtIndex(0);
        return;
    };

    let selected_font_name = NSString::from_str(selected_font_name);
    if popup_button.indexOfItemWithTitle(&selected_font_name) < 0 {
        popup_button.insertItemWithTitle_atIndex(&selected_font_name, 1);
    }
    popup_button.selectItemWithTitle(&selected_font_name);
}

pub fn populate_meter_style_popup(popup_button: &NSPopUpButton, selected_title: Option<&str>) {
    popup_button.removeAllItems();
    for (_, title) in METER_STYLE_TITLES {
        popup_button.addItemWithTitle(&NSString::from_str(title));
    }

    if let Some(selected_title) = selected_title {
        popup_button.selectItemWithTitle(&NSString::from_str(selected_title));
    }
}

pub fn populate_combo_box_with_values(
    combo_box: &NSComboBox,
    values: &[String],
    selected_value: &str,
) {
    combo_box.removeAllItems();
    for value in values {
        let string = NSString::from_str(value);
        unsafe {
            combo_box.addItemWithObjectValue(&*string);
        }
    }
    combo_box.setStringValue(&NSString::from_str(selected_value.trim()));
}

/// The provider popup's selection for a form's provider title.
#[derive(Debug, PartialEq, Eq)]
enum ProviderSelection<'a> {
    /// An item the popup always lists: the disabled entry or a supported
    /// provider, by its title.
    Listed(&'a str),
    /// A configured provider that is not supported, which `config::problems`
    /// reports.
    Unsupported(&'a str),
}

fn provider_selection(provider_title: Option<&str>) -> ProviderSelection<'_> {
    let Some(title) = provider_title
        .map(str::trim)
        .filter(|title| !title.is_empty() && *title != TRANSFORMATION_PROVIDER_DISABLED_LABEL)
    else {
        return ProviderSelection::Listed(TRANSFORMATION_PROVIDER_DISABLED_LABEL);
    };
    match title.parse::<crate::config::TransformationProvider>() {
        Ok(provider) => ProviderSelection::Listed(provider.as_str()),
        Err(_) => ProviderSelection::Unsupported(title),
    }
}

/// Lists the supported providers after the disabled entry and selects
/// `provider_title`. An unsupported provider is shown as a disabled item
/// after the disabled entry: it stays selected until another provider is
/// chosen and cannot be chosen again.
pub fn populate_transformation_provider_popup(
    popup_button: &NSPopUpButton,
    provider_title: Option<&str>,
) {
    popup_button.setAutoenablesItems(false);
    popup_button.removeAllItems();
    popup_button.addItemWithTitle(&NSString::from_str(TRANSFORMATION_PROVIDER_DISABLED_LABEL));
    for provider in crate::config::supported_transformation_providers() {
        popup_button.addItemWithTitle(&NSString::from_str(provider));
    }

    match provider_selection(provider_title) {
        ProviderSelection::Listed(title) => {
            popup_button.selectItemWithTitle(&NSString::from_str(title));
        }
        ProviderSelection::Unsupported(title) => {
            popup_button.insertItemWithTitle_atIndex(&NSString::from_str(title), 1);
            if let Some(unsupported_item) = popup_button.itemAtIndex(1) {
                unsupported_item.setEnabled(false);
            }
            popup_button.selectItemAtIndex(1);
        }
    }
}

pub fn populate_deepgram_model_popup(popup_button: &NSPopUpButton, selected_model: Option<&str>) {
    popup_button.removeAllItems();
    for model in DEEPGRAM_MODEL_OPTIONS {
        popup_button.addItemWithTitle(&NSString::from_str(model));
    }

    let selected_model = selected_model.unwrap_or_default().trim();
    if selected_model.is_empty() {
        popup_button.selectItemWithTitle(&NSString::from_str("nova-3"));
        return;
    }

    let selected_model = NSString::from_str(selected_model);
    if popup_button.indexOfItemWithTitle(&selected_model) < 0 {
        popup_button.insertItemWithTitle_atIndex(&selected_model, 0);
    }
    popup_button.selectItemWithTitle(&selected_model);
}

#[cfg(test)]
mod tests {
    use super::{provider_selection, ProviderSelection};
    use crate::settings_window::form::TRANSFORMATION_PROVIDER_DISABLED_LABEL;

    #[test]
    fn no_provider_selects_the_disabled_entry() {
        for title in [
            None,
            Some(""),
            Some("  "),
            Some(TRANSFORMATION_PROVIDER_DISABLED_LABEL),
        ] {
            assert_eq!(
                provider_selection(title),
                ProviderSelection::Listed(TRANSFORMATION_PROVIDER_DISABLED_LABEL),
                "{title:?}"
            );
        }
    }

    #[test]
    fn a_supported_provider_selects_its_canonical_title() {
        assert_eq!(
            provider_selection(Some(" OpenAI ")),
            ProviderSelection::Listed("openai")
        );
    }

    #[test]
    fn an_unsupported_provider_gets_its_own_item() {
        assert_eq!(
            provider_selection(Some(" galadriel ")),
            ProviderSelection::Unsupported("galadriel")
        );
    }
}
