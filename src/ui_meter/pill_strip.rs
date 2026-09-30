//! The pills on screen: a strip of pill layers that Core Animation moves
//! from right to left at one pill spacing per `PILL_SECONDS`, in a column
//! that fades pills in quickly on the right and out slowly on the left.
//!
//! The strip carries one linear animation of its translation, started with
//! the strip's clock, so the render server moves it at the display's rate
//! and the app does nothing per frame. Pill `k` sits at `k × PILL_PITCH` in
//! the strip and enters the column at its right edge `ENTRY_DELAY_SECONDS`
//! after its time begins, so its height is final before it can be seen. Only
//! enough layers to cover the column and the way in exist: the layer of a
//! pill that has left the column on the left becomes the next pill on the
//! right.
//!
//! With Reduce Motion on when the strip starts, it does not glide: it steps
//! one pill spacing to the left as each pill opens.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSView};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{ns_string, NSArray, NSNumber, NSString};
use objc2_quartz_core::{
    kCAFillModeForwards, CABasicAnimation, CAGradientLayer, CALayer, CAMediaTiming, CATransaction,
    CATransform3D,
};

use super::pill_levels::{ENTRY_DELAY_SECONDS, PILL_SECONDS};
use crate::overlay::reduce_motion;
use super::{cg_color_in, centred_bar_frame};

/// Width of each pill.
pub(super) const PILL_WIDTH: f64 = 4.75;
/// Gap between pills.
const PILL_SPACING: f64 = 2.0;
/// Distance from one pill to the next.
const PILL_PITCH: f64 = PILL_WIDTH + PILL_SPACING;
const PILL_CORNER_RADIUS: f64 = 2.0;
/// Every pill's opacity, in the label color: the level changes only heights.
const PILL_OPACITY: f64 = 0.5;
/// How far in from the column's right edge the pills fade in. Short, since
/// pills enter final (`ENTRY_DELAY_SECONDS`) and the fade only delays them:
/// at the strip's 67.5 pt/s, 8 pt takes 0.12 s to cross, and 25 pt took
/// 0.37 s.
const ENTRY_FADE_WIDTH: f64 = 8.0;
/// How far in from the column's left edge the pills fade out.
const EXIT_FADE_WIDTH: f64 = 25.0;
/// How long the strip's animation runs: longer than any recording. At the
/// strip's speed it moves under 3 million points in that time, where layer
/// geometry is still exact to well under a point.
const STRIP_ANIMATION_SECONDS: f64 = 24.0 * 60.0 * 60.0;
const STRIP_ANIMATION_KEY: &str = "simple-ptt.pill-strip";

#[derive(Debug)]
pub(super) struct PillStrip {
    /// The column the pills show in, clipped and masked by `fade`.
    column: Retained<CALayer>,
    fade: Retained<CAGradientLayer>,
    /// Moves; holds the pills at their places.
    strip: Retained<CALayer>,
    pills: RefCell<Vec<Retained<CALayer>>>,
    /// When the strip started moving, in media time (`CACurrentMediaTime`).
    started_at: Cell<Option<f64>>,
    /// Whether the strip steps instead of gliding: Reduce Motion was on when
    /// it started.
    steps: Cell<bool>,
}

impl PillStrip {
    /// A strip in `container`'s layer, hidden until shown.
    pub(super) fn new(container: &CALayer) -> Self {
        let column = CALayer::new();
        column.setMasksToBounds(true);
        column.setHidden(true);
        let fade = CAGradientLayer::new();
        let clear = NSColor::clearColor().CGColor();
        let opaque = NSColor::blackColor().CGColor();
        let colors: [&objc2::runtime::AnyObject; 4] =
            [clear.as_ref(), opaque.as_ref(), opaque.as_ref(), clear.as_ref()];
        // SAFETY: `colors` holds CGColors, the type `CAGradientLayer.colors`
        // documents.
        unsafe { fade.setColors(Some(&NSArray::from_slice(&colors))) };
        fade.setStartPoint(CGPoint::new(0.0, 0.5));
        fade.setEndPoint(CGPoint::new(1.0, 0.5));
        // SAFETY: the fade is a standalone layer owned by this strip, not
        // part of any other layer tree.
        unsafe { column.setMask(Some(&fade)) };
        let strip = CALayer::new();
        column.addSublayer(&strip);
        container.addSublayer(&column);
        Self {
            column,
            fade,
            strip,
            pills: RefCell::new(Vec::new()),
            started_at: Cell::new(None),
            steps: Cell::new(false),
        }
    }

