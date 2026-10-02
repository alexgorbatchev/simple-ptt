//! What the pills show: the live spectrum of the voice, one pill per band,
//! low pitches on the left. Each band's level is drawn against a range that
//! adapts to the room and the speaker: its own noise floor, the quietest it
//! has been lately, so the room's noise and hum stay quiet from the first
//! frame, and a top shared by every band, so the voice's spectral shape
//! shows and soft speech in a quiet office fills the pills as well as loud
//! speech does. A pill rises at once and falls smoothly, like an equalizer's.

use std::collections::VecDeque;

use crate::state::{SpectrumFrame, SPECTRUM_BANDS};

use super::pill_tuning::PillTuning;

/// Band levels this low or lower are digital silence (a device delivering
/// zeros), which says nothing about the room.
const SILENCE_DB: f32 = -130.0;
/// Frames older than this when they are taken in are too late to show; they
/// still teach the range.
const STALE_SECONDS: f64 = 0.25;

/// The range as it stands, in dB, for debug mode's readout: the bands'
/// average noise floor and gate, the shared top, and the loudest band of the
/// latest frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillRange {
    pub floor_db: f32,
    pub gate_db: f32,
    pub top_db: f32,
    pub loudest_db: f32,
    pub loudest_hz: f32,
}

/// The pills' heights, low pitches first, and the range they are drawn from.
#[derive(Debug)]
pub(super) struct PillLevels {
    tuning: PillTuning,
    /// Each band's recent levels, with when they were heard, oldest first.
    recent: [VecDeque<(f64, f32)>; SPECTRUM_BANDS],
    /// The recent levels above their band's gate, with when, oldest first.
    loud: VecDeque<(f64, f32)>,
    heights: [f32; SPECTRUM_BANDS],
    /// When the heights were last updated.
    updated_at: Option<f64>,
    /// The latest frame's loudest band, tilted, and its centre frequency.
    loudest: (f32, f32),
}

impl PillLevels {
    pub(super) fn new() -> Self {
        Self {
            tuning: PillTuning::default(),
            recent: std::array::from_fn(|_| VecDeque::new()),
            loud: VecDeque::new(),
            heights: [0.0; SPECTRUM_BANDS],
            updated_at: None,
            loudest: (SILENCE_DB, 0.0),
        }
    }

