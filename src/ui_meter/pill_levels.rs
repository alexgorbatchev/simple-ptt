//! What the pills show: the voice's recent history at one pill per 100 ms.
//! Like Codex's voice meter, each pill measures the loudest sample of its
//! time, and a pill is drawn as soon as its time is up. It is drawn as a
//! lively up-and-down texture, pills alternating tall and short, scaled by
//! that loudness against a range that adapts to the room and the speaker, so
//! soft speech in a quiet office fills the pills as well as loud speech does.
//! Sound within the room's noise rests as a dot. The pills are a picture of
//! sound, not a waveform. A pill is final before it enters the column, and
//! never changes after.

use std::collections::VecDeque;

use super::pill_tuning::PillTuning;

/// How long each pill stands for: Codex's meter interval.
pub(super) const PILL_SECONDS: f64 = 0.1;
/// How long after its time begins a pill is painted at the latest: its own
/// 0.1 s, 50 ms for the last audio block of its time to arrive (a callback
/// block of up to 800 frames at 16 kHz), and one overlay update (75 ms,
/// `STATUS_POLL_INTERVAL_SECONDS`). A pill not painted by then rests as a dot
/// for good.
pub(super) const LATEST_PAINT_SECONDS: f64 = PILL_SECONDS + 0.05 + 0.075;
/// How long after its time begins a pill enters the column (the strip is
/// drawn this far to the right): just after its latest paint, with 30 ms for
/// the paint to reach the screen, so the height a pill enters with is already
/// on screen.
pub(super) const ENTRY_DELAY_SECONDS: f64 = LATEST_PAINT_SECONDS + 0.03;

/// The noise floor the range starts with, in amplitude: Codex's meter's
/// noise floor, a peak of 512 in 65535 (-42 dBFS). It is counted as a pill
/// heard at the start, so it holds the floor below a voice that begins before
/// the room was heard, until it leaves the floor window.
const INITIAL_FLOOR: f32 = 512.0 / 65535.0;

/// The span of loudness, in amplitude (a sample's peak, 1 is full scale),
/// that the pills map linearly from rest to full height: from the noise gate
/// above the noise floor to a quantile of the recent pills above the gate
/// (`PillTuning`). The floor is the quietest recent pill: at 100 ms a pill,
/// the gaps between words reach the room's noise, so it stays there however
/// long someone talks, and follows the room when it gets louder.
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
    fn hear(&mut self, peak: f32, tuning: &PillTuning) {
        self.recent.push_back(peak);
        while self.recent.len() > tuning.floor_pills() {
            self.recent.pop_front();
        }
        if peak > self.gate(tuning) {
            self.loud.push_back(peak);
            while self.loud.len() > tuning.top_pills() {
                self.loud.pop_front();
            }
        }
    }

    /// Where `peak` falls in the range: 0 at or below the gate, 1 at or above
    /// the top, linear in amplitude between.
    fn height(&self, peak: f32, tuning: &PillTuning) -> f32 {
        let gate = self.gate(tuning);
        ((peak - gate) / (self.top(tuning) - gate)).clamp(0.0, 1.0)
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

/// Pill `index`'s place in the texture: even pills in the tall band, odd
/// pills in the short one, at a height within it that is random but the same
/// every time for the same pill.
fn texture(index: u64, tuning: &PillTuning) -> f32 {
    let (low, high) = if index % 2 == 0 {
        (tuning.tall_low, tuning.tall_high)
    } else {
        (tuning.short_low, tuning.short_high)
    };
    (low + ((high - low) * f64::from(unit_hash(index)))) as f32
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

/// One block of captured audio, from `start` to `end` on the strip's clock,
/// and its loudest sample (1 is full scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LevelSpan {
    pub(super) start: f64,
    pub(super) end: f64,
    pub(super) peak: f32,
}

/// A small tolerance for the clock arithmetic of adjacent blocks.
const EPSILON: f64 = 1e-6;

/// What was heard in a pill's time.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PillLoudness {
    /// Its audio is not all in yet.
    Pending,
    /// The microphone delivered nothing in its time: that says nothing about
    /// the room.
    NoAudio,
    /// The loudest sample of its audio.
    Heard(f32),
}

