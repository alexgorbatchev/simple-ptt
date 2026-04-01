use std::cell::OnceCell;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSImageScaling, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol};

use crate::icon::{make_application_icon, make_status_bar_active_icon, make_status_bar_icon};
use crate::overlay::{OverlayStyle, OverlayWindow};
use crate::state::{AppState, STATE_ERROR, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING};

const NS_VARIABLE_STATUS_ITEM_LENGTH: f64 = -1.0;

pub struct Ivars {
    overlay_style: OverlayStyle,
    active_status_bar_icon: Retained<objc2_app_kit::NSImage>,
    idle_status_bar_icon: Retained<objc2_app_kit::NSImage>,
    overlay_window: OnceCell<OverlayWindow>,
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

            let application_icon = make_application_icon(mtm);
            unsafe {
                app.setApplicationIconImage(Some(&application_icon));
            }

            if let Some(button) = status_item.button(mtm) {
                button.setImage(Some(&self.ivars().idle_status_bar_icon));
                button.setImageScaling(NSImageScaling::ScaleProportionallyDown);
                button.setTitle(ns_string!(""));
                button.setContentTintColor(None);
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
                .overlay_window
                .set(OverlayWindow::new(mtm, &self.ivars().overlay_style))
                .expect("overlay window must only be set once");
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
    pub fn new(mtm: MainThreadMarker, overlay_style: OverlayStyle) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            overlay_style,
            active_status_bar_icon: make_status_bar_active_icon(mtm),
            idle_status_bar_icon: make_status_bar_icon(mtm),
            overlay_window: OnceCell::new(),
            status_item: OnceCell::new(),
            status_menu_item: OnceCell::new(),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub fn update_ui(
        &self,
        mtm: MainThreadMarker,
        state: u8,
        overlay_text: &str,
        overlay_footer_text: &str,
    ) {
        update_status_item(self, mtm, state);
        update_overlay_window(self, mtm, state, overlay_text, overlay_footer_text);
    }
}

fn update_status_item(delegate: &AppDelegate, mtm: MainThreadMarker, state: u8) {
    if let Some(item) = delegate.ivars().status_menu_item.get() {
        let text = match state {
            STATE_RECORDING => ns_string!("Status: Listening..."),
            STATE_PROCESSING => ns_string!("Status: Transcribing..."),
            STATE_ERROR => ns_string!("Status: Error"),
            _ => ns_string!("Status: Idle"),
        };
        item.setTitle(text);
    }

    if let Some(status_item) = delegate.ivars().status_item.get() {
        if let Some(button) = status_item.button(mtm) {
            let is_active = matches!(state, STATE_RECORDING | STATE_PROCESSING);
            let icon = if is_active {
                &delegate.ivars().active_status_bar_icon
            } else {
                &delegate.ivars().idle_status_bar_icon
            };
            button.setImage(Some(icon));
            button.setContentTintColor(None);
        }
    }
}

fn update_overlay_window(
    delegate: &AppDelegate,
    mtm: MainThreadMarker,
    state: u8,
    overlay_text: &str,
    overlay_footer_text: &str,
) {
    if let Some(overlay_window) = delegate.ivars().overlay_window.get() {
        overlay_window.update(mtm, state, overlay_text, overlay_footer_text);
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
    overlay_footer_text: String,
    overlay_text: String,
    state: u8,
}

extern "C" fn perform_ui_update(ctx: *mut std::ffi::c_void) {
    let update = unsafe { Box::from_raw(ctx as *mut UiUpdate) };
    let mtm = MainThreadMarker::new().expect("perform_ui_update must run on main thread");
    let delegate = unsafe { &*(update.delegate_addr as *const AppDelegate) };
    delegate.update_ui(
        mtm,
        update.state,
        &update.overlay_text,
        &update.overlay_footer_text,
    );
}

pub fn setup_status_polling(delegate: Retained<AppDelegate>, state: Arc<AppState>) {
    let delegate_addr = Retained::as_ptr(&delegate) as usize;
    std::mem::forget(delegate);

    std::thread::Builder::new()
        .name("ui-poller".into())
        .spawn(move || {
            let mut last_overlay_footer_text = String::new();
            let mut last_overlay_text = String::new();
            let mut last_state = STATE_IDLE;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(75));
                let current_state = state.get_state();
                let current_overlay_footer_text = state.overlay_footer_text();
                let current_overlay_text = state.overlay_text();
                if current_state == last_state
                    && current_overlay_footer_text == last_overlay_footer_text
                    && current_overlay_text == last_overlay_text
                {
                    continue;
                }

                last_state = current_state;
                last_overlay_footer_text = current_overlay_footer_text.clone();
                last_overlay_text = current_overlay_text.clone();

                let label = match current_state {
                    STATE_RECORDING => "recording",
                    STATE_PROCESSING => "processing",
                    STATE_ERROR => "error",
                    _ => "idle",
                };
                log::info!(
                    "ui update: state={}, transcript_len={}",
                    label,
                    current_overlay_text.len()
                );

                let update = Box::new(UiUpdate {
                    delegate_addr,
                    overlay_footer_text: current_overlay_footer_text,
                    overlay_text: current_overlay_text,
                    state: current_state,
                });
                unsafe {
                    dispatch_async_f(
                        &_dispatch_main_q,
                        Box::into_raw(update) as *mut std::ffi::c_void,
                        perform_ui_update,
                    );
                }
            }
        })
        .expect("failed to spawn ui-poller thread");
}
