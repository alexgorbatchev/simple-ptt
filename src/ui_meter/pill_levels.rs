//! What the pills show: the voice's recent history at one pill per 100 ms,
//! Codex's meter interval. A pill measures the RMS of its audio and is drawn
//! as soon as its time is up, as a lively up-and-down texture, pills
//! alternating tall and short, scaled by that loudness against a range that
//! adapts to the room and the speaker, so soft speech in a quiet office fills
//! the pills as well as loud speech does. Sound within the room's noise rests
//! as a dot. The pills are a picture of sound, not a waveform.
//!
//! The strip moves only while there is sound: a quiet stretch scrolls by as a
//! few dots, then the strip stops until the next sound, so long silences
//! collapse. Each decided pill takes the strip's next slot, and a slot is
//! final before it enters the column and never changes after.

use std::collections::VecDeque;

use super::pill_tuning::PillTuning;

/// How long each pill stands for: Codex's meter interval.
pub(super) const PILL_SECONDS: f64 = 0.1;
/// How long after its time begins a pill is decided at the latest: its own
/// 0.1 s, 50 ms for the last audio block of its time to arrive (a callback
/// block of up to 800 frames at 16 kHz), and one overlay update (75 ms,
/// `STATUS_POLL_INTERVAL_SECONDS`). A pill not decided by then counts as
/// quiet.
pub(super) const LATEST_PAINT_SECONDS: f64 = PILL_SECONDS + 0.05 + 0.075;
/// How long a painted slot takes to reach the screen: the paint is committed
/// with the overlay's update, within a display frame or two.
const SCREEN_LAG_SECONDS: f64 = 0.03;
/// How long after its time begins, on the strip's clock, a slot enters the
/// column (the strip is drawn this far to the right): after the latest
/// decision of a pill that moving strip assigns to it, and its trip to the
/// screen.
pub(super) const ENTRY_DELAY_SECONDS: f64 = LATEST_PAINT_SECONDS + SCREEN_LAG_SECONDS;

/// The noise floor the range starts with, in amplitude (RMS, -50 dBFS). It is
/// counted as a pill heard at the start, so it holds the floor below a voice
/// that begins before the room was heard, until it leaves the floor window.
const INITIAL_FLOOR: f32 = 0.003_162_278;
/// Pills quieter than this (-90 dBFS RMS) are digital silence: a device
/// delivering zeros, which says nothing about the room.
const SILENCE_RMS: f32 = 3.162_278e-5;

/// The span of loudness, in amplitude (RMS, 1 is full scale), that the pills
/// map linearly from rest to full height: from the noise gate above the noise
/// floor to a quantile of the recent pills above the gate (`PillTuning`). The
/// floor is the quietest recent pill: at 100 ms a pill, the gaps between
/// words reach the room's noise, so it stays there however long someone
/// talks, and follows the room when it gets louder.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LoudnessRange {
    /// The loudness of the recent pills, oldest first.
    recent: VecDeque<f32>,
    /// The recent pills' loudness above the gate, oldest first.
    loud: VecDeque<f32>,
}

impl Default for LoudnessRange {
    fn default() -> Self {
        Self { recent: VecDeque::from([INITIAL_FLOOR]), loud: VecDeque::new() }
    }
}

impl LoudnessRange {
    /// Takes in a pill's loudness: the noise floor follows it, and the top
    /// follows it when it is above the gate.
    fn hear(&mut self, rms: f32, tuning: &PillTuning) {
        self.recent.push_back(rms);
        while self.recent.len() > tuning.floor_pills() {
            self.recent.pop_front();
        }
        if rms > self.gate(tuning) {
            self.loud.push_back(rms);
            while self.loud.len() > tuning.top_pills() {
                self.loud.pop_front();
            }
        }
    }

    /// Where `rms` falls in the range: 0 at or below the gate, 1 at or above
    /// the top, linear in amplitude between.
    fn height(&self, rms: f32, tuning: &PillTuning) -> f32 {
        let gate = self.gate(tuning);
        ((rms - gate) / (self.top(tuning) - gate)).clamp(0.0, 1.0)
    }

