use std::cell::{Cell, RefCell};
use std::time::Instant;

use objc2::rc::Retained;
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSColor, NSEvent, NSFloatingWindowLevel,
    NSLineBreakMode, NSPanel, NSScreen, NSScrollView, NSTextAlignment, NSTextField, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use crate::config::UiMeterStyle;
use crate::state::{
    MicMeterSnapshot, STATE_BUFFER_READY, STATE_PROCESSING, STATE_RECORDING, STATE_TRANSFORMING,
};

const OVERLAY_HEIGHT: f64 = 180.0;
const OVERLAY_WIDTH: f64 = 560.0;
const DEFAULT_TEXT_FONT_SIZE: f64 = 12.0;
const FOOTER_HEIGHT: f64 = 24.0;
const FOOTER_HINT_GAP: f64 = 12.0;
const FOOTER_HINT_WIDTH: f64 = 260.0;
const CLIP_INDICATOR_BORDER_WIDTH: f64 = 1.0;
const CLIP_INDICATOR_CORNER_RADIUS: f64 = 4.0;
const CLIP_INDICATOR_FADE_IN_SECONDS: f64 = 0.08;
const CLIP_INDICATOR_FADE_OUT_SECONDS: f64 = CLIP_INDICATOR_FADE_IN_SECONDS * 2.0;
const CLIP_INDICATOR_HOLD_SECONDS: f64 = 0.20;
const METER_BORDER_PADDING: f64 = 3.0;
const METER_BAR_COUNT: usize = 20;
const METER_BAR_SPACING: f64 = 3.0;
const METER_CLUSTER_MAX_WIDTH: f64 = 260.0;
const METER_CLUSTER_MIN_WIDTH: f64 = 180.0;
const METER_CLUSTER_WIDTH_FACTOR: f64 = 0.48;
const METER_COLOR_ONLY_BAR_HEIGHT: f64 = 4.0;
const METER_MIN_BAR_HEIGHT: f64 = 0.0;
const METER_SECTION_BOTTOM_PADDING: f64 = 5.0;
const METER_SECTION_HEIGHT: f64 = 30.8;
const METER_VIEW_HEIGHT: f64 = 19.6;
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
    pub transformation_hint: Option<String>,
}

#[derive(Debug, Default)]
struct ClipIndicatorState {
    alpha: f64,
    hold_remaining_seconds: f64,
    last_clip_event_counter: u32,
    last_updated_at: Option<Instant>,
}

