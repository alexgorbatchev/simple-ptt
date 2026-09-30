//! What the pills show: the level's recent history at about one pill per
//! syllable, each pill's height set from a level range that adapts to the
//! room and the speaker, so soft speech in a quiet office fills the pills as
//! well as loud speech does, and pills already painted never change.

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
/// How fast the top of the range falls back after a loud moment. It rises at
/// once to any louder level.
const CEILING_FALL_DB_PER_SECOND: f32 = 3.0;
/// The longest step one update counts as, so the time the overlay spent
/// hidden does not wear away what the range learned.
const MAX_STEP_SECONDS: f64 = 0.25;
/// Quieter levels count as this (digital silence reads -100 dBFS).
const LOWEST_LEVEL_DB: f32 = -90.0;
/// How far a pill's height moves toward its target per update, rising and
/// falling.
const HEIGHT_ATTACK: f32 = 0.6;
const HEIGHT_RELEASE: f32 = 0.35;
/// How long each pill stands for: about one syllable, since conversational
/// English runs at 4 to 5 syllables a second.
pub(super) const PILL_SECONDS: f64 = 0.2;
/// A height this close to its target (a thousandth of the pill's travel, far
/// below a pixel) takes the target, so a pill that falls silent comes to rest
/// as a dot.
const HEIGHT_SETTLE: f32 = 0.001;