    pub(super) fn set_hidden(&self, hidden: bool) {
        without_actions(|| self.column.setHidden(hidden));
    }

    /// The strip's clock at media time `now`: seconds since it started
    /// moving, starting it now if it has not.
    pub(super) fn clock(&self, now: f64) -> f64 {
        let started_at = match self.started_at.get() {
            Some(started_at) => started_at,
            None => {
                self.start(now);
                now
            }
        };
        now - started_at
    }

    /// Stops the strip and puts it back at its start.
    pub(super) fn stop(&self) {
        self.strip.removeAnimationForKey(&NSString::from_str(STRIP_ANIMATION_KEY));
        without_actions(|| self.strip.setTransform(CATransform3D::new_translation(0.0, 0.0, 0.0)));
        self.started_at.set(None);
    }

    /// Lays the column out `span` wide in `container` (the meter's view) and
    /// gives every pill held its height: `heights` are the pills up to
    /// `open_index`, the open one last.
    pub(super) fn render(&self, container: &NSView, span: f64, heights: &[f32], open_index: u64) {
        let pool = pool_size(span);
        let color = cg_color_in(container, &|| NSColor::labelColor().colorWithAlphaComponent(PILL_OPACITY));
        without_actions(|| {
            let bounds = container.bounds();
            let column_frame = CGRect::new(
                CGPoint::new(super::METER_BORDER_PADDING, 0.0),
                CGSize::new(span, bounds.size.height),
            );
            self.column.setFrame(column_frame);
            self.fade.setFrame(CGRect::new(CGPoint::ZERO, column_frame.size));
            let locations = fade_locations(span).map(NSNumber::new_f64);
            self.fade.setLocations(Some(&NSArray::from_retained_slice(&locations)));
            self.strip.setFrame(CGRect::new(CGPoint::new(right_anchor(span), 0.0), CGSize::new(PILL_WIDTH, bounds.size.height)));
            if self.steps.get() {
                self.strip.setTransform(CATransform3D::new_translation(stepped_offset(open_index), 0.0, 0.0));
            }

            self.ensure_pills(pool);
            let pills = self.pills.borrow();
            for pill in held_pills(open_index, pool) {
                let layer = &pills[slot(pill, pool)];
                let frame = centred_bar_frame(pill_x(pill), PILL_WIDTH, pill_height(pill, open_index, heights));
                layer.setFrame(frame);
                layer.setBackgroundColor(Some(&color));
                layer.setCornerRadius(PILL_CORNER_RADIUS);
            }
        });
    }

    fn start(&self, now: f64) {
        self.started_at.set(Some(now));
        // Read as the motion starts, like the overlay's other motion, so a
        // change applies from the next recording.
        self.steps.set(reduce_motion());
        if self.steps.get() {
            return;
        }
        let animation = CABasicAnimation::animationWithKeyPath(Some(ns_string!("transform.translation.x")));
        // SAFETY: `transform.translation.x` animates between NSNumbers.
        unsafe {
            animation.setFromValue(Some(&NSNumber::new_f64(0.0)));
            animation.setToValue(Some(&NSNumber::new_f64(strip_offset(STRIP_ANIMATION_SECONDS))));
        }
        animation.setDuration(STRIP_ANIMATION_SECONDS);
        // No timing function: nil is linear pacing, while even the linear
        // CAMediaTimingFunction is a Bezier solved to a tolerance that, over
        // this long an animation, put the strip up to 28 pt off (findings.md).
        animation.setBeginTime(self.strip.convertTime_fromLayer(now, None));
        animation.setRemovedOnCompletion(false);
        animation.setFillMode(unsafe { kCAFillModeForwards });
        self.strip.addAnimation_forKey(&animation, Some(&NSString::from_str(STRIP_ANIMATION_KEY)));
    }

    /// Adds or removes pill layers until there are `count`.
    fn ensure_pills(&self, count: usize) {
        let mut pills = self.pills.borrow_mut();
        while pills.len() > count {
            if let Some(pill) = pills.pop() {
                pill.removeFromSuperlayer();
            }
        }
        while pills.len() < count {
            let pill = CALayer::new();
            self.strip.addSublayer(&pill);
            pills.push(pill);
        }
    }
}

/// Runs `change` with Core Animation's implicit animations off: the pills
/// move only with the strip.
fn without_actions(change: impl FnOnce()) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    change();
    CATransaction::commit();
}

