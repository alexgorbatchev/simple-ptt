use std::cell::{Cell, RefCell};
use std::sync::Arc;

use objc2::runtime::NSObject;
use objc2::runtime::ProtocolObject;
use objc2::MainThreadOnly;
use objc2::{define_class, msg_send, rc::Retained, ClassType};
use objc2_app_kit::{
    NSActionCell, NSAutoresizingMaskOptions, NSBackingStoreType, NSCell, NSColor, NSEvent,
    NSFloatingWindowLevel, NSLineBreakMode, NSPanel, NSScreen, NSScrollView, NSTextAlignment,
    NSTextField, NSTextFieldCell, NSTextView, NSTextViewDelegate, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRange, NSRect, NSSize, NSString};

use crate::config::UiMeterStyle;
use crate::state::{
    AppState, DeepgramConnectionStatus, MicMeterSnapshot, STATE_BUFFER_READY, STATE_PROCESSING,
    STATE_RECORDING, STATE_TRANSFORMING,
};
use crate::ui_meter::{self, UiMeterView};

const OVERLAY_HEIGHT: f64 = 180.0;
const OVERLAY_WIDTH: f64 = 560.0;
const DEFAULT_TEXT_FONT_SIZE: f64 = 12.0;
const DEFAULT_TEXT_FONT_WEIGHT: f64 = 0.0;
const FOOTER_HEIGHT: f64 = 24.0;
const FOOTER_HINT_GAP: f64 = 12.0;
const FOOTER_HINT_WIDTH: f64 = 330.0;
const FOOTER_STATUS_DOT_DIAMETER: f64 = 6.0;
const FOOTER_STATUS_DOT_GAP: f64 = 3.0;
const METER_CLUSTER_MAX_WIDTH: f64 = 260.0;
const METER_CLUSTER_MIN_WIDTH: f64 = 180.0;
const METER_CLUSTER_WIDTH_FACTOR: f64 = 0.48;
const METER_SECTION_BOTTOM_PADDING: f64 = 5.0;
const METER_SECTION_HEIGHT: f64 = 30.8;
const OVERLAY_CORNER_RADIUS: f64 = 9.0;
const SEPARATOR_HEIGHT: f64 = 1.0;
const TEXT_HORIZONTAL_PADDING: f64 = 18.0;
const TEXT_VERTICAL_PADDING: f64 = 16.0;

#[derive(Clone, Debug)]
pub struct OverlayStyle {
    pub font_name: Option<String>,
    pub font_size: f64,
    pub footer_font_size: f64,
    pub meter_style: UiMeterStyle,
    pub shortcut_hint: Option<String>,
}

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "OverlayPanel"]
    struct OverlayPanel;

    impl OverlayPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }
    }
);

#[derive(Debug)]
pub struct OverlayWindow {
    panel: Retained<NSPanel>,
    state: Arc<AppState>,
    scroll_view: Retained<NSScrollView>,
    separator_view: Retained<NSView>,
    ui_meter_view: UiMeterView,
    text_view: Retained<NSTextView>,
    footer_status_indicator_view: Retained<NSView>,
    footer_text_field: Retained<NSTextField>,
    footer_hint_text_field: Retained<NSTextField>,
    footer_hint: RefCell<Option<String>>,
    is_visible: Cell<bool>,
    text_opacity: Cell<f64>,
}

impl OverlayWindow {
    pub fn new(mtm: MainThreadMarker, style: &OverlayStyle, state: Arc<AppState>) -> Self {
        let panel_rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(OVERLAY_WIDTH, OVERLAY_HEIGHT),
        );
        let panel: Retained<OverlayPanel> = unsafe {
            msg_send![
                OverlayPanel::alloc(mtm),
                initWithContentRect: panel_rect,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false,
                screen: NSScreen::mainScreen(mtm).as_deref()
            ]
        };
        let panel: Retained<NSPanel> = Retained::into_super(panel);

        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setWorksWhenModal(true);
        panel.setLevel(NSFloatingWindowLevel);
        panel.setOpaque(false);
        panel.setHasShadow(true);
        panel.setIgnoresMouseEvents(false);
        panel.setHidesOnDeactivate(false);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setBackgroundColor(Some(&NSColor::clearColor()));

