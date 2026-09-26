//! Factories and value accessors for the native controls used by the settings
//! panes. Controls keep their native bezels and the system font; every
//! control is created for Auto Layout (`translatesAutoresizingMaskIntoConstraints`
//! off) and sized by its intrinsic content size or by the pane layout.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBorderType, NSButton, NSColor, NSComboBox, NSControlStateValueOff, NSControlStateValueOn,
    NSFont, NSLayoutConstraintOrientation, NSLayoutManager, NSLayoutPriorityDefaultLow,
    NSLayoutPriorityDragThatCannotResizeWindow, NSLineBreakMode, NSPopUpButton, NSScrollView,
    NSSlider, NSTextField, NSTextView, NSView,
};
use objc2_foundation::{
    ns_string, NSAttributedString, NSDictionary, NSNumber, NSNumberFormatter,
    NSNumberFormatterStyle, NSSize, NSString,
};

use super::actions::SettingsAction;

extern "C" {
    static NSFontAttributeName: &'static objc2_foundation::NSAttributedStringKey;
    static NSForegroundColorAttributeName: &'static objc2_foundation::NSAttributedStringKey;
}

const SETTINGS_FONT_SIZE: f64 = 12.0;
const SETTINGS_FONT_WEIGHT: f64 = 0.0;

/// Narrowest width of a text field that fills its grid cells; together with
/// the labels it sets the minimum width of a pane.
pub const FIELD_MIN_WIDTH: f64 = 320.0;
/// Width of a numeric field, which has no intrinsic width of its own.
pub const NUMBER_FIELD_WIDTH: f64 = 80.0;

pub fn minimum_width(view: &NSView, width: f64) {
    view.widthAnchor()
        .constraintGreaterThanOrEqualToConstant(width)
        .setActive(true);
}

pub fn fixed_width(view: &NSView, width: f64) {
    view.widthAnchor()
        .constraintEqualToConstant(width)
        .setActive(true);
}

pub fn set_checked(checkbox: &NSButton, checked: bool) {
    checkbox.setState(if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
}

pub fn is_checked(checkbox: &NSButton) -> bool {
    checkbox.state() == NSControlStateValueOn
}

/// Monospaced font of the permissions dialog. The settings window itself uses
/// the system font of each native control.
pub fn settings_font() -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(SETTINGS_FONT_SIZE, SETTINGS_FONT_WEIGHT)
}

pub fn for_auto_layout<V: AsRef<NSView>>(view: V) -> V {
    view.as_ref()
        .setTranslatesAutoresizingMaskIntoConstraints(false);
    view
}

/// Trailing-aligned form label for the label column of a pane grid.
pub fn form_label(mtm: MainThreadMarker, title: &str) -> Retained<NSTextField> {
    for_auto_layout(NSTextField::labelWithString(
        &NSString::from_str(title),
        mtm,
    ))
}

/// Bold section header that sits above the rows of its section.
pub fn section_header(mtm: MainThreadMarker, title: &str) -> Retained<NSTextField> {
    let header = form_label(mtm, title);
    header.setFont(Some(
        &NSFont::boldSystemFontOfSize(NSFont::systemFontSize()),
    ));
    header
}

/// Secondary, small, wrapping explanation text shown below a control.
pub fn hint_label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let hint = for_auto_layout(NSTextField::wrappingLabelWithString(
        &NSString::from_str(text),
        mtm,
    ));
    hint.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
    allow_horizontal_compression(&hint);
    hint
}

/// Wrapping label whose width follows its layout instead of its text, so long
/// messages wrap instead of widening the window.
pub fn allow_horizontal_compression(view: &NSView) {
    view.setContentCompressionResistancePriority_forOrientation(
        NSLayoutPriorityDefaultLow,
        NSLayoutConstraintOrientation::Horizontal,
    );
}

pub fn text_field(mtm: MainThreadMarker) -> Retained<NSTextField> {
    for_auto_layout(NSTextField::textFieldWithString(ns_string!(""), mtm))
}

/// Read-only field that displays a captured hotkey.
pub fn hotkey_display_field(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let field = text_field(mtm);
    field.setEditable(false);
    field.setSelectable(false);
    field
}