/// Peaks below this (-90 dBFS) are digital silence: a device delivering
/// zeros, which says nothing about the room either.
const SILENCE_PEAK: f32 = 3.162_278e-5;

/// The loudness of the pill from `start` to `end`: the loudest sample of the
/// blocks whose middle falls in its time. Pending until a block reaches its
/// end.
fn pill_loudness((start, end): (f64, f64), levels: &[LevelSpan]) -> PillLoudness {
    if !levels.iter().any(|level| level.end >= end - EPSILON) {
        return PillLoudness::Pending;
    }
    levels
        .iter()
        .filter(|level| (start..end).contains(&((level.start + level.end) / 2.0)))
        .map(|level| level.peak)
        .reduce(f32::max)
        .filter(|peak| *peak >= SILENCE_PEAK)
        .map_or(PillLoudness::NoAudio, PillLoudness::Heard)
}

/// The pills' heights, oldest first (newest at the right), and the range
/// they are drawn from. Pill `k` stands for the time from `k × PILL_SECONDS`
/// to `(k + 1) × PILL_SECONDS` on the strip's clock (seconds since the strip
/// started), the clock the strip moves by. A pill rests as a dot until it is
/// painted, which happens before it enters the column.
#[derive(Debug)]
pub(super) struct PillLevels {
    range: LoudnessRange,
    tuning: PillTuning,
    /// Pills `open_index - len + 1` to `open_index`.
    history: Vec<f32>,
    open_index: u64,
    /// Closed pills not painted yet, oldest first.
    pending: VecDeque<u64>,
}

impl PillLevels {
    pub(super) fn new(count: usize) -> Self {
        Self {
            range: LoudnessRange::default(),
            tuning: PillTuning::default(),
            history: vec![0.0; count],
            open_index: 0,
            pending: VecDeque::new(),
        }
    }

    /// Moves to `at` on the strip's clock: the pills whose time is up wait to
    /// be painted.
    pub(super) fn advance(&mut self, at: f64) {
        let index = (at.max(0.0) / PILL_SECONDS).floor() as u64;
        if index <= self.open_index {
            return;
        }
        self.pending.extend(self.open_index..index);
        for _ in 0..(index - self.open_index).min(self.history.len() as u64) {
            push_history(&mut self.history, 0.0);
        }
        self.open_index = index;
        // A pill that scrolled out of the history can no longer be painted.
        while self.pending.front().is_some_and(|pill| self.slot(*pill).is_none()) {
            self.pending.pop_front();
        }
    }

    /// Paints the closed pills whose audio is in, oldest first, `at` on the
    /// strip's clock. A pill past its latest paint time rests as a dot for
    /// good, heard or not, so a pill never changes once it can be seen.
    pub(super) fn paint(&mut self, levels: &[LevelSpan], at: f64) {
        while let Some(pill) = self.pending.front().copied() {
            let span = (pill as f64 * PILL_SECONDS, (pill + 1) as f64 * PILL_SECONDS);
            let late = at >= span.0 + LATEST_PAINT_SECONDS;
            let height = match pill_loudness(span, levels) {
                PillLoudness::NoAudio => 0.0,
                PillLoudness::Heard(peak) => {
                    // The range still learns from a pill heard late.
                    let height = self.hear(pill, peak);
                    if late { 0.0 } else { height }
                }
                PillLoudness::Pending if late => 0.0,
                PillLoudness::Pending => break,
            };
            self.pending.pop_front();
            if let Some(slot) = self.slot(pill) {
                self.history[slot] = height;
            }
        }
    }

    /// Takes in pill `index`'s loudness, and returns its height: its texture
    /// scaled by its loudness if it was above the noise, a dot otherwise.
    fn hear(&mut self, index: u64, peak: f32) -> f32 {
        self.range.hear(peak, &self.tuning);
        let loudness = self.range.height(peak, &self.tuning);
        if loudness <= 0.0 {
            return 0.0;
        }
        let share = self.tuning.loudness_share.clamp(0.0, 1.0) as f32;
        texture(index, &self.tuning) * ((1.0 - share) + (share * loudness))
    }