#[derive(Debug)]
pub struct OverlayWindow {
    panel: Retained<NSPanel>,
    scroll_view: Retained<NSScrollView>,
    separator_view: Retained<NSView>,
    meter_container_view: Retained<NSView>,
    meter_bar_views: Vec<Retained<NSView>>,
    meter_bar_levels: RefCell<Vec<f32>>,
    clip_indicator_state: RefCell<ClipIndicatorState>,
    meter_style: Cell<UiMeterStyle>,
    text_field: Retained<NSTextField>,
    footer_text_field: Retained<NSTextField>,
    footer_hint_text_field: Retained<NSTextField>,
    footer_hint: RefCell<Option<String>>,
    is_visible: Cell<bool>,
    text_opacity: Cell<f64>,
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
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), scroll_view_frame(true, false));
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
                text_area_height(true, false) - (TEXT_VERTICAL_PADDING * 2.0),
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

        let meter_container_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            meter_container_frame(true, style.meter_style),
        );
        meter_container_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        meter_container_view.setHidden(true);
        meter_container_view.setWantsLayer(true);
        if let Some(layer) = meter_container_view.layer() {
            let border_color = clip_indicator_border_color(0.0);
            let border_cg_color = border_color.CGColor();
            layer.setBorderWidth(CLIP_INDICATOR_BORDER_WIDTH);
            layer.setBorderColor(Some(&border_cg_color));
            layer.setCornerRadius(CLIP_INDICATOR_CORNER_RADIUS);
        }

        let mut meter_bar_views = Vec::with_capacity(METER_BAR_COUNT);
        for _ in 0..METER_BAR_COUNT {
            let bar_view = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(0.0, METER_MIN_BAR_HEIGHT),
                ),
            );
            bar_view.setWantsLayer(true);
            if let Some(layer) = bar_view.layer() {
                let bar_color = inactive_meter_bar_color();
                let bar_cg_color = bar_color.CGColor();
                layer.setBackgroundColor(Some(&bar_cg_color));
                layer.setCornerRadius(METER_MIN_BAR_HEIGHT);
                layer.setMasksToBounds(true);
            }
            meter_container_view.addSubview(&bar_view);
            meter_bar_views.push(bar_view);
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
        footer_text_field.setFrame(footer_text_frame(style.transformation_hint.is_some()));
        footer_text_field.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        if let Some(cell) = footer_text_field.cell() {
            cell.setAlignment(NSTextAlignment::Left);
            cell.setLineBreakMode(NSLineBreakMode::ByClipping);
            cell.setUsesSingleLineMode(true);
        }

        let footer_hint_text_field = NSTextField::labelWithString(&NSString::from_str(""), mtm);
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
        if let Some(transformation_hint) = style.transformation_hint.as_deref() {
            footer_hint_text_field.setStringValue(&NSString::from_str(transformation_hint));
        }
        footer_hint_text_field.setHidden(style.transformation_hint.is_none());

        scroll_view.setDocumentView(Some(&text_field));
        root_view.addSubview(&scroll_view);
        root_view.addSubview(&meter_container_view);
        root_view.addSubview(&separator_view);
        root_view.addSubview(&footer_text_field);
        root_view.addSubview(&footer_hint_text_field);
        panel.setContentView(Some(&root_view));
        panel.orderOut(None);

        let overlay_window = Self {
            panel,
            scroll_view,
            separator_view,
            meter_container_view,
            meter_bar_views,
            meter_bar_levels: RefCell::new(vec![0.0; METER_BAR_COUNT]),
            clip_indicator_state: RefCell::new(ClipIndicatorState::default()),
            meter_style: Cell::new(style.meter_style),
            text_field,
            footer_text_field,
            footer_hint_text_field,
            footer_hint: RefCell::new(style.transformation_hint.clone()),
            is_visible: Cell::new(false),
            text_opacity: Cell::new(1.0),
        };
        overlay_window.render_meter_bars();
        overlay_window.render_clip_indicator();
        overlay_window
    }

    pub fn update(
        &self,
        mtm: MainThreadMarker,
        state: u8,
        overlay_text: &str,
        overlay_text_opacity: f64,
        overlay_footer_text: &str,
        mic_meter: MicMeterSnapshot,
    ) {
        let should_show = matches!(
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
            state == STATE_RECORDING && self.meter_style.get() != UiMeterStyle::None;
        self.update_layout(
            footer_is_visible,
            footer_text_is_visible,
            footer_hint_is_visible,
            meter_is_visible,
        );
        self.set_text(display_text);
        self.set_text_opacity(overlay_text_opacity);
        self.set_footer_text(overlay_footer_text);

        if meter_is_visible {
            self.update_meter(mic_meter);
            self.update_clip_indicator(mic_meter);
        } else {
            self.clear_meter();
            self.clear_clip_indicator();
        }

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
        self.clear_meter();
        self.clear_clip_indicator();
        self.set_text("");
        self.set_text_opacity(1.0);
        self.set_footer_text("");
    }

    pub fn apply_style(&self, style: &OverlayStyle) {
        self.text_field
            .setFont(Some(&resolve_overlay_font(style, style.font_size)));
        self.footer_text_field
            .setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        self.footer_hint_text_field
            .setFont(Some(&resolve_overlay_font(style, style.footer_font_size)));
        self.meter_style.set(style.meter_style);
        self.footer_hint.replace(style.transformation_hint.clone());

        if let Some(transformation_hint) = style.transformation_hint.as_deref() {
            self.footer_hint_text_field
                .setStringValue(&NSString::from_str(transformation_hint));
            self.footer_hint_text_field.setHidden(false);
        } else {
            self.footer_hint_text_field
                .setStringValue(&NSString::from_str(""));
            self.footer_hint_text_field.setHidden(true);
        }
        self.footer_text_field
            .setFrame(footer_text_frame(style.transformation_hint.is_some()));

        let footer_text_is_visible = !self
            .footer_text_field
            .stringValue()
            .to_string()
            .trim()
            .is_empty();
        let footer_hint_is_visible = style.transformation_hint.is_some();
        let footer_is_visible = footer_text_is_visible || footer_hint_is_visible;
        let meter_is_visible =
            self.is_visible.get() && self.meter_style.get() != UiMeterStyle::None;
        self.update_layout(
            footer_is_visible,
            footer_text_is_visible,
            footer_hint_is_visible,
            meter_is_visible,
        );
        if !meter_is_visible {
            self.clear_meter();
            self.clear_clip_indicator();
        }
        NSView::setNeedsDisplay(&self.text_field, true);
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

        self.text_field.setAlphaValue(next_text_opacity);
        self.text_opacity.set(next_text_opacity);
        NSView::setNeedsDisplay(&self.text_field, true);
    }

    fn set_footer_text(&self, footer_text: &str) {
        self.footer_text_field
            .setStringValue(&NSString::from_str(footer_text));
        NSView::setNeedsDisplay(&self.footer_text_field, true);
    }

    fn update_layout(
        &self,
        footer_is_visible: bool,
        footer_text_is_visible: bool,
        footer_hint_is_visible: bool,
        meter_is_visible: bool,
    ) {
        self.separator_view.setHidden(!footer_is_visible);
        self.footer_text_field.setHidden(!footer_text_is_visible);
        self.footer_hint_text_field
            .setHidden(!footer_hint_is_visible);
        self.scroll_view
            .setFrame(scroll_view_frame(footer_is_visible, meter_is_visible));
        self.meter_container_view.setFrame(meter_container_frame(
            footer_is_visible,
            self.meter_style.get(),
        ));
        self.meter_container_view.setHidden(!meter_is_visible);
    }

    fn update_meter(&self, mic_meter: MicMeterSnapshot) {
        self.meter_container_view.setHidden(false);

        let level = mic_meter.level as f32 / u8::MAX as f32;
        let peak = mic_meter.peak as f32 / u8::MAX as f32;

        match self.meter_style.get() {
            UiMeterStyle::None => {}
            UiMeterStyle::AnimatedHeight => self.update_meter_animated_height(level, peak),
            UiMeterStyle::AnimatedColor => self.update_meter_animated_color(level, peak),
        }

        self.render_meter_bars();
    }

    fn clear_meter(&self) {
        self.meter_container_view.setHidden(true);
        let mut meter_bar_levels = self.meter_bar_levels.borrow_mut();
        meter_bar_levels.fill(0.0);
        drop(meter_bar_levels);
        self.render_meter_bars();
    }

    fn update_clip_indicator(&self, mic_meter: MicMeterSnapshot) {
        let now = Instant::now();
        let mut clip_indicator_state = self.clip_indicator_state.borrow_mut();
        let delta_seconds = clip_indicator_state
            .last_updated_at
            .map(|last_updated_at| now.duration_since(last_updated_at).as_secs_f64())
            .unwrap_or(CLIP_INDICATOR_FADE_IN_SECONDS);
        clip_indicator_state.last_updated_at = Some(now);

        let clip_detected =
            mic_meter.clip_event_counter != clip_indicator_state.last_clip_event_counter;
        clip_indicator_state.last_clip_event_counter = mic_meter.clip_event_counter;

        if clip_detected {
            clip_indicator_state.hold_remaining_seconds = CLIP_INDICATOR_HOLD_SECONDS;
        }

        if clip_detected
            || (clip_indicator_state.hold_remaining_seconds > 0.0
                && clip_indicator_state.alpha < 0.995)
        {
            clip_indicator_state.alpha = animate_towards(
                clip_indicator_state.alpha,
                1.0,
                CLIP_INDICATOR_FADE_IN_SECONDS,
                delta_seconds,
            );
            if clip_indicator_state.alpha >= 0.995 {
                clip_indicator_state.alpha = 1.0;
            }
        } else if clip_indicator_state.hold_remaining_seconds > 0.0 {
            clip_indicator_state.hold_remaining_seconds =
                (clip_indicator_state.hold_remaining_seconds - delta_seconds).max(0.0);
            clip_indicator_state.alpha = 1.0;
        } else {
            clip_indicator_state.alpha = animate_towards(
                clip_indicator_state.alpha,
                0.0,
                CLIP_INDICATOR_FADE_OUT_SECONDS,
                delta_seconds,
            );
            if clip_indicator_state.alpha <= 0.001 {
                clip_indicator_state.alpha = 0.0;
            }
        }
        drop(clip_indicator_state);

        self.render_clip_indicator();
    }

    fn clear_clip_indicator(&self) {
        let mut clip_indicator_state = self.clip_indicator_state.borrow_mut();
        clip_indicator_state.alpha = 0.0;
        clip_indicator_state.hold_remaining_seconds = 0.0;
        clip_indicator_state.last_updated_at = None;
        drop(clip_indicator_state);

        self.render_clip_indicator();
    }

    fn render_clip_indicator(&self) {
        let clip_indicator_alpha = self.clip_indicator_state.borrow().alpha;
        if let Some(layer) = self.meter_container_view.layer() {
            let border_color = clip_indicator_border_color(clip_indicator_alpha);
            let border_cg_color = border_color.CGColor();
            layer.setBorderColor(Some(&border_cg_color));
        }

        NSView::setNeedsDisplay(&self.meter_container_view, true);
    }

    fn render_meter_bars(&self) {
        match self.meter_style.get() {
            UiMeterStyle::None => {}
            UiMeterStyle::AnimatedHeight => self.render_meter_bars_animated_height(),
            UiMeterStyle::AnimatedColor => self.render_meter_bars_animated_color(),
        }

        NSView::setNeedsDisplay(&self.meter_container_view, true);
    }

    fn update_meter_animated_height(&self, level: f32, peak: f32) {
        let mut meter_bar_levels = self.meter_bar_levels.borrow_mut();

        for (index, current_level) in meter_bar_levels.iter_mut().enumerate() {
            let target_level = animated_height_target_level(index, level, peak);
            let smoothing = if target_level >= *current_level {
                animated_height_attack(index)
            } else {
                animated_height_release(index)
            };
            *current_level += (target_level - *current_level) * smoothing;
        }
    }

    fn update_meter_animated_color(&self, level: f32, peak: f32) {
        let mut meter_bar_levels = self.meter_bar_levels.borrow_mut();

        for (index, current_level) in meter_bar_levels.iter_mut().enumerate() {
            let target_level = animated_color_target_level(index, level, peak);
            let smoothing = if target_level >= *current_level {
                animated_color_attack(index)
            } else {
                animated_color_release(index)
            };
            *current_level += (target_level - *current_level) * smoothing;
        }
    }

    fn render_meter_bars_animated_height(&self) {
        let meter_bar_levels = self.meter_bar_levels.borrow();
        let cluster_width = meter_cluster_width();
        let total_spacing = METER_BAR_SPACING * (METER_BAR_COUNT.saturating_sub(1)) as f64;
        let bar_width = ((cluster_width - total_spacing) / METER_BAR_COUNT as f64).max(1.0);
        let cluster_origin_x = METER_BORDER_PADDING;

        for (index, meter_bar_view) in self.meter_bar_views.iter().enumerate() {
            let meter_value = meter_bar_levels[index];
            let bar_height = meter_bar_height(meter_value);
            let x = cluster_origin_x + ((bar_width + METER_BAR_SPACING) * index as f64);
            meter_bar_view.setFrame(NSRect::new(
                NSPoint::new(x, METER_BORDER_PADDING),
                NSSize::new(bar_width, bar_height),
            ));

            if let Some(layer) = meter_bar_view.layer() {
                let bar_color = animated_height_bar_color(meter_value);
                let bar_cg_color = bar_color.CGColor();
                layer.setBackgroundColor(Some(&bar_cg_color));
                layer.setCornerRadius((bar_width.min(bar_height) / 2.0).min(3.0));
            }
        }
    }

    fn render_meter_bars_animated_color(&self) {
        let meter_bar_levels = self.meter_bar_levels.borrow();
        let cluster_width = meter_cluster_width();
        let total_spacing = METER_BAR_SPACING * (METER_BAR_COUNT.saturating_sub(1)) as f64;
        let bar_width = ((cluster_width - total_spacing) / METER_BAR_COUNT as f64).max(1.0);
        let cluster_origin_x = METER_BORDER_PADDING;
        let y = METER_BORDER_PADDING;

        for (index, meter_bar_view) in self.meter_bar_views.iter().enumerate() {
            let meter_value = meter_bar_levels[index];
            let x = cluster_origin_x + ((bar_width + METER_BAR_SPACING) * index as f64);
            meter_bar_view.setFrame(NSRect::new(
                NSPoint::new(x, y),
                NSSize::new(bar_width, METER_COLOR_ONLY_BAR_HEIGHT),
            ));

            if let Some(layer) = meter_bar_view.layer() {
                let bar_color = animated_color_bar_color(index, meter_value);
                let bar_cg_color = bar_color.CGColor();
                layer.setBackgroundColor(Some(&bar_cg_color));
                layer.setCornerRadius((bar_width.min(METER_COLOR_ONLY_BAR_HEIGHT) / 2.0).min(3.0));
            }
        }
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
        STATE_BUFFER_READY => "Ready to paste…",
        STATE_TRANSFORMING => "Transforming…",
        _ => "",
    }
}