    /// Takes in the spectrum `frames` heard since the last update, oldest
    /// first, `now` on the media clock: each pill rises at once to its band's
    /// level over them, or falls by `fall_per_second` towards it.
    pub(super) fn update(&mut self, frames: &[SpectrumFrame], now: f64) {
        let mut targets = [0.0f32; SPECTRUM_BANDS];
        if let Some(latest) = frames.last() {
            let levels = self.averaged(frames);
            self.learn(&levels, latest.at);
            if now - latest.at <= STALE_SECONDS {
                targets = self.frame_heights(&levels);
            }
            if let Some((band, level)) = levels.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)) {
                self.loudest = (level, latest.centres_hz[band]);
            }
        }
        let fall = self
            .updated_at
            .map_or(1.0, |updated_at| (now - updated_at).max(0.0) as f32 * self.tuning.fall_per_second as f32);
        for (height, target) in self.heights.iter_mut().zip(targets) {
            *height = if target >= *height { target } else { (*height - fall).max(target) };
        }
        self.updated_at = Some(now);
    }

    /// Each band's level over `frames`, tilted: their power averaged, which
    /// steadies the noise of the narrow low bands (a few FFT bins each, whose
    /// level jumps by up to 9 dB from frame to frame).
    fn averaged(&self, frames: &[SpectrumFrame]) -> [f32; SPECTRUM_BANDS] {
        let tilted: Vec<[f32; SPECTRUM_BANDS]> = frames.iter().map(|frame| self.tilted(frame)).collect();
        std::array::from_fn(|band| {
            let power: f32 = tilted.iter().map(|levels| 10f32.powf(levels[band] / 10.0)).sum::<f32>() / tilted.len() as f32;
            (10.0 * power.log10()).max(SILENCE_DB)
        })
    }

    /// `frame`'s band levels with the tilt applied: `tilt_db_per_octave` per
    /// octave above 1 kHz added, and as much taken away below, so the voice's
    /// quieter high pitches still show.
    fn tilted(&self, frame: &SpectrumFrame) -> [f32; SPECTRUM_BANDS] {
        let tilt = self.tuning.tilt_db_per_octave as f32;
        std::array::from_fn(|band| {
            let level = frame.levels_db[band];
            if level <= SILENCE_DB { level } else { level + (tilt * (frame.centres_hz[band] / 1_000.0).log2()) }
        })
    }

    /// Takes `levels`, heard `at`, into the range: each band's floor, and the
    /// shared top.
    fn learn(&mut self, levels: &[f32; SPECTRUM_BANDS], at: f64) {
        if levels.iter().all(|level| *level <= SILENCE_DB) {
            return;
        }
        let floor_since = at - self.tuning.floor_seconds;
        for (recent, level) in self.recent.iter_mut().zip(levels) {
            recent.push_back((at, *level));
            while recent.front().is_some_and(|(heard_at, _)| *heard_at < floor_since) {
                recent.pop_front();
            }
        }
        for (band, level) in levels.iter().enumerate() {
            if *level > self.gate(band) {
                self.loud.push_back((at, *level));
            }
        }
        let top_since = at - self.tuning.top_seconds;
        while self.loud.front().is_some_and(|(heard_at, _)| *heard_at < top_since) {
            self.loud.pop_front();
        }
    }

    /// Where each band of `levels` falls in the range: 0 at or below its
    /// bottom, 1 at or above the top, linear in dB between.
    fn frame_heights(&self, levels: &[f32; SPECTRUM_BANDS]) -> [f32; SPECTRUM_BANDS] {
        let top = self.top();
        std::array::from_fn(|band| {
            let (bottom, top) = self.band_range(band, top);
            ((levels[band] - bottom) / (top - bottom)).clamp(0.0, 1.0)
        })
    }

    /// Band `band`'s range, in dB: the shared `top`, at least `min_span_db`
    /// above the band's gate, down to `range_db` below it, but never below
    /// the gate. A soft voice and a loud one thus fill the pills alike.
    fn band_range(&self, band: usize, top: f32) -> (f32, f32) {
        let gate = self.gate(band);
        let top = top.max(gate + self.tuning.min_span_db as f32);
        (gate.max(top - self.tuning.range_db as f32), top)
    }

    fn floor(&self, band: usize) -> f32 {
        let floor = self.recent[band].iter().map(|(_, level)| *level).fold(f32::INFINITY, f32::min);
        // Before anything is heard, the floor is silence.
        if floor.is_finite() { floor } else { SILENCE_DB }
    }

    fn gate(&self, band: usize) -> f32 {
        self.floor(band) + self.tuning.noise_gate_db as f32
    }

    /// The `top_quantile` of the recent levels above their gates, or
    /// nothing (the least span decides) before any.
    fn top(&self) -> f32 {
        let mut loud: Vec<f32> = self.loud.iter().map(|(_, level)| *level).collect();
        if loud.is_empty() {
            return f32::NEG_INFINITY;
        }
        let index = ((loud.len() - 1) as f64 * self.tuning.top_quantile.clamp(0.0, 1.0)).round() as usize;
        // Selecting rather than sorting: there are tens of thousands of
        // levels in the window.
        *loud.select_nth_unstable_by(index, f32::total_cmp).1
    }

    pub(super) fn heights(&self) -> &[f32; SPECTRUM_BANDS] {
        &self.heights
    }

    /// Draws from now on with `tuning`.
    pub(super) fn set_tuning(&mut self, tuning: PillTuning) {
        self.tuning = tuning;
    }

    pub(super) fn tuning(&self) -> PillTuning {
        self.tuning
    }

    /// The range as it stands.
    pub(super) fn range_now(&self) -> PillRange {
        let bands = SPECTRUM_BANDS as f32;
        let floor_db = (0..SPECTRUM_BANDS).map(|band| self.floor(band)).sum::<f32>() / bands;
        let gate_db = (0..SPECTRUM_BANDS).map(|band| self.gate(band)).sum::<f32>() / bands;
        PillRange {
            floor_db,
            gate_db,
            top_db: self.top().max(gate_db + self.tuning.min_span_db as f32),
            loudest_db: self.loudest.0,
            loudest_hz: self.loudest.1,
        }
    }

    /// Lowers every pill at once. The range keeps what it learned about the
    /// room and the voice.
    pub(super) fn clear(&mut self) {
        self.heights = [0.0; SPECTRUM_BANDS];
        self.updated_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Band centres from 100 Hz to 8 kHz, evenly in octaves.
    fn centres() -> [f32; SPECTRUM_BANDS] {
        std::array::from_fn(|band| 100.0 * 80f32.powf(band as f32 / (SPECTRUM_BANDS - 1) as f32))
    }

    fn frame(at: f64, level: impl Fn(usize) -> f32) -> SpectrumFrame {
        SpectrumFrame { at, levels_db: std::array::from_fn(level), centres_hz: centres() }
    }

    /// The room: every band at -80 dB, a little uneven.
    fn room(band: usize) -> f32 {
        -80.0 - (band % 3) as f32
    }

    /// A voice: loud low-mid bands (its fundamental and first formant), a
    /// second formant, and a quiet top, `loudness` dB up or down.
    fn voice(loudness: f32) -> impl Fn(usize) -> f32 {
        move |band| {
            let shape = match band {
                0..=3 => -45.0,
                4..=12 => -30.0,
                13..=19 => -50.0,
                20..=26 => -38.0,
                _ => -60.0,
            };
            shape + loudness
        }
    }

    /// Updates as the overlay does, every 75 ms with the frames heard since
    /// (one every 25 ms), from `start` for `seconds`, hearing `level`, with
    /// no tilt unless `tuning` sets one.
    fn play(levels: &mut PillLevels, start: f64, seconds: f64, level: impl Fn(usize) -> f32) -> f64 {
        let mut at = start;
        let end = start + seconds;
        while at < end - 1e-9 {
            let frames: Vec<SpectrumFrame> = (1..=3).map(|step| frame(at + (0.025 * f64::from(step)), &level)).collect();
            at += 0.075;
            levels.update(&frames, at);
        }
        at
    }

    fn untilted() -> PillLevels {
        let mut levels = PillLevels::new();
        levels.set_tuning(PillTuning { tilt_db_per_octave: 0.0, ..PillTuning::default() });
        levels
    }

    #[test]
    fn the_rooms_noise_keeps_every_pill_down() {
        let mut levels = untilted();
        play(&mut levels, 0.0, 12.0, room);
        assert!(levels.heights().iter().all(|height| *height == 0.0), "{:?}", levels.heights());
    }

    #[test]
    fn a_noisy_bands_single_frame_spikes_stay_down() {
        // The room's narrow low bands jump from frame to frame: one frame in
        // three, every other update, 14 dB up. Averaged over an update, the
        // spike stays under the 12 dB gate.
        let mut levels = untilted();
        let mut at = 0.0;
        for update in 0..200 {
            let frames: Vec<SpectrumFrame> = (1..=3)
                .map(|step| {
                    let spike = if update % 2 == 1 && step == 3 { 14.0 } else { 0.0 };
                    frame(at + (0.025 * f64::from(step)), |_| -85.0 + spike)
                })
                .collect();
            at += 0.075;
            levels.update(&frames, at);
            assert!(levels.heights().iter().all(|height| *height == 0.0), "update {update}: {:?}", levels.heights());
        }
    }

    #[test]
    fn a_voice_raises_the_pills_in_the_shape_of_its_spectrum() {
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 3.0, room);
        play(&mut levels, at, 2.0, voice(0.0));
        let heights = levels.heights();
        // The formants stand above the trough between them, and the trough
        // above the quiet top.
        assert!(heights[8] > heights[16] && heights[23] > heights[16], "{heights:?}");
        assert!(heights[16] > heights[35], "{heights:?}");
        assert!(heights[8] > 0.8, "{heights:?}");
    }

    #[test]
    fn soft_and_loud_voices_fill_the_pills_alike() {
        // 23 dB apart, both with their whole 30 dB range above the room's
        // gate (-68 dB): below the gate there is nothing to show.
        let mut soft = untilted();
        let at = play(&mut soft, 0.0, 3.0, room);
        play(&mut soft, at, 4.0, voice(-8.0));
        let mut loud = untilted();
        let at = play(&mut loud, 0.0, 3.0, room);
        play(&mut loud, at, 4.0, voice(15.0));
        for (soft, loud) in soft.heights().iter().zip(loud.heights()) {
            assert!((soft - loud).abs() < 0.15, "{:?} {:?}", soft, loud);
        }
    }

    #[test]
    fn a_pill_rises_at_once() {
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 3.0, room);
        levels.update(&[frame(at + 0.025, voice(0.0))], at + 0.075);
        assert!(levels.heights()[8] > 0.8, "{:?}", levels.heights());
    }

    #[test]
    fn a_pill_falls_smoothly_when_the_voice_stops() {
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 3.0, room);
        let at = play(&mut levels, at, 2.0, voice(0.0));
        let before = levels.heights()[8];
        levels.update(&[frame(at + 0.025, room)], at + 0.075);
        // One update later it has fallen by `fall_per_second` × 75 ms, not
        // all the way.
        let fallen = before - levels.heights()[8];
        let expected = (PillTuning::default().fall_per_second * 0.075) as f32;
        assert!((fallen - expected).abs() < 1e-4, "{before} -> {}", levels.heights()[8]);
        play(&mut levels, at + 0.075, 1.0, room);
        assert_eq!(levels.heights()[8], 0.0);
    }

    #[test]
    fn without_audio_the_pills_fall() {
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 3.0, room);
        let at = play(&mut levels, at, 2.0, voice(0.0));
        for step in 1..=20 {
            levels.update(&[], at + (0.075 * f64::from(step)));
        }
        assert!(levels.heights().iter().all(|height| *height == 0.0), "{:?}", levels.heights());
    }

    #[test]
    fn a_steady_hum_settles_back_into_the_floor() {
        // A hum in bands 2 and 3 for 15 s: once the floor window has heard
        // only the hum, it is the floor there.
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 3.0, room);
        play(&mut levels, at, 15.0, |band| if (2..=3).contains(&band) { -40.0 } else { room(band) });
        assert_eq!(levels.heights()[2], 0.0, "{:?}", levels.heights());
    }

    #[test]
    fn digital_silence_does_not_teach_the_range() {
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 12.0, room);
        let at = play(&mut levels, at, 5.0, |_| -140.0);
        play(&mut levels, at, 1.0, room);
        assert!(levels.heights().iter().all(|height| *height == 0.0), "{:?}", levels.heights());
    }

    #[test]
    fn the_rooms_noise_before_the_first_word_stays_down_from_the_start() {
        // A noisier room (-66 dB a band, as an office reads in the high
        // bands) from the first frame on.
        let mut levels = untilted();
        play(&mut levels, 0.0, 1.0, |band| room(band) + 14.0);
        assert!(levels.heights().iter().all(|height| *height == 0.0), "{:?}", levels.heights());
    }

    #[test]
    fn a_voice_that_starts_with_the_recording_shows_after_its_first_gap() {
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 0.3, voice(0.0));
        let at = play(&mut levels, at, 0.15, room);
        play(&mut levels, at, 0.3, voice(0.0));
        assert!(levels.heights()[8] > 0.5, "{:?}", levels.heights());
    }

    #[test]
    fn late_frames_teach_the_range_but_do_not_show() {
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 3.0, room);
        levels.update(&[frame(at, voice(0.0))], at + 1.0);
        assert!(levels.heights().iter().all(|height| *height == 0.0), "{:?}", levels.heights());
        assert!(levels.range_now().loudest_db > -40.0, "{:?}", levels.range_now());
    }

    #[test]
    fn the_tilt_lifts_the_high_pitches() {
        let mut flat = untilted();
        let at = play(&mut flat, 0.0, 3.0, room);
        play(&mut flat, at, 2.0, voice(0.0));
        let mut tilted = PillLevels::new();
        tilted.set_tuning(PillTuning { tilt_db_per_octave: 6.0, ..PillTuning::default() });
        let at = play(&mut tilted, 0.0, 3.0, room);
        play(&mut tilted, at, 2.0, voice(0.0));
        assert!(tilted.heights()[23] - tilted.heights()[8] > flat.heights()[23] - flat.heights()[8], "{:?} {:?}", flat.heights(), tilted.heights());
    }

    #[test]
    fn a_higher_noise_gate_keeps_more_of_a_quiet_voice_down() {
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 12.0, room);
        levels.set_tuning(PillTuning { tilt_db_per_octave: 0.0, noise_gate_db: 25.0, ..PillTuning::default() });
        play(&mut levels, at, 1.0, voice(-25.0));
        // The quiet top (-85 dB) and trough (-75 dB) stay down; the formants
        // (-55 and -63 dB) clear the -55 dB gate barely or not at all.
        assert_eq!(levels.heights()[35], 0.0);
        assert_eq!(levels.heights()[16], 0.0);
        assert!(levels.heights()[8] < 0.1, "{:?}", levels.heights());
    }

    #[test]
    fn a_shorter_floor_window_follows_a_louder_room_sooner() {
        let louder = |band: usize| room(band) + 20.0;
        let mut long = untilted();
        let at = play(&mut long, 0.0, 3.0, room);
        play(&mut long, at, 2.0, louder);
        let mut short = PillLevels::new();
        short.set_tuning(PillTuning { tilt_db_per_octave: 0.0, floor_seconds: 1.0, ..PillTuning::default() });
        let at = play(&mut short, 0.0, 3.0, room);
        play(&mut short, at, 2.0, louder);
        assert!(long.heights().iter().any(|height| *height > 0.0), "{:?}", long.heights());
        assert!(short.heights().iter().all(|height| *height == 0.0), "{:?}", short.heights());
    }

    #[test]
    fn a_wider_least_span_keeps_a_quiet_voice_lower() {
        let mut narrow = untilted();
        let at = play(&mut narrow, 0.0, 3.0, room);
        play(&mut narrow, at, 0.3, voice(-30.0));
        let mut wide = PillLevels::new();
        wide.set_tuning(PillTuning { tilt_db_per_octave: 0.0, min_span_db: 60.0, ..PillTuning::default() });
        let at = play(&mut wide, 0.0, 3.0, room);
        play(&mut wide, at, 0.3, voice(-30.0));
        assert!(wide.heights()[8] + 0.1 < narrow.heights()[8], "{:?} {:?}", narrow.heights()[8], wide.heights()[8]);
    }

    #[test]
    fn the_top_follows_recent_loudness_over_its_window() {
        // A shout, then a soft voice: with a 1 s top window the soft voice
        // fills the pills sooner.
        let run = |top_seconds: f64| {
            let mut levels = PillLevels::new();
            levels.set_tuning(PillTuning { tilt_db_per_octave: 0.0, top_seconds, ..PillTuning::default() });
            let at = play(&mut levels, 0.0, 3.0, room);
            let at = play(&mut levels, at, 1.0, voice(20.0));
            play(&mut levels, at, 2.0, voice(-10.0));
            levels.heights()[8]
        };
        assert!(run(1.0) > run(10.0) + 0.2, "{} {}", run(1.0), run(10.0));
    }

    #[test]
    fn a_lower_top_quantile_fills_the_pills_more() {
        let run = |top_quantile: f64| {
            let mut levels = PillLevels::new();
            levels.set_tuning(PillTuning { tilt_db_per_octave: 0.0, top_quantile, ..PillTuning::default() });
            let at = play(&mut levels, 0.0, 3.0, room);
            play(&mut levels, at, 2.0, voice(0.0));
            levels.heights().iter().sum::<f32>()
        };
        assert!(run(0.5) > run(1.0) + 1.0, "{} {}", run(0.5), run(1.0));
    }

    #[test]
    fn clearing_lowers_the_pills_but_keeps_the_learned_range() {
        let mut levels = untilted();
        let at = play(&mut levels, 0.0, 3.0, room);
        play(&mut levels, at, 2.0, voice(0.0));
        let range = levels.range_now();
        levels.clear();
        assert!(levels.heights().iter().all(|height| *height == 0.0));
        assert_eq!(levels.range_now(), range);
    }

    #[test]
    fn the_range_reads_out_in_decibels() {
        let mut levels = untilted();
        play(&mut levels, 0.0, 12.0, |_| -70.0);
        let range = levels.range_now();
        assert!((range.floor_db + 70.0).abs() < 0.01, "{range:?}");
        let gate = -70.0 + PillTuning::default().noise_gate_db as f32;
        assert!((range.gate_db - gate).abs() < 0.01, "{range:?}");
        assert!((range.loudest_db + 70.0).abs() < 0.01, "{range:?}");
    }
}
