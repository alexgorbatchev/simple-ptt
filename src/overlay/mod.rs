pub mod dev;
pub mod diff;
pub mod glass;
pub mod private_effects;
mod text_effects;

pub use diff::{build_inline_correction_preview, utf16_offset, working_text_update_is_semantically_unchanged};

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use objc2::runtime::ProtocolObject;
use objc2::runtime::{AnyObject, NSObject};
use objc2::MainThreadOnly;
use objc2::{define_class, msg_send, rc::Retained, ClassType};
use objc2_app_kit::{
    NSActionCell, NSAutoresizingMaskOptions, NSBox, NSBoxType, NSCell, NSColor, NSEvent, NSFont, NSLineBreakMode,
    NSScreen, NSScrollView, NSTextAlignment, NSTextField, NSTextFieldCell, NSTextView,
    NSTextViewDelegate, NSUnderlineColorAttributeName, NSUnderlineStyle,
    NSUnderlineStyleAttributeName, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSMutableAttributedString, NSNumber, NSPoint, NSRange, NSRect, NSSize,
    NSString,
};

use crate::config::UiMeterStyle;
use crate::state::{
    AppState, DeepgramApiKeyFingerprint, DeepgramConnection, DeepgramConnectionStatus,
    MicMeterSnapshot, OverlayText, STATE_BUFFER_READY, STATE_ERROR, STATE_PROCESSING,
    STATE_RECORDING, STATE_TRANSFORMING,
};
use crate::ui_meter::{self, UiMeterView};
use glass::{
    correction_resting_frame, plan_correction_motion, CorrectionEffect, CorrectionPhase,
    GlassTuning, OverlayGlass,
};
use text_effects::{
    attributed_text, crossfade_next_change, provisional_utf16_range, reduce_motion,
    restyle_provisional, text_attributes, Shimmer,
};

const CORRECTION_OVERLAY_MIN_HEIGHT: f64 = 92.0;
const CORRECTION_OVERLAY_MAX_HEIGHT_RATIO: f64 = 0.26;
const MAIN_OVERLAY_MIN_HEIGHT: f64 = 180.0;
const OVERLAY_WIDTH: f64 = 560.0;
const DEFAULT_TEXT_FONT_SIZE: f64 = 12.0;
const DEFAULT_TEXT_FONT_WEIGHT: f64 = 0.0;
const FOOTER_HEIGHT: f64 = 40.0;
const FOOTER_LINE_HEIGHT: f64 = 14.0;
const FOOTER_VERTICAL_PADDING: f64 = 5.0;
const FOOTER_STATUS_DOT_DIAMETER: f64 = 6.0;
const FOOTER_STATUS_DOT_GAP: f64 = 3.0;
const FOOTER_STATUS_DOT_X_OFFSET: f64 = -6.0;
const FOOTER_TEXT_X_OFFSET: f64 = -4.0;
const FOOTER_TEXT_SRGB: (f64, f64, f64) = (0.5, 0.5, 0.5);
const METER_CLUSTER_MAX_WIDTH: f64 = 260.0;
const METER_CLUSTER_MIN_WIDTH: f64 = 180.0;
const METER_CLUSTER_WIDTH_FACTOR: f64 = 0.48;
const METER_TEXT_GAP: f64 = 4.0;
const METER_SECTION_BOTTOM_PADDING: f64 = 5.0;
const METER_SECTION_HEIGHT: f64 = 30.8;
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
    /// The Deepgram API key that resolves now (config or `DEEPGRAM_API_KEY`).
    /// The footer shows the connection dot only when there is one, and shows a
    /// connection status only if it was measured with this key.
    pub deepgram_api_key: Option<DeepgramApiKeyFingerprint>,
}

#[derive(Debug)]
pub struct OverlayWindow {
    glass: OverlayGlass,
    state: Arc<AppState>,
    correction_scroll_view: Retained<NSScrollView>,
    correction_text_view: Retained<NSTextView>,
    separator_view: Retained<NSBox>,
    ui_meter_view: UiMeterView,
    working_scroll_view: Retained<NSScrollView>,
    working_text_view: Retained<NSTextView>,
    footer_status_indicator_view: Retained<NSView>,
    footer_text_field: Retained<NSTextField>,
    footer_hint_text_field: Retained<NSTextField>,
    footer_hint: RefCell<Option<String>>,
    deepgram_api_key: Cell<Option<DeepgramApiKeyFingerprint>>,
    is_visible: Cell<bool>,
    is_error_color: Cell<bool>,
    text_opacity: Cell<f64>,
    meter_alpha: Cell<f64>,
    text_font: RefCell<Retained<NSFont>>,
    /// Provisional start the working text was last drawn with.
    working_provisional_start: Cell<Option<usize>>,
    /// Provisional start and failure color the correction text was last
    /// drawn with.
    correction_provisional_start: Cell<Option<usize>>,
    correction_failed: Cell<bool>,
    /// Height of the correction glass while shown, kept so the window has
    /// room for it while it pops out.
    last_correction_height: Cell<f64>,
    shimmer: Shimmer,
    /// Distance from the top of the screen the overlay is kept at, instead
    /// of centred; see `pin_to_top`.
    pinned_top: Cell<Option<f64>>,
}