fn animated_height_target_level(index: usize, level: f32, peak: f32) -> f32 {
    let center_distance = meter_center_distance(index);
    let edge_attenuation = 1.0 - (center_distance * 0.68);
    let sustained_energy = level.powf(0.88) * edge_attenuation;
    let transient_energy = peak.powf(0.72) * (0.14 + (edge_attenuation * 0.18));
    (sustained_energy + transient_energy).clamp(0.0, 1.0)
}

fn animated_height_attack(index: usize) -> f32 {
    let center_distance = meter_center_distance(index);
    (0.44 - (center_distance * 0.07)).clamp(0.24, 0.44)
}

fn animated_height_release(index: usize) -> f32 {
    let center_distance = meter_center_distance(index);
    (0.30 - (center_distance * 0.045)).clamp(0.15, 0.30)
}

fn animated_color_target_level(index: usize, level: f32, peak: f32) -> f32 {
    let gain_level = ((level.powf(0.9) * 0.82) + (peak.powf(0.78) * 0.18)).clamp(0.0, 1.0);
    if gain_level <= 0.02 {
        return 0.0;
    }

    let bar_position = animated_color_bar_position(index);
    let fade_start = (gain_level - 0.08).clamp(0.0, 1.0);
    let fade_end = (gain_level + 0.08).clamp(0.0, 1.0);

    if bar_position <= fade_start {
        return 1.0;
    }

    if bar_position >= fade_end {
        return 0.0;
    }

    1.0 - smoothstep(fade_start, fade_end, bar_position)
}

