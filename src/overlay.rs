use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::sync::Arc;

use objc2::runtime::{AnyObject, NSObject};
use objc2::runtime::ProtocolObject;
use objc2::MainThreadOnly;
use objc2::{define_class, msg_send, rc::Retained, ClassType};
use objc2_app_kit::{
    NSActionCell, NSAutoresizingMaskOptions, NSBackingStoreType, NSCell, NSColor, NSEvent,
    NSFloatingWindowLevel, NSLineBreakMode, NSPanel, NSScreen, NSScrollView, NSUnderlineStyle,
    NSUnderlineStyleAttributeName, NSTextAlignment, NSTextField, NSTextFieldCell, NSTextView,
    NSTextViewDelegate, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSMutableAttributedString, NSNumber, NSPoint, NSRange, NSRect, NSSize,
    NSString,
};

use crate::config::UiMeterStyle;
use crate::state::{
    AppState, DeepgramConnectionStatus, MicMeterSnapshot, STATE_BUFFER_READY, STATE_PROCESSING,
    STATE_RECORDING, STATE_TRANSFORMING,
};
use crate::ui_meter::{self, UiMeterView};

const CORRECTION_OVERLAY_MIN_HEIGHT: f64 = 92.0;
const CORRECTION_OVERLAY_MAX_HEIGHT_RATIO: f64 = 0.26;
const MAIN_OVERLAY_MAX_HEIGHT_RATIO: f64 = 0.48;
const MAIN_OVERLAY_MIN_HEIGHT: f64 = 180.0;
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
const METER_TEXT_GAP: f64 = 4.0;
const METER_SECTION_BOTTOM_PADDING: f64 = 5.0;
const METER_SECTION_HEIGHT: f64 = 30.8;
const OVERLAY_CORNER_RADIUS: f64 = 9.0;
const PANEL_STACK_GAP: f64 = 10.0;
const SEPARATOR_HEIGHT: f64 = 1.0;
const TEXT_HORIZONTAL_PADDING: f64 = 18.0;
const TEXT_VERTICAL_PADDING: f64 = 16.0;
const TEXT_LAYOUT_MEASUREMENT_HEIGHT: f64 = 100_000.0;

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
    correction_panel: Retained<NSPanel>,
    state: Arc<AppState>,
    correction_scroll_view: Retained<NSScrollView>,
    correction_text_view: Retained<NSTextView>,
    separator_view: Retained<NSView>,
    ui_meter_view: UiMeterView,
    working_scroll_view: Retained<NSScrollView>,
    working_text_view: Retained<NSTextView>,
    footer_status_indicator_view: Retained<NSView>,
    footer_text_field: Retained<NSTextField>,
    footer_hint_text_field: Retained<NSTextField>,
    footer_hint: RefCell<Option<String>>,
    is_visible: Cell<bool>,
    text_opacity: Cell<f64>,
}

