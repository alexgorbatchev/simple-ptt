use std::cell::OnceCell;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSImageScaling, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem, NSWorkspace,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSURL,
};

use crate::icon::{make_application_icon, make_status_bar_active_icon, make_status_bar_icon};
use crate::overlay::{OverlayStyle, OverlayWindow};
use crate::state::{AppState, STATE_ERROR, STATE_IDLE, STATE_PROCESSING, STATE_RECORDING};

const APP_DISPLAY_NAME: &str = "simple-ptt";
const GITHUB_REPO_URL: &str = "https://github.com/alexgorbatchev/simple-ptt";
const NS_VARIABLE_STATUS_ITEM_LENGTH: f64 = -1.0;

pub struct Ivars {
    billing_menu_item: OnceCell<Retained<NSMenuItem>>,
    overlay_style: OverlayStyle,
    active_status_bar_icon: Retained<objc2_app_kit::NSImage>,
    idle_status_bar_icon: Retained<objc2_app_kit::NSImage>,
    overlay_window: OnceCell<OverlayWindow>,
    status_item: OnceCell<Retained<NSStatusItem>>,
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

            let title_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &objc2_foundation::NSString::from_str(&format!(
                        "{} — Version {}",
                        APP_DISPLAY_NAME,
                        env!("CARGO_PKG_VERSION")
                    )),
                    None,
                    ns_string!(""),
                )
            };
            title_item.setEnabled(false);
            menu.addItem(&title_item);

            let github_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!("GitHub Repo"),
                    Some(sel!(openGitHubRepo:)),
                    ns_string!(""),
                )
            };
            unsafe {
                github_item.setTarget(Some(self));
            }
            menu.addItem(&github_item);

            let billing_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!(""),
                    None,
                    ns_string!(""),
                )
            };
            billing_item.setEnabled(false);
            billing_item.setHidden(true);
            menu.addItem(&billing_item);

            let quit_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    ns_string!("Close"),
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
                .billing_menu_item
                .set(billing_item)
                .expect("billing item must only be set once");

            log::info!("menu bar initialized");
        }
    }

    impl AppDelegate {
        #[unsafe(method(openGitHubRepo:))]
        fn open_github_repo(&self, _sender: Option<&AnyObject>) {
            let github_url = NSString::from_str(GITHUB_REPO_URL);
            let Some(url) = NSURL::URLWithString(&github_url) else {
                log::error!("invalid GitHub URL configured: {}", GITHUB_REPO_URL);
                return;
            };

            let opened = NSWorkspace::sharedWorkspace().openURL(&url);
            if !opened {
                log::error!("failed to open GitHub URL: {}", GITHUB_REPO_URL);
            }
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker, overlay_style: OverlayStyle) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            billing_menu_item: OnceCell::new(),
            overlay_style,
            active_status_bar_icon: make_status_bar_active_icon(mtm),
            idle_status_bar_icon: make_status_bar_icon(mtm),
            overlay_window: OnceCell::new(),
            status_item: OnceCell::new(),
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
        update_billing_menu_item(self, overlay_footer_text);
        update_overlay_window(self, mtm, state, overlay_text, overlay_footer_text);
    }
}

fn update_billing_menu_item(delegate: &AppDelegate, overlay_footer_text: &str) {
    let Some(billing_menu_item) = delegate.ivars().billing_menu_item.get() else {
        return;
    };

    match billing_menu_text(overlay_footer_text) {
        Some(billing_text) => {
            billing_menu_item.setTitle(&objc2_foundation::NSString::from_str(billing_text));
            billing_menu_item.setHidden(false);
        }
        None => billing_menu_item.setHidden(true),
    }
}

fn billing_menu_text(overlay_footer_text: &str) -> Option<&str> {
    let trimmed_overlay_footer_text = overlay_footer_text.trim();
    if trimmed_overlay_footer_text.starts_with("Billing (")
        && trimmed_overlay_footer_text.contains(": $")
    {
        Some(trimmed_overlay_footer_text)
    } else {
        None
    }
}

fn update_status_item(delegate: &AppDelegate, mtm: MainThreadMarker, state: u8) {
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