/// Where pill `index` sits in the strip.
fn pill_x(index: i64) -> f64 {
    index as f64 * PILL_PITCH
}

/// How far the strip has moved `elapsed` seconds after it started.
fn strip_offset(elapsed: f64) -> f64 {
    -elapsed * (PILL_PITCH / PILL_SECONDS)
}

/// How far the stepping strip (Reduce Motion) has moved while pill
/// `open_index` is open: the open pill stands at the right edge for its time.
fn stepped_offset(open_index: u64) -> f64 {
    strip_offset(open_index as f64 * PILL_SECONDS)
}

/// Where pill 0 sits at the strip's start in a column `span` wide: as far
/// right of the column as the strip moves in `ENTRY_DELAY_SECONDS`, so each
/// pill enters the column that long after its time begins.
fn right_anchor(span: f64) -> f64 {
    span - strip_offset(ENTRY_DELAY_SECONDS)
}

/// How many pill layers cover a column `span` wide and the way in: when pill
/// `k` opens, the layer of pill `k - pool + 1` becomes pill `k + 1`, so it
/// must have left the column by then.
pub(super) fn pool_size(span: f64) -> usize {
    ((right_anchor(span) + PILL_WIDTH) / PILL_PITCH).ceil().max(0.0) as usize + 1
}

/// The pills the layers hold while pill `open_index` is open: the `pool - 1`
/// up to it, and the next one, on its way in.
fn held_pills(open_index: u64, pool: usize) -> impl Iterator<Item = i64> {
    let next = open_index as i64 + 1;
    (next + 1 - pool as i64)..=next
}

/// The layer that holds pill `index`.
fn slot(index: i64, pool: usize) -> usize {
    index.rem_euclid(pool as i64) as usize
}

/// Pill `index`'s height: from `heights` (the pills up to `open_index`, the
/// open one last), or resting when it is older than them or not open yet.
fn pill_height(index: i64, open_index: u64, heights: &[f32]) -> f32 {
    let age = open_index as i64 - index;
    if age < 0 || age as usize >= heights.len() {
        0.0
    } else {
        heights[heights.len() - 1 - age as usize]
    }
}

