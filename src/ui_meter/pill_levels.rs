//! What the pills show: the level's recent history at about one pill per
//! syllable. A pill's height is set once the speech analysis has judged its
//! time: speech is drawn from a level range that adapts to the room and the
//! speaker, so soft speech in a quiet office fills the pills as well as loud
//! speech does; anything else (typing, a cough, a door) rests as a dot. Pills
//! already painted never change.

use std::collections::VecDeque;

/// The noise floor the range starts with, counted as a level heard at the
/// start: it holds the floor below speech that begins before the room was
/// heard, until it leaves the floor window.
const INITIAL_FLOOR_DB: f32 = -50.0;
/// The noise floor is the quietest level heard in this long. Speech pauses
/// for breath well within it, so the floor stays at the room's noise while
/// someone talks, and follows the room within this long when it gets
/// louder.
const FLOOR_WINDOW_SECONDS: f64 = 10.0;
/// Levels up to this far above the noise floor rest as dots.
const NOISE_GATE_DB: f32 = 6.0;
/// The least distance from the gate to the top of the range, so background
/// noise is never stretched to full height.
const MIN_SPEECH_SPAN_DB: f32 = 12.0;
/// How fast the top of the range falls back after loud speech. It rises at
/// once to louder speech.
const CEILING_FALL_DB_PER_SECOND: f32 = 3.0;
/// The longest step one update counts as, so the time the overlay spent
/// hidden does not wear away what the range learned.
const MAX_STEP_SECONDS: f64 = 0.25;
/// Quieter levels count as this (digital silence reads -100 dBFS).
const LOWEST_LEVEL_DB: f32 = -90.0;
/// How long each pill stands for: about one syllable, since conversational
/// English runs at 4 to 5 syllables a second.
pub(super) const PILL_SECONDS: f64 = 0.2;

/// The span of levels, in dBFS, that the pills map from rest to full
/// height: from `NOISE_GATE_DB` above the noise floor to the loudest recent
/// speech.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LevelRange {
    pub(super) floor_db: f32,
    pub(super) ceiling_db: f32,
    /// Seconds of levels heard so far, counting each step as at most
    /// `MAX_STEP_SECONDS`.
    heard_seconds: f64,
    /// The levels of the last `FLOOR_WINDOW_SECONDS`, with when they were
    /// heard, oldest first.
    recent: VecDeque<(f64, f32)>,
}

impl Default for LevelRange {
    fn default() -> Self {
        Self {
            floor_db: INITIAL_FLOOR_DB,
            ceiling_db: INITIAL_FLOOR_DB + NOISE_GATE_DB + MIN_SPEECH_SPAN_DB,
            heard_seconds: 0.0,
            recent: VecDeque::from([(0.0, INITIAL_FLOOR_DB)]),
        }
    }
}

impl LevelRange {
    /// Takes in `level_db`, heard `seconds` after the previous level, speech
    /// or not: the noise floor follows it, and the top of the range falls
    /// back with time.
    pub(super) fn observe(&mut self, level_db: f32, seconds: f64) {
        let step = seconds.clamp(0.0, MAX_STEP_SECONDS);
        let level_db = level_db.max(LOWEST_LEVEL_DB);
        self.heard_seconds += step;
        self.recent.push_back((self.heard_seconds, level_db));
        while self
            .recent
            .front()
            .is_some_and(|(heard_at, _)| *heard_at < self.heard_seconds - FLOOR_WINDOW_SECONDS)
        {
            self.recent.pop_front();
        }
        self.floor_db = self.recent.iter().map(|(_, level)| *level).fold(f32::INFINITY, f32::min);
        self.ceiling_db = (self.ceiling_db - (CEILING_FALL_DB_PER_SECOND * step as f32))
            .max(self.gate_db() + MIN_SPEECH_SPAN_DB);
    }

    /// Raises the top of the range to `level_db` of speech, if it is louder.
    pub(super) fn raise(&mut self, level_db: f32) {
        self.ceiling_db = self.ceiling_db.max(level_db.max(LOWEST_LEVEL_DB));
    }

    /// Where `level_db` falls in the range: 0 at or below the gate, 1 at or
    /// above the top.
    pub(super) fn height(&self, level_db: f32) -> f32 {
        ((level_db - self.gate_db()) / (self.ceiling_db - self.gate_db())).clamp(0.0, 1.0)
    }

    fn gate_db(&self) -> f32 {
        self.floor_db + NOISE_GATE_DB
    }
}

