use std::sync::atomic::{AtomicU64, Ordering};

/// Core Audio notifications can arrive while a replacement stream is built.
/// A rebuild acknowledges only the generation it started with.
pub(super) struct DeviceChanges {
    generation: AtomicU64,
}

impl DeviceChanges {
    pub(super) const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
        }
    }

    pub(super) fn notify(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub(super) fn has_changed_since(&self, generation: u64) -> bool {
        self.generation() != generation
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceChanges;

    #[test]
    fn a_reconnect_during_rebuilding_remains_pending() {
        let changes = DeviceChanges::new();
        let initial = changes.generation();
        assert!(!changes.has_changed_since(initial));

        changes.notify(); // Headphones disconnect.
        assert!(changes.has_changed_since(initial));
        let rebuilding = changes.generation();
        changes.notify(); // The same headphones reconnect while rebuilding.
        assert!(changes.has_changed_since(rebuilding));

        let rebuilt = changes.generation();
        assert!(!changes.has_changed_since(rebuilt));
        changes.notify();
        assert!(changes.has_changed_since(rebuilt));
    }
}
