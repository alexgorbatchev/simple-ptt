use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSMenu, NSMenuItem};
use objc2_foundation::{ns_string, NSBundle, NSString};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum UpdaterInitError {
    NotBundled,
    MissingBundleIdentifier,
    MissingFeedUrl,
    FrameworkLoad { path: PathBuf, detail: String },
    ControllerClassMissing,
    ControllerInitializationFailed,
}

impl fmt::Display for UpdaterInitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBundled => formatter.write_str("not running inside a packaged .app bundle"),
            Self::MissingBundleIdentifier => {
                formatter.write_str("app bundle has no bundle identifier")
            }
            Self::MissingFeedUrl => formatter.write_str("app bundle has no SUFeedURL"),
            Self::FrameworkLoad { path, detail } => {
                write!(formatter, "could not load {}: {detail}", path.display())
            }
            Self::ControllerClassMissing => formatter
                .write_str("loaded Sparkle.framework has no SPUStandardUpdaterController class"),
            Self::ControllerInitializationFailed => {
                formatter.write_str("could not initialize SPUStandardUpdaterController")
            }
        }
    }
}

impl std::error::Error for UpdaterInitError {}

#[derive(Debug)]
pub struct AppUpdater {
    controller: Retained<AnyObject>,
}

pub fn append_update_menu_item(menu: &NSMenu, mtm: MainThreadMarker, updater: Option<&AppUpdater>) {
    let Some(updater) = updater else {
        return;
    };
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Check for Updates…"),
            Some(sel!(checkForUpdates:)),
            ns_string!(""),
        )
    };
    unsafe {
        item.setTarget(Some(updater.controller()));
    }
    menu.addItem(&item);
}

fn load_sparkle_framework(bundle: &NSBundle) -> Result<(), UpdaterInitError> {
    if AnyClass::get(c"SPUStandardUpdaterController").is_some() {
        return Ok(());
    }
    let frameworks_path =
        bundle
            .privateFrameworksPath()
            .ok_or_else(|| UpdaterInitError::FrameworkLoad {
                path: PathBuf::from(bundle.bundlePath().to_string()),
                detail: "app bundle has no private frameworks directory".to_owned(),
            })?;
    let path = PathBuf::from(frameworks_path.to_string()).join("Sparkle.framework");
    let framework = NSBundle::bundleWithPath(&NSString::from_str(&path.to_string_lossy()))
        .ok_or_else(|| UpdaterInitError::FrameworkLoad {
            path: path.clone(),
            detail: "framework bundle is missing or invalid".to_owned(),
        })?;
    // Loading code is unsafe; this is exclusively the framework shipped inside
    // the application's own bundle, never a developer or working-directory path.
    unsafe { framework.loadAndReturnError() }.map_err(|error| UpdaterInitError::FrameworkLoad {
        path,
        detail: error.localizedDescription().to_string(),
    })
}

fn validate_sparkle_bundle(bundle: &NSBundle) -> Result<(), UpdaterInitError> {
    let bundle_path = bundle.bundlePath().to_string();
    if !Path::new(&bundle_path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    {
        return Err(UpdaterInitError::NotBundled);
    }
    if bundle.bundleIdentifier().is_none() {
        return Err(UpdaterInitError::MissingBundleIdentifier);
    }
    if bundle
        .objectForInfoDictionaryKey(ns_string!("SUFeedURL"))
        .is_none()
    {
        return Err(UpdaterInitError::MissingFeedUrl);
    }
    Ok(())
}

impl AppUpdater {
    pub fn init(_mtm: MainThreadMarker) -> Result<Self, UpdaterInitError> {
        let bundle = NSBundle::mainBundle();
        validate_sparkle_bundle(&bundle)?;
        load_sparkle_framework(&bundle)?;

        let cls = AnyClass::get(c"SPUStandardUpdaterController")
            .ok_or(UpdaterInitError::ControllerClassMissing)?;

        let controller: Option<Retained<AnyObject>> = unsafe {
            let alloc: *mut AnyObject = msg_send![cls, alloc];
            if alloc.is_null() {
                return Err(UpdaterInitError::ControllerInitializationFailed);
            }
            let instance_ptr: *mut AnyObject = msg_send![
                alloc,
                initWithStartingUpdater: true,
                updaterDelegate: std::ptr::null::<AnyObject>(),
                userDriverDelegate: std::ptr::null::<AnyObject>()
            ];
            Retained::from_raw(instance_ptr)
        };

        let controller = controller.ok_or(UpdaterInitError::ControllerInitializationFailed)?;
        log::info!("Sparkle updater initialized successfully");
        Ok(Self { controller })
    }

    pub fn controller(&self) -> &AnyObject {
        &self.controller
    }

    pub fn set_automatically_checks_for_updates(&self, enabled: bool) {
        unsafe {
            let updater: Option<Retained<AnyObject>> = msg_send![&self.controller, updater];
            if let Some(updater) = updater {
                let _: () = msg_send![&updater, setAutomaticallyChecksForUpdates: enabled];
                // One week between automatic checks.
                let _: () = msg_send![&updater, setUpdateCheckInterval: 604800.0f64];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDirectory;
    use std::path::PathBuf;

    /// A minimal packaged `.app` bundle without `SUFeedURL` or frameworks,
    /// inside a `TestDirectory` that removes it on drop.
    struct TestBundle {
        path: PathBuf,
        _directory: TestDirectory,
    }

    impl TestBundle {
        fn new() -> Self {
            let directory = TestDirectory::new();
            let path = directory.path().join("simple-ptt-updater.app");
            std::fs::create_dir_all(path.join("Contents")).unwrap();
            std::fs::write(
                path.join("Contents/Info.plist"),
                concat!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                    "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
                    "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">",
                    "<plist version=\"1.0\"><dict>",
                    "<key>CFBundleIdentifier</key><string>com.example.updater-test</string>",
                    "<key>CFBundlePackageType</key><string>APPL</string>",
                    "</dict></plist>"
                ),
            )
            .unwrap();
            Self {
                path,
                _directory: directory,
            }
        }

        fn bundle(&self) -> Retained<NSBundle> {
            NSBundle::bundleWithPath(&NSString::from_str(self.path.to_str().unwrap())).unwrap()
        }
    }

    #[test]
    fn unbundled_execution_reports_not_bundled() {
        assert!(matches!(
            validate_sparkle_bundle(&NSBundle::mainBundle()),
            Err(UpdaterInitError::NotBundled)
        ));
    }

    #[test]
    fn packaged_bundle_without_feed_url_reports_the_missing_metadata() {
        let fixture = TestBundle::new();
        assert!(matches!(
            validate_sparkle_bundle(&fixture.bundle()),
            Err(UpdaterInitError::MissingFeedUrl)
        ));
    }

    /// Foundation standardizes `bundlePath`, so a fixture under
    /// `/private/tmp` comes back as `/tmp`. The expected path is built from
    /// the bundle's own path, which must name the fixture on disk.
    #[test]
    fn missing_packaged_framework_reports_its_path() {
        let fixture = TestBundle::new();
        let bundle = fixture.bundle();
        let bundle_path = PathBuf::from(bundle.bundlePath().to_string());
        assert_eq!(
            std::fs::canonicalize(&bundle_path).unwrap(),
            std::fs::canonicalize(&fixture.path).unwrap()
        );
        let expected_path = bundle_path.join("Contents/Frameworks/Sparkle.framework");

        let error = load_sparkle_framework(&bundle).unwrap_err();

        assert!(
            matches!(
                &error,
                UpdaterInitError::FrameworkLoad { path, .. } if *path == expected_path
            ),
            "{error:?}"
        );
    }
}
