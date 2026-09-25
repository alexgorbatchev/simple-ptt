//! Item lists and selection of the settings popups and combo box.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSComboBox, NSFontManager, NSPopUpButton};
use objc2_foundation::NSString;

use super::form::{
    METER_STYLE_TITLES, SYSTEM_DEFAULT_FONT_LABEL, TRANSFORMATION_PROVIDER_DISABLED_LABEL,
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
    popup_button.addItemWithTitle(&NSString::from_str(SYSTEM_DEFAULT_FONT_LABEL));
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

pub fn populate_transformation_provider_popup(
    popup_button: &NSPopUpButton,
    selected_provider: Option<&str>,
) {
    popup_button.removeAllItems();
    popup_button.addItemWithTitle(&NSString::from_str(TRANSFORMATION_PROVIDER_DISABLED_LABEL));
    for provider in crate::config::supported_transformation_providers() {
        popup_button.addItemWithTitle(&NSString::from_str(provider));
    }

    let Some(selected_provider) = selected_provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        popup_button.selectItemAtIndex(0);
        return;
    };

    let selected_provider = NSString::from_str(selected_provider);
    if popup_button.indexOfItemWithTitle(&selected_provider) < 0 {
        popup_button.insertItemWithTitle_atIndex(&selected_provider, 1);
    }
    popup_button.selectItemWithTitle(&selected_provider);
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