    fn floor(&self) -> f32 {
        self.recent.iter().copied().fold(f32::INFINITY, f32::min)
    }

    fn gate(&self, tuning: &PillTuning) -> f32 {
        self.floor() * tuning.gate_ratio()
    }

    fn top(&self, tuning: &PillTuning) -> f32 {
        let least = self.gate(tuning) * tuning.min_span_ratio();
        let mut loud: Vec<f32> = self.loud.iter().copied().collect();
        if loud.is_empty() {
            return least;
        }
        loud.sort_by(f32::total_cmp);
        let index = ((loud.len() - 1) as f64 * tuning.top_quantile.clamp(0.0, 1.0)).round() as usize;
        loud[index].max(least)
    }
}

/// The range as it stands, in dBFS, for debug mode's readout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillRange {
    pub floor_db: f32,
    pub gate_db: f32,
    pub top_db: f32,
}

fn decibels(amplitude: f32) -> f32 {
    20.0 * amplitude.max(1e-9).log10()
}

/// Slot `slot`'s place in the texture: even slots in the tall band, odd
/// slots in the short one, at a height within it that is random but the same
/// every time for the same slot.
fn texture(slot: u64, tuning: &PillTuning) -> f32 {
    let (low, high) = if slot % 2 == 0 {
        (tuning.tall_low, tuning.tall_high)
    } else {
        (tuning.short_low, tuning.short_high)
    };
    (low + ((high - low) * f64::from(unit_hash(slot)))) as f32
}

/// A number in [0, 1) that looks random but depends only on `value`
/// (SplitMix64's finalizer).
fn unit_hash(value: u64) -> f32 {
    let mut hash = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    hash ^= hash >> 31;
    (hash >> 40) as f32 / (1u64 << 24) as f32
}

/// One block of captured audio, from `start` to `end` on the recording's
/// clock, and its mean square (its RMS squared, 1 at full scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LevelSpan {
    pub(super) start: f64,
    pub(super) end: f64,
    pub(super) mean_square: f32,
}

/// A small tolerance for the clock arithmetic of adjacent blocks.
const EPSILON: f64 = 1e-6;

/// What was heard in a pill's time.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PillLoudness {
    /// Its audio is not all in yet.
    Pending,
    /// The microphone delivered nothing in its time, or digital silence:
    /// that says nothing about the room.
    NoAudio,
    /// The RMS of its audio.
    Heard(f32),
}

/// The loudness of the pill from `start` to `end`: the RMS of the blocks
/// whose middle falls in its time, weighted by their length. Pending until a
/// block reaches its end.
fn pill_loudness((start, end): (f64, f64), levels: &[LevelSpan]) -> PillLoudness {
    if !levels.iter().any(|level| level.end >= end - EPSILON) {
        return PillLoudness::Pending;
    }
    let (energy, seconds) = levels
        .iter()
        .filter(|level| (start..end).contains(&((level.start + level.end) / 2.0)))
        .fold((0.0f64, 0.0f64), |(energy, seconds), level| {
            let length = level.end - level.start;
            (energy + (f64::from(level.mean_square) * length), seconds + length)
        });
    if seconds <= 0.0 {
        return PillLoudness::NoAudio;
    }
    let rms = (energy / seconds).sqrt() as f32;
    if rms < SILENCE_RMS { PillLoudness::NoAudio } else { PillLoudness::Heard(rms) }
}

/// The pills, and the range they are drawn from. Pill `k` stands for the
/// time from `k × PILL_SECONDS` to `(k + 1) × PILL_SECONDS` on the recording's
/// clock (seconds since it started). Once decided, a pill takes the strip's
/// next slot while the strip moves; slot `s` sits at `s × PILL_SECONDS` on
/// the strip's own clock, which runs only while the strip moves.
#[derive(Debug)]
pub(super) struct PillLevels {
    range: LoudnessRange,
    tuning: PillTuning,
    /// The pill whose time runs now.
    open_index: u64,
    /// Closed pills not decided yet, oldest first.
    pending: VecDeque<u64>,
    /// The heights of the newest slots, oldest first: slot `first_slot + i`
    /// holds `slots[i]`.
    slots: VecDeque<f32>,
    first_slot: u64,
    /// How many slots to keep.
    capacity: usize,
    /// Whether the strip moves: there was sound within the last
    /// `PillTuning::pause_after_seconds` of pills.
    moving: bool,
    /// Quiet pills since the last pill with sound.
    quiet_run: usize,
}

