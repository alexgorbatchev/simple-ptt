//! The values the pills are drawn with. Debug mode tunes them live; the
//! defaults are the values settled on there, and
//! `docs/internal/references/pill-meter.md` gives the reason for each.

use super::pill_levels::PILL_SECONDS;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillTuning {
    /// Pills up to this far above the noise floor, in dB, rest as dots.
    pub noise_gate_db: f64,
    /// The least distance from the gate to the top of the range, in dB, so
    /// background noise is never stretched to full height.
    pub min_span_db: f64,
    /// The noise floor is the quietest pill of this long.
    pub floor_seconds: f64,
    /// The top of the range is this share of the recent pills above the
    /// gate.
    pub top_quantile: f64,
    /// How long the top is taken over (counted in pills above the gate).
    pub top_seconds: f64,
    /// How much of a pill's height its loudness decides; the rest is always
    /// drawn, so the softest speech still shows its texture.
    pub loudness_share: f64,
    /// How long a quiet stretch scrolls by as dots before the strip stops;
    /// it moves again at the next sound.
    pub pause_after_seconds: f64,
    /// The texture's tall band, which even pills take a height in.
    pub tall_low: f64,
    pub tall_high: f64,
    /// The texture's short band, which odd pills take a height in.
    pub short_low: f64,
    pub short_high: f64,
}

impl Default for PillTuning {
    fn default() -> Self {
        Self {
            noise_gate_db: 6.0,
            min_span_db: 12.0,
            floor_seconds: 10.0,
            top_quantile: 0.9,
            top_seconds: 10.0,
            loudness_share: 0.5,
            pause_after_seconds: 0.4,
            tall_low: 0.6,
            tall_high: 1.0,
            short_low: 0.2,
            short_high: 0.6,
        }
    }
}

impl PillTuning {
    /// The noise gate as an amplitude ratio above the floor.
    pub(super) fn gate_ratio(&self) -> f32 {
        amplitude_ratio(self.noise_gate_db)
    }

    /// The least span as an amplitude ratio above the gate.
    pub(super) fn min_span_ratio(&self) -> f32 {
        amplitude_ratio(self.min_span_db)
    }

    /// How many pills of the room the floor is taken over.
    pub(super) fn floor_pills(&self) -> usize {
        pills_in(self.floor_seconds)
    }

    /// How many pills above the gate the top is taken over.
    pub(super) fn top_pills(&self) -> usize {
        pills_in(self.top_seconds)
    }

    /// How many quiet pills scroll by as dots before the strip stops.
    pub(super) fn pause_after_pills(&self) -> usize {
        (self.pause_after_seconds / PILL_SECONDS).round() as usize
    }
}

fn amplitude_ratio(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// Whole pills in `seconds`, at least one.
fn pills_in(seconds: f64) -> usize {
    ((seconds / PILL_SECONDS).round() as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decibels_become_amplitude_ratios_and_seconds_whole_pills() {
        let tuning = PillTuning { noise_gate_db: 6.0, min_span_db: 20.0, floor_seconds: 10.0, top_seconds: 0.01, ..PillTuning::default() };
        assert!((tuning.gate_ratio() - 1.995).abs() < 1e-3, "{}", tuning.gate_ratio());
        assert!((tuning.min_span_ratio() - 10.0).abs() < 1e-4, "{}", tuning.min_span_ratio());
        assert_eq!(tuning.floor_pills(), 100);
        // Never less than one pill.
        assert_eq!(tuning.top_pills(), 1);
    }
}
