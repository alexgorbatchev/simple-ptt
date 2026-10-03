//! The pills on screen: one capsule per spectrum band, low pitches at the
//! centre and higher pitches alternating towards both edges, growing up and
//! down from the middle line. Each update moves every pill with Core Animation's
//! implicit animation over the time until the next update, so the pills move
//! smoothly between the overlay's 75 ms updates. A pill at rest fades out, so
//! the row is empty while there is no sound, and fades in as it rises.

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSView};
use objc2_quartz_core::{kCAMediaTimingFunctionLinear, CALayer, CAMediaTimingFunction, CATransaction};

use crate::state::SPECTRUM_BANDS;

use super::{cg_color_in, centred_bar_frame};

/// Width of each pill.
const PILL_WIDTH: f64 = 3.325;
/// Gap between pills, unless the row is too narrow for it.
const PILL_SPACING: f64 = 2.0;
/// Scaled with the width (2 pt at 4.75 pt), so the pills keep their shape.
const PILL_CORNER_RADIUS: f64 = 1.4;
/// Every pill's opacity, in the label color: the level changes only heights.
const PILL_OPACITY: f64 = 0.5;
/// A pill fades in over the first this much of its height, so it grows in
/// rather than popping up, and a faint flicker of noise stays faint.
const FADE_IN_HEIGHT: f32 = 0.1;

#[derive(Debug)]
pub(super) struct PillCluster {
    pills: Vec<Retained<CALayer>>,
}

impl PillCluster {
    /// One pill per band in `container`'s layer, at rest and faded out.
    pub(super) fn new(container: &CALayer) -> Self {
        let pills = (0..SPECTRUM_BANDS)
            .map(|_| {
                let pill = CALayer::new();
                pill.setOpacity(0.0);
                pill.setCornerRadius(PILL_CORNER_RADIUS);
                container.addSublayer(&pill);
                pill
            })
            .collect();
        Self { pills }
    }

    pub(super) fn set_hidden(&self, hidden: bool) {
        without_animation(|| {
            for pill in &self.pills {
                pill.setHidden(hidden);
            }
        });
    }

    /// Lays the pills out centred in `cluster_width` of `container` (the
    /// meter's view) and moves each to its height in `heights`, over
    /// `seconds`, or at once when it is 0.
    pub(super) fn render(&self, container: &NSView, cluster_width: f64, heights: &[f32; SPECTRUM_BANDS], seconds: f64) {
        let color = cg_color_in(container, &|| NSColor::labelColor().colorWithAlphaComponent(PILL_OPACITY));
        CATransaction::begin();
        if seconds > 0.0 {
            CATransaction::setAnimationDuration(seconds);
            // SAFETY: an immutable Core Animation constant.
            CATransaction::setAnimationTimingFunction(Some(&CAMediaTimingFunction::functionWithName(unsafe {
                kCAMediaTimingFunctionLinear
            })));
        } else {
            CATransaction::setDisableActions(true);
        }
        for (index, (pill, height)) in self.pills.iter().zip(heights).enumerate() {
            pill.setFrame(centred_bar_frame(pill_x(index, cluster_width), PILL_WIDTH, *height));
            pill.setOpacity((*height / FADE_IN_HEIGHT).clamp(0.0, 1.0));
            pill.setBackgroundColor(Some(&color));
        }
        CATransaction::commit();
    }
}

/// Runs `change` without Core Animation's implicit animations.
fn without_animation(change: impl FnOnce()) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    change();
    CATransaction::commit();
}

/// The distance from one pill to the next in a row `cluster_width` wide: the
/// pill and its gap, or less if the row is too narrow for every pill.
fn pitch(cluster_width: f64) -> f64 {
    (PILL_WIDTH + PILL_SPACING).min((cluster_width - PILL_WIDTH) / (SPECTRUM_BANDS - 1) as f64)
}

/// Where frequency band `band` starts. Adjacent bands form pairs spreading
/// from the centre towards both edges, giving speech a centred silhouette.
fn pill_x(band: usize, cluster_width: f64) -> f64 {
    let centre = (SPECTRUM_BANDS - 1) / 2;
    let column = if band % 2 == 0 {
        centre - band / 2
    } else {
        centre + band / 2 + 1
    };
    let pitch = pitch(cluster_width);
    let width = (pitch * (SPECTRUM_BANDS - 1) as f64) + PILL_WIDTH;
    super::METER_BORDER_PADDING + ((cluster_width - width) / 2.0) + (pitch * column as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_meter::METER_BORDER_PADDING;

    fn ordered_positions(cluster_width: f64) -> Vec<f64> {
        let mut xs: Vec<f64> = (0..SPECTRUM_BANDS)
            .map(|band| pill_x(band, cluster_width))
            .collect();
        xs.sort_by(f64::total_cmp);
        xs
    }

    #[test]
    fn lower_frequency_bands_radiate_from_the_middle_of_the_row() {
        for cluster_width in [180.0, 251.0, 260.0] {
            let middle = METER_BORDER_PADDING + cluster_width / 2.0;
            let mut bands: Vec<usize> = (0..SPECTRUM_BANDS).collect();
            bands.sort_by(|a, b| {
                let distance = |band| (pill_x(band, cluster_width) + PILL_WIDTH / 2.0 - middle).abs();
                distance(*a).total_cmp(&distance(*b))
            });

            for (pair, nearest) in bands.chunks_exact(2).enumerate() {
                assert!(
                    nearest.contains(&(pair * 2)) && nearest.contains(&(pair * 2 + 1)),
                    "width {cluster_width}: pair {pair} contains {nearest:?}"
                );
                assert!(pill_x(pair * 2, cluster_width) < middle);
                assert!(pill_x(pair * 2 + 1, cluster_width) > middle);
            }
        }
    }

    #[test]
    fn the_pills_are_centred_in_the_row() {
        for cluster_width in [251.0, 260.0, 180.0] {
            let xs = ordered_positions(cluster_width);
            let left = xs[0] - METER_BORDER_PADDING;
            let right = cluster_width - (xs[SPECTRUM_BANDS - 1] - METER_BORDER_PADDING + PILL_WIDTH);
            assert!((left - right).abs() < 1e-9, "{cluster_width}: {left} {right}");
            assert!(left >= 0.0, "{cluster_width}: {left}");
        }
    }

    #[test]
    fn the_pills_keep_their_gap_where_they_fit_and_close_up_where_not() {
        let xs = ordered_positions(251.0);
        for pair in xs.windows(2) {
            assert!((pair[1] - pair[0] - (PILL_WIDTH + PILL_SPACING)).abs() < 1e-9);
        }
        // 40 pills at 5.325 pt need 211 pt: a 150 pt row closes them up to
        // fill it exactly.
        let xs = ordered_positions(150.0);
        let left = xs[0] - METER_BORDER_PADDING;
        let right = xs[SPECTRUM_BANDS - 1] - METER_BORDER_PADDING + PILL_WIDTH;
        assert!(left.abs() < 1e-9 && (right - 150.0).abs() < 1e-9, "{left} {right}");
    }
}
