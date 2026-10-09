use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const PAUSED: u64 = u64::MAX;

/// The callback and its watchdog share this stream's monotonic time origin.
/// Only atomics are touched by the real-time callback.
pub(super) struct StreamActivity {
    origin: Instant,
    last_callback_millis: AtomicU64,
    playing_since_millis: AtomicU64,
}

impl StreamActivity {
    pub(super) fn new(origin: Instant) -> Self {
        Self {
            origin,
            last_callback_millis: AtomicU64::new(0),
            playing_since_millis: AtomicU64::new(PAUSED),
        }
    }

    pub(super) fn record_callback_at(&self, now: Instant) {
        self.last_callback_millis
            .store(self.millis_at(now), Ordering::Relaxed);
    }

    pub(super) fn set_playing_at(&self, playing: bool, now: Instant) {
        if playing {
            // Repeated play requests must not extend a stalled stream's deadline.
            let _ = self.playing_since_millis.compare_exchange(
                PAUSED,
                self.millis_at(now),
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        } else {
            self.playing_since_millis.store(PAUSED, Ordering::Relaxed);
        }
    }

    pub(super) fn is_stalled_at(&self, now: Instant) -> bool {
        let playing_since = self.playing_since_millis.load(Ordering::Relaxed);
        if playing_since == PAUSED {
            return false;
        }
        let last_callback = self.last_callback_millis.load(Ordering::Relaxed);
        // A fresh start or resume also bounds the wait for the first callback.
        let last_activity = last_callback.max(playing_since);
        self.millis_at(now).saturating_sub(last_activity)
            > super::STREAM_STALL_TIMEOUT.as_millis() as u64
    }

    fn millis_at(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.origin).as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::StreamActivity;
    use std::time::{Duration, Instant};

    #[test]
    fn a_late_watchdog_detects_callbacks_that_stopped() {
        let start = Instant::now();
        let activity = StreamActivity::new(start);
        activity.set_playing_at(true, start);
        activity.record_callback_at(start + Duration::from_secs(30));

        assert!(!activity.is_stalled_at(start + Duration::from_millis(31_500)));
        assert!(activity.is_stalled_at(start + Duration::from_millis(31_501)));
    }

    #[test]
    fn a_stream_that_never_delivers_its_first_callback_times_out() {
        let start = Instant::now();
        let activity = StreamActivity::new(start);
        activity.set_playing_at(true, start);

        assert!(!activity.is_stalled_at(start + Duration::from_millis(1500)));
        assert!(activity.is_stalled_at(start + Duration::from_millis(1501)));
    }

    #[test]
    fn resuming_after_a_long_pause_gets_a_fresh_callback_deadline() {
        let start = Instant::now();
        let activity = StreamActivity::new(start);
        activity.set_playing_at(true, start);
        activity.record_callback_at(start + Duration::from_millis(100));
        activity.set_playing_at(false, start + Duration::from_millis(200));
        assert!(!activity.is_stalled_at(start + Duration::from_secs(60)));

        let resumed = start + Duration::from_secs(60);
        activity.set_playing_at(true, resumed);
        assert!(!activity.is_stalled_at(resumed));
        assert!(activity.is_stalled_at(resumed + Duration::from_millis(1501)));
        activity.record_callback_at(resumed + Duration::from_millis(1502));
        assert!(!activity.is_stalled_at(resumed + Duration::from_millis(1503)));
    }

    #[test]
    fn repeated_play_requests_do_not_restart_the_callback_deadline() {
        let start = Instant::now();
        let activity = StreamActivity::new(start);
        activity.set_playing_at(true, start);
        activity.set_playing_at(true, start + Duration::from_millis(1000));
        assert!(activity.is_stalled_at(start + Duration::from_millis(1501)));
    }
}