impl OverlayWindow {
    pub fn new(mtm: MainThreadMarker, style: &OverlayStyle, state: Arc<AppState>) -> Self {
        let glass = OverlayGlass::new(mtm, OVERLAY_WIDTH, MAIN_OVERLAY_MIN_HEIGHT, GlassTuning::default());
        let root_view = glass.main_content_view.clone();
        let correction_root_view = glass.correction_content_view.clone();

        let working_scroll_view = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            main_text_view_frame(true, false, style.meter_style, MAIN_OVERLAY_MIN_HEIGHT),
        );
        configure_scroll_view(&working_scroll_view);
        // Its frame is set on every update; kept at the bottom as the main
        // glass grows at its top for the `Expand` correction effect.
        working_scroll_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        let working_text_view = make_text_view(
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
        // Its frame is set on every update; kept at the top of the correction
        // glass rather than stretched when the glass is resized.
        // `layout_panels` sets this again whenever it changes hosts.
        correction_scroll_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let correction_text_view = make_text_view(mtm, style, CORRECTION_OVERLAY_MIN_HEIGHT, false);

        let separator_view =
            NSBox::initWithFrame(NSBox::alloc(mtm), separator_frame(MAIN_OVERLAY_MIN_HEIGHT));
        separator_view.setBoxType(NSBoxType::Separator);
        separator_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);

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
        footer_text_field.setTextColor(Some(&footer_text_color()));
        footer_text_field.setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        footer_text_field.setFrame(footer_text_frame(style.shortcut_hint.is_some()));
        footer_text_field.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        if let Some(cell) = footer_text_field.cell() {
            cell.setAlignment(NSTextAlignment::Left);
            cell.setLineBreakMode(NSLineBreakMode::ByClipping);
            cell.setUsesSingleLineMode(true);
        }

