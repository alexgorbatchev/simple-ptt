//! What the pills show: the voice's recent history at about one pill per
//! syllable. Speech is drawn as a lively up-and-down texture, pills
//! alternating tall and short, scaled by the loudness of each pill's 0.2 s
//! against a range that adapts to the room and the speaker, so soft speech in
//! a quiet office fills the pills as well as loud speech does. Everything
//! else (pauses, typing, a cough, a door) rests as a dot. The pills are a
//! picture of talking, not a waveform. A pill is final before it enters the
//! column, and never changes after.

use std::collections::VecDeque;

/// How long each pill stands for: about one syllable, since conversational
/// English runs at 4 to 5 syllables a second.
pub(super) const PILL_SECONDS: f64 = 0.2;
/// How long after its time begins a pill is painted at the latest: its own
/// 0.2 s, up to 0.25 s until the classifier's next result, and the analysis
/// itself. A pill not painted by then rests as a dot for good.
pub(super) const LATEST_PAINT_SECONDS: f64 = 0.6;
/// How long after its time begins a pill enters the column (the strip is
/// drawn this far to the right): 0.1 s after its latest paint, more than the
/// overlay's 75 ms between updates (`STATUS_POLL_INTERVAL_SECONDS`), so the
/// height a pill enters with is already on screen.
pub(super) const ENTRY_DELAY_SECONDS: f64 = LATEST_PAINT_SECONDS + 0.1;

/// The noise floor the range starts with, in amplitude (-50 dBFS), counted
/// as a pill heard at the start: it holds the floor below speech that begins
/// before the room was heard, until it leaves the floor window.
const INITIAL_FLOOR: f32 = 0.003_162_278;
/// The noise floor is the quietest of the last this many pills that were not
/// speech (10 s of them), so it stays at the room's noise however long
/// someone talks, and follows the room within 10 s when it gets louder.
/// Without speech analysis every pill counts, and speech pauses for breath
/// well within it.
const FLOOR_PILLS: usize = 50;
/// Pills up to this far above the noise floor (6 dB) rest as dots.
const NOISE_GATE_RATIO: f32 = 2.0;
/// The least distance from the gate to the top of the range (12 dB), so
/// background noise is never stretched to full height.
const MIN_SPAN_RATIO: f32 = 4.0;
/// The top of the range is this share of the recent speech pills, so one
/// loud word does not shrink the speech around it.
const TOP_QUANTILE: f64 = 0.9;
/// Speech pills the top is taken over: the last 10 s of speech.
const TOP_PILLS: usize = 50;
/// How much of a speech pill's height its loudness decides; the rest is
/// always drawn, so the softest speech still shows its texture.
const LOUDNESS_SHARE: f32 = 0.5;
/// The texture's two bands: pills alternate between a tall one and a short
/// one, at a random height within each.
const TALL_BAND: (f32, f32) = (0.75, 1.0);
const SHORT_BAND: (f32, f32) = (0.15, 0.4);

/// The span of loudness, in amplitude (RMS, 1 is full scale), that the pills
/// map linearly from rest to full height: from `NOISE_GATE_RATIO` above the
/// noise floor to `TOP_QUANTILE` of the recent speech.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LoudnessRange {
    /// The loudness of the last `FLOOR_PILLS` pills the floor learns from,
    /// oldest first.
    recent: VecDeque<f32>,
    /// The last `TOP_PILLS` speech pills' loudness above the gate.
    speech: VecDeque<f32>,
}

impl Default for LoudnessRange {
    fn default() -> Self {
        Self { recent: VecDeque::from([INITIAL_FLOOR]), speech: VecDeque::new() }
    }
}

impl LoudnessRange {
    /// Takes in the loudness of a pill of the room's sound: the noise floor
    /// follows it.
    fn observe(&mut self, rms: f32) {
        self.recent.push_back(rms);
        while self.recent.len() > FLOOR_PILLS {
            self.recent.pop_front();
        }
    }