        let root_view = NSView::initWithFrame(NSView::alloc(mtm), panel_rect);
        root_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        root_view.setWantsLayer(true);
        if let Some(layer) = root_view.layer() {
            let background_color =
                NSColor::colorWithSRGBRed_green_blue_alpha(0.08, 0.08, 0.09, 0.92);
            let background_cg_color = background_color.CGColor();
            layer.setBackgroundColor(Some(&background_cg_color));
            layer.setCornerRadius(OVERLAY_CORNER_RADIUS);
            layer.setMasksToBounds(true);
        }

        let scroll_view =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), scroll_view_frame(true, false));
        scroll_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll_view.setDrawsBackground(false);
        scroll_view.setHasVerticalScroller(true);
        scroll_view.setHasHorizontalScroller(false);

        let text_view = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(OVERLAY_WIDTH, text_area_height(true, false)),
            ),
        );
        text_view
            .setTextContainerInset(NSSize::new(TEXT_HORIZONTAL_PADDING, TEXT_VERTICAL_PADDING));
        text_view.setDrawsBackground(false);
        text_view.setEditable(true);
        text_view.setSelectable(true);
        text_view.setRichText(false);
        text_view.setTextColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0.98, 0.98, 0.99, 1.0,
        )));
        text_view.setFont(Some(&resolve_overlay_font(style, style.font_size)));
        text_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);

        let separator_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, FOOTER_HEIGHT),
                NSSize::new(OVERLAY_WIDTH, SEPARATOR_HEIGHT),
            ),
        );
        separator_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        separator_view.setWantsLayer(true);
        if let Some(layer) = separator_view.layer() {
            let separator_color = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.12);
            let separator_cg_color = separator_color.CGColor();
            layer.setBackgroundColor(Some(&separator_cg_color));
        }

        let ui_meter_view = UiMeterView::new(mtm, style.meter_style);
        ui_meter_view.set_frame(meter_container_frame(true, style.meter_style));
        ui_meter_view
            .view()
            .setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        ui_meter_view.set_hidden(true);

        let footer_text_field = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        let custom_cell: Retained<VerticallyCenteredTextFieldCell> = unsafe {
            msg_send![
                VerticallyCenteredTextFieldCell::alloc(mtm),
                initTextCell: &*NSString::from_str("")
            ]
        };
        footer_text_field.setCell(Some(custom_cell.as_super()));
        footer_text_field.setDrawsBackground(false);
        footer_text_field.setBordered(false);
        footer_text_field.setBezeled(false);
        footer_text_field.setEditable(false);
        footer_text_field.setSelectable(false);
        footer_text_field.setTextColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0.72, 0.72, 0.75, 1.0,
        )));
        footer_text_field.setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        footer_text_field.setFrame(footer_text_frame(style.shortcut_hint.is_some()));
        footer_text_field.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        if let Some(cell) = footer_text_field.cell() {
            cell.setAlignment(NSTextAlignment::Left);
            cell.setLineBreakMode(NSLineBreakMode::ByClipping);
            cell.setUsesSingleLineMode(true);
        }

        let footer_status_indicator_view =
            NSView::initWithFrame(NSView::alloc(mtm), footer_status_indicator_frame());
        footer_status_indicator_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxXMargin);
        footer_status_indicator_view.setWantsLayer(true);
        if let Some(layer) = footer_status_indicator_view.layer() {
            let color = footer_connection_status_color(DeepgramConnectionStatus::Unknown);
            let color = color.CGColor();
            layer.setBackgroundColor(Some(&color));
            layer.setCornerRadius(FOOTER_STATUS_DOT_DIAMETER / 2.0);
            layer.setMasksToBounds(true);
        }
        footer_status_indicator_view.setHidden(true);

        let footer_hint_text_field = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        let custom_hint_cell: Retained<VerticallyCenteredTextFieldCell> = unsafe {
            msg_send![
                VerticallyCenteredTextFieldCell::alloc(mtm),
                initTextCell: &*NSString::from_str("")
            ]
        };
        footer_hint_text_field.setCell(Some(custom_hint_cell.as_super()));
        footer_hint_text_field.setDrawsBackground(false);
        footer_hint_text_field.setBordered(false);
        footer_hint_text_field.setBezeled(false);
        footer_hint_text_field.setEditable(false);
        footer_hint_text_field.setSelectable(false);
        footer_hint_text_field.setTextColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0.72, 0.72, 0.75, 1.0,
        )));
        footer_hint_text_field.setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        footer_hint_text_field.setFrame(footer_hint_frame());
        footer_hint_text_field.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        if let Some(cell) = footer_hint_text_field.cell() {
            cell.setAlignment(NSTextAlignment::Right);
            cell.setLineBreakMode(NSLineBreakMode::ByClipping);
            cell.setUsesSingleLineMode(true);
        }
        if let Some(shortcut_hint) = style.shortcut_hint.as_deref() {
            footer_hint_text_field.setStringValue(&NSString::from_str(shortcut_hint));
        }
        footer_hint_text_field.setHidden(style.shortcut_hint.is_none());

        scroll_view.setDocumentView(Some(&text_view));
        root_view.addSubview(&scroll_view);
        root_view.addSubview(ui_meter_view.view());
        root_view.addSubview(&separator_view);
        root_view.addSubview(&footer_status_indicator_view);
        root_view.addSubview(&footer_text_field);
        root_view.addSubview(&footer_hint_text_field);
        panel.setContentView(Some(&root_view));
        panel.orderOut(None);

        let overlay_window = Self {
            panel,
            state,
            scroll_view,
            separator_view,
            ui_meter_view,
            text_view,
            footer_status_indicator_view,
            footer_text_field,
            footer_hint_text_field,
            footer_hint: RefCell::new(style.shortcut_hint.clone()),
            is_visible: Cell::new(false),
            text_opacity: Cell::new(1.0),
        };
        overlay_window.ui_meter_view.clear(meter_cluster_width());
        overlay_window
    }

    pub fn update(
        &self,
        mtm: MainThreadMarker,
        state: u8,
        deepgram_connection_status: DeepgramConnectionStatus,
        overlay_dismissed: bool,
        overlay_text: &str,
        overlay_text_opacity: f64,
        overlay_footer_text: &str,
        mic_meter: MicMeterSnapshot,
    ) {
        let should_show = !overlay_dismissed
            && matches!(
                state,
                STATE_RECORDING | STATE_PROCESSING | STATE_BUFFER_READY | STATE_TRANSFORMING
            );
        if !should_show {
            self.hide();
            return;
        }

        let display_text = if overlay_text.trim().is_empty() {
            default_overlay_text(state)
        } else {
            overlay_text
        };
        let footer_text_is_visible = !overlay_footer_text.trim().is_empty();
        let footer_hint_is_visible = self.footer_hint.borrow().is_some();
        let footer_is_visible = footer_text_is_visible || footer_hint_is_visible;
        let meter_is_visible =
            state == STATE_RECORDING && self.ui_meter_view.style() != UiMeterStyle::None;
        self.update_layout(
            footer_is_visible,
            footer_text_is_visible,
            footer_hint_is_visible,
            meter_is_visible,
        );
        self.set_text(display_text);
        self.set_text_opacity(overlay_text_opacity);
        self.set_footer_status_indicator(deepgram_connection_status);
        self.set_footer_text(overlay_footer_text);

        if meter_is_visible {
            self.ui_meter_view.update(mic_meter, meter_cluster_width());
        } else {
            self.ui_meter_view.clear(meter_cluster_width());
        }

        if !self.is_visible.get() {
            self.position_on_mouse_screen(mtm);
            self.panel.orderFrontRegardless();
            self.panel.makeKeyWindow();
            self.panel.makeFirstResponder(Some(&self.text_view));
            self.is_visible.set(true);
        }

        self.state.set_overlay_window_visible(true);
    }

    pub fn hide(&self) {
        if self.is_visible.replace(false) {
            self.panel.orderOut(None);
        }
        self.state.set_overlay_window_visible(false);
        self.ui_meter_view.clear(meter_cluster_width());
        self.set_text("");
        self.set_text_opacity(1.0);
        self.set_footer_text("");
    }

    pub fn text(&self) -> String {
        self.text_view.string().to_string()
    }

    pub fn set_delegate(&self, delegate: &ProtocolObject<dyn NSTextViewDelegate>) {
        self.text_view.setDelegate(Some(delegate));
    }

    pub fn apply_style(&self, style: &OverlayStyle) {
        self.text_view
            .setFont(Some(&resolve_overlay_font(style, style.font_size)));
        self.footer_text_field
            .setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        self.footer_hint_text_field
            .setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        self.ui_meter_view.set_style(style.meter_style);
        self.footer_hint.replace(style.shortcut_hint.clone());

        if let Some(shortcut_hint) = style.shortcut_hint.as_deref() {
            self.footer_hint_text_field
                .setStringValue(&NSString::from_str(shortcut_hint));
            self.footer_hint_text_field.setHidden(false);
        } else {
            self.footer_hint_text_field
                .setStringValue(&NSString::from_str(""));
            self.footer_hint_text_field.setHidden(true);
        }
        self.footer_text_field
            .setFrame(footer_text_frame(style.shortcut_hint.is_some()));
        self.footer_status_indicator_view
            .setFrame(footer_status_indicator_frame());

        let footer_text_is_visible = !self
            .footer_text_field
            .stringValue()
            .to_string()
            .trim()
            .is_empty();
        let footer_hint_is_visible = style.shortcut_hint.is_some();
        let footer_is_visible = footer_text_is_visible || footer_hint_is_visible;
        let meter_is_visible =
            self.is_visible.get() && self.ui_meter_view.style() != UiMeterStyle::None;
        self.update_layout(
            footer_is_visible,
            footer_text_is_visible,
            footer_hint_is_visible,
            meter_is_visible,
        );
        if !meter_is_visible {
            self.ui_meter_view.clear(meter_cluster_width());
        }
        NSView::setNeedsDisplay(&self.text_view, true);
        NSView::setNeedsDisplay(&self.footer_status_indicator_view, true);
        NSView::setNeedsDisplay(&self.footer_text_field, true);
        NSView::setNeedsDisplay(&self.footer_hint_text_field, true);
    }

    fn position_on_mouse_screen(&self, mtm: MainThreadMarker) {
        let mouse_location = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
        let selected_frame = find_screen_visible_frame_for_point(&screens, mouse_location)
            .or_else(|| NSScreen::mainScreen(mtm).map(|screen| screen.visibleFrame()))
            .unwrap_or_else(|| {
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(OVERLAY_WIDTH, OVERLAY_HEIGHT),
                )
            });

        let frame_origin = NSPoint::new(
            selected_frame.origin.x + (selected_frame.size.width - OVERLAY_WIDTH) / 2.0,
            selected_frame.origin.y + (selected_frame.size.height - OVERLAY_HEIGHT) / 2.0,
        );
        let centered_frame = NSRect::new(frame_origin, NSSize::new(OVERLAY_WIDTH, OVERLAY_HEIGHT));
        self.panel.setFrame_display(centered_frame, true);
    }

    fn set_text(&self, text: &str) {
        let current_text = self.text_view.string().to_string();
        if current_text != text {
            let ns_text = NSString::from_str(text);
            self.text_view.setString(&ns_text);

            // Move cursor to the end
            let length = text.encode_utf16().count();
            self.text_view.setSelectedRange(NSRange::new(length, 0));
            self.text_view.scrollRangeToVisible(NSRange::new(length, 0));
        }
    }

    fn set_text_opacity(&self, target_text_opacity: f64) {
        let clamped_target_opacity = target_text_opacity.clamp(0.0, 1.0);
        let current_text_opacity = self.text_opacity.get();
        let next_text_opacity = if clamped_target_opacity >= current_text_opacity
            || (current_text_opacity - clamped_target_opacity).abs() <= 0.03
        {
            clamped_target_opacity
        } else {
            current_text_opacity + ((clamped_target_opacity - current_text_opacity) * 0.4)
        };

        self.text_view.setAlphaValue(next_text_opacity);
        self.text_opacity.set(next_text_opacity);
        NSView::setNeedsDisplay(&self.text_view, true);
    }

    fn set_footer_text(&self, footer_text: &str) {
        self.footer_text_field
            .setStringValue(&NSString::from_str(footer_text));
        NSView::setNeedsDisplay(&self.footer_text_field, true);
    }

    fn set_footer_status_indicator(&self, status: DeepgramConnectionStatus) {
        if let Some(layer) = self.footer_status_indicator_view.layer() {
            let color = footer_connection_status_color(status);
            let color = color.CGColor();
            layer.setBackgroundColor(Some(&color));
        }

        NSView::setNeedsDisplay(&self.footer_status_indicator_view, true);
    }

    fn update_layout(
        &self,
        footer_is_visible: bool,
        footer_text_is_visible: bool,
        footer_hint_is_visible: bool,
        meter_is_visible: bool,
    ) {
        self.separator_view.setHidden(!footer_is_visible);
        self.footer_status_indicator_view
            .setHidden(!footer_text_is_visible);
        self.footer_text_field.setHidden(!footer_text_is_visible);
        self.footer_hint_text_field
            .setHidden(!footer_hint_is_visible);
        self.scroll_view
            .setFrame(scroll_view_frame(footer_is_visible, meter_is_visible));
        self.ui_meter_view.set_frame(meter_container_frame(
            footer_is_visible,
            self.ui_meter_view.style(),
        ));
        self.ui_meter_view.set_hidden(!meter_is_visible);
    }
}