impl OverlayWindow {
    pub fn new(mtm: MainThreadMarker, style: &OverlayStyle, state: Arc<AppState>) -> Self {
        let main_panel_rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(OVERLAY_WIDTH, MAIN_OVERLAY_MIN_HEIGHT),
        );
        let (panel, root_view) = make_overlay_panel(mtm, main_panel_rect, false);
        let (correction_panel, correction_root_view) = make_overlay_panel(
            mtm,
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(OVERLAY_WIDTH, CORRECTION_OVERLAY_MIN_HEIGHT),
            ),
            true,
        );

        let working_scroll_view =
            NSScrollView::initWithFrame(
                NSScrollView::alloc(mtm),
                main_text_view_frame(true, false, style.meter_style, MAIN_OVERLAY_MIN_HEIGHT),
            );
        configure_scroll_view(&working_scroll_view);
        let working_text_view =
            make_text_view(
                mtm,
                style,
                main_text_area_height(true, false, style.meter_style, MAIN_OVERLAY_MIN_HEIGHT),
                true,
            );

        let correction_scroll_view = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            correction_text_view_frame(CORRECTION_OVERLAY_MIN_HEIGHT),
        );
        configure_scroll_view(&correction_scroll_view);
        let correction_text_view =
            make_text_view(mtm, style, CORRECTION_OVERLAY_MIN_HEIGHT, false);
        correction_text_view.setTextColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
            0.82, 0.86, 0.94, 1.0,
        )));

        let separator_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            separator_frame(MAIN_OVERLAY_MIN_HEIGHT),
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

        working_scroll_view.setDocumentView(Some(&working_text_view));
        correction_scroll_view.setDocumentView(Some(&correction_text_view));
        root_view.addSubview(&working_scroll_view);
        root_view.addSubview(ui_meter_view.view());
        root_view.addSubview(&separator_view);
        root_view.addSubview(&footer_status_indicator_view);
        root_view.addSubview(&footer_text_field);
        root_view.addSubview(&footer_hint_text_field);
        correction_root_view.addSubview(&correction_scroll_view);
        panel.orderOut(None);
        correction_panel.orderOut(None);

        let overlay_window = Self {
            panel,
            correction_panel,
            state,
            correction_scroll_view,
            correction_text_view,
            separator_view,
            ui_meter_view,
            working_scroll_view,
            working_text_view,
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
        overlay_correction_text: &str,
        overlay_correction_active: bool,
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
        let correction_is_visible = overlay_correction_active;
        let meter_is_visible =
            state == STATE_RECORDING && self.ui_meter_view.style() != UiMeterStyle::None;

        let inline_correction_preview = (state == STATE_TRANSFORMING
            && !overlay_correction_active
            && !overlay_correction_text.trim().is_empty())
            .then_some(overlay_correction_text);

        self.set_working_text(display_text, inline_correction_preview);
        self.set_correction_text(if correction_is_visible {
            overlay_correction_text
        } else {
            ""
        });
        self.layout_panels(
            mtm,
            correction_is_visible,
            footer_is_visible,
            footer_text_is_visible,
            footer_hint_is_visible,
            meter_is_visible,
        );
        self.set_working_text_opacity(overlay_text_opacity);
        self.set_footer_status_indicator(deepgram_connection_status);
        self.set_footer_text(overlay_footer_text);

        if meter_is_visible {
            self.ui_meter_view.update(mic_meter, meter_cluster_width());
        } else {
            self.ui_meter_view.clear(meter_cluster_width());
        }

        if !self.is_visible.get() {
            if correction_is_visible {
                self.correction_panel.orderFrontRegardless();
            }
            self.panel.orderFrontRegardless();
            self.panel.makeKeyWindow();
            self.panel.makeFirstResponder(Some(&self.working_text_view));
            self.is_visible.set(true);
        } else if correction_is_visible {
            self.correction_panel.orderFrontRegardless();
        }

        self.state.set_overlay_window_visible(true);
    }

    pub fn hide(&self) {
        if self.is_visible.replace(false) {
            self.panel.orderOut(None);
            self.correction_panel.orderOut(None);
        }
        self.state.set_overlay_window_visible(false);
        self.ui_meter_view.clear(meter_cluster_width());
        self.set_working_text("", None);
        self.set_correction_text("");
        self.set_working_text_opacity(1.0);
        self.set_footer_text("");
    }

    pub fn text(&self) -> String {
        self.working_text_view.string().to_string()
    }

    pub fn set_delegate(&self, delegate: &ProtocolObject<dyn NSTextViewDelegate>) {
        self.working_text_view.setDelegate(Some(delegate));
    }

    pub fn apply_style(&self, style: &OverlayStyle) {
        self.working_text_view
            .setFont(Some(&resolve_overlay_font(style, style.font_size)));
        self.correction_text_view
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

        if !self.is_visible.get() {
            self.ui_meter_view.clear(meter_cluster_width());
        }

        NSView::setNeedsDisplay(&self.working_text_view, true);
        NSView::setNeedsDisplay(&self.correction_text_view, true);
        NSView::setNeedsDisplay(&self.footer_status_indicator_view, true);
        NSView::setNeedsDisplay(&self.footer_text_field, true);
        NSView::setNeedsDisplay(&self.footer_hint_text_field, true);
    }

    fn layout_panels(
        &self,
        mtm: MainThreadMarker,
        correction_is_visible: bool,
        footer_is_visible: bool,
        footer_text_is_visible: bool,
        footer_hint_is_visible: bool,
        meter_is_visible: bool,
    ) {
        let visible_frame = self.selected_visible_frame(mtm);
        let meter_style = self.ui_meter_view.style();
        let main_content_height = measured_text_height(&self.working_text_view);
        let correction_content_height = correction_is_visible
            .then(|| measured_text_height(&self.correction_text_view))
            .unwrap_or(CORRECTION_OVERLAY_MIN_HEIGHT);
        let main_height = main_panel_height(
            main_content_height,
            footer_is_visible,
            meter_is_visible,
            meter_style,
            visible_frame.size.height,
        );
        let main_reserved_height =
            bottom_reserved_height(footer_is_visible, meter_is_visible, meter_style);
        let main_min_text_area_height = (MAIN_OVERLAY_MIN_HEIGHT - main_reserved_height).max(0.0);
        let main_desired_height = main_reserved_height + main_content_height.max(main_min_text_area_height);
        let main_is_clamped = main_height + 1.0 < main_desired_height;
        let correction_height = correction_is_visible.then(|| {
            correction_panel_height(
                correction_content_height,
                visible_frame.size.height,
            )
        });
        let correction_desired_height = correction_content_height.max(CORRECTION_OVERLAY_MIN_HEIGHT);
        let correction_is_clamped = correction_height
            .map(|height| height + 1.0 < correction_desired_height)
            .unwrap_or(false);

        let current_main_origin_y = self.is_visible.get().then(|| self.panel.frame().origin.y);
        let (main_frame, correction_frame) = stacked_panel_frames(
            visible_frame,
            main_height,
            correction_height,
            current_main_origin_y,
        );

        self.panel.setFrame_display(main_frame, true);
        self.separator_view.setHidden(!footer_is_visible);
        self.footer_status_indicator_view
            .setHidden(!footer_text_is_visible);
        self.footer_text_field.setHidden(!footer_text_is_visible);
        self.footer_hint_text_field
            .setHidden(!footer_hint_is_visible);
        self.working_scroll_view.setFrame(main_text_view_frame(
            footer_is_visible,
            meter_is_visible,
            meter_style,
            main_height,
        ));
        self.resize_text_view(
            &self.working_scroll_view,
            &self.working_text_view,
            main_text_area_height(footer_is_visible, meter_is_visible, meter_style, main_height),
            main_is_clamped,
        );
        self.separator_view.setFrame(separator_frame(main_height));
        self.ui_meter_view.set_frame(meter_container_frame(
            footer_is_visible,
            self.ui_meter_view.style(),
        ));
        self.ui_meter_view.set_hidden(!meter_is_visible);

        match correction_frame {
            Some(frame) => {
                self.correction_panel.setFrame_display(frame, true);
                self.correction_scroll_view
                    .setFrame(correction_text_view_frame(frame.size.height));
                self.resize_text_view(
                    &self.correction_scroll_view,
                    &self.correction_text_view,
                    frame.size.height,
                    correction_is_clamped,
                );
                self.correction_panel.orderFrontRegardless();
            }
            None => {
                self.correction_panel.orderOut(None);
            }
        }
    }

    fn selected_visible_frame(&self, mtm: MainThreadMarker) -> NSRect {
        let screens = NSScreen::screens(mtm);
        if self.is_visible.get() {
            let current_frame = self.panel.frame();
            let current_center = NSPoint::new(
                current_frame.origin.x + (current_frame.size.width / 2.0),
                current_frame.origin.y + (current_frame.size.height / 2.0),
            );
            if let Some(frame) = find_screen_visible_frame_for_point(&screens, current_center) {
                return frame;
            }
        }

        let mouse_location = NSEvent::mouseLocation();
        find_screen_visible_frame_for_point(&screens, mouse_location)
            .or_else(|| NSScreen::mainScreen(mtm).map(|screen| screen.visibleFrame()))
            .unwrap_or_else(|| {
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(OVERLAY_WIDTH, MAIN_OVERLAY_MIN_HEIGHT),
                )
            })
    }

    fn set_working_text(&self, text: &str, inline_correction_preview: Option<&str>) {
        if let Some(preview_text) = inline_correction_preview {
            self.set_working_text_with_preview(text, preview_text);
            return;
        }

        let current_text = self.working_text_view.string().to_string();
        if working_text_update_is_semantically_unchanged(&current_text, text) {
            return;
        }

        let ns_text = NSString::from_str(text);
        self.working_text_view.setString(&ns_text);

        // Move cursor to the end
        let length = text.encode_utf16().count();
        self.working_text_view.setSelectedRange(NSRange::new(length, 0));
        self.working_text_view
            .scrollRangeToVisible(NSRange::new(length, 0));
    }

    fn set_working_text_with_preview(&self, original_text: &str, preview_text: &str) {
        let Some(rendered_preview) = build_inline_correction_preview(original_text, preview_text)
        else {
            self.set_working_text(original_text, None);
            return;
        };

        let ns_text = NSString::from_str(&rendered_preview.text);
        let typing_attributes = self.working_text_view.typingAttributes();
        let base_attributed_text = unsafe {
            objc2_foundation::NSAttributedString::new_with_attributes(&ns_text, &typing_attributes)
        };
        let attributed_text = NSMutableAttributedString::from_attributed_nsstring(&base_attributed_text);
        let underline_style = NSNumber::new_isize(NSUnderlineStyle::Single.bits() as isize);

        for range in &rendered_preview.underlined_byte_ranges {
            let location = utf16_offset(&rendered_preview.text, range.start);
            let length = rendered_preview.text[range.start..range.end].encode_utf16().count();
            if length == 0 {
                continue;
            }

            unsafe {
                attributed_text.addAttribute_value_range(
                    NSUnderlineStyleAttributeName,
                    underline_style.as_ref(),
                    NSRange::new(location, length),
                );
            }
        }

        if let Some(text_storage) = unsafe { self.working_text_view.textStorage() } {
            text_storage.beginEditing();
            text_storage.setAttributedString(&attributed_text);
            text_storage.endEditing();
        } else {
            self.working_text_view.setString(&ns_text);
        }

        let length = rendered_preview.text.encode_utf16().count();
        self.working_text_view.setSelectedRange(NSRange::new(length, 0));
        self.working_text_view
            .scrollRangeToVisible(NSRange::new(length, 0));
    }

    fn set_correction_text(&self, text: &str) {
        let current_text = self.correction_text_view.string().to_string();
        if current_text != text {
            let ns_text = NSString::from_str(text);
            self.correction_text_view.setString(&ns_text);
            let length = text.encode_utf16().count();
            self.correction_text_view
                .scrollRangeToVisible(NSRange::new(length, 0));
        }
    }

    fn resize_text_view(
        &self,
        scroll_view: &NSScrollView,
        text_view: &NSTextView,
        visible_height: f64,
        shows_vertical_scroller: bool,
    ) {
        let document_height = measured_text_height(text_view).max(visible_height);
        scroll_view.setHasVerticalScroller(shows_vertical_scroller);
        text_view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(OVERLAY_WIDTH, document_height),
        ));
    }

    fn set_working_text_opacity(&self, target_text_opacity: f64) {
        let clamped_target_opacity = target_text_opacity.clamp(0.0, 1.0);
        let current_text_opacity = self.text_opacity.get();
        let next_text_opacity = if clamped_target_opacity >= current_text_opacity
            || (current_text_opacity - clamped_target_opacity).abs() <= 0.03
        {
            clamped_target_opacity
        } else {
            current_text_opacity + ((clamped_target_opacity - current_text_opacity) * 0.4)
        };

        self.working_text_view.setAlphaValue(next_text_opacity);
        self.text_opacity.set(next_text_opacity);
        NSView::setNeedsDisplay(&self.working_text_view, true);
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
}

