//! The values the pills are drawn with. Debug mode tunes them live; the
//! defaults are the values settled on there, and
//! `docs/internal/references/pill-meter.md` gives the reason for each.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillTuning {
    /// A band up to this far above its noise floor, in dB, keeps its pill
    /// down.
    pub noise_gate_db: f64,
    /// The least distance from a band's gate to the top of the range, in dB,
    /// so the room's noise is never stretched to full height.
    pub min_span_db: f64,
    /// How far below the top of the range, in dB, a pill rests: a soft voice
    /// and a loud one fill the pills alike.
    pub range_db: f64,
    /// A band's noise floor is its quietest level of this long.
    pub floor_seconds: f64,
    /// The top of the range is this share of the recent band levels above
    /// their gates.
    pub top_quantile: f64,
    /// How long the top is taken over.
    pub top_seconds: f64,
    /// Added per octave above 1 kHz, and taken away below, in dB, so the
    /// voice's quieter high pitches still show.
    pub tilt_db_per_octave: f64,
    /// How fast a pill falls, in heights per second (a pill rises at once).
    pub fall_per_second: f64,
}

impl Default for PillTuning {
    fn default() -> Self {
        Self {
            noise_gate_db: 12.0,
            min_span_db: 24.0,
            range_db: 30.0,
            floor_seconds: 10.0,
            top_quantile: 0.98,
            top_seconds: 10.0,
            tilt_db_per_octave: 3.0,
            fall_per_second: 2.5,
        }
    }
}