#[derive(Clone, Copy, Debug)]
pub enum NumberFieldKind {
    /// Non-negative whole numbers up to `maximum`, which must not exceed
    /// `i64::MAX`: `NSNumberFormatter` rejects larger integers.
    UnsignedInteger { maximum: u64 },
    /// Non-negative decimal numbers.
    Decimal,
}

/// Text field with an `NSNumberFormatter`, so text that is not a number in
/// range is rejected while editing instead of on Save.
pub fn number_field(mtm: MainThreadMarker, kind: NumberFieldKind) -> Retained<NSTextField> {
    let field = text_field(mtm);
    field.setFormatter(Some(&number_formatter(kind)));
    field
}

/// Formatter of a number field. Zero passes a decimal field's formatter;
/// `validate_settings_config` rejects a zero font size on Save.
fn number_formatter(kind: NumberFieldKind) -> Retained<NSNumberFormatter> {
    let formatter = NSNumberFormatter::new();
    formatter.setUsesGroupingSeparator(false);
    formatter.setMinimum(Some(&NSNumber::numberWithUnsignedLongLong(0)));
    match kind {
        NumberFieldKind::UnsignedInteger { maximum } => {
            assert!(
                maximum <= i64::MAX as u64,
                "NSNumberFormatter rejects integers above i64::MAX"
            );
            formatter.setNumberStyle(NSNumberFormatterStyle::NoStyle);
            formatter.setAllowsFloats(false);
            // Decimal numbers keep every digit of large integers exact.
            formatter.setGeneratesDecimalNumbers(true);
            formatter.setMaximum(Some(&NSNumber::numberWithUnsignedLongLong(maximum)));
        }
        NumberFieldKind::Decimal => {
            formatter.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
            formatter.setAllowsFloats(true);
            // Show every digit an `f64` holds exactly (`f64::DIGITS`), so
            // loading and saving a config value does not round it.
            formatter.setUsesSignificantDigits(true);
            formatter.setMaximumSignificantDigits(f64::DIGITS as usize);
        }
    }
    formatter
}

pub fn set_unsigned_value(field: &NSTextField, value: Option<u64>) {
    let number = value.map(NSNumber::numberWithUnsignedLongLong);
    set_number_value(field, number.as_deref());
}

pub fn set_decimal_value(field: &NSTextField, value: Option<f64>) {
    let number = value.map(NSNumber::numberWithDouble);
    set_number_value(field, number.as_deref());
}

fn set_number_value(field: &NSTextField, number: Option<&NSNumber>) {
    let object: Option<&AnyObject> = number.map(|number| number.as_ref());
    // SAFETY: a number field's formatter produces and accepts `NSNumber`.
    unsafe { field.setObjectValue(object) };
}

pub fn unsigned_value(field: &NSTextField) -> Option<u64> {
    number_value(field).map(|number| number.unsignedLongLongValue())
}

pub fn decimal_value(field: &NSTextField) -> Option<f64> {
    number_value(field).map(|number| number.doubleValue())
}

fn number_value(field: &NSTextField) -> Option<Retained<NSNumber>> {
    field.objectValue()?.downcast::<NSNumber>().ok()
}

pub fn text_value(field: &NSTextField) -> String {
    field.stringValue().to_string()
}

pub fn set_text_value(field: &NSTextField, value: &str) {
    field.setStringValue(&NSString::from_str(value));
}

pub fn push_button(
    mtm: MainThreadMarker,
    title: &str,
    target: &AnyObject,
    action: SettingsAction,
) -> Retained<NSButton> {
    for_auto_layout(unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action.selector()),
            mtm,
        )
    })
}

pub fn checkbox(mtm: MainThreadMarker, title: &str) -> Retained<NSButton> {
    for_auto_layout(unsafe {
        NSButton::checkboxWithTitle_target_action(&NSString::from_str(title), None, None, mtm)
    })
}

/// Popup sized to its widest item. Items loaded later (device and font
/// names) can be wider than the pane allows, so the popup truncates its title
/// rather than grow the window past its minimum size.
pub fn pop_up_button(mtm: MainThreadMarker) -> Retained<NSPopUpButton> {
    let popup_button = for_auto_layout(NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        Default::default(),
        false,
    ));
    popup_button.setContentCompressionResistancePriority_forOrientation(
        NSLayoutPriorityDragThatCannotResizeWindow,
        NSLayoutConstraintOrientation::Horizontal,
    );
    popup_button
}