fn resolve_overlay_font(style: &OverlayStyle, font_size: f64) -> Retained<objc2_app_kit::NSFont> {
    let normalized_font_size = normalized_font_size(font_size);

    let Some(font_name) = style
        .font_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return default_overlay_font(normalized_font_size);
    };

    let ns_font_name = NSString::from_str(font_name);
    match objc2_app_kit::NSFont::fontWithName_size(&ns_font_name, normalized_font_size) {
        Some(font) => font,
        None => {
            log::warn!(
                "ui.font_name '{}' was not found; falling back to the monospaced system font",
                font_name
            );
            default_overlay_font(normalized_font_size)
        }
    }
}

fn default_overlay_font(font_size: f64) -> Retained<objc2_app_kit::NSFont> {
    objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(font_size, DEFAULT_TEXT_FONT_WEIGHT)
}

fn normalized_font_size(font_size: f64) -> f64 {
    if font_size.is_finite() && font_size > 0.0 {
        font_size
    } else {
        DEFAULT_TEXT_FONT_SIZE
    }
}

fn default_overlay_text(_state: u8) -> &'static str {
    ""
}

fn meter_cluster_width() -> f64 {
    (usable_text_width() * METER_CLUSTER_WIDTH_FACTOR)
        .clamp(METER_CLUSTER_MIN_WIDTH, METER_CLUSTER_MAX_WIDTH)
}