    /// Takes in a speech pill's loudness: the top of the range follows it.
    fn hear_speech(&mut self, rms: f32) {
        if rms > self.gate() {
            self.speech.push_back(rms);
            while self.speech.len() > TOP_PILLS {
                self.speech.pop_front();
            }
        }
    }

    /// Where `rms` falls in the range: 0 at or below the gate, 1 at or above
    /// the top, linear in amplitude between.
    fn height(&self, rms: f32) -> f32 {
        let gate = self.gate();
        ((rms - gate) / (self.top() - gate)).clamp(0.0, 1.0)
    }

    fn gate(&self) -> f32 {
        self.recent.iter().copied().fold(f32::INFINITY, f32::min) * NOISE_GATE_RATIO
    }

    fn top(&self) -> f32 {
        let least = self.gate() * MIN_SPAN_RATIO;
        let mut speech: Vec<f32> = self.speech.iter().copied().collect();
        if speech.is_empty() {
            return least;
        }
        speech.sort_by(f32::total_cmp);
        let index = ((speech.len() - 1) as f64 * TOP_QUANTILE).round() as usize;
        speech[index].max(least)
    }
}

/// Pill `index`'s place in the speech texture: even pills in the tall band,
/// odd pills in the short one, at a height within it that is random but the
/// same every time for the same pill.
fn texture(index: u64) -> f32 {
    let (low, high) = if index % 2 == 0 { TALL_BAND } else { SHORT_BAND };
    low + ((high - low) * unit_hash(index))
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

/// One speech analysis result: whether the audio from `start` to `end`, on
/// the strip's clock, was speech.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SpeechSpan {
    pub(super) start: f64,
    pub(super) end: f64,
    pub(super) speech: bool,
}

/// One block of captured audio, from `start` to `end` on the strip's clock,
/// and its mean square (its RMS squared).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LevelSpan {
    pub(super) start: f64,
    pub(super) end: f64,
    pub(super) mean_square: f32,
}

/// What the pills are painted from.
#[derive(Clone, Copy, Debug)]
pub(super) struct PillAudio<'a> {
    /// The speech analysis results so far, or `None` when speech analysis
    /// does not run: then every pill counts as speech, so the pills show
    /// every sound above the noise.
    pub(super) speech: Option<&'a [SpeechSpan]>,
    /// The captured audio's loudness so far.
    pub(super) levels: &'a [LevelSpan],
}

/// A small tolerance for the clock arithmetic of adjacent windows and blocks.
const EPSILON: f64 = 1e-6;

/// Whether the pill from `start` to `end` was speech: undecided until a
/// result reaches its end, then speech if any result overlapping it was.
fn pill_speech((start, end): (f64, f64), windows: &[SpeechSpan]) -> Option<bool> {
    windows.iter().any(|window| window.end >= end - EPSILON).then(|| {
        windows
            .iter()
            .any(|window| window.speech && window.start < end - EPSILON && window.end > start + EPSILON)
    })
}

/// What was heard in a pill's time.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PillLoudness {
    /// Its audio is not all in yet.
    Pending,
    /// The microphone delivered nothing in its time: that says nothing about
    /// the room.
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
    if seconds > 0.0 {
        PillLoudness::Heard((energy / seconds).sqrt() as f32)
    } else {
        PillLoudness::NoAudio
    }
}

/// The pills' heights, oldest first (newest at the right), and the range
/// they are drawn from. Pill `k` stands for the time from `k × PILL_SECONDS`
/// to `(k + 1) × PILL_SECONDS` on the strip's clock (seconds since the strip
/// started), the clock the strip moves by. A pill rests as a dot until it is
/// painted, which happens before it enters the column.
#[derive(Debug)]
pub(super) struct PillLevels {
    range: LoudnessRange,
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

