use std::cell::Cell;

use objc2::rc::Retained;
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSColor, NSEvent, NSFloatingWindowLevel,
    NSLineBreakMode, NSPanel, NSScreen, NSScrollView, NSTextAlignment, NSTextField, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use crate::state::{STATE_PROCESSING, STATE_RECORDING};

const OVERLAY_HEIGHT: f64 = 180.0;
const OVERLAY_WIDTH: f64 = 560.0;
const DEFAULT_TEXT_FONT_SIZE: f64 = 12.0;
const FOOTER_HEIGHT: f64 = 24.0;
const OVERLAY_CORNER_RADIUS: f64 = 9.0;
const SEPARATOR_HEIGHT: f64 = 1.0;
const TEXT_HORIZONTAL_PADDING: f64 = 18.0;
const TEXT_VERTICAL_PADDING: f64 = 16.0;

#[derive(Clone, Debug)]
pub struct OverlayStyle {
    pub font_name: Option<String>,
    pub font_size: f64,
    pub footer_font_size: f64,
}

#[derive(Debug)]
pub struct OverlayWindow {
    panel: Retained<NSPanel>,
    scroll_view: Retained<NSScrollView>,
    separator_view: Retained<NSView>,
    text_field: Retained<NSTextField>,
    footer_text_field: Retained<NSTextField>,
    is_visible: Cell<bool>,
}

impl OverlayWindow {
    pub fn new(mtm: MainThreadMarker, style: &OverlayStyle) -> Self {
        let panel_rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(OVERLAY_WIDTH, OVERLAY_HEIGHT),
        );
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer_screen(
            NSPanel::alloc(mtm),
            panel_rect,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
            NSScreen::mainScreen(mtm).as_deref(),
        );

        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setWorksWhenModal(true);
        panel.setLevel(NSFloatingWindowLevel);
        panel.setOpaque(false);
        panel.setHasShadow(true);
        panel.setIgnoresMouseEvents(true);
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
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), scroll_view_frame(true));
        scroll_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll_view.setDrawsBackground(false);
        scroll_view.setHasVerticalScroller(true);
        scroll_view.setHasHorizontalScroller(false);

        let text_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        text_field.setDrawsBackground(false);
        text_field.setBordered(false);
        text_field.setBezeled(false);
        text_field.setEditable(false);
        text_field.setSelectable(false);
        text_field.setTextColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0.98, 0.98, 0.99, 1.0,
        )));
        text_field.setFont(Some(&resolve_overlay_font(style, style.font_size)));
        text_field.setPreferredMaxLayoutWidth(usable_text_width());
        text_field.setFrame(NSRect::new(
            NSPoint::new(TEXT_HORIZONTAL_PADDING, TEXT_VERTICAL_PADDING),
            NSSize::new(
                usable_text_width(),
                text_area_height(true) - (TEXT_VERTICAL_PADDING * 2.0),
            ),
        ));
        text_field.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);

        if let Some(cell) = text_field.cell() {
            cell.setAlignment(NSTextAlignment::Left);
            cell.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
            cell.setUsesSingleLineMode(false);
        }

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

        let footer_text_field = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        footer_text_field.setDrawsBackground(false);
        footer_text_field.setBordered(false);
        footer_text_field.setBezeled(false);
        footer_text_field.setEditable(false);
        footer_text_field.setSelectable(false);
        footer_text_field.setTextColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0.72, 0.72, 0.75, 1.0,
        )));
        footer_text_field.setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        footer_text_field.setFrame(NSRect::new(
            NSPoint::new(TEXT_HORIZONTAL_PADDING, 6.0),
            NSSize::new(usable_text_width(), FOOTER_HEIGHT - 8.0),
        ));
        footer_text_field.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        if let Some(cell) = footer_text_field.cell() {
            cell.setAlignment(NSTextAlignment::Left);
            cell.setLineBreakMode(NSLineBreakMode::ByClipping);
            cell.setUsesSingleLineMode(true);
        }

        scroll_view.setDocumentView(Some(&text_field));
        root_view.addSubview(&scroll_view);
        root_view.addSubview(&separator_view);
        root_view.addSubview(&footer_text_field);
        panel.setContentView(Some(&root_view));
        panel.orderOut(None);

        Self {
            panel,
            scroll_view,
            separator_view,
            text_field,
            footer_text_field,
            is_visible: Cell::new(false),
        }
    }

    pub fn update(
        &self,
        mtm: MainThreadMarker,
        state: u8,
        overlay_text: &str,
        overlay_footer_text: &str,
    ) {
        let should_show = matches!(state, STATE_RECORDING | STATE_PROCESSING);
        if !should_show {
            self.hide();
            return;
        }

        let display_text = if overlay_text.trim().is_empty() {
            default_overlay_text(state)
        } else {
            overlay_text
        };
        self.update_footer_visibility(!overlay_footer_text.trim().is_empty());
        self.set_text(display_text);
        self.set_footer_text(overlay_footer_text);

        if !self.is_visible.get() {
            self.position_on_mouse_screen(mtm);
            self.panel.orderFrontRegardless();
            self.is_visible.set(true);
        }
    }

    pub fn hide(&self) {
        if self.is_visible.replace(false) {
            self.panel.orderOut(None);
        }
        self.set_text("");
        self.set_footer_text("");
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
        let ns_text = NSString::from_str(text);
        self.text_field.setStringValue(&ns_text);
        self.text_field
            .setPreferredMaxLayoutWidth(usable_text_width());

        if let Some(cell) = self.text_field.cell() {
            let measured_size = cell.cellSizeForBounds(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(usable_text_width(), f64::MAX),
            ));
            let field_height = measured_size.height.max(self.text_content_min_height());
            self.text_field.setFrame(NSRect::new(
                NSPoint::new(TEXT_HORIZONTAL_PADDING, TEXT_VERTICAL_PADDING),
                NSSize::new(usable_text_width(), field_height),
            ));

            let clip_view = self.scroll_view.contentView();
            let visible_height = self.scroll_view.contentSize().height;
            let scroll_origin_y =
                (field_height + (TEXT_VERTICAL_PADDING * 2.0) - visible_height).max(0.0);
            clip_view.scrollToPoint(NSPoint::new(0.0, scroll_origin_y));
            self.scroll_view.reflectScrolledClipView(&clip_view);
        }

        NSView::setNeedsDisplay(&self.text_field, true);
    }

    fn set_footer_text(&self, footer_text: &str) {
        self.footer_text_field
            .setStringValue(&NSString::from_str(footer_text));
        NSView::setNeedsDisplay(&self.footer_text_field, true);
    }

    fn update_footer_visibility(&self, footer_is_visible: bool) {
        self.separator_view.setHidden(!footer_is_visible);
        self.footer_text_field.setHidden(!footer_is_visible);
        self.scroll_view
            .setFrame(scroll_view_frame(footer_is_visible));
    }

    fn text_content_min_height(&self) -> f64 {
        self.scroll_view.contentSize().height - (TEXT_VERTICAL_PADDING * 2.0)
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
        return objc2_app_kit::NSFont::systemFontOfSize(normalized_font_size);
    };

    let ns_font_name = NSString::from_str(font_name);
    match objc2_app_kit::NSFont::fontWithName_size(&ns_font_name, normalized_font_size) {
        Some(font) => font,
        None => {
            log::warn!(
                "ui.font_name '{}' was not found; falling back to the system font",
                font_name
            );
            objc2_app_kit::NSFont::systemFontOfSize(normalized_font_size)
        }
    }
}