fn animated_color_attack(_index: usize) -> f32 {
    0.52
}

fn animated_color_release(_index: usize) -> f32 {
    0.68
}

fn meter_center_distance(index: usize) -> f32 {
    let center = (METER_BAR_COUNT as f32 - 1.0) / 2.0;
    ((index as f32 - center).abs() / center).clamp(0.0, 1.0)
}

fn inactive_meter_bar_color() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.12)
}

fn clip_indicator_border_color(alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(0.95, 0.28, 0.24, alpha.clamp(0.0, 1.0))
}

fn animated_height_bar_color(meter_value: f32) -> Retained<NSColor> {
    if meter_value >= 0.92 {
        return NSColor::colorWithSRGBRed_green_blue_alpha(0.95, 0.28, 0.24, 1.0);
    }

    if meter_value >= 0.72 {
        return NSColor::colorWithSRGBRed_green_blue_alpha(0.82, 0.50, 0.08, 0.98);
    }

    if meter_value >= 0.18 {
        return NSColor::colorWithSRGBRed_green_blue_alpha(0.26, 0.86, 0.54, 0.95);
    }

    if meter_value > 0.04 {
        return NSColor::colorWithSRGBRed_green_blue_alpha(0.48, 0.56, 0.68, 0.7);
    }

    NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.14)
}

