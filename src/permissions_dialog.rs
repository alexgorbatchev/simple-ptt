use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::AnyObject as RuntimeAnyObject;
use objc2::{sel, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSColor,
    NSEventModifierFlags, NSFont, NSForegroundColorAttributeName, NSTextAlignment, NSTextField,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize,
    NSString,
};

use crate::permissions::{GlobalHotkeyPermissionFlow, GlobalHotkeyPermissionState};
use crate::settings_window::settings_font;

extern "C" {
    static NSFontAttributeName: &'static objc2_foundation::NSAttributedStringKey;
}

const WINDOW_HEIGHT: f64 = 250.0;
const WINDOW_WIDTH: f64 = 680.0;
const HORIZONTAL_PADDING: f64 = 20.0;
const BUTTON_HEIGHT: f64 = 30.0;
const BUTTON_GAP: f64 = 10.0;
const TOP_BUTTON_ROW_Y: f64 = 54.0;
const BOTTOM_BUTTON_ROW_Y: f64 = 18.0;
const LABEL_WIDTH: f64 = WINDOW_WIDTH - (HORIZONTAL_PADDING * 2.0);
const THREE_BUTTON_ROW_WIDTH: f64 = (LABEL_WIDTH - (BUTTON_GAP * 2.0)) / 3.0;
const TITLE_FONT_SIZE: f64 = 15.0;
const TITLE_FONT_WEIGHT: f64 = 0.0;
const BODY_FONT_SIZE: f64 = 12.0;

#[derive(Debug)]
pub struct PermissionsDialog {
    window: Retained<NSWindow>,
    summary_text_field: Retained<NSTextField>,
    accessibility_button: Retained<NSButton>,
    input_monitoring_button: Retained<NSButton>,
    microphone_button: Retained<NSButton>,
    completion_button: Retained<NSButton>,
}

impl PermissionsDialog {
    pub fn new(target: &AnyObject, mtm: MainThreadMarker) -> Self {
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
        set_view_frame(
            &title_text_field,
            HORIZONTAL_PADDING,
            192.0,
            LABEL_WIDTH,
            22.0,
        );
        root_view.addSubview(&title_text_field);

        let summary_text_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        configure_wrapping_label(&summary_text_field, BODY_FONT_SIZE);
        set_view_frame(
            &summary_text_field,
            HORIZONTAL_PADDING,
            104.0,
            LABEL_WIDTH,
            78.0,
        );
        root_view.addSubview(&summary_text_field);

        let accessibility_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Request Accessibility"),
                Some(target),
                Some(sel!(requestAccessibilityPermission:)),
                mtm,
            )
        };
        accessibility_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*accessibility_button,
            HORIZONTAL_PADDING,
            TOP_BUTTON_ROW_Y,
            THREE_BUTTON_ROW_WIDTH,
            BUTTON_HEIGHT,
        );
        root_view.addSubview(&accessibility_button);

        let input_monitoring_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Request Input Monitoring"),
                Some(target),
                Some(sel!(requestInputMonitoringPermission:)),
                mtm,
            )
        };
        input_monitoring_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*input_monitoring_button,
            HORIZONTAL_PADDING + THREE_BUTTON_ROW_WIDTH + BUTTON_GAP,
            TOP_BUTTON_ROW_Y,
            THREE_BUTTON_ROW_WIDTH,
            BUTTON_HEIGHT,
        );
        root_view.addSubview(&input_monitoring_button);

        let microphone_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Request Microphone"),
                Some(target),
                Some(sel!(requestMicrophonePermission:)),
                mtm,
            )
        };
        microphone_button.setFont(Some(&settings_font()));
        set_view_frame(
            &*microphone_button,
            HORIZONTAL_PADDING + (THREE_BUTTON_ROW_WIDTH * 2.0) + (BUTTON_GAP * 2.0),
            TOP_BUTTON_ROW_Y,
            THREE_BUTTON_ROW_WIDTH,
            BUTTON_HEIGHT,
        );
        root_view.addSubview(&microphone_button);

        let reset_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Reset Permissions"),
                Some(target),
                Some(sel!(resetHotkeyPermissions:)),
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
                Some(target),
                Some(sel!(recheckHotkeyPermissions:)),
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
                Some(target),
                Some(sel!(quitFromPermissionsDialog:)),
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

        let dialog = Self {
            window,
            summary_text_field,
            accessibility_button,
            input_monitoring_button,
            microphone_button,
            completion_button: quit_button,
        };
        dialog.sync(&GlobalHotkeyPermissionFlow::unknown());
        dialog
    }

    pub fn show(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
        self.window.makeKeyAndOrderFront(None);
    }

    pub fn show_startup(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
    }

    pub fn hide(&self) {
        self.window.orderOut(None);
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    pub fn sync(&self, flow: &GlobalHotkeyPermissionFlow) {
        self.summary_text_field
            .setStringValue(&NSString::from_str(&permissions_dialog_summary_text(flow)));
        sync_permission_button(
            &self.accessibility_button,
            "Accessibility",
            flow.accessibility_state,
            "Open Accessibility",
            "Open Accessibility",
        );
        sync_permission_button(
            &self.input_monitoring_button,
            "Input Monitoring",
            flow.input_monitoring_state,
            "Request Input Monitoring",
            "Open Input Monitoring",
        );
        sync_permission_button(
            &self.microphone_button,
            "Microphone",
            flow.microphone_state,
            "Request Microphone",
            "Open Microphone",
        );
        sync_completion_button(&self.completion_button, flow);
    }
}

