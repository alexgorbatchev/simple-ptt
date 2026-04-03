use std::cell::{Cell, OnceCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::runtime::AnyObject as RuntimeAnyObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSButton, NSColor, NSEventMask, NSEventModifierFlags, NSFont, NSForegroundColorAttributeName,
    NSTextAlignment, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSDate, NSDictionary, NSAttributedString,
    NSDefaultRunLoopMode, NSObject, NSPoint, NSRect, NSSize, NSString,
};

use crate::permissions::{self, GlobalHotkeyPermissions};
use crate::settings_window::settings_font;

extern "C" {
    static NSFontAttributeName: &'static objc2_foundation::NSAttributedStringKey;
}

const WINDOW_HEIGHT: f64 = 250.0;
const WINDOW_WIDTH: f64 = 570.0;
const HORIZONTAL_PADDING: f64 = 20.0;
const BUTTON_HEIGHT: f64 = 30.0;
const BUTTON_GAP: f64 = 10.0;
const TOP_BUTTON_ROW_Y: f64 = 54.0;
const BOTTOM_BUTTON_ROW_Y: f64 = 18.0;
const LABEL_WIDTH: f64 = WINDOW_WIDTH - (HORIZONTAL_PADDING * 2.0);
const TWO_BUTTON_ROW_WIDTH: f64 = (LABEL_WIDTH - BUTTON_GAP) / 2.0;
const THREE_BUTTON_ROW_WIDTH: f64 = (LABEL_WIDTH - (BUTTON_GAP * 2.0)) / 3.0;
const TITLE_FONT_SIZE: f64 = 15.0;
const TITLE_FONT_WEIGHT: f64 = 0.0;
const BODY_FONT_SIZE: f64 = 12.0;

struct Ivars {
    window: OnceCell<Retained<NSWindow>>,
    summary_text_field: OnceCell<Retained<NSTextField>>,
    accessibility_button: OnceCell<Retained<NSButton>>,
    input_monitoring_button: OnceCell<Retained<NSButton>>,
    quit_requested: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SimplePttPermissionsDialogController"]
    #[ivars = Ivars]
    struct PermissionsDialogController;

    unsafe impl NSObjectProtocol for PermissionsDialogController {}
    unsafe impl NSApplicationDelegate for PermissionsDialogController {
        #[unsafe(method(applicationShouldTerminate:))]
        fn application_should_terminate(
            &self,
            _sender: &NSApplication,
        ) -> NSApplicationTerminateReply {
            self.ivars().quit_requested.set(true);
            self.hide();
            NSApplicationTerminateReply::TerminateCancel
        }
    }

    impl PermissionsDialogController {
        #[unsafe(method(requestAccessibility:))]
        fn request_accessibility(&self, _sender: Option<&AnyObject>) {
            self.hide();
            if let Err(error) = permissions::request_accessibility_access() {
                log::error!("failed to open Accessibility settings: {}", error);
            }
            self.sync_status();
            self.show_after_permission_request();
        }

        #[unsafe(method(requestInputMonitoring:))]
        fn request_input_monitoring(&self, _sender: Option<&AnyObject>) {
            self.hide();
            if let Err(error) = permissions::request_input_monitoring_access() {
                log::error!("failed to open Input Monitoring settings: {}", error);
            }
            self.sync_status();
            self.show_after_permission_request();
        }

        #[unsafe(method(resetPermissions:))]
        fn reset_permissions(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = permissions::reset_global_hotkey_permissions() {
                log::error!("failed to reset macOS permissions: {}", error);
            }
            self.sync_status();
        }

        #[unsafe(method(recheckPermissions:))]
        fn recheck_permissions(&self, _sender: Option<&AnyObject>) {
            self.sync_status();
        }

        #[unsafe(method(quitPermissions:))]
        fn quit_permissions(&self, _sender: Option<&AnyObject>) {
            self.ivars().quit_requested.set(true);
            self.hide();
        }
    }
);

