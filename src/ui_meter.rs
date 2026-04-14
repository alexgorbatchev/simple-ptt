use std::cell::{Cell, RefCell};
use std::time::Instant;

use objc2::{rc::Retained, MainThreadOnly};
use objc2_app_kit::{NSColor, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::config::UiMeterStyle;
use crate::state::MicMeterSnapshot;
use crate::MainThreadMarker;

pub const CLIP_INDICATOR_BORDER_WIDTH: f64 = 1.0;
pub const CLIP_INDICATOR_CORNER_RADIUS: f64 = 4.0;
pub const CLIP_INDICATOR_FADE_IN_SECONDS: f64 = 0.08;
pub const CLIP_INDICATOR_FADE_OUT_SECONDS: f64 = CLIP_INDICATOR_FADE_IN_SECONDS * 2.0;
pub const CLIP_INDICATOR_HOLD_SECONDS: f64 = 0.20;

pub const METER_BORDER_PADDING: f64 = 3.0;
pub const METER_BAR_COUNT: usize = 20;
pub const METER_BAR_SPACING: f64 = 3.0;
pub const METER_COLOR_ONLY_BAR_HEIGHT: f64 = 4.0;
pub const METER_MIN_BAR_HEIGHT: f64 = 0.0;
pub const METER_VIEW_HEIGHT: f64 = 19.6;

#[derive(Debug, Default)]
struct ClipIndicatorState {
    alpha: f64,
    hold_remaining_seconds: f64,
    last_clip_event_counter: u32,
    last_updated_at: Option<Instant>,
}

#[derive(Debug)]
pub struct UiMeterView {
    container_view: Retained<NSView>,
    meter_bar_views: Vec<Retained<NSView>>,
    meter_bar_levels: RefCell<Vec<f32>>,
    clip_indicator_state: RefCell<ClipIndicatorState>,
    meter_style: Cell<UiMeterStyle>,
}

impl UiMeterView {
    pub fn new(mtm: MainThreadMarker, style: UiMeterStyle) -> Self {
        let container_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
        );
        container_view.setWantsLayer(true);
        if let Some(layer) = container_view.layer() {
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
            container_view.addSubview(&bar_view);
            meter_bar_views.push(bar_view);
        }

        Self {
            container_view,
            meter_bar_views,
            meter_bar_levels: RefCell::new(vec![0.0; METER_BAR_COUNT]),
            clip_indicator_state: RefCell::new(ClipIndicatorState::default()),
            meter_style: Cell::new(style),
        }
    }

    pub fn view(&self) -> &NSView {
        &self.container_view
    }

    pub fn set_frame(&self, frame: NSRect) {
        self.container_view.setFrame(frame);
    }

    pub fn set_hidden(&self, hidden: bool) {
        self.container_view.setHidden(hidden);
    }

    pub fn set_style(&self, style: UiMeterStyle) {
        self.meter_style.set(style);
    }

    pub fn style(&self) -> UiMeterStyle {
        self.meter_style.get()
    }

    pub fn update(&self, mic_meter: MicMeterSnapshot, cluster_width: f64) {
        self.container_view.setHidden(false);

        let level = mic_meter.level as f32 / u8::MAX as f32;
        let peak = mic_meter.peak as f32 / u8::MAX as f32;

        match self.meter_style.get() {
            UiMeterStyle::None => {}
            UiMeterStyle::AnimatedHeight => self.update_meter_animated_height(level, peak),
            UiMeterStyle::AnimatedColor => self.update_meter_animated_color(level, peak),
        }

        self.render_meter_bars(cluster_width);
        self.update_clip_indicator(mic_meter);
    }

    pub fn clear(&self, cluster_width: f64) {
        self.container_view.setHidden(true);
        let mut meter_bar_levels = self.meter_bar_levels.borrow_mut();
        meter_bar_levels.fill(0.0);
        drop(meter_bar_levels);
        self.render_meter_bars(cluster_width);

        self.clear_clip_indicator();
    }

    pub fn update_clip_indicator(&self, mic_meter: MicMeterSnapshot) {
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
        } else if clip_indicator_state.alpha > 0.005 {
            clip_indicator_state.alpha = animate_towards(
                clip_indicator_state.alpha,
                0.0,
                CLIP_INDICATOR_FADE_OUT_SECONDS,
                delta_seconds,
            );
            if clip_indicator_state.alpha <= 0.005 {
                clip_indicator_state.alpha = 0.0;
            }
        } else {
            clip_indicator_state.alpha = 0.0;
        }

        drop(clip_indicator_state);
        self.render_clip_indicator();
    }

    pub fn clear_clip_indicator(&self) {
        let mut clip_indicator_state = self.clip_indicator_state.borrow_mut();
        clip_indicator_state.alpha = 0.0;
        clip_indicator_state.hold_remaining_seconds = 0.0;
        clip_indicator_state.last_updated_at = None;
        drop(clip_indicator_state);
        self.render_clip_indicator();
    }

    fn render_clip_indicator(&self) {
        let clip_indicator_alpha = self.clip_indicator_state.borrow().alpha;
        if let Some(layer) = self.container_view.layer() {
            let border_color = clip_indicator_border_color(clip_indicator_alpha);
            let border_cg_color = border_color.CGColor();
            layer.setBorderColor(Some(&border_cg_color));
        }

        NSView::setNeedsDisplay(&self.container_view, true);
    }

    fn render_meter_bars(&self, cluster_width: f64) {
        match self.meter_style.get() {
            UiMeterStyle::None => {}
            UiMeterStyle::AnimatedHeight => self.render_meter_bars_animated_height(cluster_width),
            UiMeterStyle::AnimatedColor => self.render_meter_bars_animated_color(cluster_width),
        }

        NSView::setNeedsDisplay(&self.container_view, true);
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

    fn render_meter_bars_animated_height(&self, cluster_width: f64) {
        let meter_bar_levels = self.meter_bar_levels.borrow();
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

    fn render_meter_bars_animated_color(&self, cluster_width: f64) {
        let meter_bar_levels = self.meter_bar_levels.borrow();
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

    1.0 - ((bar_position - fade_start) / (fade_end - fade_start)).clamp(0.0, 1.0)
}

fn animated_color_attack(_index: usize) -> f32 {
    0.82
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

pub fn meter_container_height(meter_style: UiMeterStyle) -> f64 {
    let graph_height = match meter_style {
        UiMeterStyle::None => 0.0,
        UiMeterStyle::AnimatedHeight => METER_VIEW_HEIGHT,
        UiMeterStyle::AnimatedColor => METER_COLOR_ONLY_BAR_HEIGHT,
    };

    graph_height + (METER_BORDER_PADDING * 2.0)
}