/// One result of the speech analysis: whether the audio from `start` to
/// `end`, on the strip's clock, was speech.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SpeechSpan {
    pub(super) start: f64,
    pub(super) end: f64,
    pub(super) speech: bool,
}

/// What the pills know about speech.
#[derive(Clone, Copy, Debug)]
pub(super) enum SpeechInput<'a> {
    /// No speech analysis runs: every pill counts as speech once its time is
    /// up, so the pills show the level alone.
    Unavailable,
    /// The analysis results so far.
    Windows(&'a [SpeechSpan]),
}

/// Whether the pill from `start` to `end` was speech: undecided until a
/// result reaches its end, then speech if any result overlapping it was.
fn pill_speech((start, end): (f64, f64), windows: &[SpeechSpan]) -> Option<bool> {
    // A small tolerance for the clock arithmetic of adjacent windows.
    const EPSILON: f64 = 1e-6;
    windows.iter().any(|window| window.end >= end - EPSILON).then(|| {
        windows
            .iter()
            .any(|window| window.speech && window.start < end - EPSILON && window.end > start + EPSILON)
    })
}

/// A closed pill waiting for its speech result, with the loudest level of
/// its time (`None` when the microphone delivered nothing).
#[derive(Clone, Copy, Debug)]
struct PendingPill {
    index: u64,
    peak_db: Option<f32>,
}

/// The pills' heights, oldest first (newest at the right), and the range
/// they are drawn from. Pill `k` stands for the time from `k × PILL_SECONDS`
/// to `(k + 1) × PILL_SECONDS` on the strip's clock (seconds since the strip
/// started), the clock the strip moves by. A pill rests as a dot while its
/// time runs and until its speech result is in, and is painted then.
#[derive(Debug)]
pub(super) struct PillLevels {
    pub(super) range: LevelRange,
    /// Pills `open_index - len + 1` to `open_index`.
    history: Vec<f32>,
    open_index: u64,
    /// The loudest level of the open pill's time so far.
    open_peak_db: Option<f32>,
    /// Closed pills whose speech result is not in yet, oldest first.
    pending: VecDeque<PendingPill>,
    /// When the last level arrived, on the strip's clock.
    last_at: Option<f64>,
}

impl PillLevels {
    pub(super) fn new(count: usize) -> Self {
        Self {
            range: LevelRange::default(),
            history: vec![0.0; count],
            open_index: 0,
            open_peak_db: None,
            pending: VecDeque::new(),
            last_at: None,
        }
    }

    /// Takes in `level_db`, heard `at` on the strip's clock.
    pub(super) fn push(&mut self, level_db: f32, at: f64) {
        let seconds = self.step_to(at);
        self.range.observe(level_db, seconds);
        self.advance(at);
        self.open_peak_db = Some(self.open_peak_db.map_or(level_db, |peak| peak.max(level_db)));
    }

    /// Rests, `at` on the strip's clock, while the microphone delivers
    /// nothing; its silence says nothing about the room, so the range does
    /// not take it in.
    pub(super) fn push_rest(&mut self, at: f64) {
        self.step_to(at);
        self.advance(at);
    }