#[derive(Debug, PartialEq, Eq)]
struct InlinePreviewRender {
    text: String,
    underlined_byte_ranges: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TokenSpan {
    byte_range: Range<usize>,
}

fn build_inline_correction_preview(
    original_text: &str,
    preview_text: &str,
) -> Option<InlinePreviewRender> {
    let original_tokens = non_whitespace_tokens(original_text);
    let preview_tokens = non_whitespace_tokens(preview_text);

    if original_tokens.is_empty() && preview_tokens.is_empty() {
        return None;
    }

    let shared_prefix_len =
        shared_prefix_len(original_text, &original_tokens, preview_text, &preview_tokens);
    let shared_suffix_len = shared_suffix_len(
        original_text,
        &original_tokens,
        preview_text,
        &preview_tokens,
        shared_prefix_len,
    );
    let original_has_divergence = shared_prefix_len + shared_suffix_len < original_tokens.len();
    let preview_has_divergence = shared_prefix_len + shared_suffix_len < preview_tokens.len();

    let prefix_end = original_tokens
        .get(shared_prefix_len)
        .map(|token| token.byte_range.start)
        .unwrap_or(original_text.len());
    let mut rendered_text = original_text[..prefix_end].to_owned();
    let mut underlined_byte_ranges = Vec::new();

    if shared_prefix_len > 0 {
        underlined_byte_ranges.push(original_tokens[shared_prefix_len - 1].byte_range.clone());
    }

    if preview_has_divergence {
        let preview_segment_start = if shared_prefix_len < original_tokens.len() || shared_prefix_len == 0 {
            preview_tokens[shared_prefix_len].byte_range.start
        } else {
            preview_tokens[shared_prefix_len - 1].byte_range.end
        };
        let preview_segment_end = if shared_suffix_len > 0 {
            preview_tokens[preview_tokens.len() - shared_suffix_len]
                .byte_range
                .start
        } else {
            preview_text.len()
        };
        let preview_insert_offset = rendered_text.len();
        rendered_text.push_str(&preview_text[preview_segment_start..preview_segment_end]);

        let first_preview_token = &preview_tokens[shared_prefix_len];
        let last_preview_token = &preview_tokens[preview_tokens.len() - shared_suffix_len - 1];
        underlined_byte_ranges.push(
            (preview_insert_offset + (first_preview_token.byte_range.start - preview_segment_start))
                ..(preview_insert_offset + (last_preview_token.byte_range.end - preview_segment_start)),
        );
    }

    if shared_suffix_len > 0 {
        let suffix_start = original_tokens[original_tokens.len() - shared_suffix_len]
            .byte_range
            .start;
        rendered_text.push_str(&original_text[suffix_start..]);
    } else if original_has_divergence {
        let preserved_tail_start = original_tokens[shared_prefix_len].byte_range.end;
        rendered_text.push_str(&original_text[preserved_tail_start..]);
    }

    Some(InlinePreviewRender {
        text: rendered_text,
        underlined_byte_ranges,
    })
}

fn non_whitespace_tokens(text: &str) -> Vec<TokenSpan> {
    let mut tokens = Vec::new();
    let mut current_start = None;

    for (index, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = current_start.take() {
                tokens.push(TokenSpan {
                    byte_range: start..index,
                });
            }
        } else if current_start.is_none() {
            current_start = Some(index);
        }
    }

