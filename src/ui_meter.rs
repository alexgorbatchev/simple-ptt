use std::cell::{Cell, RefCell};
use std::time::Instant;

use block2::StackBlock;
use objc2::{rc::Retained, MainThreadOnly};
use objc2_app_kit::{NSAppearanceCustomization, NSColor, NSView};
use objc2_core_graphics::CGColor;
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

/// Width of each pill (`UiMeterStyle::Pills`).
const PILL_WIDTH: f64 = 4.75;
/// Least gap between pills; the gap grows a little so the pills reach both
/// ends of the text column.
const PILL_MIN_SPACING: f64 = 2.0;
const PILL_CORNER_RADIUS: f64 = 2.0;
/// Every pill's opacity, in the label color: the level changes only heights.
const PILL_OPACITY: f64 = 0.5;

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
    /// The pills, as many as fit the text column (`pill_layout`).
    pill_views: RefCell<Vec<Retained<NSView>>>,
    meter_bar_levels: RefCell<Vec<f32>>,
    clip_indicator_state: RefCell<ClipIndicatorState>,
    meter_style: Cell<UiMeterStyle>,
    /// The pills' recent levels, oldest first (newest at the right), one per
    /// update.
    history: RefCell<Vec<f32>>,
    /// The level, smoothed, for the pills.
    smoothed_level: Cell<f32>,
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
            pill_views: RefCell::new(Vec::new()),
            meter_bar_levels: RefCell::new(vec![0.0; METER_BAR_COUNT]),
            clip_indicator_state: RefCell::new(ClipIndicatorState::default()),
            meter_style: Cell::new(style),
            history: RefCell::new(vec![0.0; METER_BAR_COUNT]),
            smoothed_level: Cell::new(0.0),
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

    /// Whether the meter spans the whole text column rather than a centred
    /// cluster.
    pub fn spans_text_width(&self) -> bool {
        self.meter_style.get() == UiMeterStyle::Pills
    }

    pub fn update(&self, mic_meter: MicMeterSnapshot, cluster_width: f64) {
        self.container_view.setHidden(false);

        let level = mic_meter.level as f32 / u8::MAX as f32;
        let peak = mic_meter.peak as f32 / u8::MAX as f32;

        match self.meter_style.get() {
            UiMeterStyle::None => {}
            UiMeterStyle::AnimatedHeight => self.update_meter_animated_height(level, peak),
            UiMeterStyle::AnimatedColor => self.update_meter_animated_color(level, peak),
            UiMeterStyle::Pills => self.update_smoothed_level(level, peak),
        }

        self.render_meter_bars(cluster_width);
        self.update_clip_indicator(mic_meter);
    }

    pub fn clear(&self, cluster_width: f64) {
        self.container_view.setHidden(true);
        let mut meter_bar_levels = self.meter_bar_levels.borrow_mut();
        meter_bar_levels.fill(0.0);
        drop(meter_bar_levels);
        self.history.borrow_mut().fill(0.0);
        self.smoothed_level.set(0.0);
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
        // The pills draw with their own views, the other styles with the bars.
        let pills = self.meter_style.get() == UiMeterStyle::Pills;
        for meter_bar_view in &self.meter_bar_views {
            meter_bar_view.setHidden(pills);
        }
        for pill_view in self.pill_views.borrow().iter() {
            pill_view.setHidden(!pills);
        }
        match self.meter_style.get() {
            UiMeterStyle::None => {}
            UiMeterStyle::AnimatedHeight => self.render_meter_bars_animated_height(cluster_width),
            UiMeterStyle::AnimatedColor => self.render_meter_bars_animated_color(cluster_width),
            UiMeterStyle::Pills => self.render_pills(cluster_width),
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

    /// Smooths the level for the pills, and flows it
    /// into the pills' history.
    fn update_smoothed_level(&self, level: f32, peak: f32) {
        let target = ((level.powf(0.9) * 0.82) + (peak.powf(0.78) * 0.18)).clamp(0.0, 1.0);
        let current = self.smoothed_level.get();
        let smoothing = if target >= current { 0.6 } else { 0.35 };
        let smoothed = current + ((target - current) * smoothing);
        self.smoothed_level.set(smoothed);
        push_history(&mut self.history.borrow_mut(), smoothed);
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

    /// One pill per level of the history, newest at the right, across
    /// `span` (the text column), in the label color at `PILL_OPACITY`.
    fn render_pills(&self, span: f64) {
        let (count, pitch) = pill_layout(span);
        self.ensure_pill_views(count);
        fit_history(&mut self.history.borrow_mut(), count);
        let history = self.history.borrow();
        for (index, pill_view) in self.pill_views.borrow().iter().enumerate() {
            let x = METER_BORDER_PADDING + (pitch * index as f64);
            pill_view.setFrame(centred_bar_frame(x, PILL_WIDTH, history[index]));
            if let Some(layer) = pill_view.layer() {
                let color = cg_color_in(pill_view, &|| NSColor::labelColor().colorWithAlphaComponent(PILL_OPACITY));
                layer.setBackgroundColor(Some(&color));
                layer.setCornerRadius(PILL_CORNER_RADIUS);
            }
        }
    }

    /// Adds or removes pill views until there are `count`.
    fn ensure_pill_views(&self, count: usize) {
        let mut pill_views = self.pill_views.borrow_mut();
        while pill_views.len() > count {
            if let Some(pill_view) = pill_views.pop() {
                pill_view.removeFromSuperview();
            }
        }
        let mtm = MainThreadMarker::from(&*self.container_view);
        while pill_views.len() < count {
            let pill_view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
            pill_view.setWantsLayer(true);
            if let Some(layer) = pill_view.layer() {
                layer.setMasksToBounds(true);
            }
            self.container_view.addSubview(&pill_view);
            pill_views.push(pill_view);
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

/// How many `PILL_WIDTH` pills fit `span` with at least `PILL_MIN_SPACING`
/// between them, and the distance from one to the next that puts the first
/// at the start of `span` and the last at its end.
fn pill_layout(span: f64) -> (usize, f64) {
    let count = (((span + PILL_MIN_SPACING) / (PILL_WIDTH + PILL_MIN_SPACING)).floor() as usize).max(1);
    let pitch = if count > 1 { (span - PILL_WIDTH) / (count - 1) as f64 } else { 0.0 };
    (count, pitch)
}

/// A capsule at `x` for `level`, centred on the track's middle line: a dot
/// as tall as it is wide when silent, the whole track at full level.
fn centred_bar_frame(x: f64, bar_width: f64, level: f32) -> NSRect {
    let rest = bar_width.min(METER_VIEW_HEIGHT);
    let height = rest + ((METER_VIEW_HEIGHT - rest) * (level.clamp(0.0, 1.0) as f64).powf(0.9));
    NSRect::new(
        NSPoint::new(x, METER_BORDER_PADDING + ((METER_VIEW_HEIGHT - height) / 2.0)),
        NSSize::new(bar_width, height),
    )
}

/// Flows `level` into `history` as its newest (last, rightmost) value; the
/// oldest leaves at the left.
fn push_history(history: &mut Vec<f32>, level: f32) {
    if !history.is_empty() {
        history.remove(0);
        history.push(level);
    }
}

/// Makes `history` `count` long, keeping its newest (rightmost) levels:
/// silence is added, or the oldest levels dropped, at the left.
fn fit_history(history: &mut Vec<f32>, count: usize) {
    if history.len() > count {
        history.drain(..history.len() - count);
    } else {
        history.splice(0..0, std::iter::repeat_n(0.0, count - history.len()));
    }
}

/// `color()` as a layer color, resolved in `view`'s appearance: the one the
/// glass content takes for legibility, not the app's.
fn cg_color_in(view: &NSView, color: &dyn Fn() -> Retained<NSColor>) -> Retained<CGColor> {
    let resolved = RefCell::new(None);
    let resolve = StackBlock::new(|| {
        resolved.replace(Some(color().CGColor()));
    });
    view.effectiveAppearance().performAsCurrentDrawingAppearance(&resolve);
    resolved
        .into_inner()
        .expect("performAsCurrentDrawingAppearance runs its block before returning")
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
        UiMeterStyle::AnimatedHeight | UiMeterStyle::Pills => METER_VIEW_HEIGHT,
        UiMeterStyle::AnimatedColor => METER_COLOR_ONLY_BAR_HEIGHT,
    };

    graph_height + (METER_BORDER_PADDING * 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pills_grow_both_ways_from_the_centre_line_and_rest_as_dots() {
        // Silent: a dot as tall as the pill is wide, on the centre line.
        let rest = centred_bar_frame(10.0, 6.0, 0.0);
        assert_eq!(rest.size, NSSize::new(6.0, 6.0));
        assert_eq!(rest.origin.y + (rest.size.height / 2.0), METER_BORDER_PADDING + (METER_VIEW_HEIGHT / 2.0));
        // Full: the whole track.
        let full = centred_bar_frame(10.0, 6.0, 1.0);
        assert_eq!(full.size.height, METER_VIEW_HEIGHT);
        assert_eq!(full.origin.y, METER_BORDER_PADDING);
        assert_eq!(full.origin.x, 10.0);
    }

    #[test]
    fn pills_history_flows_right_to_left_one_level_per_update() {
        let mut history = vec![0.0; 4];
        push_history(&mut history, 0.5);
        push_history(&mut history, 0.9);
        assert_eq!(history, vec![0.0, 0.0, 0.5, 0.9]);
    }

    #[test]
    fn pills_history_keeps_its_newest_levels_when_the_count_changes() {
        let mut history = vec![0.1, 0.2, 0.3];
        fit_history(&mut history, 5);
        assert_eq!(history, vec![0.0, 0.0, 0.1, 0.2, 0.3]);
        fit_history(&mut history, 2);
        assert_eq!(history, vec![0.2, 0.3]);
    }

    #[test]
    fn pills_fill_the_span_edge_to_edge_at_a_fixed_width() {
        let span = 514.0;
        let (count, pitch) = pill_layout(span);
        // As many pills as fit with at least the minimum gap between them.
        assert_eq!(count, 76);
        assert!(pitch - PILL_WIDTH >= PILL_MIN_SPACING);
        // One more pill at the minimum gap would not fit.
        assert!((count as f64 * (PILL_WIDTH + PILL_MIN_SPACING)) + PILL_WIDTH > span);
        // The last pill ends where the span ends.
        let last_right_edge = (pitch * (count - 1) as f64) + PILL_WIDTH;
        assert!((last_right_edge - span).abs() < 1e-9);
        assert_eq!(PILL_WIDTH, 4.75);
        assert_eq!(PILL_CORNER_RADIUS, 2.0);
        assert_eq!(PILL_OPACITY, 0.5);
    }
}