    /// Paints the closed pills whose loudness and speech result are in,
    /// oldest first, `at` on the strip's clock. A pill past its latest paint
    /// time rests as a dot for good, decided or not, so a pill never changes
    /// once it can be seen.
    pub(super) fn paint(&mut self, audio: PillAudio, at: f64) {
        while let Some(pill) = self.pending.front().copied() {
            let span = (pill as f64 * PILL_SECONDS, (pill + 1) as f64 * PILL_SECONDS);
            let loudness = pill_loudness(span, audio.levels);
            let speech = audio.speech.map_or(Some(true), |windows| pill_speech(span, windows));
            let late = at >= span.0 + LATEST_PAINT_SECONDS;
            let height = match (loudness, speech) {
                (PillLoudness::NoAudio, _) => 0.0,
                (PillLoudness::Heard(rms), Some(speech)) => {
                    // The range still learns from a pill decided late.
                    let height = self.hear(pill, rms, speech, audio.speech.is_some());
                    if late { 0.0 } else { height }
                }
                _ if late => 0.0,
                _ => break,
            };
            self.pending.pop_front();
            if let Some(slot) = self.slot(pill) {
                self.history[slot] = height;
            }
        }
    }

    /// Takes in pill `index`'s loudness, and returns its height: its texture
    /// scaled by its loudness if it was speech above the noise, a dot
    /// otherwise. The noise floor learns from the pills that were not speech,
    /// or from every pill when speech was not `analysed`.
    fn hear(&mut self, index: u64, rms: f32, speech: bool, analysed: bool) -> f32 {
        if !speech || !analysed {
            self.range.observe(rms);
        }
        if !speech {
            return 0.0;
        }
        self.range.hear_speech(rms);
        let loudness = self.range.height(rms);
        if loudness <= 0.0 {
            return 0.0;
        }
        texture(index) * ((1.0 - LOUDNESS_SHARE) + (LOUDNESS_SHARE * loudness))
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

    /// A voice from pill 0 on: one level block and one speech result per
    /// pill.
    struct Voice {
        levels: Vec<LevelSpan>,
        windows: Vec<SpeechSpan>,
        /// Pills so far, with or without audio.
        pills: usize,
    }

    impl Voice {
        fn new() -> Self {
            Self { levels: Vec::new(), windows: Vec::new(), pills: 0 }
        }

        /// The next `pills` pills at `db(n)` dBFS for the `n`th of them,
        /// speech or not.
        fn say(&mut self, pills: usize, db: impl Fn(usize) -> f32, speech: bool) -> &mut Self {
            for pill in 0..pills {
                let (start, end) = (self.pills as f64 * PILL_SECONDS, (self.pills + 1) as f64 * PILL_SECONDS);
                self.levels.push(LevelSpan { start, end, mean_square: amplitude(db(pill)).powi(2) });
                self.windows.push(SpeechSpan { start, end, speech });
                self.pills += 1;
            }
            self
        }

        /// The next `pills` pills with no audio: the microphone delivered
        /// nothing, and the analysis heard no speech.
        fn pause(&mut self, pills: usize) -> &mut Self {
            let (start, end) = (self.pills as f64 * PILL_SECONDS, (self.pills + pills) as f64 * PILL_SECONDS);
            self.windows.push(SpeechSpan { start, end, speech: false });
            self.pills += pills;
            self
        }

        fn audio(&self, analysed: bool) -> PillAudio<'_> {
            PillAudio { speech: analysed.then_some(&self.windows[..]), levels: &self.levels }
        }

        /// Plays the voice into `levels` from its open pill to the voice's
        /// end, painting as each pill closes, as the overlay's updates do.
        fn play(&self, levels: &mut PillLevels, analysed: bool) {
            for pill in levels.open_index()..self.pills as u64 {
                let at = (pill + 1) as f64 * PILL_SECONDS;
                levels.advance(at);
                levels.paint(self.audio(analysed), at);
            }
        }

        /// `count` pills painted from the whole voice, with speech analysis
        /// unless `analysed` is false.
        fn painted(&self, analysed: bool, count: usize) -> PillLevels {
            let mut levels = PillLevels::new(count);
            self.play(&mut levels, analysed);
            levels
        }
    }

    /// Office background noise, a little uneven.
    fn office_noise(pill: usize) -> f32 {
        if pill % 3 == 0 { -55.0 } else { -58.0 }
    }

    /// Soft speech, syllable by syllable: -46 to -37 dBFS.
    fn soft_speech(pill: usize) -> f32 {
        [-40.0, -37.0, -43.0, -39.0, -46.0, -38.0, -41.0][pill % 7]
    }