fn permissions_dialog_summary_text(flow: &GlobalHotkeyPermissionFlow) -> String {
    if matches!(
        flow.accessibility_state,
        GlobalHotkeyPermissionState::Unknown
    ) || matches!(
        flow.input_monitoring_state,
        GlobalHotkeyPermissionState::Unknown
    ) {
        return "Checking macOS permission status…".to_owned();
    }

    if flow.relaunch_required() {
        return concat!(
            "Permissions are granted. Click Quit and Reopen to restart simple-ptt with working global shortcuts and paste automation. ",
            "Use Reset Permissions only if macOS gets stuck or stops showing the app in Privacy & Security."
        )
        .to_owned();
    }

    if matches!(
        flow.accessibility_state,
        GlobalHotkeyPermissionState::Requested
    ) || matches!(
        flow.input_monitoring_state,
        GlobalHotkeyPermissionState::Requested
    ) || matches!(
        flow.microphone_state,
        GlobalHotkeyPermissionState::Requested
    ) {
        return concat!(
            "Finish the remaining grants in macOS, then click Re-check. ",
            "If a permission button switches to Open, macOS now requires that privacy pane instead of another prompt. ",
            "Microphone access can start working in the current app session after Re-check. Accessibility and Input Monitoring still commonly require quitting and reopening simple-ptt before the full hotkey flow becomes active. ",
            "If you leave this window, reopen it from the menu bar with Application Permissions…."
        )
        .to_owned();
    }

    if flow.all_granted() {
        return "Accessibility, Input Monitoring, and Microphone are already granted for this app bundle."
            .to_owned();
    }

    concat!(
        "simple-ptt needs Input Monitoring to listen for the global CGEventTap hotkeys, Accessibility to drive the synthetic paste shortcut, and Microphone access to capture audio. ",
        "Click each permission button to ask macOS for access. If macOS stops prompting or a grant looks stale, use Reset Permissions and try again. ",
        "If you leave this window, reopen it from the menu bar with Application Permissions…."
    )
    .to_owned()
}

fn sync_completion_button(button: &NSButton, flow: &GlobalHotkeyPermissionFlow) {
    let title = permissions_completion_button_title(flow);
    button.setTitle(&NSString::from_str(title));
}