impl PermissionsDialogController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            window: OnceCell::new(),
            summary_text_field: OnceCell::new(),
            accessibility_button: OnceCell::new(),
            input_monitoring_button: OnceCell::new(),
            quit_requested: Cell::new(false),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.build_window(mtm);
        this
    }

    fn build_window(&self, mtm: MainThreadMarker) {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
                ),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("Application Permissions"));
        window.center();

        let root_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
            ),
        );
        root_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        let title_text_field = NSTextField::labelWithString(
            ns_string!("simple-ptt needs permissions for global shortcuts"),
            mtm,
        );
        title_text_field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
            TITLE_FONT_SIZE,
            TITLE_FONT_WEIGHT,
        )));
        set_view_frame(&title_text_field, HORIZONTAL_PADDING, 192.0, LABEL_WIDTH, 22.0);
        root_view.addSubview(&title_text_field);

        let summary_text_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        configure_wrapping_label(&summary_text_field, BODY_FONT_SIZE);
        set_view_frame(&summary_text_field, HORIZONTAL_PADDING, 104.0, LABEL_WIDTH, 78.0);
        root_view.addSubview(&summary_text_field);

        let accessibility_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Grant Accessibility"),
                Some(self),
                Some(sel!(requestAccessibility:)),
                mtm,
            )
        };
        accessibility_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*accessibility_button,
            HORIZONTAL_PADDING,
            TOP_BUTTON_ROW_Y,
            TWO_BUTTON_ROW_WIDTH,
            BUTTON_HEIGHT,
        );
        root_view.addSubview(&accessibility_button);

        let input_monitoring_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Grant Input Monitoring"),
                Some(self),
                Some(sel!(requestInputMonitoring:)),
                mtm,
            )
        };
        input_monitoring_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*input_monitoring_button,
            HORIZONTAL_PADDING + TWO_BUTTON_ROW_WIDTH + BUTTON_GAP,
            TOP_BUTTON_ROW_Y,
            TWO_BUTTON_ROW_WIDTH,
            BUTTON_HEIGHT,
        );
        root_view.addSubview(&input_monitoring_button);

        let reset_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Reset Permissions"),
                Some(self),
                Some(sel!(resetPermissions:)),
                mtm,
            )
        };
        reset_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*reset_button,
            HORIZONTAL_PADDING,
            BOTTOM_BUTTON_ROW_Y,
            THREE_BUTTON_ROW_WIDTH,
            BUTTON_HEIGHT,
        );
        root_view.addSubview(&reset_button);

        let recheck_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Re-check"),
                Some(self),
                Some(sel!(recheckPermissions:)),
                mtm,
            )
        };
        recheck_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*recheck_button,
            HORIZONTAL_PADDING + THREE_BUTTON_ROW_WIDTH + BUTTON_GAP,
            BOTTOM_BUTTON_ROW_Y,
            THREE_BUTTON_ROW_WIDTH,
            BUTTON_HEIGHT,
        );
        root_view.addSubview(&recheck_button);

        let quit_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Quit"),
                Some(self),
                Some(sel!(quitPermissions:)),
                mtm,
            )
        };
        quit_button.setFont(Some(&settings_font()));
        quit_button.setKeyEquivalent(ns_string!("q"));
        quit_button.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
        set_view_frame(
            &*quit_button,
            HORIZONTAL_PADDING + (THREE_BUTTON_ROW_WIDTH * 2.0) + (BUTTON_GAP * 2.0),
            BOTTOM_BUTTON_ROW_Y,
            THREE_BUTTON_ROW_WIDTH,
            BUTTON_HEIGHT,
        );
        root_view.addSubview(&quit_button);

        window.setContentView(Some(&root_view));

        self.ivars()
            .window
            .set(window)
            .expect("permissions window must only be set once");
        self.ivars()
            .summary_text_field
            .set(summary_text_field)
            .expect("summary field must only be set once");
        self.ivars()
            .accessibility_button
            .set(accessibility_button)
            .expect("accessibility button must only be set once");
        self.ivars()
            .input_monitoring_button
            .set(input_monitoring_button)
            .expect("input monitoring button must only be set once");
    }

    fn show(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        self.sync_status();
        app.activate();
        if let Some(window) = self.ivars().window.get() {
            window.makeKeyAndOrderFront(None);
            window.orderFrontRegardless();
        }
    }

    fn show_after_permission_request(&self) {
        if let Some(window) = self.ivars().window.get() {
            window.orderFront(None);
        }
    }

    fn hide(&self) {
        if let Some(window) = self.ivars().window.get() {
            window.orderOut(None);
        }
    }

    fn quit_requested(&self) -> bool {
        self.ivars().quit_requested.get()
    }

    fn sync_status(&self) {
        let permissions = GlobalHotkeyPermissions::current();

        self.ivars()
            .summary_text_field
            .get()
            .expect("summary field must exist")
            .setStringValue(&NSString::from_str(&permissions_dialog_summary_text()));
        sync_permission_button(
            self.ivars()
                .accessibility_button
                .get()
                .expect("accessibility button must exist"),
            "Accessibility",
            permissions.accessibility_granted,
        );
        sync_permission_button(
            self.ivars()
                .input_monitoring_button
                .get()
                .expect("input monitoring button must exist"),
            "Input Monitoring",
            permissions.input_monitoring_granted,
        );
    }
}