fn meter_container_width() -> f64 {
    meter_cluster_width() + (ui_meter::METER_BORDER_PADDING * 2.0)
}

fn footer_total_height() -> f64 {
    FOOTER_HEIGHT + SEPARATOR_HEIGHT
}

fn bottom_reserved_height(footer_is_visible: bool, meter_is_visible: bool) -> f64 {
    let footer_height = if footer_is_visible {
        footer_total_height()
    } else {
        0.0
    };
    let meter_height = if meter_is_visible {
        METER_SECTION_HEIGHT
    } else {
        0.0
    };

    footer_height + meter_height
}

fn text_area_height(footer_is_visible: bool, meter_is_visible: bool) -> f64 {
    OVERLAY_HEIGHT - bottom_reserved_height(footer_is_visible, meter_is_visible)
}

fn scroll_view_frame(footer_is_visible: bool, meter_is_visible: bool) -> NSRect {
    let origin_y = bottom_reserved_height(footer_is_visible, meter_is_visible);

    NSRect::new(
        NSPoint::new(0.0, origin_y),
        NSSize::new(
            OVERLAY_WIDTH,
            text_area_height(footer_is_visible, meter_is_visible),
        ),
    )
}

fn meter_container_frame(footer_is_visible: bool, meter_style: UiMeterStyle) -> NSRect {
    let origin_y = if footer_is_visible {
        footer_total_height() + METER_SECTION_BOTTOM_PADDING
    } else {
        METER_SECTION_BOTTOM_PADDING
    };
    let container_width = meter_container_width();
    let container_height = ui_meter::meter_container_height(meter_style);
    let origin_x = TEXT_HORIZONTAL_PADDING + ((usable_text_width() - container_width) / 2.0);

    NSRect::new(
        NSPoint::new(origin_x, origin_y),
        NSSize::new(container_width, container_height),
    )
}