/// The span of levels, in dBFS, that the pills map from rest to full
/// height: from `NOISE_GATE_DB` above the noise floor to the loudest recent
/// level.
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
    /// Takes in `level_db`, heard `seconds` after the previous level.
    pub(super) fn follow(&mut self, level_db: f32, seconds: f64) {
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
        self.ceiling_db = if level_db > self.ceiling_db {
            level_db
        } else {
            self.ceiling_db - (CEILING_FALL_DB_PER_SECOND * step as f32)
        }
        .max(self.gate_db() + MIN_SPEECH_SPAN_DB);
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

/// The pills' heights, oldest first (newest at the right), and the range
/// the next one is measured with. Pill `k` stands for the time from
/// `k × PILL_SECONDS` to `(k + 1) × PILL_SECONDS` on the strip's clock
/// (seconds since the strip started), the clock the strip moves by. The
/// newest pill is open: it takes the loudest height of its time until the
/// time is up, and is painted then.
#[derive(Debug)]
pub(super) struct PillLevels {
    pub(super) range: LevelRange,
    /// The level's height, smoothed from one update to the next.
    height: f32,
    /// Pills `open_index - len + 1` to `open_index`.
    history: Vec<f32>,
    open_index: u64,
    /// When the last level arrived, on the strip's clock.
    last_at: Option<f64>,
}

impl PillLevels {
    pub(super) fn new(count: usize) -> Self {
        Self {
            range: LevelRange::default(),
            height: 0.0,
            history: vec![0.0; count],
            open_index: 0,
            last_at: None,
        }
    }

    /// Takes in `level_db`, heard `at` on the strip's clock. The range
    /// changes only the pills painted after it.
    pub(super) fn push(&mut self, level_db: f32, at: f64) {
        let seconds = self.step_to(at);
        self.range.follow(level_db, seconds);
        let target = self.range.height(level_db);
        let smoothing = if target >= self.height { HEIGHT_ATTACK } else { HEIGHT_RELEASE };
        self.height += (target - self.height) * smoothing;
        if (target - self.height).abs() < HEIGHT_SETTLE {
            self.height = target;
        }
        self.advance(at);
    }

    /// Rests, `at` on the strip's clock, while the microphone delivers
    /// nothing; its silence says nothing about the room, so the range does
    /// not take it in.
    pub(super) fn push_rest(&mut self, at: f64) {
        self.step_to(at);
        self.height = 0.0;
        self.advance(at);
    }

    /// The index of the open pill.
    pub(super) fn open_index(&self) -> u64 {
        self.open_index
    }

    /// Seconds from the previous level to `at`.
    fn step_to(&mut self, at: f64) -> f64 {
        let seconds = self.last_at.map_or(0.0, |last_at| at - last_at);
        self.last_at = Some(at);
        seconds
    }

    /// Opens the pill whose time `at` falls in, painting the ones before it
    /// (a pill whose whole time passed between updates rests), and raises the
    /// open pill to the current height.
    fn advance(&mut self, at: f64) {
        let index = (at.max(0.0) / PILL_SECONDS).floor() as u64;
        let opened = index.saturating_sub(self.open_index);
        for _ in 0..opened.min(self.history.len() as u64) {
            push_history(&mut self.history, 0.0);
        }
        self.open_index = self.open_index.max(index);
        if let Some(open) = self.history.last_mut() {
            *open = open.max(self.height);
        }
    }

    /// Empties the pills and restarts the clock. The range keeps what it
    /// learned about the room.
    pub(super) fn clear(&mut self) {
        self.history.fill(0.0);
        self.height = 0.0;
        self.open_index = 0;
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

    fn max_of(heights: &[f32]) -> f32 {
        heights.iter().copied().fold(0.0, f32::max)
    }

    #[test]
    fn background_noise_rests_as_dots() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 10.0, office_noise);
        assert_eq!(max_of(levels.heights()), 0.0);
    }

    #[test]
    fn soft_speech_fills_the_pills_once_the_range_adapts() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 2.0, office_noise);
        push_for(&mut levels, 4.0, soft_speech);
        // The last two seconds of soft speech reach near full height, and its
        // quieter syllables stand well clear of the dots.
        let recent = &levels.heights()[levels.heights().len() - 10..];
        assert!(max_of(recent) >= 0.85, "{recent:?}");
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.follow(office_noise(tick), TICK);
        }
        for tick in 0..53 {
            range.follow(soft_speech(tick), TICK);
        }
        assert!(range.height(-42.0) >= 0.5, "{range:?}");
        assert_eq!(range.height(-37.0), 1.0, "{range:?}");
    }

    #[test]
    fn pills_already_painted_keep_their_height_when_speech_gets_louder() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 2.0, office_noise);
        push_for(&mut levels, 4.0, soft_speech);
        // The newest pill is still open; the others are painted.
        let painted = levels.heights()[..levels.heights().len() - 1].to_vec();
        let open_before = levels.open_index();

        push_for(&mut levels, 1.5, loud_speech);

        // Every painted pill still on screen moved left by one per pill
        // opened since, and kept the height it was painted with.
        let shift = (levels.open_index() - open_before) as usize;
        // 1.5 seconds opens 7 or 8 pills, by how long the open one had stood.
        assert!((7..=8).contains(&shift), "{shift}");
        assert_eq!(&levels.heights()[..painted.len() - shift], &painted[shift..]);
    }

    #[test]
    fn louder_speech_raises_the_range_instead_of_pinning_the_pills() {
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.follow(office_noise(tick), TICK);
        }
        for tick in 0..53 {
            range.follow(soft_speech(tick), TICK);
        }
        for tick in 0..27 {
            range.follow(loud_speech(tick), TICK);
        }
        // The loudest syllable is the top of the range, and the quieter one
        // stays below it rather than both pinned at full height.
        assert_eq!(range.height(-20.0), 1.0, "{range:?}");
        assert!(range.height(-25.0) < 0.9, "{range:?}");
    }

    #[test]
    fn the_range_falls_back_to_soft_speech_after_a_loud_moment() {
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.follow(office_noise(tick), TICK);
        }
        range.follow(-15.0, TICK);
        for tick in 0..107 {
            range.follow(soft_speech(tick), TICK);
        }
        assert_eq!(range.height(-37.0), 1.0, "{range:?}");
    }

    #[test]
    fn a_long_gap_between_updates_keeps_the_learned_range() {
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.follow(office_noise(tick), TICK);
        }
        for tick in 0..53 {
            range.follow(soft_speech(tick), TICK);
        }
        let learned = range.clone();
        // The overlay was hidden for a minute: the next update counts as at
        // most one short step.
        range.follow(-58.0, 60.0);
        assert!((range.ceiling_db - learned.ceiling_db).abs() <= 1.0, "{learned:?} -> {range:?}");
        assert!(range.height(-37.0) >= 0.9, "{range:?}");
    }

    #[test]
    fn a_long_soft_passage_does_not_fade_into_the_noise() {
        let mut range = LevelRange::default();
        for tick in 0..27 {
            range.follow(office_noise(tick), TICK);
        }
        for tick in 0..400 {
            range.follow(soft_speech(tick), TICK);
        }
        assert!(range.height(-42.0) >= 0.5, "{range:?}");
        assert_eq!(range.height(-37.0), 1.0, "{range:?}");
    }

    #[test]
    fn a_room_that_gets_louder_settles_back_to_dots() {
        let mut levels = PillLevels::new(76);
        push_for(&mut levels, 2.0, office_noise);
        // A fan starts: steady noise 10 dB up, for a second longer than the
        // floor window.
        push_for(&mut levels, 13.0, |tick| office_noise(tick) + 10.0);
        let recent = &levels.heights()[levels.heights().len() - 8..];
        assert_eq!(max_of(recent), 0.0, "{recent:?}");
    }

    #[test]
    fn speech_from_the_first_moment_shows() {
        let mut levels = PillLevels::new(76);
        // Talking as the recording starts, before the room was heard.
        push_for(&mut levels, 0.6, soft_speech);
        assert!(max_of(levels.heights()) >= 0.5, "{:?}", levels.range);
    }

    #[test]
    fn an_inactive_microphone_rests_without_teaching_the_range() {
        let mut levels = PillLevels::new(8);
        push_for(&mut levels, 2.0, office_noise);
        push_for(&mut levels, 4.0, soft_speech);
        let learned = levels.range.clone();
        for _ in 0..27 {
            levels.push_rest(next_at(&levels));
        }
        assert_eq!(levels.heights(), &[0.0; 8]);
        assert_eq!(levels.range, learned);
    }

    #[test]
    fn pills_follow_the_pace_of_speech_not_of_updates() {
        let mut levels = PillLevels::new(76);
        // Two seconds of steady speech at the overlay's update interval, the
        // last update 1.95 seconds in.
        push_for(&mut levels, 2.0, |_| -30.0);
        // One pill per PILL_SECONDS of the strip's clock: pills 0 to 9.
        assert_eq!(levels.open_index(), 9);
        let spoken = levels.heights().iter().filter(|height| **height > 0.0).count();
        assert_eq!(spoken, 10, "{:?}", levels.heights());
    }

    #[test]
    fn a_pill_shows_the_loudest_moment_of_its_time() {
        let mut levels = PillLevels::new(8);
        // A short syllable between quieter moments, all within the first
        // pill's time.
        levels.push(-55.0, 0.02);
        levels.push(-35.0, 0.07);
        let syllable = levels.height;
        assert!(syllable > 0.0);
        levels.push(-55.0, 0.12);
        assert!(levels.height < syllable);
        assert_eq!(levels.open_index(), 0);
        assert_eq!(levels.heights().last(), Some(&syllable), "{:?}", levels.heights());
    }

    #[test]
    fn a_late_update_opens_every_pill_whose_time_began() {
        let mut levels = PillLevels::new(8);
        levels.push(-30.0, 0.0);
        // The overlay stalled: the next update lands 0.65 seconds in, when
        // pill 3 is open; pills 1 and 2 passed without a level.
        levels.push(-30.0, 0.65);
        assert_eq!(levels.open_index(), 3);
        let heights = levels.heights();
        assert_eq!(heights[heights.len() - 3..heights.len() - 1], [0.0, 0.0]);
        assert!(heights[heights.len() - 1] > 0.0);
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
        let learned = levels.range.clone();
        levels.clear();
        assert_eq!(levels.heights(), &[0.0; 8]);
        assert_eq!(levels.range, learned);
        // The next strip starts its clock and its pills from zero.
        assert_eq!(levels.open_index(), 0);
        assert_eq!(levels.last_at, None);
    }
}