pub fn show_hotkey_permissions_dialog() -> bool {
    let mtm = MainThreadMarker::new().expect("must run on main thread");
    let app = NSApplication::sharedApplication(mtm);
    let previous_activation_policy = app.activationPolicy();
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let controller = PermissionsDialogController::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    controller.show(mtm);

    let should_continue_startup = loop {
        let permissions = GlobalHotkeyPermissions::current();
        if permissions.all_granted() {
            break true;
        }

        if controller.quit_requested() {
            break false;
        }

        let expiration = NSDate::distantFuture();
        let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&expiration),
            unsafe { NSDefaultRunLoopMode },
            true,
        ) else {
            continue;
        };
        app.sendEvent(&event);
        app.updateWindows();
    };

    controller.hide();
    app.setDelegate(None);
    app.setActivationPolicy(previous_activation_policy);
    should_continue_startup
}

fn permissions_dialog_summary_text() -> String {
    concat!(
        "Use these buttons to open the matching System Settings panes. After approving access there, click Re-check. Accessibility may not turn green until the app is relaunched because macOS can keep the current process in an untrusted state.\n\n",
        "If permissions look stuck or prompts stop appearing, click Reset Permissions, then grant access again."
    )
    .to_owned()
}

fn sync_permission_button(button: &NSButton, permission_name: &str, granted: bool) {
    let title = if granted {
        format!("{} Granted", permission_name)
    } else {
        format!("Grant {}", permission_name)
    };
    let title_string = NSString::from_str(&title);

    button.setEnabled(!granted);
    if granted {
        let bezel_color = NSColor::systemGreenColor();
        let text_color: Retained<RuntimeAnyObject> = NSColor::whiteColor().into();
        let font: Retained<RuntimeAnyObject> = settings_font().into();
        let attributes = NSDictionary::<objc2_foundation::NSAttributedStringKey, RuntimeAnyObject>::from_retained_objects(
            &[unsafe { NSForegroundColorAttributeName }, unsafe { NSFontAttributeName }],
            &[text_color, font],
        );
        let attributed_title = unsafe { NSAttributedString::new_with_attributes(&title_string, &attributes) };

        button.setTitle(&title_string);
        button.setAttributedTitle(&attributed_title);
        button.setBezelColor(Some(&bezel_color));
        return;
    }

    let attributed_title = NSAttributedString::from_nsstring(&title_string);
    button.setTitle(&title_string);
    button.setAttributedTitle(&attributed_title);
    button.setBezelColor(None);
}

fn configure_wrapping_label(text_field: &NSTextField, font_size: f64) {
    text_field.setEditable(false);
    text_field.setSelectable(false);
    text_field.setBezeled(false);
    text_field.setBordered(false);
    text_field.setDrawsBackground(false);
    text_field.setAlignment(NSTextAlignment::Left);
    text_field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(font_size, 0.0)));
    text_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
}

fn set_view_frame(view: &NSView, x: f64, y: f64, width: f64, height: f64) {
    view.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
}

#[cfg(test)]
mod tests {
    use super::permissions_dialog_summary_text;

    #[test]
    fn permissions_dialog_summary_text_mentions_recheck_and_relaunch() {
        let text = permissions_dialog_summary_text();

        assert!(text.contains("Re-check"));
        assert!(text.contains("relaunched"));
    }
}
