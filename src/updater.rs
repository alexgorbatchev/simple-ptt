use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel, MainThreadMarker};

#[derive(Debug)]
pub struct AppUpdater {
    controller: Retained<AnyObject>,
}

impl AppUpdater {
    pub fn init(_mtm: MainThreadMarker) -> Option<Self> {
        let cls = AnyClass::get(c"SPUStandardUpdaterController")?;

        let controller: Option<Retained<AnyObject>> = unsafe {
            let alloc: *mut AnyObject = msg_send![cls, alloc];
            if alloc.is_null() {
                return None;
            }
            let instance_ptr: *mut AnyObject = msg_send![
                alloc,
                initWithStartingUpdater: true,
                updaterDelegate: std::ptr::null::<AnyObject>(),
                userDriverDelegate: std::ptr::null::<AnyObject>()
            ];
            Retained::from_raw(instance_ptr)
        };

        let controller = controller?;
        log::info!("Sparkle updater initialized successfully");
        Some(Self { controller })
    }

    pub fn controller(&self) -> &AnyObject {
        &self.controller
    }

    pub fn check_for_updates_selector() -> Sel {
        sel!(checkForUpdates:)
    }
}