    /// Draws the pills painted from now on with `tuning`; those painted keep
    /// their height.
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

    /// The index of the open pill.
    pub(super) fn open_index(&self) -> u64 {
        self.open_index
    }

    /// Where pill `index` sits in the history, if it is still there.
    fn slot(&self, index: u64) -> Option<usize> {
        let age = usize::try_from(self.open_index.checked_sub(index)?).ok()?;
        self.history.len().checked_sub(age + 1)
    }

    /// Empties the pills and restarts the clock. The range keeps what it
    /// learned about the room and the voice.
    pub(super) fn clear(&mut self) {
        self.history.fill(0.0);
        self.open_index = 0;
        self.pending.clear();
    }

    /// Keeps the newest `count` heights (see `fit_history`).
    pub(super) fn fit(&mut self, count: usize) {
        fit_history(&mut self.history, count);
    }

    pub(super) fn heights(&self) -> &[f32] {
        &self.history
    }
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

        /// The next `pills` pills with a peak of `db(n)` dBFS for the `n`th
        /// of them.
        fn say(&mut self, pills: usize, db: impl Fn(usize) -> f32) -> &mut Self {
            for pill in 0..pills {
                let (start, end) = (self.pills as f64 * PILL_SECONDS, (self.pills + 1) as f64 * PILL_SECONDS);
                self.levels.push(LevelSpan { start, end, peak: amplitude(db(pill)) });
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

        /// Plays the voice into `levels` from its open pill to the voice's
        /// end, painting as each pill closes, as the overlay's updates do.
        fn play(&self, levels: &mut PillLevels) {
            for pill in levels.open_index()..self.pills as u64 {
                let at = (pill + 1) as f64 * PILL_SECONDS;
                levels.advance(at);
                levels.paint(&self.levels, at);
            }
        }

        /// `count` pills painted from the whole voice.
        fn painted(&self, count: usize) -> PillLevels {
            self.painted_with(count, PillTuning::default())
        }

        /// `count` pills painted from the whole voice with `tuning`.
        fn painted_with(&self, count: usize, tuning: PillTuning) -> PillLevels {
            let mut levels = PillLevels::new(count);
            levels.set_tuning(tuning);
            self.play(&mut levels);
            levels
        }
    }

    /// Office background noise, a little uneven.
    fn office_noise(pill: usize) -> f32 {
        if pill % 3 == 0 { -55.0 } else { -58.0 }
    }

    /// Soft speech, syllable by syllable: peaks of -46 to -37 dBFS.
    fn soft_speech(pill: usize) -> f32 {
        [-40.0, -37.0, -43.0, -39.0, -46.0, -38.0, -41.0][pill % 7]
    }

    /// Soft speech with a gap between words every 12 pills, at the room's
    /// noise.
    fn talking(pill: usize) -> f32 {
        if pill % 12 == 11 { office_noise(pill) } else { soft_speech(pill) }
    }

    fn max_of(heights: &[f32]) -> f32 {
        heights.iter().copied().fold(0.0, f32::max)
    }

    fn mean(heights: &[f32]) -> f32 {
        heights.iter().sum::<f32>() / heights.len() as f32
    }

    /// The newest `count` pills painted: all but the open one.
    fn newest(levels: &PillLevels, count: usize) -> Vec<f32> {
        let heights = levels.heights();
        heights[heights.len() - 1 - count..heights.len() - 1].to_vec()
    }

    #[test]
    fn background_noise_rests_as_dots() {
        let mut voice = Voice::new();
        voice.say(100, office_noise);
        assert_eq!(max_of(voice.painted(152).heights()), 0.0);
    }

    #[test]
    fn a_steady_voice_goes_up_and_down_from_pill_to_pill() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(80, |_| -35.0);
        let heights = newest(&voice.painted(152), 60);
        for pair in heights.windows(2) {
            assert!((pair[0] - pair[1]).abs() >= 0.15, "{heights:?}");
        }
        assert!(heights.iter().all(|height| *height > 0.0), "{heights:?}");
    }

