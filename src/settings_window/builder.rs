use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_foundation::ns_string;
use objc2_app_kit::{
    NSButton, NSColor, NSComboBox, NSFontManager, NSPopUpButton, NSScrollView, NSSlider,
    NSTextAlignment, NSTextField, NSTextView, NSView,
};
use objc2_foundation::{
    NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString,
};
use objc2_quartz_core::CALayer;

extern "C" {
    static NSFontAttributeName: &'static objc2_foundation::NSAttributedStringKey;
    static NSForegroundColorAttributeName: &'static objc2_foundation::NSAttributedStringKey;
}

use crate::config::UiMeterStyle;
use crate::ui_meter::{self, UiMeterView};
use super::helpers::{
    DEEPGRAM_MODEL_OPTIONS, SYSTEM_DEFAULT_FONT_LABEL, TRANSFORMATION_PROVIDER_DISABLED_LABEL,
};

pub const WINDOW_HEIGHT: f64 = 760.0;
pub const WINDOW_WIDTH: f64 = 760.0;
pub const CONTENT_HEIGHT: f64 = 1380.0;
pub const CONTENT_TOP_PADDING: f64 = 28.0;
pub const HORIZONTAL_PADDING: f64 = 20.0;
pub const LABEL_WIDTH: f64 = 180.0;
pub const FIELD_HEIGHT: f64 = 24.0;
pub const FIELD_WIDTH: f64 = 500.0;
pub const HOTKEY_FIELD_WIDTH: f64 = 390.0;
pub const CAPTURE_BUTTON_WIDTH: f64 = 100.0;
pub const CAPTURE_BUTTON_GAP: f64 = 10.0;
pub const MODEL_COMBO_BOX_WIDTH: f64 = 320.0;
pub const MODEL_ACTION_BUTTON_WIDTH: f64 = 80.0;
pub const MODEL_ACTION_BUTTON_GAP: f64 = 10.0;
pub const FIELD_WITH_ACTION_BUTTON_WIDTH: f64 =
    FIELD_WIDTH - MODEL_ACTION_BUTTON_GAP - MODEL_ACTION_BUTTON_WIDTH;
pub const FIELD_X: f64 = HORIZONTAL_PADDING + LABEL_WIDTH + 12.0;
pub const ROW_GAP: f64 = 10.0;
pub const SECTION_BREAK_GAP: f64 = 12.0;
pub const SECTION_GAP: f64 = 20.0;
pub const SECTION_HEIGHT: f64 = 22.0;
pub const SECTION_TITLE_BOTTOM_GAP: f64 = 10.0;
pub const PROMPT_HEIGHT: f64 = 270.0;
pub const STATUS_HEIGHT: f64 = 40.0;
pub const BUTTON_BAR_Y: f64 = 10.0;
pub const BUTTON_BAR_HEIGHT: f64 = 30.0;
pub const ENV_HINT_HEIGHT: f64 = 18.0;
pub const SETTINGS_FONT_SIZE: f64 = 12.0;
pub const SETTINGS_FONT_WEIGHT: f64 = 0.0;
pub const SETTINGS_HINT_FONT_SCALE: f64 = 0.8;
pub const SETTINGS_SECTION_TITLE_FONT_WEIGHT: f64 = 0.4;
pub const INPUT_BORDER_HIGHLIGHT_LEVEL: f64 = 0.5;
pub const INPUT_BORDER_WIDTH: f64 = 1.0;
pub const INPUT_CORNER_RADIUS: f64 = 6.0;
pub const LABEL_VISUAL_CENTER_NUDGE: f64 = -4.0;

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "SettingsScrollContentView"]
    pub struct SettingsScrollContentView;

    impl SettingsScrollContentView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