pub fn pop_up_button_with_action(
    mtm: MainThreadMarker,
    target: &AnyObject,
    action: SettingsAction,
) -> Retained<NSPopUpButton> {
    let popup_button = pop_up_button(mtm);
    unsafe {
        popup_button.setTarget(Some(target));
        popup_button.setAction(Some(action.selector()));
    }
    popup_button
}

pub fn selected_title(popup_button: &NSPopUpButton) -> Option<String> {
    popup_button
        .titleOfSelectedItem()
        .map(|title| title.to_string())
}

pub fn combo_box(mtm: MainThreadMarker) -> Retained<NSComboBox> {
    let combo_box = for_auto_layout(NSComboBox::initWithFrame(
        NSComboBox::alloc(mtm),
        Default::default(),
    ));
    combo_box.setCompletes(true);
    combo_box.setNumberOfVisibleItems(20);
    combo_box
}

pub fn slider(
    mtm: MainThreadMarker,
    target: &AnyObject,
    action: SettingsAction,
    minimum: f64,
    maximum: f64,
) -> Retained<NSSlider> {
    let slider = for_auto_layout(unsafe {
        NSSlider::sliderWithTarget_action(Some(target), Some(action.selector()), mtm)
    });
    slider.setMinValue(minimum);
    slider.setMaxValue(maximum);
    slider.setContinuous(true);
    slider
}

/// Multi-line plain editor in a bezeled scroll view. `scrollableTextView`
/// returns the text system configuration Apple documents for a text view in a
/// scroll view: the text view resizes with the clip view and its text
/// container tracks the text view width, so the text wraps to whatever width
/// the pane layout gives the scroll view. The scroll view shows at least
/// `minimum_visible_lines` lines of its font unless the window would have to
/// grow for them.
pub fn prompt_editor(
    mtm: MainThreadMarker,
    minimum_visible_lines: usize,
) -> (Retained<NSScrollView>, Retained<NSTextView>) {
    let scroll_view = for_auto_layout(NSTextView::scrollableTextView(mtm));
    scroll_view.setBorderType(NSBorderType::BezelBorder);
    scroll_view.setHasVerticalScroller(true);
    scroll_view.setHasHorizontalScroller(false);
    scroll_view.setAutohidesScrollers(true);

    let text_view = scroll_view
        .documentView()
        .and_then(|document_view| document_view.downcast::<NSTextView>().ok())
        .expect("scrollableTextView has an NSTextView document view");
    let font = NSFont::systemFontOfSize(NSFont::systemFontSize());
    text_view.setFont(Some(&font));

    let text_height = NSLayoutManager::new().defaultLineHeightForFont(&font)
        * minimum_visible_lines as f64
        + 2.0 * text_view.textContainerInset().height;
    let vertical_scroller = scroll_view
        .verticalScroller()
        .expect("a scroll view with a vertical scroller has one");
    // SAFETY: the scroller classes are the scroll view's own: none for the
    // horizontal scroller it does not have, and its vertical scroller's class.
    let minimum_size = unsafe {
        NSScrollView::frameSizeForContentSize_horizontalScrollerClass_verticalScrollerClass_borderType_controlSize_scrollerStyle(
            NSSize::new(0.0, text_height),
            None,
            Some(vertical_scroller.class()),
            scroll_view.borderType(),
            vertical_scroller.controlSize(),
            scroll_view.scrollerStyle(),
            mtm,
        )
    };
    // Below the priority at which the window keeps its size: when a hint row
    // appears above the editors at the minimum window size, the editors give
    // up its height instead of the window growing.
    let minimum_height = scroll_view
        .heightAnchor()
        .constraintGreaterThanOrEqualToConstant(minimum_size.height);
    minimum_height.setPriority(NSLayoutPriorityDragThatCannotResizeWindow);
    minimum_height.setActive(true);
    (scroll_view, text_view)
}

pub fn set_capture_button_state(button: &NSButton, is_active: bool, is_enabled: bool) {
    button.setTitle(if is_active {
        ns_string!("Capturing…")
    } else {
        ns_string!("Capture…")
    });
    button.setEnabled(is_enabled);
}

