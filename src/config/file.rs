use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use toml_edit::DocumentMut;

pub(super) fn load_document(path: &Path) -> Result<DocumentMut, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => contents
            .parse::<DocumentMut>()
            .map_err(|error| format!("failed to parse {} for editing: {}", path.display(), error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(error) => Err(format!("failed to read {}: {}", path.display(), error)),
    }
}

pub(super) fn write_document_atomically(path: &Path, contents: &str) -> Result<(), String> {
    // Resolve the destination before replacing its directory entry: renaming over
    // the configured path itself would replace a user's symlink.
    let (destination, permissions) = match std::fs::symlink_metadata(path) {
        Ok(_) => {
            let destination = std::fs::canonicalize(path).map_err(|error| {
                format!("failed to resolve config file {}: {error}", path.display())
            })?;
            let permissions = std::fs::metadata(&destination)
                .map_err(|error| {
                    format!("failed to inspect config file {}: {error}", path.display())
                })?
                .permissions();
            (destination, Some(permissions))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (path.to_owned(), None),
        Err(error) => {
            return Err(format!(
                "failed to inspect config file {}: {error}",
                path.display()
            ));
        }
    };
    let parent_directory = destination.parent().ok_or_else(|| {
        format!(
            "failed to determine parent directory for {}",
            path.display()
        )
    })?;
    std::fs::create_dir_all(parent_directory).map_err(|error| {
        format!(
            "failed to create config directory {}: {}",
            parent_directory.display(),
            error
        )
    })?;

    let (temporary_path, mut temporary_file) = create_temporary_config(parent_directory)
        .map_err(|error| format!("failed to create temporary config file: {error}"))?;
    let result = (|| {
        temporary_file
            .write_all(contents.as_bytes())
            .map_err(|error| {
                format!(
                    "failed to write temporary config file {}: {error}",
                    temporary_path.display()
                )
            })?;
        if let Some(permissions) = permissions {
            temporary_file
                .set_permissions(permissions)
                .map_err(|error| format!("failed to preserve config file permissions: {error}"))?;
        }
        temporary_file.sync_all().map_err(|error| {
            format!(
                "failed to sync temporary config file {}: {error}",
                temporary_path.display()
            )
        })?;
        drop(temporary_file);
        std::fs::rename(&temporary_path, &destination)
            .map_err(|error| format!("failed to replace config file {}: {error}", path.display()))
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

fn create_temporary_config(directory: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    for _ in 0..128 {
        let path = directory.join(format!(
            ".simple-ptt-config-{}-{}.tmp",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        // Exclusive creation rejects existing files and symlinks. Secret-bearing
        // contents are private from the moment the temporary file is created.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not reserve a temporary config file",
    ))
}

/// The config file exactly as it was on disk, taken before a save so a failed
/// apply can put the file back. Re-saving the previous `Config` instead would
/// not restore it: the resolved `Config` no longer records which keys the file
/// left out, so the default prompts and model would be written back.
#[derive(Debug)]
pub struct ConfigFileSnapshot {
    path: PathBuf,
    /// `None` when there was no file.
    contents: Option<String>,
}

impl ConfigFileSnapshot {
    pub fn take(path: &Path) -> Result<Self, String> {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("failed to read {}: {}", path.display(), error)),
        };
        Ok(Self {
            path: path.to_owned(),
            contents,
        })
    }

    /// Writes the snapshot back, or removes the file when there was none.
    pub fn restore(&self) -> Result<(), String> {
        match &self.contents {
            Some(contents) => write_document_atomically(&self.path, contents),
            None => match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!(
                    "failed to remove {}: {}",
                    self.path.display(),
                    error
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigFileSnapshot;
    use crate::config::test_support::EXAMPLE_CONFIG;
    use crate::config::{save_config, Config, DefaultResets};
    use crate::test_support::TestDirectory;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;

    /// A failed apply restores the file byte for byte, so keys the file left
    /// out stay out instead of coming back as defaults.
    #[test]
    fn config_file_snapshot_restores_the_file_saved_over() {
        let directory = TestDirectory::with_config(EXAMPLE_CONFIG);
        let path = directory.config();

        let snapshot = ConfigFileSnapshot::take(&path).unwrap();
        let mut config: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();
        config.transformation.model = "claude-test".to_owned();
        config.transformation.system_prompt = "Custom dictation prompt.".to_owned();
        save_config(&path, &config, &DefaultResets::default()).unwrap();
        assert_ne!(std::fs::read_to_string(&path).unwrap(), EXAMPLE_CONFIG);

        snapshot.restore().unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), EXAMPLE_CONFIG);
    }

    #[test]
    fn config_file_snapshot_of_a_missing_file_removes_the_saved_file() {
        let directory = TestDirectory::new();
        let path = directory.config();

        let snapshot = ConfigFileSnapshot::take(&path).unwrap();
        save_config(&path, &Config::default(), &DefaultResets::default()).unwrap();
        assert!(path.exists());

        snapshot.restore().unwrap();

        assert!(!path.exists());
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn existing_config_permissions_survive_save_and_rollback() {
        for permissions in [0o600, 0o640] {
            let directory = TestDirectory::with_config("# original\n");
            let path = directory.config();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(permissions)).unwrap();
            let snapshot = ConfigFileSnapshot::take(&path).unwrap();

            save_config(&path, &Config::default(), &DefaultResets::default()).unwrap();
            assert_eq!(mode(&path), permissions);
            snapshot.restore().unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "# original\n");
            assert_eq!(mode(&path), permissions);
        }
    }

    #[test]
    fn new_config_is_private() {
        let directory = TestDirectory::new();
        let path = directory.config();
        save_config(&path, &Config::default(), &DefaultResets::default()).unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn relative_symlink_and_target_survive_save_and_rollback() {
        let directory = TestDirectory::new();
        let target_directory = directory.path().join("private");
        std::fs::create_dir(&target_directory).unwrap();
        let target = target_directory.join("actual.toml");
        std::fs::write(&target, "# original\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let path = directory.config();
        symlink("private/actual.toml", &path).unwrap();
        let snapshot = ConfigFileSnapshot::take(&path).unwrap();
        let mut config = Config::default();
        config.mic.gain = 6.0;

        save_config(&path, &config, &DefaultResets::default()).unwrap();
        assert_eq!(
            std::fs::read_link(&path).unwrap(),
            Path::new("private/actual.toml")
        );
        assert_ne!(std::fs::read_to_string(&target).unwrap(), "# original\n");
        assert_eq!(mode(&target), 0o600);
        snapshot.restore().unwrap();
        assert_eq!(
            std::fs::read_link(&path).unwrap(),
            Path::new("private/actual.toml")
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "# original\n");
        assert_eq!(mode(&target), 0o600);
    }

    #[test]
    fn dangling_symlink_is_preserved_when_save_fails() {
        let directory = TestDirectory::new();
        let path = directory.config();
        symlink("missing.toml", &path).unwrap();
        assert!(save_config(&path, &Config::default(), &DefaultResets::default()).is_err());
        assert_eq!(
            std::fs::read_link(&path).unwrap(),
            Path::new("missing.toml")
        );
        assert!(!directory.path().join("missing.toml").exists());
    }
}