fn calculate_centered_title_rect(cell: &SettingsCenteredTextFieldCell, bounds: NSRect) -> NSRect {
    let mut rect: NSRect = unsafe { msg_send![super(cell), titleRectForBounds: bounds] };
    let font_size = cell.font().map(|font| font.pointSize()).unwrap_or(12.0);
    let text_height = (font_size * 1.25).ceil();

    if bounds.size.height > text_height {
        rect.origin.y = bounds.origin.y + ((bounds.size.height - text_height) / 2.0).floor();
        rect.size.height = text_height;
    }

    rect
}

define_class!(
    #[unsafe(super(objc2_app_kit::NSTextFieldCell))]
    #[thread_kind = MainThreadOnly]
    #[name = "SettingsCenteredTextFieldCell"]
    pub struct SettingsCenteredTextFieldCell;

    impl SettingsCenteredTextFieldCell {
        #[unsafe(method(titleRectForBounds:))]
        fn title_rect_for_bounds(&self, bounds: NSRect) -> NSRect {
            calculate_centered_title_rect(self, bounds)
        }

        #[unsafe(method(drawInteriorWithFrame:inView:))]
        fn draw_interior_with_frame(&self, cell_frame: NSRect, control_view: &NSView) {
            let title_rect = calculate_centered_title_rect(self, cell_frame);
            unsafe {
                let _: () = msg_send![super(self), drawInteriorWithFrame: title_rect, inView: control_view];
            }
        }

        #[unsafe(method(selectWithFrame:inView:editor:delegate:start:length:))]
        fn select_with_frame(
            &self,
            rect: NSRect,
            control_view: &NSView,
            text_obj: &AnyObject,
            delegate: Option<&AnyObject>,
            sel_start: isize,
            sel_length: isize,
        ) {
            let title_rect = calculate_centered_title_rect(self, rect);
            unsafe {
                let _: () = msg_send![
                    super(self),
                    selectWithFrame: title_rect,
                    inView: control_view,
                    editor: text_obj,
                    delegate: delegate,
                    start: sel_start,
                    length: sel_length
                ];
            }
        }
    }
);

pub fn settings_font() -> Retained<objc2_app_kit::NSFont> {
    objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(SETTINGS_FONT_SIZE, SETTINGS_FONT_WEIGHT)
}

pub fn settings_section_title_font() -> Retained<objc2_app_kit::NSFont> {
    objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(
        SETTINGS_FONT_SIZE,
        SETTINGS_SECTION_TITLE_FONT_WEIGHT,
    )
}

pub fn settings_hint_font() -> Retained<objc2_app_kit::NSFont> {
    objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(
        SETTINGS_FONT_SIZE * SETTINGS_HINT_FONT_SCALE,
        SETTINGS_FONT_WEIGHT,
    )
}

pub fn input_border_color() -> Retained<NSColor> {
    NSColor::separatorColor()
        .highlightWithLevel(INPUT_BORDER_HIGHLIGHT_LEVEL)
        .unwrap_or_else(NSColor::separatorColor)
}

pub fn configure_input_border(view: &AnyObject) {
    unsafe {
        let _: () = msg_send![view, setWantsLayer: true];
        let layer: Option<Retained<CALayer>> = msg_send![view, layer];
        let Some(layer) = layer else {
            return;
        };

        layer.setCornerRadius(INPUT_CORNER_RADIUS);
        layer.setBorderWidth(INPUT_BORDER_WIDTH);
        let border_color = input_border_color();
        let border_cg_color = border_color.CGColor();
        layer.setBorderColor(Some(&border_cg_color));
    }
}

pub fn configure_wrapping_label(label: &NSTextField) {
    label.setDrawsBackground(false);
    label.setBordered(false);
    label.setBezeled(false);
    label.setEditable(false);
    label.setSelectable(false);
    label.setFont(Some(&settings_font()));
    if let Some(cell) = label.cell() {
        cell.setAlignment(NSTextAlignment::Left);
    }
}

pub fn configure_hint_label(label: &NSTextField) {
    label.setDrawsBackground(false);
    label.setBordered(false);
    label.setBezeled(false);
    label.setEditable(false);
    label.setSelectable(false);
    label.setFont(Some(&settings_hint_font()));
    if let Some(cell) = label.cell() {
        cell.setAlignment(NSTextAlignment::Left);
    }
}

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