    if let Some(start) = current_start {
        tokens.push(TokenSpan {
            byte_range: start..text.len(),
        });
    }

    tokens
}

fn shared_prefix_len(
    original_text: &str,
    original_tokens: &[TokenSpan],
    preview_text: &str,
    preview_tokens: &[TokenSpan],
) -> usize {
    original_tokens
        .iter()
        .zip(preview_tokens)
        .take_while(|(original, preview)| {
            original_text[original.byte_range.clone()] == preview_text[preview.byte_range.clone()]
        })
        .count()
}

fn shared_suffix_len(
    original_text: &str,
    original_tokens: &[TokenSpan],
    preview_text: &str,
    preview_tokens: &[TokenSpan],
    shared_prefix_len: usize,
) -> usize {
    let max_original_suffix = original_tokens.len().saturating_sub(shared_prefix_len);
    let max_preview_suffix = preview_tokens.len().saturating_sub(shared_prefix_len);
    let max_suffix_len = max_original_suffix.min(max_preview_suffix);
    let mut shared_suffix_len = 0;

    while shared_suffix_len < max_suffix_len {
        let original = &original_tokens[original_tokens.len() - shared_suffix_len - 1];
        let preview = &preview_tokens[preview_tokens.len() - shared_suffix_len - 1];
        if original_text[original.byte_range.clone()] != preview_text[preview.byte_range.clone()] {
            break;
        }
        shared_suffix_len += 1;
    }

    shared_suffix_len
}