    #[test]
    fn speech_varies_instead_of_pinning_at_full_height() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(140, talking);
        let heights = newest(&voice.painted(152), 100);
        let full = heights.iter().filter(|height| **height >= 0.95).count();
        assert!(full <= 20, "{full} of 100 at full height: {heights:?}");
        let mut sorted = heights.clone();
        sorted.sort_by(f32::total_cmp);
        assert!(sorted[50] < 0.8, "median {}", sorted[50]);
    }

    #[test]
    fn a_louder_syllable_is_drawn_taller() {
        let mut steady = Voice::new();
        steady.say(20, office_noise).say(35, soft_speech);
        let mut louder = Voice::new();
        // Pill 50, -43 dBFS in `steady`, is -40 here.
        louder.say(20, office_noise).say(30, soft_speech).say(1, |_| -40.0).say(4, |pill| soft_speech(pill + 31));
        let (steady, louder) = (steady.painted(152), louder.painted(152));
        let pill = |levels: &PillLevels| levels.heights()[levels.slot(50).unwrap()];
        assert!(pill(&louder) > pill(&steady) + 0.05, "{} {}", pill(&louder), pill(&steady));
    }

    #[test]
    fn soft_and_loud_speech_fill_the_pills_alike() {
        let mut soft = Voice::new();
        soft.say(20, office_noise).say(120, talking);
        let mut loud = Voice::new();
        loud.say(20, office_noise).say(120, |pill| talking(pill) + 20.0);
        let soft_heights = newest(&soft.painted(152), 42);
        let loud_heights = newest(&loud.painted(152), 42);
        assert!((mean(&soft_heights) - mean(&loud_heights)).abs() < 0.1, "{soft_heights:?} {loud_heights:?}");
    }

    #[test]
    fn gaps_between_words_rest_as_dots() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(5, soft_speech).say(2, office_noise).say(5, soft_speech);
        let heights = newest(&voice.painted(152), 12);
        assert_eq!(heights[5..7], [0.0, 0.0], "{heights:?}");
        assert!(heights[..5].iter().chain(&heights[7..]).all(|height| *height > 0.0), "{heights:?}");
    }

    #[test]
    fn one_loud_word_does_not_shrink_the_speech_around_it() {
        let mut steady = Voice::new();
        steady.say(20, office_noise).say(140, talking);
        let mut shout = Voice::new();
        shout.say(20, office_noise).say(70, talking).say(1, |_| -20.0).say(69, |pill| talking(pill + 71));
        let steady_heights = newest(&steady.painted(152), 28);
        let shout_heights = newest(&shout.painted(152), 28);
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
        let levels = voice.painted(152);
        assert!(levels.range_now().floor_db < -54.0, "{:?}", levels.range_now());
        let spoken = newest(&levels, 22);
        let raised = spoken.iter().filter(|height| **height > 0.0).count();
        assert!(raised >= 19, "{spoken:?}");
    }

    #[test]
    fn a_room_that_gets_louder_settles_back_to_dots() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(120, |pill| office_noise(pill) + 10.0);
        assert_eq!(max_of(&newest(&voice.painted(152), 16)), 0.0);
    }

    #[test]
    fn a_voice_above_codexs_noise_floor_shows_from_the_first_moment() {
        // Before the room is heard, the floor is Codex's -42 dBFS.
        let mut voice = Voice::new();
        voice.say(3, |pill| soft_speech(pill) + 10.0);
        let heights = newest(&voice.painted(152), 3);
        assert!(heights.iter().all(|height| *height >= 0.1), "{heights:?}");
    }

    #[test]
    fn a_stretch_without_audio_does_not_teach_the_range() {
        // The microphone delivers nothing for 2 s: that is not the room going
        // silent, so the room's noise after it still rests as dots.
        let mut voice = Voice::new();
        voice.say(20, office_noise).pause(20).say(20, office_noise);
        let heights = voice.painted(152);
        assert_eq!(max_of(heights.heights()), 0.0, "{:?}", heights.heights());
    }

    #[test]
    fn digital_silence_does_not_teach_the_range() {
        // A device that delivers zeros (starting, or muted) is not a silent
        // room: the room's noise after it still rests as dots.
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(20, |_| -100.0).say(20, office_noise);
        let heights = voice.painted(152);
        assert_eq!(max_of(heights.heights()), 0.0, "{:?}", heights.heights());
    }

    #[test]
    fn a_pill_is_painted_as_soon_as_its_audio_is_in() {
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0);
        let mut levels = PillLevels::new(8);
        levels.advance(PILL_SECONDS + 0.01);
        levels.paint(&[], PILL_SECONDS + 0.01);
        assert_eq!(max_of(levels.heights()), 0.0);
        levels.paint(&voice.levels, PILL_SECONDS + 0.02);
        assert!(levels.heights()[6] > 0.0, "{:?}", levels.heights());
    }

    #[test]
    fn a_pill_whose_audio_is_late_stays_a_dot() {
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0);
        let mut levels = PillLevels::new(8);
        levels.advance(LATEST_PAINT_SECONDS);
        // Pill 0's latest paint time comes with none of its audio in.
        levels.paint(&[], LATEST_PAINT_SECONDS);
        assert_ne!(levels.pending.front(), Some(&0), "pill 0 stopped waiting");
        // Its audio arrives late: the pill does not change.
        levels.paint(&voice.levels, LATEST_PAINT_SECONDS + 0.05);
        assert_eq!(max_of(levels.heights()), 0.0);
    }

    #[test]
    fn a_pill_heard_but_not_painted_by_its_latest_paint_stays_a_dot() {
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0);
        let mut levels = PillLevels::new(8);
        // The first update after pill 0 closed comes after its latest paint
        // time, with its audio in.
        levels.advance(LATEST_PAINT_SECONDS + 0.01);
        levels.paint(&voice.levels, LATEST_PAINT_SECONDS + 0.01);
        assert_eq!(max_of(levels.heights()), 0.0);
    }

    #[test]
    fn pills_already_painted_keep_their_height() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(60, soft_speech);
        let mut levels = PillLevels::new(152);
        voice.play(&mut levels);
        let painted = levels.heights().to_vec();
        voice.say(20, |pill| soft_speech(pill) + 15.0);
        voice.play(&mut levels);
        // Every pill painted before (all but the one open then) is unchanged.
        assert_eq!(&levels.heights()[..painted.len() - 21], &painted[20..painted.len() - 1]);
    }

    #[test]
    fn a_pills_loudness_is_the_loudest_sample_in_its_time() {
        let block = |start: f64, end: f64, peak: f32| LevelSpan { start, end, peak };
        // Pending until a block reaches the pill's end.
        assert_eq!(pill_loudness((0.2, 0.3), &[block(0.2, 0.25, 0.1)]), PillLoudness::Pending);
        assert_eq!(
            pill_loudness((0.2, 0.3), &[block(0.2, 0.25, 0.1), block(0.25, 0.3, 0.4), block(0.3, 0.35, 0.9)]),
            PillLoudness::Heard(0.4)
        );
        // A block belongs to the pill its middle falls in: none falls in this one.
        assert_eq!(pill_loudness((0.2, 0.3), &[block(0.15, 0.22, 0.5), block(0.3, 0.4, 0.5)]), PillLoudness::NoAudio);
    }

    #[test]
    fn a_higher_noise_gate_rests_more_of_the_quiet_speech_as_dots() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(70, soft_speech);
        let raised = |levels: &PillLevels| newest(levels, 70).iter().filter(|height| **height > 0.0).count();
        // A gate 19.5 dB above the -58 dBFS room (-38.5 dBFS) leaves only the
        // two loudest syllables, -37 and -38 dBFS: two in seven.
        let strict = voice.painted_with(152, PillTuning { noise_gate_db: 19.5, ..PillTuning::default() });
        assert_eq!(raised(&voice.painted(152)), 70);
        assert_eq!(raised(&strict), 20);
    }

    #[test]
    fn a_shorter_floor_window_follows_a_louder_room_sooner() {
        // The room gets 13 dB louder for 2 s, then soft speech at -40 dBFS.
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(20, |_| -45.0).say(20, |_| -40.0);
        let short = voice.painted_with(152, PillTuning { floor_seconds: 1.0, ..PillTuning::default() });
        // Over 10 s the floor is still the quiet room, so the speech shows;
        // over 1 s it is the louder room, and the speech is within its gate.
        assert!(max_of(&newest(&voice.painted(152), 20)) > 0.0);
        assert_eq!(max_of(&newest(&short, 20)), 0.0);
    }

    #[test]
    fn a_shorter_top_window_lets_soft_speech_fill_the_pills_sooner_after_loud() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(20, |_| -20.0).say(40, |_| -40.0);
        let short = voice.painted_with(152, PillTuning { top_seconds: 1.0, ..PillTuning::default() });
        let long = voice.painted(152);
        assert!(mean(&newest(&short, 20)) > mean(&newest(&long, 20)) + 0.2, "{:?} {:?}", newest(&short, 20), newest(&long, 20));
    }

    #[test]
    fn a_lower_top_quantile_draws_more_speech_at_full_loudness() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(80, soft_speech);
        let low = voice.painted_with(152, PillTuning { top_quantile: 0.5, ..PillTuning::default() });
        let high = voice.painted_with(152, PillTuning { top_quantile: 1.0, ..PillTuning::default() });
        assert!(mean(&newest(&low, 40)) > mean(&newest(&high, 40)) + 0.05);
    }

    #[test]
    fn a_wider_least_span_keeps_quiet_speech_low() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(40, |_| -45.0);
        let wide = voice.painted_with(152, PillTuning { min_span_db: 30.0, ..PillTuning::default() });
        assert!(mean(&newest(&wide, 20)) + 0.1 < mean(&newest(&voice.painted(152), 20)));
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
        let levels = voice.painted_with(152, tuning);
        // Pills 20 to 59; 59 is the newest painted one.
        for pill in 20..60u64 {
            let height = levels.heights()[levels.slot(pill).unwrap()];
            let expected = if pill % 2 == 0 { 0.8 } else { 0.3 };
            assert!((height - expected).abs() < 1e-6, "pill {pill}: {height}");
        }
    }

    #[test]
    fn the_range_reads_out_in_decibels() {
        let mut voice = Voice::new();
        voice.say(120, |_| -58.0).say(20, |_| -30.0);
        let range = voice.painted(152).range_now();
        assert!((range.floor_db + 58.0).abs() < 0.01, "{range:?}");
        assert!((range.gate_db + 52.0).abs() < 0.01, "{range:?}");
        assert!((range.top_db + 30.0).abs() < 0.01, "{range:?}");
    }

    #[test]
    fn pills_follow_the_strips_clock() {
        let mut levels = PillLevels::new(8);
        levels.advance(0.95);
        assert_eq!(levels.open_index(), 9);
        levels.advance(0.5);
        assert_eq!(levels.open_index(), 9);
    }

    #[test]
    fn pills_waiting_after_they_scrolled_away_are_dropped() {
        let mut levels = PillLevels::new(8);
        levels.advance(10.0);
        assert!(levels.pending.len() <= 8, "{}", levels.pending.len());
    }

    #[test]
    fn history_flows_right_to_left_one_level_per_pill() {
        let mut history = vec![0.0; 4];
        push_history(&mut history, 0.5);
        push_history(&mut history, 0.9);
        assert_eq!(history, vec![0.0, 0.0, 0.5, 0.9]);
    }

    #[test]
    fn history_keeps_its_newest_levels_when_the_count_changes() {
        let mut history = vec![0.1, 0.2, 0.3];
        fit_history(&mut history, 5);
        assert_eq!(history, vec![0.0, 0.0, 0.1, 0.2, 0.3]);
        fit_history(&mut history, 2);
        assert_eq!(history, vec![0.2, 0.3]);
    }

    #[test]
    fn clearing_empties_the_pills_but_keeps_the_learned_range() {
        let mut voice = Voice::new();
        voice.say(20, office_noise).say(40, soft_speech);
        let mut levels = voice.painted(8);
        let learned = levels.range.clone();
        levels.clear();
        assert_eq!(levels.heights(), &[0.0; 8]);
        assert_eq!(levels.range, learned);
        assert_eq!(levels.open_index(), 0);
        assert!(levels.pending.is_empty());
    }
}