fn animated_color_bar_color(index: usize, meter_value: f32) -> Retained<NSColor> {
    if meter_value <= 0.02 {
        return inactive_meter_bar_color();
    }

    let bar_position = animated_color_bar_position(index);
    let intensity = (0.35 + (meter_value * 0.65)).clamp(0.0, 1.0) as f64;

    if bar_position >= 0.88 {
        return NSColor::colorWithSRGBRed_green_blue_alpha(0.95, 0.28, 0.24, intensity);
    }

    if bar_position >= 0.68 {
        return NSColor::colorWithSRGBRed_green_blue_alpha(0.82, 0.50, 0.08, intensity);
    }

    NSColor::colorWithSRGBRed_green_blue_alpha(0.26, 0.86, 0.54, intensity)
}

fn meter_bar_height(meter_value: f32) -> f64 {
    let normalized_meter_value = meter_value.clamp(0.0, 1.0) as f64;
    METER_MIN_BAR_HEIGHT
        + ((METER_VIEW_HEIGHT - METER_MIN_BAR_HEIGHT) * normalized_meter_value.powf(0.9))
}

fn animated_color_bar_position(index: usize) -> f32 {
    meter_center_distance(index)
}

fn smoothstep(start: f32, end: f32, value: f32) -> f32 {
    if (end - start).abs() <= f32::EPSILON {
        return if value >= end { 1.0 } else { 0.0 };
    }

    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - (2.0 * t))
}