fn utf16_offset(text: &str, byte_offset: usize) -> usize {
    text[..byte_offset].encode_utf16().count()
}

fn working_text_update_is_semantically_unchanged(current_text: &str, next_text: &str) -> bool {
    current_text == next_text || current_text.trim_end() == next_text.trim_end()
}

#[cfg(test)]
mod tests {
    use super::{build_inline_correction_preview, working_text_update_is_semantically_unchanged};

    #[test]
    fn renders_inline_replacement_with_suffix_anchor() {
        let preview =
            build_inline_correction_preview("the quick brown fox jumps", "the quick red fox jumps")
                .expect("preview should render");

        assert_eq!(preview.text, "the quick red fox jumps");
        assert_eq!(
            underlined_segments(&preview.text, &preview.underlined_byte_ranges),
            vec!["quick", "red"]
        );
    }

    #[test]
    fn preserves_tail_while_stream_has_not_reanchored() {
        let preview = build_inline_correction_preview("the quick brown fox jumps", "the quick red")
            .expect("preview should render");

        assert_eq!(preview.text, "the quick red fox jumps");
        assert_eq!(
            underlined_segments(&preview.text, &preview.underlined_byte_ranges),
            vec!["quick", "red"]
        );
    }

    #[test]
    fn preserves_spacing_for_insertions_at_end() {
        let preview =
            build_inline_correction_preview("hello", "hello world").expect("preview should render");

        assert_eq!(preview.text, "hello world");
        assert_eq!(
            underlined_segments(&preview.text, &preview.underlined_byte_ranges),
            vec!["hello", "world"]
        );
    }