    /// Paints the closed pills whose speech result is in, oldest first: from
    /// the range if their time was speech (which may raise the range), as a
    /// dot otherwise.
    pub(super) fn classify(&mut self, speech: SpeechInput) {
        while let Some(pill) = self.pending.front().copied() {
            let span = (pill.index as f64 * PILL_SECONDS, (pill.index + 1) as f64 * PILL_SECONDS);
            let is_speech = match speech {
                SpeechInput::Unavailable => true,
                SpeechInput::Windows(windows) => match pill_speech(span, windows) {
                    Some(is_speech) => is_speech,
                    None => break,
                },
            };
            self.pending.pop_front();
            let height = match pill.peak_db {
                Some(peak_db) if is_speech => {
                    self.range.raise(peak_db);
                    self.range.height(peak_db)
                }
                _ => 0.0,
            };
            if let Some(slot) = self.slot(pill.index) {
                self.history[slot] = height;
            }
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

    /// Seconds from the previous level to `at`.
    fn step_to(&mut self, at: f64) -> f64 {
        let seconds = self.last_at.map_or(0.0, |last_at| at - last_at);
        self.last_at = Some(at);
        seconds
    }

    /// Opens the pill whose time `at` falls in. The open pill closes and
    /// waits for its speech result; a pill whose whole time passed between
    /// updates rests.
    fn advance(&mut self, at: f64) {
        let index = (at.max(0.0) / PILL_SECONDS).floor() as u64;
        if index <= self.open_index {
            return;
        }
        self.pending.push_back(PendingPill { index: self.open_index, peak_db: self.open_peak_db.take() });
        for _ in 0..(index - self.open_index).min(self.history.len() as u64) {
            push_history(&mut self.history, 0.0);
        }
        self.open_index = index;
        // A pill that scrolled out of the history can no longer be painted.
        while self.pending.front().is_some_and(|pill| self.slot(pill.index).is_none()) {
            self.pending.pop_front();
        }
    }

    /// Empties the pills and restarts the clock. The range keeps what it
    /// learned about the room.
    pub(super) fn clear(&mut self) {
        self.history.fill(0.0);
        self.open_index = 0;
        self.open_peak_db = None;
        self.pending.clear();
        self.last_at = None;
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

    /// The overlay's update interval while recording.
    const TICK: f64 = 0.075;

    /// When the next update lands: one `TICK` after the last, or at the
    /// strip's start.
    fn next_at(levels: &PillLevels) -> f64 {
        levels.last_at.map_or(0.0, |at| at + TICK)
    }

    fn push_for(levels: &mut PillLevels, seconds: f64, level_db: impl Fn(usize) -> f32) {
        let ticks = (seconds / TICK).round() as usize;
        for tick in 0..ticks {
            levels.push(level_db(tick), next_at(levels));
        }
    }

    /// Hears `level_db` as speech: the range takes it in and may rise to it.
    fn hear(range: &mut LevelRange, level_db: f32, seconds: f64) {
        range.observe(level_db, seconds);
        range.raise(level_db);
    }

    /// Office background noise, a little uneven.
    fn office_noise(tick: usize) -> f32 {
        if tick % 3 == 0 { -55.0 } else { -58.0 }
    }

    /// Soft speech: syllables at -42 and -37 dBFS with short dips between
    /// words, and a breath at the room's noise every 2.4 seconds.
    fn soft_speech(tick: usize) -> f32 {
        if tick % 32 >= 28 {
            return -57.0;
        }
        match tick % 8 {
            0..=2 => -42.0,
            3..=5 => -37.0,
            _ => -50.0,
        }
    }

    /// The same speech 17 dB louder.
    fn loud_speech(tick: usize) -> f32 {
        soft_speech(tick) + 17.0
    }

    /// One analysis result for `start` to `end` on the strip's clock.
    fn window(start: f64, end: f64, speech: bool) -> SpeechSpan {
        SpeechSpan { start, end, speech }
    }

    fn max_of(heights: &[f32]) -> f32 {
        heights.iter().copied().fold(0.0, f32::max)
    }

    #[test]
    fn background_noise_rests_as_dots() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 10.0, office_noise);
        levels.classify(SpeechInput::Unavailable);
        assert_eq!(max_of(levels.heights()), 0.0);
    }

    #[test]
    fn soft_speech_fills_the_pills_once_the_range_adapts() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 2.0, office_noise);
        push_for(&mut levels, 4.0, soft_speech);
        levels.classify(SpeechInput::Unavailable);
        // The last two seconds of soft speech reach full height.
        let recent = &levels.heights()[levels.heights().len() - 10..];
        assert!(max_of(recent) >= 0.99, "{recent:?}");
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.observe(office_noise(tick), TICK);
        }
        for tick in 0..53 {
            hear(&mut range, soft_speech(tick), TICK);
        }
        assert!(range.height(-42.0) >= 0.5, "{range:?}");
        assert_eq!(range.height(-37.0), 1.0, "{range:?}");
    }

    #[test]
    fn a_pill_waits_as_a_dot_until_its_time_is_analysed() {
        let mut levels = PillLevels::new(8);
        push_for(&mut levels, 0.3, |_| -35.0);
        // Pill 0 (0 to 0.2 s) has closed, but no result covers it yet.
        levels.classify(SpeechInput::Windows(&[window(-0.5, 0.0, true)]));
        assert_eq!(max_of(levels.heights()), 0.0);
        // A window reaching past its end decides it.
        levels.classify(SpeechInput::Windows(&[window(-0.25, 0.25, true)]));
        let heights = levels.heights();
        assert!(heights[heights.len() - 2] > 0.0, "{heights:?}");
    }

    #[test]
    fn a_pill_the_analysis_calls_not_speech_rests_as_a_dot() {
        let mut levels = PillLevels::new(8);
        push_for(&mut levels, 2.0, |_| -20.0);
        levels.classify(SpeechInput::Windows(&[window(0.0, 2.5, false)]));
        assert_eq!(max_of(levels.heights()), 0.0);
    }

    #[test]
    fn a_loud_sound_that_is_not_speech_does_not_shrink_the_speech_after_it() {
        // Room noise, a 0.3 s slam at -15 dBFS, then soft speech.
        let run = |speech: SpeechInput| {
            let mut levels = PillLevels::new(76);
            push_for(&mut levels, 2.0, office_noise);
            push_for(&mut levels, 0.3, |_| -15.0);
            push_for(&mut levels, 1.2, soft_speech);
            levels.classify(speech);
            let heights = levels.heights();
            max_of(&heights[heights.len() - 4..])
        };
        let analysed = [window(0.0, 2.4, false), window(2.4, 4.0, true)];
        assert_eq!(run(SpeechInput::Windows(&analysed)), 1.0);
        // Without the analysis the slam raises the range, and the speech
        // after it is drawn smaller.
        assert!(run(SpeechInput::Unavailable) < 0.8);
    }

    #[test]
    fn without_speech_analysis_every_pill_counts_as_speech_when_it_closes() {
        let mut levels = PillLevels::new(8);
        push_for(&mut levels, 0.3, |_| -35.0);
        levels.classify(SpeechInput::Unavailable);
        let heights = levels.heights();
        // Pill 0 closed and is painted; pill 1 is still open, a dot.
        assert!(heights[heights.len() - 2] > 0.0, "{heights:?}");
        assert_eq!(heights[heights.len() - 1], 0.0);
    }

    #[test]
    fn a_pill_takes_the_loudest_moment_of_its_time() {
        let mut quiet_then_loud = PillLevels::new(8);
        quiet_then_loud.push(-55.0, 0.0);
        quiet_then_loud.push(-35.0, 0.07);
        quiet_then_loud.push(-55.0, 0.14);
        quiet_then_loud.push(-55.0, 0.21);
        let mut steady = PillLevels::new(8);
        steady.push(-35.0, 0.0);
        steady.push(-55.0, 0.21);
        quiet_then_loud.classify(SpeechInput::Unavailable);
        steady.classify(SpeechInput::Unavailable);
        let pill = |levels: &PillLevels| levels.heights()[levels.heights().len() - 2];
        assert!(pill(&quiet_then_loud) > 0.0);
        // The same peak, within the rounding of the range's time steps.
        assert!((pill(&quiet_then_loud) - pill(&steady)).abs() < 1e-5);
    }

    #[test]
    fn pills_already_painted_keep_their_height_when_speech_gets_louder() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 2.0, office_noise);
        push_for(&mut levels, 4.0, soft_speech);
        levels.classify(SpeechInput::Unavailable);
        // The newest pill is still open; the others are painted.
        let painted = levels.heights()[..levels.heights().len() - 1].to_vec();
        let open_before = levels.open_index();

        push_for(&mut levels, 1.5, loud_speech);
        levels.classify(SpeechInput::Unavailable);

        let shift = (levels.open_index() - open_before) as usize;
        assert!((7..=8).contains(&shift), "{shift}");
        assert_eq!(&levels.heights()[..painted.len() - shift], &painted[shift..]);
    }

    #[test]
    fn pills_waiting_for_a_result_after_they_scrolled_away_are_dropped() {
        let mut levels = PillLevels::new(8);
        // The analysis never answers.
        push_for(&mut levels, 10.0, |_| -35.0);
        levels.classify(SpeechInput::Windows(&[]));
        assert!(levels.pending.len() <= 8, "{}", levels.pending.len());
    }

    #[test]
    fn a_pills_speech_is_decided_once_a_window_reaches_its_end() {
        let pill = (0.2, 0.4);
        assert_eq!(pill_speech(pill, &[window(0.0, 0.3, true)]), None);
        assert_eq!(pill_speech(pill, &[window(0.0, 0.3, true), window(0.25, 0.75, false)]), Some(true));
        assert_eq!(pill_speech(pill, &[window(0.25, 0.75, false)]), Some(false));
        // A window that ends before the pill starts says nothing about it.
        assert_eq!(pill_speech(pill, &[window(-0.4, 0.1, true), window(0.3, 0.8, false)]), Some(false));
    }

    #[test]
    fn louder_speech_raises_the_range_instead_of_pinning_the_pills() {
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.observe(office_noise(tick), TICK);
        }
        for tick in 0..53 {
            hear(&mut range, soft_speech(tick), TICK);
        }
        for tick in 0..27 {
            hear(&mut range, loud_speech(tick), TICK);
        }
        assert_eq!(range.height(-20.0), 1.0, "{range:?}");
        assert!(range.height(-25.0) < 0.9, "{range:?}");
    }

    #[test]
    fn the_range_falls_back_to_soft_speech_after_a_loud_moment() {
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.observe(office_noise(tick), TICK);
        }
        hear(&mut range, -15.0, TICK);
        for tick in 0..107 {
            hear(&mut range, soft_speech(tick), TICK);
        }
        assert_eq!(range.height(-37.0), 1.0, "{range:?}");
    }

    #[test]
    fn a_long_gap_between_updates_keeps_the_learned_range() {
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.observe(office_noise(tick), TICK);
        }
        for tick in 0..53 {
            hear(&mut range, soft_speech(tick), TICK);
        }
        let learned = range.clone();
        range.observe(-58.0, 60.0);
        assert!((range.ceiling_db - learned.ceiling_db).abs() <= 1.0, "{learned:?} -> {range:?}");
        assert!(range.height(-37.0) >= 0.9, "{range:?}");
    }

    #[test]
    fn a_long_soft_passage_does_not_fade_into_the_noise() {
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.observe(office_noise(tick), TICK);
        }
        for tick in 0..400 {
            hear(&mut range, soft_speech(tick), TICK);
        }
        assert!(range.height(-42.0) >= 0.5, "{range:?}");
        assert_eq!(range.height(-37.0), 1.0, "{range:?}");
    }

    #[test]
    fn a_room_that_gets_louder_settles_back_to_dots() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 2.0, office_noise);
        push_for(&mut levels, 13.0, |tick| office_noise(tick) + 10.0);
        levels.classify(SpeechInput::Unavailable);
        let recent = &levels.heights()[levels.heights().len() - 8..];
        assert_eq!(max_of(recent), 0.0, "{recent:?}");
    }

    #[test]
    fn speech_from_the_first_moment_shows() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 0.6, soft_speech);
        levels.classify(SpeechInput::Unavailable);
        assert!(max_of(levels.heights()) >= 0.5, "{:?}", levels.range);
    }

    #[test]
    fn an_inactive_microphone_rests_without_teaching_the_range() {
        let mut levels = PillLevels::new(8);
        push_for(&mut levels, 2.0, office_noise);
        push_for(&mut levels, 4.0, soft_speech);
        levels.classify(SpeechInput::Unavailable);
        let learned = levels.range.clone();
        for _ in 0..27 {
            levels.push_rest(next_at(&levels));
        }
        levels.classify(SpeechInput::Unavailable);
        assert_eq!(levels.heights(), &[0.0; 8]);
        assert_eq!(levels.range, learned);
    }

    #[test]
    fn pills_follow_the_pace_of_speech_not_of_updates() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 2.0, |_| -30.0);
        levels.classify(SpeechInput::Unavailable);
        assert_eq!(levels.open_index(), 9);
        // Pills 0 to 8 are painted; pill 9 is open.
        let spoken = levels.heights().iter().filter(|height| **height > 0.0).count();
        assert_eq!(spoken, 9, "{:?}", levels.heights());
    }

    #[test]
    fn a_late_update_opens_every_pill_whose_time_began() {
        let mut levels = PillLevels::new(8);
        levels.push(-30.0, 0.0);
        levels.push(-30.0, 0.65);
        levels.classify(SpeechInput::Unavailable);
        assert_eq!(levels.open_index(), 3);
        let heights = levels.heights();
        // Pill 0 is painted, pills 1 and 2 passed without a level, pill 3 is open.
        assert!(heights[heights.len() - 4] > 0.0, "{heights:?}");
        assert_eq!(heights[heights.len() - 3..], [0.0, 0.0, 0.0]);
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
        let mut levels = PillLevels::new(8);
        push_for(&mut levels, 2.0, office_noise);
        push_for(&mut levels, 4.0, soft_speech);
        levels.classify(SpeechInput::Unavailable);
        let learned = levels.range.clone();
        levels.clear();
        assert_eq!(levels.heights(), &[0.0; 8]);
        assert_eq!(levels.range, learned);
        assert_eq!(levels.open_index(), 0);
        assert_eq!(levels.last_at, None);
        assert!(levels.pending.is_empty());
    }
}