fn footer_text_frame(has_footer_hint: bool) -> NSRect {
    let footer_text_width = if has_footer_hint {
        usable_text_width() - FOOTER_HINT_WIDTH - FOOTER_HINT_GAP
    } else {
        usable_text_width()
    };
    let footer_text_origin_x =
        TEXT_HORIZONTAL_PADDING + FOOTER_STATUS_DOT_DIAMETER + FOOTER_STATUS_DOT_GAP;

    NSRect::new(
        NSPoint::new(footer_text_origin_x, 6.0),
        NSSize::new(
            (footer_text_width - FOOTER_STATUS_DOT_DIAMETER - FOOTER_STATUS_DOT_GAP).max(0.0),
            FOOTER_HEIGHT - 8.0,
        ),
    )
}

fn footer_status_indicator_frame() -> NSRect {
    NSRect::new(
        NSPoint::new(
            TEXT_HORIZONTAL_PADDING - 4.0,
            5.0 + ((FOOTER_HEIGHT - 8.0 - FOOTER_STATUS_DOT_DIAMETER) / 2.0),
        ),
        NSSize::new(FOOTER_STATUS_DOT_DIAMETER, FOOTER_STATUS_DOT_DIAMETER),
    )
}

fn footer_hint_frame() -> NSRect {
    NSRect::new(
        NSPoint::new(
            OVERLAY_WIDTH - TEXT_HORIZONTAL_PADDING - FOOTER_HINT_WIDTH,
            6.0,
        ),
        NSSize::new(FOOTER_HINT_WIDTH, FOOTER_HEIGHT - 8.0),
    )
}