    #[test]
    fn handles_pure_deletions() {
        let preview =
            build_inline_correction_preview("hello old world", "hello world")
                .expect("preview should render");

        assert_eq!(preview.text, "hello world");
        assert_eq!(
            underlined_segments(&preview.text, &preview.underlined_byte_ranges),
            vec!["hello"]
        );
    }

    #[test]
    fn treats_trailing_whitespace_only_updates_as_unchanged() {
        assert!(working_text_update_is_semantically_unchanged(
            "hello world",
            "hello world "
        ));
        assert!(working_text_update_is_semantically_unchanged(
            "hello world  ",
            "hello world"
        ));
        assert!(!working_text_update_is_semantically_unchanged(
            "hello world",
            "hello there"
        ));
    }

    fn underlined_segments<'a>(text: &'a str, ranges: &[std::ops::Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|range| &text[range.clone()]).collect()
    }
}

fn make_overlay_panel(
    mtm: MainThreadMarker,
    panel_rect: NSRect,
    ignores_mouse_events: bool,
) -> (Retained<NSPanel>, Retained<NSView>) {
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
    panel.setIgnoresMouseEvents(ignores_mouse_events);
    panel.setHidesOnDeactivate(false);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::MoveToActiveSpace
            | NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    panel.setBackgroundColor(Some(&NSColor::clearColor()));

    let root_view = NSView::initWithFrame(NSView::alloc(mtm), panel_rect);
    root_view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    root_view.setWantsLayer(true);
    if let Some(layer) = root_view.layer() {
        let background_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.08, 0.08, 0.09, 0.92);
        let background_cg_color = background_color.CGColor();
        layer.setBackgroundColor(Some(&background_cg_color));
        layer.setCornerRadius(OVERLAY_CORNER_RADIUS);
        layer.setMasksToBounds(true);
    }
    panel.setContentView(Some(&root_view));

    (panel, root_view)
}