pub fn add_section_title(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: f64,
    title: &str,
) -> f64 {
    let title_y = current_y - SECTION_BREAK_GAP;
    let title_field = make_section_title(mtm, title);
    set_view_frame(
        &*title_field,
        HORIZONTAL_PADDING,
        title_y,
        260.0,
        SECTION_HEIGHT,
    );
    content_view.addSubview(&title_field);
    title_y - SECTION_HEIGHT - SECTION_TITLE_BOTTOM_GAP
}

pub fn make_section_title(mtm: MainThreadMarker, title: &str) -> Retained<NSTextField> {
    let title_field = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    title_field.setFont(Some(&settings_section_title_font()));
    title_field.setTextColor(Some(&NSColor::labelColor()));
    title_field
}

pub fn add_labeled_text_field(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> Retained<NSTextField> {
    let text_field_y = *current_y - 2.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(text_field_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    configure_input_border(&text_field);
    set_view_frame(
        &*text_field,
        FIELD_X,
        text_field_y,
        FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    text_field
}

pub fn add_labeled_slider_with_meter(
    content_view: &NSView,
    target: &AnyObject,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
    slider_action: Sel,
) -> (Retained<NSTextField>, Retained<NSSlider>, UiMeterView) {
    let base_y = *current_y - 2.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(base_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let slider =
        unsafe { NSSlider::sliderWithTarget_action(Some(target), Some(slider_action), mtm) };
    slider.setMinValue(0.0);
    slider.setMaxValue(10.0);
    slider.setContinuous(true);
    let slider_width = 150.0;
    set_view_frame(&*slider, FIELD_X, base_y, slider_width, FIELD_HEIGHT);
    content_view.addSubview(&slider);

    let value_field = NSTextField::labelWithString(&NSString::from_str("3.0"), mtm);
    value_field.setFont(Some(&settings_font()));
    let value_width = 40.0;
    set_view_frame(
        &*value_field,
        FIELD_X + slider_width + 10.0,
        vertically_centered_label_y(base_y, FIELD_HEIGHT),
        value_width,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&value_field);

    let meter = UiMeterView::new(mtm, UiMeterStyle::AnimatedColor);
    let meter_height = ui_meter::meter_container_height(UiMeterStyle::AnimatedColor);
    let meter_width = 180.0;
    set_view_frame(
        meter.view(),
        FIELD_X + slider_width + 10.0 + value_width + 10.0,
        base_y + (FIELD_HEIGHT - meter_height) / 2.0,
        meter_width,
        meter_height,
    );
    content_view.addSubview(meter.view());

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    (value_field, slider, meter)
}

pub fn add_labeled_hotkey_field(
    content_view: &NSView,
    target: &AnyObject,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
    action: Sel,
) -> (Retained<NSTextField>, Retained<NSButton>) {
    let text_field_y = *current_y - 2.0;
    let button_height = FIELD_HEIGHT + 2.0;
    let button_y = text_field_y - 1.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(text_field_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    text_field.setEditable(false);
    text_field.setSelectable(false);
    configure_input_border(&text_field);
    set_view_frame(
        &*text_field,
        FIELD_X,
        text_field_y,
        HOTKEY_FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Capture…"),
            Some(target),
            Some(action),
            mtm,
        )
    };
    button.setFont(Some(&settings_font()));
    set_view_frame(
        &*button,
        FIELD_X + HOTKEY_FIELD_WIDTH + CAPTURE_BUTTON_GAP,
        button_y,
        CAPTURE_BUTTON_WIDTH,
        button_height,
    );
    content_view.addSubview(&button);

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    (text_field, button)
}

pub fn add_labeled_combo_box_with_buttons(
    content_view: &NSView,
    target: &AnyObject,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
    leading_button_title: &str,
    leading_action: Sel,
    trailing_button_title: &str,
    trailing_action: Sel,
) -> (
    Retained<NSComboBox>,
    Retained<NSButton>,
    Retained<NSButton>,
) {
    let combo_box_y = *current_y - 2.0;
    let button_y = *current_y - 4.0;
    let button_height = FIELD_HEIGHT + 4.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(combo_box_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let combo_box = NSComboBox::initWithFrame(
        NSComboBox::alloc(mtm),
        NSRect::new(
            NSPoint::new(FIELD_X, combo_box_y),
            NSSize::new(MODEL_COMBO_BOX_WIDTH, FIELD_HEIGHT),
        ),
    );
    combo_box.setFont(Some(&settings_font()));
    combo_box.setCompletes(true);
    combo_box.setNumberOfVisibleItems(20);
    configure_input_border(&combo_box);
    content_view.addSubview(&combo_box);

    let leading_button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(leading_button_title),
            Some(target),
            Some(leading_action),
            mtm,
        )
    };
    leading_button.setFont(Some(&settings_font()));
    set_view_frame(
        &*leading_button,
        FIELD_X + MODEL_COMBO_BOX_WIDTH + MODEL_ACTION_BUTTON_GAP,
        button_y,
        MODEL_ACTION_BUTTON_WIDTH,
        button_height,
    );
    content_view.addSubview(&leading_button);

    let trailing_button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(trailing_button_title),
            Some(target),
            Some(trailing_action),
            mtm,
        )
    };
    trailing_button.setFont(Some(&settings_font()));
    set_view_frame(
        &*trailing_button,
        FIELD_X
            + MODEL_COMBO_BOX_WIDTH
            + MODEL_ACTION_BUTTON_GAP
            + MODEL_ACTION_BUTTON_WIDTH
            + MODEL_ACTION_BUTTON_GAP,
        button_y,
        MODEL_ACTION_BUTTON_WIDTH,
        button_height,
    );
    content_view.addSubview(&trailing_button);

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    (combo_box, leading_button, trailing_button)
}

pub fn add_labeled_pop_up_button_with_action(
    content_view: &NSView,
    target: &AnyObject,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
    action: Sel,
) -> Retained<NSPopUpButton> {
    let popup_button = add_labeled_pop_up_button(content_view, mtm, current_y, label);
    unsafe {
        popup_button.setTarget(Some(target));
        popup_button.setAction(Some(action));
    }
    popup_button
}

pub fn add_labeled_pop_up_button(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> Retained<NSPopUpButton> {
    let popup_button_y = *current_y - 3.0;
    let popup_button_height = FIELD_HEIGHT + 6.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(popup_button_y, popup_button_height),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let popup_button = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(
            NSPoint::new(FIELD_X, popup_button_y),
            NSSize::new(FIELD_WIDTH, popup_button_height),
        ),
        false,
    );
    popup_button.setFont(Some(&settings_font()));
    configure_input_border(&popup_button);
    content_view.addSubview(&popup_button);

    *current_y -= FIELD_HEIGHT + ROW_GAP;
    popup_button
}

pub fn add_labeled_text_field_with_hint_and_button(
    content_view: &NSView,
    target: &AnyObject,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
    button_title: &str,
    action: Sel,
) -> (
    Retained<NSTextField>,
    Retained<NSButton>,
    Retained<NSTextField>,
) {
    let text_field_y = *current_y - 2.0;
    let button_y = *current_y - 4.0;
    let button_height = FIELD_HEIGHT + 4.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(text_field_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    configure_input_border(&text_field);
    set_view_frame(
        &*text_field,
        FIELD_X,
        text_field_y,
        FIELD_WITH_ACTION_BUTTON_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(button_title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    button.setFont(Some(&settings_font()));
    set_view_frame(
        &*button,
        FIELD_X + FIELD_WITH_ACTION_BUTTON_WIDTH + MODEL_ACTION_BUTTON_GAP,
        button_y,
        MODEL_ACTION_BUTTON_WIDTH,
        button_height,
    );
    content_view.addSubview(&button);

    let hint_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    configure_hint_label(&hint_field);
    hint_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &hint_field,
        FIELD_X,
        *current_y - ENV_HINT_HEIGHT - 4.0,
        FIELD_WIDTH,
        ENV_HINT_HEIGHT,
    );
    content_view.addSubview(&hint_field);

    *current_y -= FIELD_HEIGHT + ENV_HINT_HEIGHT + ROW_GAP;
    (text_field, button, hint_field)
}

pub fn add_labeled_text_field_with_hint(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> (Retained<NSTextField>, Retained<NSTextField>) {
    let text_field_y = *current_y - 2.0;

    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        vertically_centered_label_y(text_field_y, FIELD_HEIGHT),
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let text_field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    text_field.setFont(Some(&settings_font()));
    configure_input_border(&text_field);
    set_view_frame(
        &*text_field,
        FIELD_X,
        text_field_y,
        FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&text_field);

    let hint_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    configure_hint_label(&hint_field);
    hint_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &hint_field,
        FIELD_X,
        *current_y - ENV_HINT_HEIGHT - 4.0,
        FIELD_WIDTH,
        ENV_HINT_HEIGHT,
    );
    content_view.addSubview(&hint_field);

    *current_y -= FIELD_HEIGHT + ENV_HINT_HEIGHT + ROW_GAP;
    (text_field, hint_field)
}

pub fn add_checkbox(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    title: &str,
) -> Retained<NSButton> {
    let checkbox = unsafe {
        NSButton::checkboxWithTitle_target_action(&NSString::from_str(title), None, None, mtm)
    };
    checkbox.setFont(Some(&settings_font()));
    set_view_frame(
        &*checkbox,
        FIELD_X,
        *current_y - 2.0,
        FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&checkbox);
    *current_y -= FIELD_HEIGHT + ROW_GAP;
    checkbox
}

pub fn add_checkbox_with_hint(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    title: &str,
    hint: &str,
) -> Retained<NSButton> {
    let checkbox = unsafe {
        NSButton::checkboxWithTitle_target_action(&NSString::from_str(title), None, None, mtm)
    };
    checkbox.setFont(Some(&settings_font()));
    set_view_frame(
        &*checkbox,
        FIELD_X,
        *current_y - 2.0,
        FIELD_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&checkbox);

    let hint_field = NSTextField::wrappingLabelWithString(&NSString::from_str(hint), mtm);
    configure_hint_label(&hint_field);
    hint_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &hint_field,
        FIELD_X + 22.0,
        *current_y - ENV_HINT_HEIGHT - 4.0,
        FIELD_WIDTH - 22.0,
        ENV_HINT_HEIGHT,
    );
    content_view.addSubview(&hint_field);

    *current_y -= FIELD_HEIGHT + ENV_HINT_HEIGHT + ROW_GAP;
    checkbox
}

pub fn add_prompt_editor(
    content_view: &NSView,
    mtm: MainThreadMarker,
    current_y: &mut f64,
    label: &str,
) -> Retained<NSTextView> {
    let label_field = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label_field.setFont(Some(&settings_font()));
    label_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    set_view_frame(
        &*label_field,
        HORIZONTAL_PADDING,
        *current_y,
        LABEL_WIDTH,
        FIELD_HEIGHT,
    );
    content_view.addSubview(&label_field);

    let prompt_scroll_view = NSScrollView::initWithFrame(
        NSScrollView::alloc(mtm),
        NSRect::new(
            NSPoint::new(FIELD_X, *current_y - PROMPT_HEIGHT + 20.0),
            NSSize::new(FIELD_WIDTH, PROMPT_HEIGHT),
        ),
    );
    prompt_scroll_view.setHasVerticalScroller(true);
    prompt_scroll_view.setHasHorizontalScroller(false);
    prompt_scroll_view.setDrawsBackground(true);
    configure_input_border(&prompt_scroll_view);
    let prompt_view = NSTextView::initWithFrame(
        NSTextView::alloc(mtm),
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(FIELD_WIDTH, PROMPT_HEIGHT),
        ),
    );
    prompt_view.setEditable(true);
    prompt_view.setSelectable(true);
    prompt_view.setDrawsBackground(true);
    prompt_view.setBackgroundColor(&NSColor::textBackgroundColor());
    prompt_view.setFont(Some(&settings_font()));
    prompt_view.setHorizontallyResizable(false);
    prompt_view.setVerticallyResizable(true);
    prompt_scroll_view.setDocumentView(Some(&prompt_view));
    content_view.addSubview(&prompt_scroll_view);

    *current_y -= PROMPT_HEIGHT + SECTION_GAP;
    prompt_view
}

pub fn populate_meter_style_popup(popup_button: &NSPopUpButton, selected_meter_style: UiMeterStyle) {
    popup_button.removeAllItems();
    popup_button.addItemWithTitle(ns_string!("animated-color"));
    popup_button.addItemWithTitle(ns_string!("animated-height"));
    popup_button.addItemWithTitle(ns_string!("none"));

    let selected_title = NSString::from_str(match selected_meter_style {
        UiMeterStyle::AnimatedColor => "animated-color",
        UiMeterStyle::AnimatedHeight => "animated-height",
        UiMeterStyle::None => "none",
    });
    popup_button.selectItemWithTitle(&selected_title);
}

pub fn populate_combo_box_with_values(combo_box: &NSComboBox, values: &[String], selected_value: &str) {
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

pub fn populate_deepgram_model_popup(popup_button: &NSPopUpButton, selected_model: &str) {
    popup_button.removeAllItems();
    for model in DEEPGRAM_MODEL_OPTIONS {
        popup_button.addItemWithTitle(&NSString::from_str(model));
    }

    let selected_model = selected_model.trim();
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

pub fn set_capture_button_state(button: &NSButton, is_active: bool, is_enabled: bool) {
    button.setTitle(if is_active {
        ns_string!("Capturing…")
    } else {
        ns_string!("Capture…")
    });
    button.setEnabled(is_enabled);
}

pub fn set_hint_text(label: &NSTextField, message: Option<String>) {
    label.setStringValue(&NSString::from_str(message.as_deref().unwrap_or("")));
}

pub fn vertically_centered_label_y(control_y: f64, control_height: f64) -> f64 {
    control_y + ((control_height - FIELD_HEIGHT) / 2.0) + LABEL_VISUAL_CENTER_NUDGE
}

pub fn set_view_frame(view: &AnyObject, x: f64, y: f64, width: f64, height: f64) {
    unsafe {
        let _: () = objc2::msg_send![view, setFrame: NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))];
    }
}

pub fn style_button_bezel_and_text(
    button: &NSButton,
    title: &str,
    bezel_color: Option<Retained<NSColor>>,
    text_color: Option<Retained<NSColor>>,
) {
    let title_string = NSString::from_str(title);

    if let (Some(bezel), Some(text)) = (bezel_color, text_color) {
        let font: Retained<objc2::runtime::AnyObject> = settings_font().into();
        let text_any: Retained<objc2::runtime::AnyObject> = text.into();
        let attributes = NSDictionary::<
            objc2_foundation::NSAttributedStringKey,
            objc2::runtime::AnyObject,
        >::from_retained_objects(
            &[unsafe { NSForegroundColorAttributeName }, unsafe {
                NSFontAttributeName
            }],
            &[text_any, font],
        );
        let attributed_title =
            unsafe { NSAttributedString::new_with_attributes(&title_string, &attributes) };

        button.setAttributedTitle(&attributed_title);
        button.setBezelColor(Some(&bezel));
    } else {
        let attributed_title = NSAttributedString::from_nsstring(&title_string);
        button.setAttributedTitle(&attributed_title);
        button.setBezelColor(None);
    }
}