fn usable_text_width() -> f64 {
    OVERLAY_WIDTH - (TEXT_HORIZONTAL_PADDING * 2.0)
}

fn footer_connection_status_color(status: DeepgramConnectionStatus) -> Retained<NSColor> {
    match status {
        DeepgramConnectionStatus::Connected => {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.26, 0.86, 0.54, 1.0)
        }
        DeepgramConnectionStatus::Unknown | DeepgramConnectionStatus::Disconnected => {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.95, 0.28, 0.24, 1.0)
        }
    }
}

fn find_screen_visible_frame_for_point(
    screens: &objc2_foundation::NSArray<NSScreen>,
    point: NSPoint,
) -> Option<NSRect> {
    for index in 0..screens.count() {
        let screen = unsafe { screens.objectAtIndex_unchecked(index) };
        let visible_frame = screen.visibleFrame();
        if rect_contains_point(visible_frame, point) {
            return Some(visible_frame);
        }
    }

    None
}

fn calculate_centered_title_rect(cell: &VerticallyCenteredTextFieldCell, bounds: NSRect) -> NSRect {
    let mut rect: NSRect = unsafe { msg_send![super(cell), titleRectForBounds: bounds] };
    if let Some(font) = cell.font() {
        let font_height = font.ascender() - font.descender();
        let y_offset = (rect.size.height - font_height) / 2.0;
        rect.origin.y += y_offset;
        rect.size.height = font_height;
    }
    rect
}

define_class!(
    #[unsafe(super(NSTextFieldCell, NSActionCell, NSCell, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "VerticallyCenteredTextFieldCell"]
    pub struct VerticallyCenteredTextFieldCell;

    impl VerticallyCenteredTextFieldCell {
        #[unsafe(method(titleRectForBounds:))]
        fn title_rect_for_bounds(&self, bounds: NSRect) -> NSRect {
            calculate_centered_title_rect(self, bounds)
        }

        #[unsafe(method(drawInteriorWithFrame:inView:))]
        fn draw_interior_with_frame_in_view(&self, cell_frame: NSRect, control_view: &NSView) {
            let rect = calculate_centered_title_rect(self, cell_frame);
            let _: () = unsafe { msg_send![super(self), drawInteriorWithFrame: rect, inView: control_view] };
        }
    }
);

fn rect_contains_point(rect: NSRect, point: NSPoint) -> bool {
    let max_x = rect.origin.x + rect.size.width;
    let max_y = rect.origin.y + rect.size.height;

    point.x >= rect.origin.x && point.x <= max_x && point.y >= rect.origin.y && point.y <= max_y
}