fn configure_scroll_view(scroll_view: &NSScrollView) {
    scroll_view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    scroll_view.setDrawsBackground(false);
    scroll_view.setHasVerticalScroller(false);
    scroll_view.setHasHorizontalScroller(false);
    unsafe {
        let _: () = msg_send![scroll_view, setAutohidesScrollers: true];
    }
}

fn make_text_view(
    mtm: MainThreadMarker,
    style: &OverlayStyle,
    height: f64,
    editable: bool,
) -> Retained<NSTextView> {
    let text_view = NSTextView::initWithFrame(
        NSTextView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(OVERLAY_WIDTH, height)),
    );
    text_view.setTextContainerInset(NSSize::new(TEXT_HORIZONTAL_PADDING, TEXT_VERTICAL_PADDING));
    text_view.setDrawsBackground(false);
    text_view.setEditable(editable);
    text_view.setSelectable(true);
    text_view.setRichText(false);
    text_view.setTextColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
        0.98, 0.98, 0.99, 1.0,
    )));
    text_view.setFont(Some(&resolve_overlay_font(style, style.font_size)));
    text_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    text_view
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

fn measured_text_height(text_view: &NSTextView) -> f64 {
    let Some(text_container): Option<Retained<AnyObject>> = (unsafe { msg_send![text_view, textContainer] }) else {
        return TEXT_VERTICAL_PADDING * 2.0;
    };
    let Some(layout_manager): Option<Retained<AnyObject>> = (unsafe { msg_send![text_view, layoutManager] }) else {
        return TEXT_VERTICAL_PADDING * 2.0;
    };

    let line_fragment_padding: f64 = unsafe { msg_send![&*text_container, lineFragmentPadding] };
    let container_width = (usable_text_width() - (line_fragment_padding * 2.0)).max(1.0);
    unsafe {
        let _: () = msg_send![&*text_container, setContainerSize: NSSize::new(container_width, TEXT_LAYOUT_MEASUREMENT_HEIGHT)];
        let _: NSRange = msg_send![&*layout_manager, glyphRangeForTextContainer: &*text_container];
        let used_rect: NSRect = msg_send![&*layout_manager, usedRectForTextContainer: &*text_container];
        (TEXT_VERTICAL_PADDING * 2.0) + used_rect.size.height.ceil().max(1.0)
    }
}

fn main_panel_height(
    text_height: f64,
    footer_is_visible: bool,
    meter_is_visible: bool,
    meter_style: UiMeterStyle,
    screen_height: f64,
) -> f64 {
    let reserved_height = bottom_reserved_height(footer_is_visible, meter_is_visible, meter_style);
    let min_text_area_height = (MAIN_OVERLAY_MIN_HEIGHT - reserved_height).max(0.0);
    let desired_text_area_height = text_height.max(min_text_area_height);
    let max_height = (screen_height * MAIN_OVERLAY_MAX_HEIGHT_RATIO).max(MAIN_OVERLAY_MIN_HEIGHT);

    (reserved_height + desired_text_area_height)
        .max(MAIN_OVERLAY_MIN_HEIGHT)
        .min(max_height)
}

fn correction_panel_height(text_height: f64, screen_height: f64) -> f64 {
    let max_height =
        (screen_height * CORRECTION_OVERLAY_MAX_HEIGHT_RATIO).max(CORRECTION_OVERLAY_MIN_HEIGHT);

    text_height
        .max(CORRECTION_OVERLAY_MIN_HEIGHT)
        .min(max_height)
}

