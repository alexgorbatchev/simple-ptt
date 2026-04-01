use std::cell::OnceCell;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSMenu, NSMenuItem,
    NSStatusBar, NSStatusItem,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol};

use crate::state::{AppState, STATE_ERROR, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING};

/// NSVariableStatusItemLength = -1.0
const NS_VARIABLE_STATUS_ITEM_LENGTH: f64 = -1.0;

pub struct Ivars {
    status_item: OnceCell<Retained<NSStatusItem>>,
    status_menu_item: OnceCell<Retained<NSMenuItem>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppDelegate"]
    #[ivars = Ivars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = MainThreadMarker::from(self);
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

            let status_bar = NSStatusBar::systemStatusBar();
            let status_item = status_bar.statusItemWithLength(NS_VARIABLE_STATUS_ITEM_LENGTH);

            if let Some(button) = status_item.button(mtm) {
                button.setTitle(ns_string!("🎤"));
            }

            let menu = NSMenu::new(mtm);

            let status_line = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!("Status: Idle"),
                    None,
                    ns_string!(""),
                )
            };
            status_line.setEnabled(false);
            menu.addItem(&status_line);

            menu.addItem(&NSMenuItem::separatorItem(mtm));

            let quit_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!("Quit Jarvis"),
                    Some(sel!(terminate:)),
                    ns_string!("q"),
                )
            };
            menu.addItem(&quit_item);

            status_item.setMenu(Some(&menu));

            self.ivars()
                .status_item
                .set(status_item)
                .expect("status item must only be set once");
            self.ivars()
                .status_menu_item
                .set(status_line)
                .expect("status line must only be set once");

            log::info!("menu bar initialized");
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            status_item: OnceCell::new(),
            status_menu_item: OnceCell::new(),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub fn update_status(&self, mtm: MainThreadMarker, state: u8) {
        if let Some(item) = self.ivars().status_menu_item.get() {
            let text = match state {
                STATE_RECORDING => ns_string!("Status: Listening..."),
                STATE_PROCESSING => ns_string!("Status: Transcribing..."),
                STATE_ERROR => ns_string!("Status: Error"),
                _ => ns_string!("Status: Idle"),
            };
            item.setTitle(text);
        }
        if let Some(status_item) = self.ivars().status_item.get() {
            if let Some(button) = status_item.button(mtm) {
                let icon = match state {
                    STATE_RECORDING => ns_string!("🔴"),
                    STATE_PROCESSING => ns_string!("⏳"),
                    STATE_ERROR => ns_string!("⚠️"),
                    _ => ns_string!("🎤"),
                };
                button.setTitle(icon);
            }
        }
    }
}

extern "C" {
    static _dispatch_main_q: std::ffi::c_void;
    fn dispatch_async_f(
        queue: *const std::ffi::c_void,
        context: *mut std::ffi::c_void,
        work: extern "C" fn(*mut std::ffi::c_void),
    );
}

struct UiUpdate {
    delegate_addr: usize,
    state: u8,
}

extern "C" fn perform_ui_update(ctx: *mut std::ffi::c_void) {
    let update = unsafe { Box::from_raw(ctx as *mut UiUpdate) };
    let mtm = MainThreadMarker::new().expect("perform_ui_update must run on main thread");
    let delegate = unsafe { &*(update.delegate_addr as *const AppDelegate) };
    delegate.update_status(mtm, update.state);
}

pub fn setup_status_polling(delegate: Retained<AppDelegate>, state: Arc<AppState>) {
    let delegate_addr = Retained::as_ptr(&delegate) as usize;
    std::mem::forget(delegate);

    std::thread::Builder::new()
        .name("ui-poller".into())
        .spawn(move || {
            let mut last_state = STATE_IDLE;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let current = state.get_state();
                if current != last_state {
                    last_state = current;
                    let label = match current {
                        STATE_RECORDING => "recording",
                        STATE_PROCESSING => "processing",
                        STATE_ERROR => "error",
                        _ => "idle",
                    };
                    log::info!("state changed: {}", label);

                    let update = Box::new(UiUpdate {
                        delegate_addr,
                        state: current,
                    });
                    unsafe {
                        dispatch_async_f(
                            &_dispatch_main_q,
                            Box::into_raw(update) as *mut std::ffi::c_void,
                            perform_ui_update,
                        );
                    }
                }
            }
        })
        .expect("failed to spawn ui-poller thread");
}