fn sync_permission_button(
    button: &NSButton,
    permission_name: &str,
    state: GlobalHotkeyPermissionState,
    missing_title: &str,
    requested_title: &str,
) {
    let title = permission_button_title(permission_name, state, missing_title, requested_title);
    let title_string = NSString::from_str(&title);
    let granted = matches!(
        state,
        GlobalHotkeyPermissionState::Granted | GlobalHotkeyPermissionState::NeedsRelaunch
    );

    button.setEnabled(!granted && !matches!(state, GlobalHotkeyPermissionState::Unknown));
    if granted {
        let bezel_color = NSColor::systemGreenColor();
        let text_color: Retained<RuntimeAnyObject> = NSColor::whiteColor().into();
        let font: Retained<RuntimeAnyObject> = settings_font().into();
        let attributes = NSDictionary::<
            objc2_foundation::NSAttributedStringKey,
            RuntimeAnyObject,
        >::from_retained_objects(
            &[unsafe { NSForegroundColorAttributeName }, unsafe { NSFontAttributeName }],
            &[text_color, font],
        );
        let attributed_title =
            unsafe { NSAttributedString::new_with_attributes(&title_string, &attributes) };

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

fn permissions_completion_button_title(flow: &GlobalHotkeyPermissionFlow) -> &'static str {
    if flow.relaunch_required() {
        return "Quit and Reopen";
    }

    if flow.all_granted() {
        return "Close";
    }

    "Quit"
}

fn permission_button_title(
    permission_name: &str,
    state: GlobalHotkeyPermissionState,
    missing_title: &str,
    requested_title: &str,
) -> String {
    match state {
        GlobalHotkeyPermissionState::Unknown => format!("Checking {}…", permission_name),
        GlobalHotkeyPermissionState::Missing => missing_title.to_owned(),
        GlobalHotkeyPermissionState::Requested => requested_title.to_owned(),
        GlobalHotkeyPermissionState::Granted | GlobalHotkeyPermissionState::NeedsRelaunch => {
            format!("{} Granted", permission_name)
        }
    }
}

fn configure_wrapping_label(text_field: &NSTextField, font_size: f64) {
    text_field.setEditable(false);
    text_field.setSelectable(false);
    text_field.setBezeled(false);
    text_field.setBordered(false);
    text_field.setDrawsBackground(false);
    text_field.setAlignment(NSTextAlignment::Left);
    text_field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
        font_size, 0.0,
    )));
    text_field.setTextColor(Some(&NSColor::secondaryLabelColor()));
}

fn set_view_frame(view: &NSView, x: f64, y: f64, width: f64, height: f64) {
    view.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
}

#[cfg(test)]
mod tests {
    use super::{permissions_completion_button_title, permissions_dialog_summary_text};
    use crate::permissions::{
        GlobalHotkeyPermissionFlow, GlobalHotkeyPermissionState, GlobalHotkeyPermissions,
    };

    #[test]
    fn permissions_dialog_summary_mentions_recheck_after_request() {
        let text = permissions_dialog_summary_text(&GlobalHotkeyPermissionFlow {
            permissions: GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
                microphone_granted: false,
            },
            accessibility_state: GlobalHotkeyPermissionState::Requested,
            input_monitoring_state: GlobalHotkeyPermissionState::Missing,
            microphone_state: GlobalHotkeyPermissionState::Requested,
        });

        assert!(text.contains("Re-check"));
        assert!(text.contains("Microphone access can start working in the current app session"));
    }

    #[test]
    fn permissions_dialog_summary_mentions_relaunch_after_grant() {
        let text = permissions_dialog_summary_text(&GlobalHotkeyPermissionFlow {
            permissions: GlobalHotkeyPermissions {
                accessibility_granted: true,
                input_monitoring_granted: true,
                microphone_granted: true,
            },
            accessibility_state: GlobalHotkeyPermissionState::NeedsRelaunch,
            input_monitoring_state: GlobalHotkeyPermissionState::NeedsRelaunch,
            microphone_state: GlobalHotkeyPermissionState::Granted,
        });

        assert!(text.contains("Quit and Reopen"));
        assert!(text.contains("paste automation"));
    }

    #[test]
    fn permissions_completion_button_switches_to_relaunch_when_needed() {
        let title = permissions_completion_button_title(&GlobalHotkeyPermissionFlow {
            permissions: GlobalHotkeyPermissions {
                accessibility_granted: true,
                input_monitoring_granted: true,
                microphone_granted: true,
            },
            accessibility_state: GlobalHotkeyPermissionState::NeedsRelaunch,
            input_monitoring_state: GlobalHotkeyPermissionState::NeedsRelaunch,
            microphone_state: GlobalHotkeyPermissionState::Granted,
        });

        assert_eq!(title, "Quit and Reopen");
    }

    #[test]
    fn permissions_completion_button_switches_to_close_when_everything_is_ready() {
        let title = permissions_completion_button_title(&GlobalHotkeyPermissionFlow {
            permissions: GlobalHotkeyPermissions {
                accessibility_granted: true,
                input_monitoring_granted: true,
                microphone_granted: true,
            },
            accessibility_state: GlobalHotkeyPermissionState::Granted,
            input_monitoring_state: GlobalHotkeyPermissionState::Granted,
            microphone_state: GlobalHotkeyPermissionState::Granted,
        });

        assert_eq!(title, "Close");
    }
}