fn stacked_panel_frames(
    visible_frame: NSRect,
    main_height: f64,
    correction_height: Option<f64>,
    current_main_origin_y: Option<f64>,
) -> (NSRect, Option<NSRect>) {
    let visible_max_y = visible_frame.origin.y + visible_frame.size.height;
    let centered_origin_x = visible_frame.origin.x + ((visible_frame.size.width - OVERLAY_WIDTH) / 2.0);
    let stacked_height = main_height
        + correction_height
            .map(|height| height + PANEL_STACK_GAP)
            .unwrap_or(0.0);

    let mut main_origin_y = current_main_origin_y.unwrap_or_else(|| {
        visible_frame.origin.y + ((visible_frame.size.height - stacked_height) / 2.0)
    });
    main_origin_y = main_origin_y.max(visible_frame.origin.y);
    main_origin_y = main_origin_y.min(visible_max_y - main_height);

    let mut correction_origin_y = correction_height.map(|_| main_origin_y + main_height + PANEL_STACK_GAP);
    if let (Some(correction_height), Some(current_correction_origin_y)) =
        (correction_height, correction_origin_y.as_mut())
    {
        let overflow = (*current_correction_origin_y + correction_height) - visible_max_y;
        if overflow > 0.0 {
            let available_shift = main_origin_y - visible_frame.origin.y;
            let shift = overflow.min(available_shift);
            main_origin_y -= shift;
            *current_correction_origin_y -= shift;
        }
    }

    let main_frame = NSRect::new(
        NSPoint::new(centered_origin_x, main_origin_y),
        NSSize::new(OVERLAY_WIDTH, main_height),
    );
    let correction_frame = correction_height.zip(correction_origin_y).map(|(height, origin_y)| {
        NSRect::new(
            NSPoint::new(centered_origin_x, origin_y),
            NSSize::new(OVERLAY_WIDTH, height),
        )
    });

    (main_frame, correction_frame)
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

fn bottom_reserved_height(
    footer_is_visible: bool,
    meter_is_visible: bool,
    meter_style: UiMeterStyle,
) -> f64 {
    let footer_height = if footer_is_visible {
        footer_total_height()
    } else {
        0.0
    };
    let meter_height = if meter_is_visible {
        meter_reserved_height(meter_style)
    } else {
        0.0
    };

    footer_height + meter_height
}

fn main_text_area_height(
    footer_is_visible: bool,
    meter_is_visible: bool,
    meter_style: UiMeterStyle,
    panel_height: f64,
) -> f64 {
    panel_height - bottom_reserved_height(footer_is_visible, meter_is_visible, meter_style)
}

fn main_text_view_frame(
    footer_is_visible: bool,
    meter_is_visible: bool,
    meter_style: UiMeterStyle,
    panel_height: f64,
) -> NSRect {
    let origin_y = bottom_reserved_height(footer_is_visible, meter_is_visible, meter_style);

    NSRect::new(
        NSPoint::new(0.0, origin_y),
        NSSize::new(
            OVERLAY_WIDTH,
            main_text_area_height(footer_is_visible, meter_is_visible, meter_style, panel_height),
        ),
    )
}

fn meter_reserved_height(meter_style: UiMeterStyle) -> f64 {
    match meter_style {
        UiMeterStyle::AnimatedColor => {
            ui_meter::meter_container_height(meter_style)
                + METER_SECTION_BOTTOM_PADDING
                + METER_TEXT_GAP
        }
        UiMeterStyle::AnimatedHeight | UiMeterStyle::None => METER_SECTION_HEIGHT,
    }
}

fn correction_text_view_frame(panel_height: f64) -> NSRect {
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(OVERLAY_WIDTH, panel_height))
}

fn separator_frame(_panel_height: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(0.0, FOOTER_HEIGHT),
        NSSize::new(OVERLAY_WIDTH, SEPARATOR_HEIGHT),
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