impl PillLevels {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            range: LoudnessRange::default(),
            tuning: PillTuning::default(),
            open_index: 0,
            pending: VecDeque::new(),
            slots: VecDeque::new(),
            first_slot: 0,
            capacity,
            moving: false,
            quiet_run: 0,
        }
    }

    /// Moves to `at` on the recording's clock: the pills whose time is up
    /// wait to be decided.
    pub(super) fn advance(&mut self, at: f64) {
        let index = (at.max(0.0) / PILL_SECONDS).floor() as u64;
        if index > self.open_index {
            self.pending.extend(self.open_index..index);
            self.open_index = index;
        }
    }

    /// Decides the closed pills whose audio is in, oldest first, `at` on the
    /// recording's clock, while the strip's clock reads `strip_at`. A pill
    /// past its latest decision time counts as quiet, heard or not. A pill
    /// with sound starts the strip moving; a quiet one scrolls by as a dot
    /// until `pause_after_pills` of them in a row stop it.
    pub(super) fn paint(&mut self, levels: &[LevelSpan], at: f64, strip_at: f64) {
        while let Some(pill) = self.pending.front().copied() {
            let span = (pill as f64 * PILL_SECONDS, (pill + 1) as f64 * PILL_SECONDS);
            let late = at >= span.0 + LATEST_PAINT_SECONDS;
            let loudness = match pill_loudness(span, levels) {
                PillLoudness::NoAudio => 0.0,
                PillLoudness::Heard(rms) => {
                    // The range still learns from a pill heard late.
                    let loudness = self.hear(rms);
                    if late { 0.0 } else { loudness }
                }
                PillLoudness::Pending if late => 0.0,
                PillLoudness::Pending => break,
            };
            self.pending.pop_front();
            if loudness > 0.0 {
                self.moving = true;
                self.quiet_run = 0;
            } else if self.moving {
                self.quiet_run += 1;
                if self.quiet_run > self.tuning.pause_after_pills() {
                    self.moving = false;
                }
            }
            if self.moving {
                self.push_slot(loudness, strip_at);
            }
        }
    }

    /// Takes in a pill's RMS, and returns its loudness in the range: 0
    /// within the room's noise.
    fn hear(&mut self, rms: f32) -> f32 {
        self.range.hear(rms, &self.tuning);
        self.range.height(rms, &self.tuning)
    }

    /// Puts a pill of `loudness` in the strip's next slot that is not in view
    /// yet, `strip_at` on the strip's clock: a slot the strip has already
    /// brought into view rests as a dot rather than change there.
    fn push_slot(&mut self, loudness: f32, strip_at: f64) {
        let first_unseen = ((strip_at + SCREEN_LAG_SECONDS - ENTRY_DELAY_SECONDS) / PILL_SECONDS).floor() + 1.0;
        let slot = self.next_slot().max(first_unseen.max(0.0) as u64);
        while self.next_slot() < slot {
            self.slots.push_back(0.0);
        }
        let height = if loudness > 0.0 {
            let share = self.tuning.loudness_share.clamp(0.0, 1.0) as f32;
            texture(slot, &self.tuning) * ((1.0 - share) + (share * loudness))
        } else {
            0.0
        };
        self.slots.push_back(height);
        while self.slots.len() > self.capacity {
            self.slots.pop_front();
            self.first_slot += 1;
        }
    }

    /// The slot the next pill takes, unless it is already in view.
    fn next_slot(&self) -> u64 {
        self.first_slot + self.slots.len() as u64
    }

    /// Whether the strip moves.
    pub(super) fn moving(&self) -> bool {
        self.moving
    }

    /// The heights of the `count` slots up to `last`, oldest first: `None`
    /// for a slot never filled, or no longer kept, which shows no pill, so
    /// the strip is empty before the first sound.
    pub(super) fn slot_heights(&self, last: u64, count: usize) -> Vec<Option<f32>> {
        (0..count as u64)
            .rev()
            .map(|back| {
                last.checked_sub(back)
                    .and_then(|slot| slot.checked_sub(self.first_slot))
                    .and_then(|offset| self.slots.get(offset as usize).copied())
            })
            .collect()
    }

    /// Draws the pills decided from now on with `tuning`; those in slots
    /// keep their height.
    pub(super) fn set_tuning(&mut self, tuning: PillTuning) {
        self.tuning = tuning;
    }

    pub(super) fn tuning(&self) -> PillTuning {
        self.tuning
    }

    /// The range the next pill is drawn from.
    pub(super) fn range_now(&self) -> PillRange {
        PillRange {
            floor_db: decibels(self.range.floor()),
            gate_db: decibels(self.range.gate(&self.tuning)),
            top_db: decibels(self.range.top(&self.tuning)),
        }
    }

    /// Empties the strip and restarts both clocks. The range keeps what it
    /// learned about the room and the voice.
    pub(super) fn clear(&mut self) {
        self.open_index = 0;
        self.pending.clear();
        self.slots.clear();
        self.first_slot = 0;
        self.moving = false;
        self.quiet_run = 0;
    }

    /// Keeps the newest `count` slots.
    pub(super) fn fit(&mut self, count: usize) {
        self.capacity = count;
        while self.slots.len() > self.capacity {
            self.slots.pop_front();
            self.first_slot += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loudness `db` dBFS as an amplitude.
    fn amplitude(db: f32) -> f32 {
        10f32.powf(db / 20.0)
    }

    /// A voice from pill 0 on: one level block per pill.
    struct Voice {
        levels: Vec<LevelSpan>,
        /// Pills so far, with or without audio.
        pills: usize,
    }

    impl Voice {
        fn new() -> Self {
            Self { levels: Vec::new(), pills: 0 }
        }

        /// The next `pills` pills at an RMS of `db(n)` dBFS for the `n`th of
        /// them.
        fn say(&mut self, pills: usize, db: impl Fn(usize) -> f32) -> &mut Self {
            for pill in 0..pills {
                let (start, end) = (self.pills as f64 * PILL_SECONDS, (self.pills + 1) as f64 * PILL_SECONDS);
                self.levels.push(LevelSpan { start, end, mean_square: amplitude(db(pill)).powi(2) });
                self.pills += 1;
            }
            self
        }

        /// The next `pills` pills with no audio: the microphone delivered
        /// nothing.
        fn pause(&mut self, pills: usize) -> &mut Self {
            self.pills += pills;
            self
        }

        /// `capacity` slots played from the whole voice.
        fn played(&self, capacity: usize) -> Played {
            self.played_with(capacity, PillTuning::default())
        }

        /// `capacity` slots played from the whole voice with `tuning`.
        fn played_with(&self, capacity: usize, tuning: PillTuning) -> Played {
            let mut played = Played::new(capacity);
            played.levels.set_tuning(tuning);
            played.play(self);
            played
        }
    }

    /// Pills played as the overlay's updates play them: the strip's clock
    /// runs only while the strip moves.
    struct Played {
        levels: PillLevels,
        /// Updates so far, and those the strip moved through.
        updates: u64,
        moved: u64,
        /// Whether the strip moved, before each update.
        moves: Vec<bool>,
    }

    impl Played {
        fn new(capacity: usize) -> Self {
            Self { levels: PillLevels::new(capacity), updates: 0, moved: 0, moves: Vec::new() }
        }

        /// Plays `voice` from where it stands to the voice's end, an update
        /// as each pill closes.
        fn play(&mut self, voice: &Voice) {
            while self.updates < voice.pills as u64 {
                self.moves.push(self.levels.moving());
                if self.levels.moving() {
                    self.moved += 1;
                }
                self.updates += 1;
                let at = self.updates as f64 * PILL_SECONDS;
                self.levels.advance(at);
                self.levels.paint(&voice.levels, at, self.moved as f64 * PILL_SECONDS);
            }
        }

        /// Every slot filled so far, oldest first.
        fn slots(&self) -> Vec<f32> {
            self.levels.slots.iter().copied().collect()
        }

        /// The newest `count` slots filled.
        fn newest(&self, count: usize) -> Vec<f32> {
            let slots = self.slots();
            slots[slots.len() - count..].to_vec()
        }
    }

    /// Office background noise, a little uneven.
    fn office_noise(pill: usize) -> f32 {
        if pill % 3 == 0 { -55.0 } else { -58.0 }
    }

    /// Soft speech, syllable by syllable: -46 to -37 dBFS RMS.
    fn soft_speech(pill: usize) -> f32 {
        [-40.0, -37.0, -43.0, -39.0, -46.0, -38.0, -41.0][pill % 7]
    }

    /// Soft speech with a gap between words every 12 pills, at the room's
    /// noise.
    fn talking(pill: usize) -> f32 {
        if pill % 12 == 11 { office_noise(pill) } else { soft_speech(pill) }
    }

    fn mean(heights: &[f32]) -> f32 {
        heights.iter().sum::<f32>() / heights.len() as f32
    }

    #[test]
    fn the_strip_stays_still_and_empty_through_the_rooms_noise() {
        let mut voice = Voice::new();
        voice.say(100, office_noise);
        let played = voice.played(152);
        assert!(played.slots().is_empty(), "{:?}", played.slots());
        assert!(played.moves.iter().all(|moving| !moving));
    }

    #[test]
    fn the_strip_moves_from_the_first_sound() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(10, soft_speech);
        let played = voice.played(152);
        assert_eq!(played.moves.iter().position(|moving| *moving), Some(21));
        assert_eq!(played.slots().len(), 10);
        assert!(played.slots().iter().all(|height| *height > 0.0), "{:?}", played.slots());
    }

    #[test]
    fn gaps_between_words_scroll_by_as_dots() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(5, soft_speech).say(3, office_noise).say(5, soft_speech);
        let played = voice.played(152);
        let slots = played.slots();
        assert_eq!(slots.len(), 13, "{slots:?}");
        assert_eq!(slots[5..8], [0.0, 0.0, 0.0], "{slots:?}");
        assert!(slots[..5].iter().chain(&slots[8..]).all(|height| *height > 0.0), "{slots:?}");
    }

    #[test]
    fn a_longer_quiet_stops_the_strip_after_a_few_dots() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(5, soft_speech).say(30, office_noise).say(5, soft_speech);
        let played = voice.played(152);
        // Four dots, then the strip stops until the next sound.
        let slots = played.slots();
        assert_eq!(slots.len(), 14, "{slots:?}");
        assert_eq!(slots[5..9], [0.0; 4], "{slots:?}");
        assert!(slots[9..].iter().all(|height| *height > 0.0), "{slots:?}");
        let still = played.moves[30..50].iter().filter(|moving| !**moving).count();
        assert_eq!(still, 20, "{:?}", played.moves);
    }

    #[test]
    fn the_quiet_before_the_strip_stops_is_tunable() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(5, soft_speech).say(30, office_noise).say(5, soft_speech);
        let played = voice.played_with(152, PillTuning { pause_after_seconds: 1.0, ..PillTuning::default() });
        assert_eq!(played.slots().len(), 20, "{:?}", played.slots());
    }

    #[test]
    fn a_slot_already_in_view_rests_as_a_dot() {
        let mut levels = PillLevels::new(16);
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0);
        levels.advance(PILL_SECONDS);
        // The strip has run on to where slot 2 is already in view.
        let strip_at = (2.0 * PILL_SECONDS) + ENTRY_DELAY_SECONDS;
        levels.paint(&voice.levels, PILL_SECONDS, strip_at);
        assert_eq!(levels.slot_heights(3, 4)[..3], [Some(0.0); 3]);
        assert!(levels.slot_heights(3, 4)[3].is_some_and(|height| height > 0.0), "{:?}", levels.slot_heights(3, 4));
    }

    #[test]
    fn a_steady_voice_goes_up_and_down_from_pill_to_pill() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(80, |_| -35.0);
        let heights = voice.played(152).newest(60);
        let apart = heights.windows(2).filter(|pair| (pair[0] - pair[1]).abs() >= 0.1).count();
        assert!(apart * 10 >= 59 * 9, "{apart} of 59: {heights:?}");
    }

    #[test]
    fn speech_varies_instead_of_pinning_at_full_height() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(140, talking);
        let heights = voice.played(152).newest(100);
        let full = heights.iter().filter(|height| **height >= 0.95).count();
        assert!(full <= 10, "{full} of 100 at full height: {heights:?}");
        let mut sorted = heights.clone();
        sorted.sort_by(f32::total_cmp);
        assert!(sorted[50] < 0.8, "median {}", sorted[50]);
    }

    #[test]
    fn a_louder_syllable_is_drawn_taller() {
        let mut steady = Voice::new();
        steady.say(20, office_noise).say(35, soft_speech);
        let mut louder = Voice::new();
        // The 31st speech pill, -43 dBFS in `steady`, is -38 here.
        louder.say(20, office_noise).say(30, soft_speech).say(1, |_| -38.0).say(4, |pill| soft_speech(pill + 31));
        let (steady, louder) = (steady.played(152).slots(), louder.played(152).slots());
        assert!(louder[30] > steady[30] + 0.05, "{} {}", louder[30], steady[30]);
    }

    #[test]
    fn soft_and_loud_speech_fill_the_pills_alike() {
        let mut soft = Voice::new();
        soft.say(20, office_noise).say(120, talking);
        let mut loud = Voice::new();
        loud.say(20, office_noise).say(120, |pill| talking(pill) + 20.0);
        let soft_heights = soft.played(152).newest(42);
        let loud_heights = loud.played(152).newest(42);
        assert!((mean(&soft_heights) - mean(&loud_heights)).abs() < 0.1, "{soft_heights:?} {loud_heights:?}");
    }

    #[test]
    fn one_loud_word_does_not_shrink_the_speech_around_it() {
        let mut steady = Voice::new();
        steady.say(20, office_noise).say(140, talking);
        let mut shout = Voice::new();
        shout.say(20, office_noise).say(70, talking).say(1, |_| -20.0).say(69, |pill| talking(pill + 71));
        let steady_heights = steady.played(152).newest(28);
        let shout_heights = shout.played(152).newest(28);
        for (steady_height, shout_height) in steady_heights.iter().zip(&shout_heights) {
            assert!((steady_height - shout_height).abs() < 0.1, "{steady_heights:?} {shout_heights:?}");
        }
    }

    #[test]
    fn a_long_talk_keeps_the_noise_floor_at_the_room() {
        // 30 s of soft speech: the gaps between words keep the floor at the
        // room's noise, so the speech keeps showing.
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(300, talking);
        let played = voice.played(152);
        assert!(played.levels.range_now().floor_db < -54.0, "{:?}", played.levels.range_now());
        let spoken = played.newest(22);
        let raised = spoken.iter().filter(|height| **height > 0.0).count();
        assert!(raised >= 19, "{spoken:?}");
    }

    #[test]
    fn a_room_that_gets_louder_settles_back_to_stillness() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(120, |pill| office_noise(pill) + 10.0);
        let played = voice.played(152);
        // It moves until the 10 s floor window takes in the louder room.
        assert!(!played.levels.moving());
        assert!(played.moves[130..].iter().all(|moving| !moving), "{:?}", played.moves);
    }

    #[test]
    fn a_voice_above_the_starting_floor_shows_from_the_first_moment() {
        // Before the room is heard, the floor is -50 dBFS.
        let mut voice = Voice::new();
        voice.say(3, soft_speech);
        let slots = voice.played(152).slots();
        assert_eq!(slots.len(), 3);
        assert!(slots.iter().all(|height| *height >= 0.1), "{slots:?}");
    }

    #[test]
    fn a_stretch_without_audio_does_not_teach_the_range() {
        // The microphone delivers nothing for 2 s: that is not the room going
        // silent, so the room's noise after it still does not move the strip.
        let mut voice = Voice::new();
        voice.say(20, office_noise).pause(20).say(20, office_noise);
        let played = voice.played(152);
        assert!(played.slots().is_empty(), "{:?}", played.slots());
    }

    #[test]
    fn digital_silence_does_not_teach_the_range() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(20, |_| -100.0).say(20, office_noise);
        let played = voice.played(152);
        assert!(played.slots().is_empty(), "{:?}", played.slots());
    }

    #[test]
    fn a_pill_is_decided_as_soon_as_its_audio_is_in() {
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0);
        let mut levels = PillLevels::new(8);
        levels.advance(PILL_SECONDS + 0.01);
        levels.paint(&[], PILL_SECONDS + 0.01, 0.0);
        assert!(!levels.moving());
        levels.paint(&voice.levels, PILL_SECONDS + 0.02, 0.0);
        assert!(levels.moving());
        assert!(levels.slot_heights(0, 1)[0].is_some_and(|height| height > 0.0), "{:?}", levels.slot_heights(0, 1));
    }

    #[test]
    fn a_pill_whose_audio_is_late_counts_as_quiet() {
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0);
        let mut levels = PillLevels::new(8);
        levels.advance(LATEST_PAINT_SECONDS);
        levels.paint(&[], LATEST_PAINT_SECONDS, 0.0);
        assert_ne!(levels.pending.front(), Some(&0), "pill 0 stopped waiting");
        levels.paint(&voice.levels, LATEST_PAINT_SECONDS + 0.05, 0.0);
        assert!(!levels.moving());
        assert!(levels.slots.is_empty());
    }

    #[test]
    fn a_pill_heard_but_not_decided_by_its_latest_time_counts_as_quiet() {
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0);
        let mut levels = PillLevels::new(8);
        levels.advance(LATEST_PAINT_SECONDS + 0.01);
        levels.paint(&voice.levels, LATEST_PAINT_SECONDS + 0.01, 0.0);
        assert!(!levels.moving());
    }

    #[test]
    fn slots_already_filled_keep_their_height() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(60, soft_speech);
        let mut played = Played::new(152);
        played.play(&voice);
        let filled = played.slots();
        voice.say(20, |pill| soft_speech(pill) + 15.0);
        played.play(&voice);
        assert_eq!(played.slots()[..filled.len()], filled[..]);
    }

    #[test]
    fn a_pills_loudness_is_the_rms_of_the_audio_in_its_time() {
        let block = |start: f64, end: f64, rms: f32| LevelSpan { start, end, mean_square: rms * rms };
        // Pending until a block reaches the pill's end.
        assert_eq!(pill_loudness((0.2, 0.3), &[block(0.2, 0.25, 0.1)]), PillLoudness::Pending);
        // Blocks weighted by their length: 0.1 for 0.05 s, 0.2 for 0.05 s.
        let PillLoudness::Heard(rms) = pill_loudness((0.2, 0.3), &[block(0.2, 0.25, 0.1), block(0.25, 0.3, 0.2)]) else {
            panic!("the pill's audio is in");
        };
        assert!((rms - 0.025f32.sqrt()).abs() < 1e-6, "{rms}");
        // A block belongs to the pill its middle falls in: none falls in this one.
        assert_eq!(pill_loudness((0.2, 0.3), &[block(0.15, 0.22, 0.5), block(0.3, 0.4, 0.5)]), PillLoudness::NoAudio);
    }

    #[test]
    fn a_higher_noise_gate_rests_more_of_the_quiet_speech_as_dots() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(70, soft_speech);
        let raised = |played: &Played| played.slots().iter().filter(|height| **height > 0.0).count();
        // A gate 19.5 dB above the -58 dBFS room (-38.5 dBFS) leaves only the
        // two loudest syllables, -37 and -38 dBFS: two in seven.
        let strict = voice.played_with(152, PillTuning { noise_gate_db: 19.5, ..PillTuning::default() });
        assert_eq!(raised(&voice.played(152)), 70);
        assert_eq!(raised(&strict), 20);
    }

    #[test]
    fn a_shorter_floor_window_follows_a_louder_room_sooner() {
        // The room gets 13 dB louder for 2 s, then soft speech at -40 dBFS.
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(20, |_| -45.0).say(20, |_| -40.0);
        let short = voice.played_with(152, PillTuning { floor_seconds: 1.0, ..PillTuning::default() });
        // Over 10 s the floor is still the quiet room, so the speech shows;
        // over 1 s it is the louder room, and the speech is within its gate:
        // the strip stops.
        let long = voice.played(152);
        assert!(long.levels.moving());
        assert!(long.newest(5).iter().all(|height| *height > 0.0), "{:?}", long.newest(5));
        assert!(!short.levels.moving());
        assert!(short.moves[50..].iter().all(|moving| !moving), "{:?}", short.moves);
    }

    #[test]
    fn a_shorter_top_window_lets_soft_speech_fill_the_pills_sooner_after_loud() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(20, |_| -20.0).say(40, |_| -40.0);
        let short = voice.played_with(152, PillTuning { top_seconds: 1.0, ..PillTuning::default() });
        let long = voice.played(152);
        assert!(mean(&short.newest(20)) > mean(&long.newest(20)) + 0.2, "{:?} {:?}", short.newest(20), long.newest(20));
    }

    #[test]
    fn a_lower_top_quantile_draws_more_speech_at_full_loudness() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(80, soft_speech);
        let low = voice.played_with(152, PillTuning { top_quantile: 0.5, ..PillTuning::default() });
        let high = voice.played_with(152, PillTuning { top_quantile: 1.0, ..PillTuning::default() });
        assert!(mean(&low.newest(40)) > mean(&high.newest(40)) + 0.05);
    }

    #[test]
    fn a_wider_least_span_keeps_quiet_speech_low() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(40, |_| -45.0);
        let wide = voice.played_with(152, PillTuning { min_span_db: 30.0, ..PillTuning::default() });
        assert!(mean(&wide.newest(20)) + 0.1 < mean(&voice.played(152).newest(20)));
    }

    #[test]
    fn the_texture_takes_the_tuned_bands_and_loudness_share() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(40, soft_speech);
        let tuning = PillTuning {
            loudness_share: 0.0,
            tall_low: 0.8,
            tall_high: 0.8,
            short_low: 0.3,
            short_high: 0.3,
            ..PillTuning::default()
        };
        let played = voice.played_with(152, tuning);
        for (slot, height) in played.slots().iter().enumerate() {
            let expected = if slot % 2 == 0 { 0.8 } else { 0.3 };
            assert!((height - expected).abs() < 1e-6, "slot {slot}: {height}");
        }
    }

    #[test]
    fn the_range_reads_out_in_decibels() {
        let mut voice = Voice::new();
        voice.say(120, |_| -58.0).say(20, |_| -30.0);
        let range = voice.played(152).levels.range_now();
        assert!((range.floor_db + 58.0).abs() < 0.01, "{range:?}");
        assert!((range.gate_db + 52.0).abs() < 0.01, "{range:?}");
        assert!((range.top_db + 30.0).abs() < 0.01, "{range:?}");
    }

    #[test]
    fn slot_heights_read_the_kept_slots_and_leave_the_rest_empty() {
        let mut levels = PillLevels::new(3);
        levels.first_slot = 4;
        levels.slots = VecDeque::from([0.1, 0.0, 0.3]);
        // A slot filled with a dot is a dot; one never filled, or no longer
        // kept, has no pill.
        assert_eq!(levels.slot_heights(7, 5), vec![None, Some(0.1), Some(0.0), Some(0.3), None]);
        assert_eq!(levels.slot_heights(1, 3), vec![None, None, None]);
    }

    #[test]
    fn fitting_keeps_the_newest_slots() {
        let mut levels = PillLevels::new(5);
        levels.slots = VecDeque::from([0.1, 0.2, 0.3, 0.4]);
        levels.fit(2);
        assert_eq!(levels.slot_heights(3, 4), vec![None, None, Some(0.3), Some(0.4)]);
    }

    #[test]
    fn clearing_empties_the_strip_but_keeps_the_learned_range() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(40, soft_speech);
        let mut played = voice.played(8);
        let learned = played.levels.range.clone();
        played.levels.clear();
        assert!(played.levels.slots.is_empty());
        assert_eq!(played.levels.range, learned);
        assert!(!played.levels.moving());
        assert!(played.levels.pending.is_empty());
    }
}