    fn max_of(heights: &[f32]) -> f32 {
        heights.iter().copied().fold(0.0, f32::max)
    }

    /// The newest `count` pills painted: all but the open one.
    fn newest(levels: &PillLevels, count: usize) -> Vec<f32> {
        let heights = levels.heights();
        heights[heights.len() - 1 - count..heights.len() - 1].to_vec()
    }

    #[test]
    fn background_noise_rests_as_dots() {
        let mut voice = Voice::new();
        voice.say(50, office_noise, false);
        assert_eq!(max_of(voice.painted(true, 76).heights()), 0.0);
        assert_eq!(max_of(voice.painted(false, 76).heights()), 0.0);
    }

    #[test]
    fn steady_speech_goes_up_and_down_from_pill_to_pill() {
        let mut voice = Voice::new();
        voice.say(10, office_noise, false).say(40, |_| -35.0, true);
        let heights = newest(&voice.painted(true, 76), 30);
        for pair in heights.windows(2) {
            assert!((pair[0] - pair[1]).abs() >= 0.15, "{heights:?}");
        }
        assert!(heights.iter().all(|height| *height > 0.0), "{heights:?}");
    }

    #[test]
    fn speech_varies_instead_of_pinning_at_full_height() {
        let mut voice = Voice::new();
        voice.say(10, office_noise, false).say(70, soft_speech, true);
        let heights = newest(&voice.painted(true, 76), 50);
        let full = heights.iter().filter(|height| **height >= 0.95).count();
        assert!(full <= 10, "{full} of 50 at full height: {heights:?}");
        let mut sorted = heights.clone();
        sorted.sort_by(f32::total_cmp);
        assert!(sorted[25] < 0.8, "median {}", sorted[25]);
    }

    #[test]
    fn a_louder_syllable_is_drawn_taller() {
        let mut steady = Voice::new();
        steady.say(10, office_noise, false).say(35, soft_speech, true);
        let mut louder = Voice::new();
        // Pill 40, -43 dBFS in `steady`, is -40 here.
        louder.say(10, office_noise, false).say(30, soft_speech, true).say(1, |_| -40.0, true).say(4, |pill| soft_speech(pill + 31), true);
        let (steady, louder) = (steady.painted(true, 76), louder.painted(true, 76));
        let pill = |levels: &PillLevels| levels.heights()[levels.slot(40).unwrap()];
        assert!(pill(&louder) > pill(&steady) + 0.05, "{} {}", pill(&louder), pill(&steady));
    }

    #[test]
    fn soft_and_loud_speech_fill_the_pills_alike() {
        let mut soft = Voice::new();
        soft.say(10, office_noise, false).say(60, soft_speech, true);
        let mut loud = Voice::new();
        loud.say(10, office_noise, false).say(60, |pill| soft_speech(pill) + 20.0, true);
        let soft_heights = newest(&soft.painted(true, 76), 21);
        let loud_heights = newest(&loud.painted(true, 76), 21);
        let mean = |heights: &[f32]| heights.iter().sum::<f32>() / heights.len() as f32;
        assert!((mean(&soft_heights) - mean(&loud_heights)).abs() < 0.1, "{soft_heights:?} {loud_heights:?}");
    }

    #[test]
    fn sounds_that_are_not_speech_rest_as_dots() {
        let mut voice = Voice::new();
        voice.say(10, office_noise, false).say(10, |_| -15.0, false);
        assert_eq!(max_of(voice.painted(true, 76).heights()), 0.0);
    }

    #[test]
    fn a_long_monologue_keeps_the_noise_floor_at_the_room() {
        // 14 seconds of talking without a pause: the speech does not become
        // the floor, so it keeps showing.
        let mut voice = Voice::new();
        voice.say(10, office_noise, false).say(70, soft_speech, true);
        let heights = newest(&voice.painted(true, 76), 20);
        assert!(heights.iter().all(|height| *height > 0.0), "{heights:?}");
    }