fn normalized_font_size(font_size: f64) -> f64 {
    if font_size.is_finite() && font_size > 0.0 {
        font_size
    } else {
        DEFAULT_TEXT_FONT_SIZE
    }
}

fn default_overlay_text(state: u8) -> &'static str {
    match state {
        STATE_RECORDING => "Listening…",
        STATE_PROCESSING => "Transcribing…",
        _ => "",
    }
}

fn footer_total_height() -> f64 {
    FOOTER_HEIGHT + SEPARATOR_HEIGHT
}

fn text_area_height(footer_is_visible: bool) -> f64 {
    if footer_is_visible {
        OVERLAY_HEIGHT - footer_total_height()
    } else {
        OVERLAY_HEIGHT
    }
}

fn scroll_view_frame(footer_is_visible: bool) -> NSRect {
    let origin_y = if footer_is_visible {
        footer_total_height()
    } else {
        0.0
    };

    NSRect::new(
        NSPoint::new(0.0, origin_y),
        NSSize::new(OVERLAY_WIDTH, text_area_height(footer_is_visible)),
    )
}

fn usable_text_width() -> f64 {
    OVERLAY_WIDTH - (TEXT_HORIZONTAL_PADDING * 2.0)
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

fn rect_contains_point(rect: NSRect, point: NSPoint) -> bool {
    let max_x = rect.origin.x + rect.size.width;
    let max_y = rect.origin.y + rect.size.height;

    point.x >= rect.origin.x && point.x <= max_x && point.y >= rect.origin.y && point.y <= max_y
}
