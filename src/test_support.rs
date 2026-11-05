use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh directory under the system temp directory, unique to this process
/// and guard, removed with everything in it when the guard drops, so a test
/// that fails an assertion still cleans up.
pub(crate) struct TestDirectory(PathBuf);

impl TestDirectory {
    pub(crate) fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "simple-ptt-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    /// A new directory whose `config.toml` holds `contents`.
    pub(crate) fn with_config(contents: &str) -> Self {
        let directory = Self::new();
        std::fs::write(directory.config(), contents).unwrap();
        directory
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// The `config.toml` path inside the directory; the file is not created.
    pub(crate) fn config(&self) -> PathBuf {
        self.0.join("config.toml")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