fn animate_towards(current: f64, target: f64, duration_seconds: f64, delta_seconds: f64) -> f64 {
    if duration_seconds <= f64::EPSILON {
        return target;
    }

    let progress = (delta_seconds / duration_seconds).clamp(0.0, 1.0) as f32;
    let eased_progress = smoothstep(0.0, 1.0, progress) as f64;
    current + ((target - current) * eased_progress)
}

fn meter_cluster_width() -> f64 {
    (usable_text_width() * METER_CLUSTER_WIDTH_FACTOR)
        .clamp(METER_CLUSTER_MIN_WIDTH, METER_CLUSTER_MAX_WIDTH)
}

fn meter_container_height(meter_style: UiMeterStyle) -> f64 {
    let graph_height = match meter_style {
        UiMeterStyle::None => 0.0,
        UiMeterStyle::AnimatedHeight => METER_VIEW_HEIGHT,
        UiMeterStyle::AnimatedColor => METER_COLOR_ONLY_BAR_HEIGHT,
    };

    graph_height + (METER_BORDER_PADDING * 2.0)
}

fn meter_container_width() -> f64 {
    meter_cluster_width() + (METER_BORDER_PADDING * 2.0)
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
    let container_height = meter_container_height(meter_style);
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

    NSRect::new(
        NSPoint::new(TEXT_HORIZONTAL_PADDING, 6.0),
        NSSize::new(footer_text_width.max(0.0), FOOTER_HEIGHT - 8.0),
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