/// Tints a push button (for a check result) or restores its native look when
/// both colors are `None`. The title keeps the button's own font.
pub fn style_button_bezel_and_text(
    button: &NSButton,
    title: &str,
    bezel_color: Option<Retained<NSColor>>,
    text_color: Option<Retained<NSColor>>,
) {
    let title_string = NSString::from_str(title);

    if let (Some(bezel), Some(text)) = (bezel_color, text_color) {
        let font: Retained<AnyObject> = button
            .font()
            .expect("push buttons always have a font")
            .into();
        let text_any: Retained<AnyObject> = text.into();
        let attributes =
            NSDictionary::<objc2_foundation::NSAttributedStringKey, AnyObject>::from_retained_objects(
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
        // A plain title replaces the attributed one and draws in the
        // button's own font and color.
        button.setTitle(&title_string);
        button.setBezelColor(None);
    }
}

/// Wrapping status label; `maximum_lines` bounds how tall a long message can
/// make the button bar.
pub fn status_label(mtm: MainThreadMarker, maximum_lines: isize) -> Retained<NSTextField> {
    let label = for_auto_layout(NSTextField::wrappingLabelWithString(ns_string!(""), mtm));
    label.setMaximumNumberOfLines(maximum_lines);
    label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    if let Some(cell) = label.cell() {
        cell.setTruncatesLastVisibleLine(true);
    }
    allow_horizontal_compression(&label);
    label
}

#[cfg(test)]
mod tests {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSNumber, NSNumberFormatter, NSString};

    use super::{number_formatter, NumberFieldKind};

    /// Parses `text` the way a control does when editing ends.
    fn parse(formatter: &NSNumberFormatter, text: &str) -> Option<Retained<NSNumber>> {
        let mut object: Option<Retained<AnyObject>> = None;
        // SAFETY: a number formatter produces `NSNumber` objects.
        let accepted = unsafe {
            formatter.getObjectValue_forString_errorDescription(
                Some(&mut object),
                &NSString::from_str(text),
                None,
            )
        };
        accepted
            .then(|| object?.downcast::<NSNumber>().ok())
            .flatten()
    }

    const SAMPLE_RATE: NumberFieldKind = NumberFieldKind::UnsignedInteger {
        maximum: u32::MAX as u64,
    };

    #[test]
    fn integer_fields_reject_text_that_is_not_a_whole_number_in_range() {
        let formatter = number_formatter(SAMPLE_RATE);

        for text in ["abc", "-1", "12.5", "4294967296"] {
            assert!(parse(&formatter, text).is_none(), "{text:?} was accepted");
        }
    }

    #[test]
    fn integer_fields_accept_whole_numbers_up_to_their_maximum() {
        let formatter = number_formatter(SAMPLE_RATE);
        assert_eq!(
            parse(&formatter, "48000").map(|number| number.unsignedLongLongValue()),
            Some(48000)
        );
        assert_eq!(
            parse(&formatter, "4294967295").map(|number| number.unsignedLongLongValue()),
            Some(u64::from(u32::MAX))
        );

        let formatter = number_formatter(NumberFieldKind::UnsignedInteger {
            maximum: i64::MAX as u64,
        });
        assert_eq!(
            parse(&formatter, &i64::MAX.to_string()).map(|number| number.unsignedLongLongValue()),
            Some(i64::MAX as u64)
        );
    }

    #[test]
    fn decimal_fields_reject_text_that_is_not_a_non_negative_number() {
        let formatter = number_formatter(NumberFieldKind::Decimal);

        for text in ["abc", "-1"] {
            assert!(parse(&formatter, text).is_none(), "{text:?} was accepted");
        }
    }

    #[test]
    fn decimal_fields_show_config_values_without_rounding() {
        let formatter = number_formatter(NumberFieldKind::Decimal);

        for value in [12.0, 15.5, 10.25, 14.12345] {
            let text = formatter
                .stringFromNumber(&NSNumber::numberWithDouble(value))
                .expect("formatted number");
            assert_eq!(
                parse(&formatter, &text.to_string()).map(|number| number.doubleValue()),
                Some(value),
                "{value} displayed as {text}"
            );
        }
    }
}