    #[test]
    fn a_stretch_without_audio_does_not_teach_the_range() {
        // The microphone delivers nothing for 2 s: that is not the room going
        // silent, so the room's noise after it still rests as dots.
        let mut voice = Voice::new();
        voice.say(10, office_noise, false).pause(10).say(10, office_noise, false);
        let heights = voice.painted(false, 76);
        assert_eq!(max_of(heights.heights()), 0.0, "{:?}", heights.heights());
    }

    #[test]
    fn pauses_between_words_rest_as_dots() {
        let mut voice = Voice::new();
        voice.say(10, office_noise, false).say(5, soft_speech, true).say(2, office_noise, true).say(5, soft_speech, true);
        let heights = newest(&voice.painted(true, 76), 12);
        assert_eq!(heights[5..7], [0.0, 0.0], "{heights:?}");
        assert!(heights[..5].iter().chain(&heights[7..]).all(|height| *height > 0.0), "{heights:?}");
    }

    #[test]
    fn a_loud_sound_that_is_not_speech_does_not_shrink_the_speech_after_it() {
        let mut quiet_room = Voice::new();
        quiet_room.say(10, office_noise, false).say(2, office_noise, false).say(33, |pill| soft_speech(pill + 2), true);
        let mut slammed = Voice::new();
        slammed.say(10, office_noise, false).say(2, |_| -12.0, false).say(33, |pill| soft_speech(pill + 2), true);
        assert_eq!(newest(&slammed.painted(true, 76), 14), newest(&quiet_room.painted(true, 76), 14));
    }

    #[test]
    fn one_loud_word_does_not_shrink_the_speech_around_it() {
        let mut steady = Voice::new();
        steady.say(10, office_noise, false).say(70, soft_speech, true);
        let mut shout = Voice::new();
        shout.say(10, office_noise, false).say(35, soft_speech, true).say(1, |_| -20.0, true).say(34, |pill| soft_speech(pill + 36), true);
        let steady_heights = newest(&steady.painted(true, 76), 14);
        let shout_heights = newest(&shout.painted(true, 76), 14);
        for (steady_height, shout_height) in steady_heights.iter().zip(&shout_heights) {
            assert!((steady_height - shout_height).abs() < 0.1, "{steady_heights:?} {shout_heights:?}");
        }
    }

    #[test]
    fn without_speech_analysis_every_sound_above_the_noise_shows() {
        let mut voice = Voice::new();
        voice.say(10, office_noise, false).say(10, |_| -30.0, false);
        assert!(max_of(voice.painted(false, 76).heights()) > 0.0);
    }

    #[test]
    fn a_room_that_gets_louder_settles_back_to_dots() {
        let mut voice = Voice::new();
        voice.say(10, office_noise, false).say(60, |pill| office_noise(pill) + 10.0, false);
        assert_eq!(max_of(&newest(&voice.painted(false, 76), 8)), 0.0);
    }

    #[test]
    fn speech_from_the_first_moment_shows() {
        let mut voice = Voice::new();
        voice.say(3, soft_speech, true);
        let heights = newest(&voice.painted(true, 76), 3);
        assert!(heights.iter().all(|height| *height >= 0.1), "{heights:?}");
    }