/// The fade mask's stops across a column `span` wide: clear at both ends,
/// opaque from `EXIT_FADE_WIDTH` in from the left to `ENTRY_FADE_WIDTH` in
/// from the right. A column narrower than both fades shares its width
/// between them in proportion.
fn fade_locations(span: f64) -> [f64; 4] {
    let scale = (span / (EXIT_FADE_WIDTH + ENTRY_FADE_WIDTH)).min(1.0);
    let exit = (EXIT_FADE_WIDTH * scale) / span;
    let entry = (ENTRY_FADE_WIDTH * scale) / span;
    [0.0, exit, 1.0 - entry, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_meter::pill_levels::LATEST_PAINT_SECONDS;

    const COLUMN: f64 = 514.0;

    /// Where pill `index`'s left edge shows in the column `at` on the strip's
    /// clock.
    fn visible_x(index: i64, at: f64) -> f64 {
        right_anchor(COLUMN) + pill_x(index) + strip_offset(at)
    }

    #[test]
    fn a_pill_enters_the_column_after_its_entry_delay() {
        for index in [0_i64, 1, 7, 1_000, 18_000] {
            let entry = (index as f64 * PILL_SECONDS) + ENTRY_DELAY_SECONDS;
            assert!((visible_x(index, entry) - COLUMN).abs() < 1e-6, "pill {index}: {}", visible_x(index, entry));
        }
    }

    #[test]
    fn a_pill_is_still_out_of_view_when_it_is_painted_at_its_latest() {
        for index in [0_i64, 7, 18_000] {
            let latest = (index as f64 * PILL_SECONDS) + LATEST_PAINT_SECONDS;
            assert!(visible_x(index, latest) >= COLUMN, "pill {index}: {}", visible_x(index, latest));
        }
    }

    #[test]
    fn with_reduce_motion_the_strip_steps_one_pill_as_each_opens() {
        // Pill k enters the column only as a pill opens after its latest
        // paint time.
        for index in [0_u64, 1, 7, 500] {
            let stepped_x = |open: u64| right_anchor(COLUMN) + pill_x(index as i64) + stepped_offset(open);
            let enters = (index..).find(|open| stepped_x(*open) < COLUMN).unwrap();
            assert!(
                enters as f64 * PILL_SECONDS >= (index as f64 * PILL_SECONDS) + LATEST_PAINT_SECONDS,
                "pill {index} enters as pill {enters} opens"
            );
        }
        // Each opening pill moves the strip one spacing to the left.
        assert!((stepped_offset(3) - stepped_offset(2) + PILL_PITCH).abs() < 1e-9);
    }

    #[test]
    fn the_strip_moves_one_pill_spacing_per_pill_time() {
        assert!((strip_offset(PILL_SECONDS) + PILL_PITCH).abs() < 1e-9);
        assert!((strip_offset(3.0 * PILL_SECONDS) + (3.0 * PILL_PITCH)).abs() < 1e-9);
    }

    #[test]
    fn the_pool_recycles_only_pills_that_left_the_column() {
        for column in [60.0, 100.0, 513.3, COLUMN] {
            let pool = pool_size(column);
            // When pill k opens, the layer of pill k - pool + 1 becomes pill
            // k + 1: it must have left the column on the left.
            let recycled_right_edge =
                right_anchor(column) + pill_x(1 - pool as i64) + PILL_WIDTH;
            assert!(recycled_right_edge <= 0.0, "{column}: {recycled_right_edge}");
            // One layer fewer would recycle a pill still in the column.
            let too_few_right_edge =
                right_anchor(column) + pill_x(2 - pool as i64) + PILL_WIDTH;
            assert!(too_few_right_edge > 0.0, "{column}: {too_few_right_edge}");
        }
    }

    #[test]
    fn the_pills_held_at_once_each_have_their_own_layer() {
        let pool = pool_size(COLUMN);
        for open_index in 0..(3 * pool as u64) {
            let mut slots: Vec<usize> = held_pills(open_index, pool).map(|pill| slot(pill, pool)).collect();
            assert_eq!(slots.len(), pool);
            slots.sort_unstable();
            assert_eq!(slots, (0..pool).collect::<Vec<_>>(), "open pill {open_index}");
            // The next pill waits at the right edge before its time begins,
            // so it is in place however late the update that opens it lands.
            let held: Vec<i64> = held_pills(open_index, pool).collect();
            assert_eq!(held.last(), Some(&(open_index as i64 + 1)));
            assert_eq!(held.first(), Some(&(open_index as i64 + 2 - pool as i64)));
        }
    }

    #[test]
    fn pills_fade_in_quickly_on_the_right_and_out_slowly_on_the_left() {
        // Opaque from 25 pt in from the left edge to 8 pt in from the right.
        let [start, left_opaque, right_opaque, end] = fade_locations(COLUMN);
        assert_eq!((start, end), (0.0, 1.0));
        assert!((left_opaque * COLUMN - 25.0).abs() < 1e-9, "{left_opaque}");
        assert!(((1.0 - right_opaque) * COLUMN - 8.0).abs() < 1e-9, "{right_opaque}");
        // A column narrower than both fades shares it between them in
        // proportion: 30 pt split 25 : 8.
        let [_, left_opaque, right_opaque, _] = fade_locations(30.0);
        assert!((left_opaque - (25.0 / 33.0)).abs() < 1e-9, "{left_opaque}");
        assert!((right_opaque - left_opaque).abs() < 1e-9, "{right_opaque}");
    }

    #[test]
    fn a_pill_is_fully_shown_within_a_quarter_second_of_entering() {
        // It crosses its own width and the 8 pt fade at 67.5 pt/s: 0.19 s.
        // Across a 25 pt fade it took 0.44 s.
        let entry = ENTRY_DELAY_SECONDS;
        let [_, _, right_opaque, _] = fade_locations(COLUMN);
        let fully_shown = (0..=1000)
            .map(|step| entry + (f64::from(step) * 0.001))
            .find(|at| visible_x(0, *at) + PILL_WIDTH <= right_opaque * COLUMN)
            .unwrap();
        assert!(fully_shown - entry < 0.25, "{}", fully_shown - entry);
    }

    #[test]
    fn each_pill_takes_its_own_height() {
        // Pills 5 to 8, pill 8 open.
        let heights = [0.1, 0.2, 0.3, 0.4];
        assert_eq!(pill_height(8, 8, &heights), 0.4);
        assert_eq!(pill_height(5, 8, &heights), 0.1);
        // Older than the history, and the next pill, rest.
        assert_eq!(pill_height(4, 8, &heights), 0.0);
        assert_eq!(pill_height(9, 8, &heights), 0.0);
    }
}