        let footer_status_indicator_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            footer_status_indicator_frame(style.shortcut_hint.is_some()),
        );
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
        footer_hint_text_field.setTextColor(Some(&footer_text_color()));
        footer_hint_text_field.setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        footer_hint_text_field.setFrame(footer_hint_frame());
        footer_hint_text_field.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        if let Some(cell) = footer_hint_text_field.cell() {
            cell.setAlignment(NSTextAlignment::Left);
            cell.setLineBreakMode(NSLineBreakMode::ByClipping);
            cell.setUsesSingleLineMode(true);
        }
        if let Some(shortcut_hint) = style.shortcut_hint.as_deref() {
            footer_hint_text_field.setStringValue(&NSString::from_str(shortcut_hint));
        }
        footer_hint_text_field.setHidden(style.shortcut_hint.is_none());

        set_text_document(&working_scroll_view, &working_text_view);
        set_text_document(&correction_scroll_view, &correction_text_view);
        root_view.addSubview(&working_scroll_view);
        root_view.addSubview(ui_meter_view.view());
        root_view.addSubview(&separator_view);
        root_view.addSubview(&footer_status_indicator_view);
        root_view.addSubview(&footer_text_field);
        root_view.addSubview(&footer_hint_text_field);
        correction_root_view.addSubview(&correction_scroll_view);
        glass.panel.orderOut(None);

        let overlay_window = Self {
            glass,
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
            deepgram_api_key: Cell::new(style.deepgram_api_key),
            is_visible: Cell::new(false),
            is_error_color: Cell::new(false),
            text_opacity: Cell::new(1.0),
            meter_alpha: Cell::new(0.0),
            text_font: RefCell::new(resolve_overlay_font(style, style.font_size)),
            working_provisional_start: Cell::new(None),
            correction_provisional_start: Cell::new(None),
            correction_failed: Cell::new(false),
            last_correction_height: Cell::new(CORRECTION_OVERLAY_MIN_HEIGHT),
            shimmer: Shimmer::new(),
            pinned_top: Cell::new(None),
        };
        overlay_window.ui_meter_view.clear(meter_cluster_width());
        overlay_window
    }

    pub fn update(
        &self,
        mtm: MainThreadMarker,
        state: u8,
        deepgram_connection: DeepgramConnection,
        overlay_dismissed: bool,
        overlay_text: &OverlayText,
        overlay_error_text: &str,
        overlay_correction_text: &OverlayText,
        overlay_correction_active: bool,
        overlay_text_opacity: f64,
        overlay_footer_text: &str,
        mic_meter: MicMeterSnapshot,
    ) {
        let should_show = !overlay_dismissed
            && matches!(
                state,
                STATE_RECORDING | STATE_PROCESSING | STATE_BUFFER_READY | STATE_TRANSFORMING | STATE_ERROR
            );
        if !should_show {
            self.hide();
            return;
        }

        let is_error = state == STATE_ERROR;

        let (display_text, display_provisional_start) = if is_error {
            if overlay_error_text.trim().is_empty() {
                ("An unexpected error occurred", None)
            } else {
                (overlay_error_text, None)
            }
        } else if overlay_text.text.trim().is_empty() {
            (default_overlay_text(state), None)
        } else {
            (&*overlay_text.text, overlay_text.provisional_start)
        };
        let overlay_correction_text_value = &*overlay_correction_text.text;
        let current_api_key = self.deepgram_api_key.get();
        let deepgram_connection_status = deepgram_connection.status_for(current_api_key);
        let footer_status_text = footer_status_text(
            overlay_footer_text,
            current_api_key.is_some(),
            deepgram_connection_status,
        );
        let footer_status_is_visible = footer_status_text.is_some();
        let footer_hint_is_visible = self.footer_hint.borrow().is_some();
        let footer_is_visible = footer_status_is_visible || footer_hint_is_visible;
        let correction_is_visible = overlay_correction_active;
        let meter_is_visible =
            state == STATE_RECORDING && self.ui_meter_view.style() != UiMeterStyle::None && !is_error;

        let inline_correction_preview = (state == STATE_TRANSFORMING
            && !overlay_correction_active
            && !overlay_correction_text_value.trim().is_empty())
        .then_some(overlay_correction_text_value);

        if is_error != self.is_error_color.get() {
            self.is_error_color.set(is_error);
            self.working_text_view
                .setTextColor(Some(&self.working_text_color()));
        }

        self.set_working_text(
            display_text,
            display_provisional_start,
            inline_correction_preview,
        );

        let is_error_correction = correction_is_visible
            && overlay_correction_text_value.starts_with("Transformation failed:");
        if correction_is_visible {
            self.set_correction_text(
                overlay_correction_text_value,
                overlay_correction_text.provisional_start,
                is_error_correction,
            );
        } else {
            self.set_correction_text("", None, false);
        }

        let target_alpha = if meter_is_visible && mic_meter.mic_active {
            1.0
        } else {
            0.0
        };

        let current_alpha = self.meter_alpha.get();
        if state != STATE_RECORDING || is_error {
            self.meter_alpha.set(0.0);
        } else if (current_alpha - target_alpha).abs() > 0.01 {
            let step = 0.375;
            let next_alpha = if current_alpha < target_alpha {
                (current_alpha + step).min(target_alpha)
            } else {
                (current_alpha - step).max(target_alpha)
            };
            self.meter_alpha.set(next_alpha);
        }

        self.layout_panels(
            mtm,
            correction_is_visible,
            footer_is_visible,
            footer_status_is_visible,
            footer_hint_is_visible,
            meter_is_visible,
        );
        if state == STATE_TRANSFORMING {
            self.shimmer.start(&self.working_scroll_view);
        } else {
            self.shimmer.stop(&self.working_scroll_view);
        }
        self.set_working_text_opacity(overlay_text_opacity);
        self.set_footer_status_indicator(deepgram_connection_status);
        self.set_footer_text(footer_status_text.unwrap_or(""));

        if meter_is_visible {
            self.ui_meter_view.update(mic_meter, meter_cluster_width());
        } else {
            self.ui_meter_view.clear(meter_cluster_width());
        }

        if !self.is_visible.get() {
            self.glass.pop_in(reduce_motion());
            self.glass.panel.makeKeyWindow();
            self.glass
                .panel
                .makeFirstResponder(Some(&self.working_text_view));
            self.is_visible.set(true);
        }

        let text_length = self.working_text_view.string().length();
        self.working_text_view
            .setSelectedRange(NSRange::new(text_length, 0));
        self.working_text_view
            .scrollRangeToVisible(NSRange::new(text_length, 0));

        if correction_is_visible {
            let correction_length = self.correction_text_view.string().length();
            self.correction_text_view
                .scrollRangeToVisible(NSRange::new(correction_length, 0));
        }

        self.state.set_overlay_window_visible(true);
    }

    pub fn hide(&self) {
        if self.is_visible.replace(false) {
            self.glass.pop_out(reduce_motion());
        }
        self.shimmer.stop(&self.working_scroll_view);
        self.state.set_overlay_window_visible(false);
        self.ui_meter_view.clear(meter_cluster_width());
        self.set_working_text("", None, None);
        self.set_correction_text("", None, false);
        self.set_working_text_opacity(1.0);
        self.set_footer_text("");
    }

    pub fn glass_tuning(&self) -> GlassTuning {
        self.glass.tuning()
    }

    pub fn halo_is_progressive(&self) -> bool {
        self.glass.halo_is_progressive()
    }

    /// The glass's inner parameters as drawn now; the overlay tuner starts
    /// its sliders from them.
    pub fn glass_internals_now(&self) -> Option<crate::overlay::private_effects::GlassInternals> {
        self.glass.glass_internals_now()
    }

    /// Keeps the overlay `distance` below the top of the screen instead of
    /// centred on it, or centres it again with `None`. The overlay tuner
    /// uses it to leave room for its controls below.
    pub fn pin_to_top(&self, distance: Option<f64>) {
        self.pinned_top.set(distance);
    }

    /// Applies `tuning` now; window geometry follows at the next `update`.
    pub fn set_glass_tuning(&self, tuning: GlassTuning) {
        self.glass.set_tuning(tuning);
    }

    pub fn text(&self) -> String {
        self.working_text_view.string().to_string()
    }

    pub fn set_delegate(&self, delegate: &ProtocolObject<dyn NSTextViewDelegate>) {
        self.working_text_view.setDelegate(Some(delegate));
    }

    pub fn apply_style(&self, style: &OverlayStyle) {
        let text_font = resolve_overlay_font(style, style.font_size);
        self.working_text_view.setFont(Some(&text_font));
        self.correction_text_view.setFont(Some(&text_font));
        self.text_font.replace(text_font);
        self.footer_text_field
            .setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        self.footer_hint_text_field
            .setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        self.ui_meter_view.set_style(style.meter_style);
        self.footer_hint.replace(style.shortcut_hint.clone());
        self.deepgram_api_key.set(style.deepgram_api_key);

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
            .setFrame(footer_status_indicator_frame(style.shortcut_hint.is_some()));

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
        footer_status_is_visible: bool,
        footer_hint_is_visible: bool,
        meter_is_visible: bool,
    ) {
        let visible_frame = self.selected_visible_frame(mtm);
        let meter_style = self.ui_meter_view.style();
        let reduce_motion = reduce_motion();
        let main_content_height = measured_text_height(&self.working_text_view);
        let correction_content_height = correction_is_visible
            .then(|| measured_text_height(&self.correction_text_view))
            .unwrap_or(CORRECTION_OVERLAY_MIN_HEIGHT);
        let pinned_top = self.pinned_top.get();
        let current_main_origin_y = (self.is_visible.get() && pinned_top.is_none())
            .then(|| self.glass.main_origin_y());
        let correction_height = correction_is_visible
            .then(|| correction_panel_height(correction_content_height, visible_frame.size.height));
        if let Some(height) = correction_height {
            self.last_correction_height.set(height);
        }
        // The window keeps room for the correction glass while it pops out.
        let (next_correction_phase, _) = plan_correction_motion(
            self.glass.correction_phase(),
            correction_is_visible,
            reduce_motion,
        );
        let correction_room_height = correction_height.or_else(|| {
            (next_correction_phase != CorrectionPhase::Hidden)
                .then(|| self.last_correction_height.get())
        });

        let visible_max_y = visible_frame.origin.y + visible_frame.size.height;
        let top_edge_limit = visible_max_y - 100.0;
        let current_correction_stack_height = correction_room_height
            .map(|height| height + glass::STACK_GAP)
            .unwrap_or(0.0);

        let max_main_height = if let Some(origin_y) = current_main_origin_y {
            (top_edge_limit - origin_y - current_correction_stack_height)
                .max(MAIN_OVERLAY_MIN_HEIGHT)
        } else {
            // When first showing, limit it to 60% of screen height
            (visible_frame.size.height * 0.6).max(MAIN_OVERLAY_MIN_HEIGHT)
        };

        let main_height = main_panel_height(
            main_content_height,
            footer_is_visible,
            meter_is_visible,
            meter_style,
            max_main_height,
        );
        let main_reserved_height =
            bottom_reserved_height(footer_is_visible, meter_is_visible, meter_style);
        let main_min_text_area_height = (MAIN_OVERLAY_MIN_HEIGHT - main_reserved_height).max(0.0);
        let main_desired_height =
            main_reserved_height + main_content_height.max(main_min_text_area_height);
        let main_is_clamped = main_height + 1.0 < main_desired_height;
        let correction_desired_height =
            correction_content_height.max(CORRECTION_OVERLAY_MIN_HEIGHT);
        let correction_is_clamped = correction_height
            .map(|height| height + 1.0 < correction_desired_height)
            .unwrap_or(false);

        // Pinned, the room above the main glass is kept for the correction
        // glass whether or not it shows, so showing it does not move the main
        // glass.
        let pinned_origin_y = pinned_top.map(|top| {
            let correction_room = correction_room_height
                .unwrap_or(0.0)
                .max(self.last_correction_height.get())
                + glass::STACK_GAP;
            visible_max_y - top - self.glass.tuning().margin() - main_height - correction_room
        });
        let (main_frame, correction_frame) = stacked_panel_frames(
            visible_frame,
            main_height,
            correction_room_height,
            pinned_origin_y.or(current_main_origin_y),
        );

        self.glass
            .set_frames(
                glass::window_frame(main_frame, correction_frame, self.glass.tuning().margin()),
                main_height,
            );
        self.separator_view.setHidden(!footer_is_visible);
        self.footer_status_indicator_view
            .setHidden(!footer_status_is_visible);
        self.footer_text_field.setHidden(!footer_status_is_visible);
        self.footer_hint_text_field
            .setHidden(!footer_hint_is_visible);
        self.working_scroll_view.setFrame(main_text_view_frame(
            footer_is_visible,
            meter_is_visible,
            meter_style,
            main_height,
        ));
        self.shimmer.track_bounds(&self.working_scroll_view);
        self.resize_text_view(
            &self.working_scroll_view,
            &self.working_text_view,
            main_text_area_height(
                footer_is_visible,
                meter_is_visible,
                meter_style,
                main_height,
            ),
            main_is_clamped,
        );
        self.separator_view.setFrame(separator_frame(main_height));
        self.ui_meter_view.set_frame(meter_container_frame(
            footer_is_visible,
            self.ui_meter_view.style(),
        ));
        let meter_alpha = self.meter_alpha.get();
        self.ui_meter_view.set_hidden(meter_alpha == 0.0);
        self.ui_meter_view.view().setAlphaValue(meter_alpha);

        // The correction text lives on its own glass, or, for `Expand`, in the
        // area at the top of the grown main glass.
        let expands = self.glass.tuning().correction_effect == CorrectionEffect::Expand;
        let host: &NSView = if expands {
            &self.glass.correction_expansion_view
        } else {
            &self.glass.correction_content_view
        };
        // SAFETY: reading the superview of a live view on the main thread.
        let hosted = unsafe { self.correction_scroll_view.superview() }
            .is_some_and(|superview| std::ptr::eq(&*superview, host));
        if !hosted {
            host.addSubview(&self.correction_scroll_view);
            // It keeps its place when its host is resized: above the divider
            // at the bottom of the expansion area, or at the top of the
            // correction glass.
            let vertical = if expands {
                NSAutoresizingMaskOptions::ViewMaxYMargin
            } else {
                NSAutoresizingMaskOptions::ViewMinYMargin
            };
            self.correction_scroll_view
                .setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | vertical);
        }
        if let Some(height) = correction_height {
            let frame = if expands {
                NSRect::new(NSPoint::new(0.0, glass::STACK_GAP), NSSize::new(OVERLAY_WIDTH, height))
            } else {
                correction_text_view_frame(height)
            };
            self.correction_scroll_view.setFrame(frame);
            self.resize_text_view(
                &self.correction_scroll_view,
                &self.correction_text_view,
                height,
                correction_is_clamped,
            );
        }
        let margin = self.glass.tuning().margin();
        self.glass.update_correction(
            correction_height
                .map(|height| correction_resting_frame(main_height, OVERLAY_WIDTH, height, margin)),
            reduce_motion,
        );
    }

    fn selected_visible_frame(&self, mtm: MainThreadMarker) -> NSRect {
        let screens = NSScreen::screens(mtm);
        if self.is_visible.get() {
            let current_frame = self.glass.panel.frame();
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

    fn working_text_color(&self) -> Retained<NSColor> {
        if self.is_error_color.get() {
            NSColor::systemRedColor()
        } else {
            NSColor::labelColor()
        }
    }

    fn set_working_text(
        &self,
        text: &str,
        provisional_start: Option<usize>,
        inline_correction_preview: Option<&str>,
    ) {
        if let Some(preview_text) = inline_correction_preview {
            self.set_working_text_with_preview(text, preview_text);
            return;
        }

        let current_text = self.working_text_view.string().to_string();
        if working_text_update_is_semantically_unchanged(&current_text, text) {
            // Interim words that became final settle in place.
            if self.working_provisional_start.replace(provisional_start) != provisional_start {
                crossfade_next_change(&self.working_scroll_view);
                restyle_provisional(
                    &self.working_text_view,
                    &self.working_text_color(),
                    provisional_utf16_range(text, provisional_start)
                        .map(|(location, length)| NSRange::new(location, length)),
                );
            }
            return;
        }

        let attributes = text_attributes(&self.text_font.borrow(), &self.working_text_color());
        crossfade_next_change(&self.working_scroll_view);
        self.replace_text(
            &self.working_text_view,
            &attributed_text(text, provisional_start, &attributes),
        );
        self.working_provisional_start.set(provisional_start);

        // Move cursor to the end
        let length = text.encode_utf16().count();
        self.working_text_view
            .setSelectedRange(NSRange::new(length, 0));
        // SAFETY: `attributes` maps attribute keys to values of their
        // documented types (an `NSFont` and an `NSColor`).
        unsafe { self.working_text_view.setTypingAttributes(&attributes) };
        self.working_text_view
            .scrollRangeToVisible(NSRange::new(length, 0));
    }

    fn set_working_text_with_preview(&self, original_text: &str, preview_text: &str) {
        let Some(rendered_preview) = build_inline_correction_preview(original_text, preview_text)
        else {
            self.set_working_text(original_text, None, None);
            return;
        };

        let attributes = text_attributes(&self.text_font.borrow(), &self.working_text_color());
        let attributed_text = attributed_text(&rendered_preview.text, None, &attributes);
        let underline_style = NSNumber::new_isize(NSUnderlineStyle::Single.bits() as isize);
        let underline_color = NSColor::secondaryLabelColor();

        for range in &rendered_preview.underlined_byte_ranges {
            let location = utf16_offset(&rendered_preview.text, range.start);
            let length = rendered_preview.text[range.start..range.end]
                .encode_utf16()
                .count();
            if length == 0 {
                continue;
            }

            unsafe {
                attributed_text.addAttribute_value_range(
                    NSUnderlineStyleAttributeName,
                    underline_style.as_ref(),
                    NSRange::new(location, length),
                );
                attributed_text.addAttribute_value_range(
                    NSUnderlineColorAttributeName,
                    underline_color.as_ref(),
                    NSRange::new(location, length),
                );
            }
        }

        crossfade_next_change(&self.working_scroll_view);
        self.replace_text(&self.working_text_view, &attributed_text);
        self.working_provisional_start.set(None);

        let length = rendered_preview.text.encode_utf16().count();
        self.working_text_view
            .setSelectedRange(NSRange::new(length, 0));
        // SAFETY: `attributes` maps attribute keys to values of their
        // documented types (an `NSFont` and an `NSColor`).
        unsafe { self.working_text_view.setTypingAttributes(&attributes) };
        self.working_text_view
            .scrollRangeToVisible(NSRange::new(length, 0));
    }

    fn set_correction_text(&self, text: &str, provisional_start: Option<usize>, failed: bool) {
        let current_text = self.correction_text_view.string().to_string();
        if current_text == text
            && self.correction_provisional_start.get() == provisional_start
            && self.correction_failed.get() == failed
        {
            return;
        }
        self.correction_provisional_start.set(provisional_start);
        self.correction_failed.set(failed);

        let color = if failed {
            NSColor::systemRedColor()
        } else {
            NSColor::labelColor()
        };
        let attributes = text_attributes(&self.text_font.borrow(), &color);
        crossfade_next_change(&self.correction_scroll_view);
        self.replace_text(
            &self.correction_text_view,
            &attributed_text(text, provisional_start, &attributes),
        );
        let length = text.encode_utf16().count();
        self.correction_text_view
            .scrollRangeToVisible(NSRange::new(length, 0));
    }

    fn replace_text(&self, text_view: &NSTextView, text: &NSMutableAttributedString) {
        // SAFETY: the storage belongs to `text_view`, which is only used on
        // the main thread; edits are bracketed by begin/end editing.
        if let Some(text_storage) = unsafe { text_view.textStorage() } {
            text_storage.beginEditing();
            text_storage.setAttributedString(text);
            text_storage.endEditing();
        } else {
            text_view.setString(&text.string());
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
        // Text that fits has nothing to scroll; a clip view left scrolled from
        // an earlier, shorter document would show it shifted.
        if document_height <= visible_height {
            let clip_view = scroll_view.contentView();
            if clip_view.bounds().origin != NSPoint::new(0.0, 0.0) {
                clip_view.scrollToPoint(NSPoint::new(0.0, 0.0));
                scroll_view.reflectScrolledClipView(&clip_view);
            }
        }
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

/// Makes `text_view` the document of `scroll_view`, whose text then keeps its
/// place while the glass's corners pass it (see
/// `private_effects::disallow_corner_content_insets`). A text view turns the
/// corner insets back on when it moves into a scroll view (macOS 26.6), so
/// they are turned off after.
fn set_text_document(scroll_view: &NSScrollView, text_view: &NSTextView) {
    scroll_view.setDocumentView(Some(text_view));
    if !private_effects::disallow_corner_content_insets(scroll_view) {
        log::warn!("overlay text may shift while the glass's corners pass it: the scroll view corner inset switch is unavailable");
    }
}

fn configure_scroll_view(scroll_view: &NSScrollView) {
    scroll_view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    // The crossfade and shimmer animate the scroll view's layer.
    scroll_view.setWantsLayer(true);
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
    text_view.setTextColor(Some(&NSColor::labelColor()));
    text_view.setFont(Some(&resolve_overlay_font(style, style.font_size)));
    text_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    // `resize_text_view` sets its height on every update. Left to size itself
    // to its text as well, it briefly shrinks between updates and the text
    // jumps.
    text_view.setVerticallyResizable(false);
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
    let Some(text_container): Option<Retained<AnyObject>> =
        (unsafe { msg_send![text_view, textContainer] })
    else {
        return TEXT_VERTICAL_PADDING * 2.0;
    };
    let Some(layout_manager): Option<Retained<AnyObject>> =
        (unsafe { msg_send![text_view, layoutManager] })
    else {
        return TEXT_VERTICAL_PADDING * 2.0;
    };

    let line_fragment_padding: f64 = unsafe { msg_send![&*text_container, lineFragmentPadding] };
    let container_width = (usable_text_width() - (line_fragment_padding * 2.0)).max(1.0);
    unsafe {
        let _: () = msg_send![&*text_container, setContainerSize: NSSize::new(container_width, TEXT_LAYOUT_MEASUREMENT_HEIGHT)];
        let _: NSRange = msg_send![&*layout_manager, glyphRangeForTextContainer: &*text_container];
        let used_rect: NSRect =
            msg_send![&*layout_manager, usedRectForTextContainer: &*text_container];
        (TEXT_VERTICAL_PADDING * 2.0) + used_rect.size.height.ceil().max(1.0)
    }
}

fn main_panel_height(
    text_height: f64,
    footer_is_visible: bool,
    meter_is_visible: bool,
    meter_style: UiMeterStyle,
    max_allowed_height: f64,
) -> f64 {
    let reserved_height = bottom_reserved_height(footer_is_visible, meter_is_visible, meter_style);
    let min_text_area_height = (MAIN_OVERLAY_MIN_HEIGHT - reserved_height).max(0.0);
    let desired_text_area_height = text_height.max(min_text_area_height);
    let max_height = max_allowed_height.max(MAIN_OVERLAY_MIN_HEIGHT);

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
    let centered_origin_x =
        visible_frame.origin.x + ((visible_frame.size.width - OVERLAY_WIDTH) / 2.0);
    let stacked_height = main_height
        + correction_height
            .map(|height| height + glass::STACK_GAP)
            .unwrap_or(0.0);

    let mut main_origin_y = current_main_origin_y.unwrap_or_else(|| {
        visible_frame.origin.y + ((visible_frame.size.height - stacked_height) / 2.0)
    });
    main_origin_y = main_origin_y.max(visible_frame.origin.y);
    main_origin_y = main_origin_y.min(visible_max_y - main_height);

    let mut correction_origin_y =
        correction_height.map(|_| main_origin_y + main_height + glass::STACK_GAP);
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
    let correction_frame = correction_height
        .zip(correction_origin_y)
        .map(|(height, origin_y)| {
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
            main_text_area_height(
                footer_is_visible,
                meter_is_visible,
                meter_style,
                panel_height,
            ),
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
    NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(OVERLAY_WIDTH, panel_height),
    )
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
    let footer_text_origin_x = TEXT_HORIZONTAL_PADDING
        + FOOTER_STATUS_DOT_DIAMETER
        + FOOTER_STATUS_DOT_GAP
        + FOOTER_TEXT_X_OFFSET;

    NSRect::new(
        NSPoint::new(
            footer_text_origin_x,
            footer_bottom_line_origin_y(has_footer_hint) + 1.0,
        ),
        NSSize::new(
            (OVERLAY_WIDTH - footer_text_origin_x - TEXT_HORIZONTAL_PADDING).max(0.0),
            FOOTER_LINE_HEIGHT,
        ),
    )
}

fn footer_status_indicator_frame(has_footer_hint: bool) -> NSRect {
    NSRect::new(
        NSPoint::new(
            footer_status_indicator_origin_x(),
            footer_bottom_line_origin_y(has_footer_hint)
                + ((FOOTER_LINE_HEIGHT - FOOTER_STATUS_DOT_DIAMETER) / 2.0),
        ),
        NSSize::new(FOOTER_STATUS_DOT_DIAMETER, FOOTER_STATUS_DOT_DIAMETER),
    )
}

fn footer_hint_frame() -> NSRect {
    NSRect::new(
        NSPoint::new(
            footer_status_indicator_origin_x(),
            footer_top_line_origin_y(),
        ),
        NSSize::new(
            OVERLAY_WIDTH - footer_status_indicator_origin_x() - TEXT_HORIZONTAL_PADDING,
            FOOTER_LINE_HEIGHT,
        ),
    )
}

fn footer_status_indicator_origin_x() -> f64 {
    TEXT_HORIZONTAL_PADDING + FOOTER_STATUS_DOT_X_OFFSET
}

fn footer_top_line_origin_y() -> f64 {
    FOOTER_HEIGHT - FOOTER_VERTICAL_PADDING - FOOTER_LINE_HEIGHT
}

fn footer_bottom_line_origin_y(has_footer_hint: bool) -> f64 {
    if has_footer_hint {
        FOOTER_VERTICAL_PADDING
    } else {
        (FOOTER_HEIGHT - FOOTER_LINE_HEIGHT) / 2.0
    }
}

fn usable_text_width() -> f64 {
    OVERLAY_WIDTH - (TEXT_HORIZONTAL_PADDING * 2.0)
}

/// Decides the footer status line: the Deepgram connection dot and the text
/// beside it. `None` hides both. The dot shows whenever a Deepgram API key
/// resolves; the text is the billing text `BillingController` publishes when a
/// project ID is configured, and the connection label when it is empty.
fn footer_status_text(
    billing_text: &str,
    deepgram_api_key_configured: bool,
    status: DeepgramConnectionStatus,
) -> Option<&str> {
    if !deepgram_api_key_configured {
        return None;
    }

    if billing_text.trim().is_empty() {
        Some(footer_connection_label(status))
    } else {
        Some(billing_text)
    }
}

/// Text beside the connection dot when there is no billing text to show (no
/// Deepgram project ID configured). It follows the billing text's
/// "Deepgram ...: <state>" form and states what the dot's color shows.
pub fn footer_connection_label(status: DeepgramConnectionStatus) -> &'static str {
    match status {
        DeepgramConnectionStatus::Connected => "Deepgram: connected",
        DeepgramConnectionStatus::Disconnected => "Deepgram: disconnected",
        DeepgramConnectionStatus::Unknown => "Deepgram: ...",
    }
}

fn footer_connection_status_color(status: DeepgramConnectionStatus) -> Retained<NSColor> {
    let (red, green, blue) = footer_connection_status_srgb(status);
    NSColor::colorWithSRGBRed_green_blue_alpha(red, green, blue, 1.0)
}

fn footer_text_color() -> Retained<NSColor> {
    let (red, green, blue) = FOOTER_TEXT_SRGB;
    NSColor::colorWithSRGBRed_green_blue_alpha(red, green, blue, 1.0)
}

/// Unknown (no connection attempt has finished yet) uses the footer text's
/// gray, so the dot does not claim a failure before one happened.
fn footer_connection_status_srgb(status: DeepgramConnectionStatus) -> (f64, f64, f64) {
    match status {
        DeepgramConnectionStatus::Connected => (0.26, 0.86, 0.54),
        DeepgramConnectionStatus::Disconnected => (0.95, 0.28, 0.24),
        DeepgramConnectionStatus::Unknown => FOOTER_TEXT_SRGB,
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

#[cfg(test)]
mod tests {
    use super::{footer_connection_label, footer_connection_status_srgb, footer_status_text};
    use crate::state::DeepgramConnectionStatus;

    const ALL_STATUSES: [DeepgramConnectionStatus; 3] = [
        DeepgramConnectionStatus::Unknown,
        DeepgramConnectionStatus::Disconnected,
        DeepgramConnectionStatus::Connected,
    ];

    #[test]
    fn footer_shows_connection_label_when_key_resolves_without_project_id() {
        for status in ALL_STATUSES {
            assert_eq!(
                footer_status_text("", true, status),
                Some(footer_connection_label(status))
            );
            assert_eq!(
                footer_status_text("  ", true, status),
                Some(footer_connection_label(status))
            );
        }
    }

    #[test]
    fn footer_connection_label_states_the_connection_status() {
        assert_eq!(
            footer_connection_label(DeepgramConnectionStatus::Connected),
            "Deepgram: connected"
        );
        assert_eq!(
            footer_connection_label(DeepgramConnectionStatus::Disconnected),
            "Deepgram: disconnected"
        );
        assert_eq!(
            footer_connection_label(DeepgramConnectionStatus::Unknown),
            "Deepgram: ..."
        );
    }

    #[test]
    fn footer_passes_billing_text_through_unchanged_when_project_id_is_configured() {
        for billing_text in [
            "Deepgram (Apr 2026): ...",
            "Deepgram (Apr 2026): $12.34",
            "Deepgram (Apr 2026): unavailable",
            "Admin- or owner-level project API key required for billing reporting.",
        ] {
            for status in ALL_STATUSES {
                assert_eq!(
                    footer_status_text(billing_text, true, status),
                    Some(billing_text)
                );
            }
        }
    }

    #[test]
    fn footer_hides_dot_and_text_without_an_api_key() {
        for billing_text in ["", "Deepgram (Apr 2026): $12.34"] {
            for status in ALL_STATUSES {
                assert_eq!(footer_status_text(billing_text, false, status), None);
            }
        }
    }

    #[test]
    fn unknown_connection_status_is_drawn_apart_from_disconnected() {
        assert_ne!(
            footer_connection_status_srgb(DeepgramConnectionStatus::Unknown),
            footer_connection_status_srgb(DeepgramConnectionStatus::Disconnected)
        );
        assert_ne!(
            footer_connection_status_srgb(DeepgramConnectionStatus::Unknown),
            footer_connection_status_srgb(DeepgramConnectionStatus::Connected)
        );
    }
}