    #[test]
    fn a_pill_waits_until_its_loudness_and_speech_are_in() {
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0, true);
        let mut levels = PillLevels::new(8);
        levels.advance(0.25);
        levels.paint(PillAudio { speech: Some(&[]), levels: &[] }, 0.25);
        assert_eq!(max_of(levels.heights()), 0.0);
        // The loudness alone is not enough.
        levels.paint(PillAudio { speech: Some(&[]), levels: &voice.levels }, 0.3);
        assert_eq!(max_of(levels.heights()), 0.0);
        levels.paint(voice.audio(true), 0.35);
        assert!(levels.heights()[6] > 0.0, "{:?}", levels.heights());
    }

    #[test]
    fn a_pill_undecided_by_its_latest_paint_stays_a_dot() {
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0, true);
        let mut levels = PillLevels::new(8);
        levels.advance(LATEST_PAINT_SECONDS);
        // Pill 0's latest paint time comes with no speech result.
        levels.paint(PillAudio { speech: Some(&[]), levels: &voice.levels }, LATEST_PAINT_SECONDS);
        assert_ne!(levels.pending.front(), Some(&0), "pill 0 stopped waiting");
        // The result arrives late: the pill does not change.
        levels.paint(voice.audio(true), LATEST_PAINT_SECONDS + 0.1);
        assert_eq!(max_of(levels.heights()), 0.0);
    }

    #[test]
    fn a_pill_decided_but_not_painted_by_its_latest_paint_stays_a_dot() {
        let mut voice = Voice::new();
        voice.say(1, |_| -35.0, true);
        let mut levels = PillLevels::new(8);
        // The first update after pill 0 closed comes after its latest paint
        // time, with its result in.
        levels.advance(LATEST_PAINT_SECONDS + 0.05);
        levels.paint(voice.audio(true), LATEST_PAINT_SECONDS + 0.05);
        assert_eq!(max_of(levels.heights()), 0.0);
    }

    #[test]
    fn pills_already_painted_keep_their_height() {
        let mut voice = Voice::new();
        voice.say(10, office_noise, false).say(30, soft_speech, true);
        let mut levels = PillLevels::new(76);
        voice.play(&mut levels, true);
        let painted = levels.heights().to_vec();
        voice.say(10, |pill| soft_speech(pill) + 15.0, true);
        voice.play(&mut levels, true);
        // Every pill painted before (all but the one open then) is unchanged.
        assert_eq!(&levels.heights()[..painted.len() - 11], &painted[10..painted.len() - 1]);
    }

    #[test]
    fn a_pills_loudness_is_the_rms_of_the_audio_in_its_time() {
        let block = |start: f64, end: f64, rms: f32| LevelSpan { start, end, mean_square: rms * rms };
        // Pending until a block reaches the pill's end.
        assert_eq!(pill_loudness((0.2, 0.4), &[block(0.2, 0.3, 0.1)]), PillLoudness::Pending);
        // Blocks weighted by their length: 0.1 for 0.1 s, 0.2 for 0.1 s.
        let PillLoudness::Heard(rms) = pill_loudness((0.2, 0.4), &[block(0.2, 0.3, 0.1), block(0.3, 0.4, 0.2)]) else {
            panic!("the pill's audio is in");
        };
        assert!((rms - 0.025f32.sqrt()).abs() < 1e-6, "{rms}");
        // A block belongs to the pill its middle falls in: none falls in this one.
        assert_eq!(pill_loudness((0.2, 0.4), &[block(0.15, 0.22, 0.5), block(0.4, 0.5, 0.5)]), PillLoudness::NoAudio);
    }

    #[test]
    fn a_pills_speech_is_decided_once_a_window_reaches_its_end() {
        let window = |start: f64, end: f64, speech: bool| SpeechSpan { start, end, speech };
        let pill = (0.2, 0.4);
        assert_eq!(pill_speech(pill, &[window(0.0, 0.3, true)]), None);
        assert_eq!(pill_speech(pill, &[window(0.0, 0.3, true), window(0.25, 0.75, false)]), Some(true));
        assert_eq!(pill_speech(pill, &[window(0.25, 0.75, false)]), Some(false));
        // A window that ends before the pill starts says nothing about it.
        assert_eq!(pill_speech(pill, &[window(-0.4, 0.1, true), window(0.3, 0.8, false)]), Some(false));
    }

    #[test]
    fn pills_follow_the_strips_clock() {
        let mut levels = PillLevels::new(8);
        levels.advance(1.95);
        assert_eq!(levels.open_index(), 9);
        levels.advance(1.0);
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
        voice.say(10, office_noise, false).say(20, soft_speech, true);
        let mut levels = voice.painted(true, 8);
        let learned = levels.range.clone();
        levels.clear();
        assert_eq!(levels.heights(), &[0.0; 8]);
        assert_eq!(levels.range, learned);
        assert_eq!(levels.open_index(), 0);
        assert!(levels.pending.is_empty());
    }
}
