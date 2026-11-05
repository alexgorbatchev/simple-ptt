use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSColor, NSEventModifierFlags, NSFont,
    NSLayoutAttribute, NSLayoutConstraint, NSLayoutConstraintOrientation,
    NSLayoutPriorityDefaultHigh, NSStackView, NSStackViewDistribution, NSTextAlignment,
    NSTextField, NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSArray, NSEdgeInsets, NSRect, NSString};

use crate::app::actions::ApplicationAction;
use crate::permissions::{GlobalHotkeyPermissionFlow, GlobalHotkeyPermissionState};
use crate::settings_window::style_button_bezel_and_text;

/// Width of the title, the summary, and the button rows. The summary wraps at
/// this width, and the window is as tall as the wrapped summary needs.
const CONTENT_WIDTH: f64 = 530.0;
const HORIZONTAL_PADDING: f64 = 20.0;
const TOP_PADDING: f64 = 36.0;
const BOTTOM_PADDING: f64 = 20.0;
const TITLE_TO_SUMMARY_SPACING: f64 = 12.0;
const SUMMARY_TO_BUTTONS_SPACING: f64 = 24.0;
const BUTTON_ROW_SPACING: f64 = 12.0;
const BUTTON_GAP: f64 = 10.0;
const TITLE_FONT_SIZE: f64 = 15.0;
const TITLE_FONT_WEIGHT: f64 = 0.0;
const BODY_FONT_SIZE: f64 = 12.0;
const BUTTON_FONT_SIZE: f64 = 12.0;
const BUTTON_FONT_WEIGHT: f64 = 0.0;
const RESET_BUTTON_TITLE: &str = "Reset and Relaunch";
const RECHECK_BUTTON_TITLE: &str = "Re-check";
const RELAUNCH_BUTTON_TITLE: &str = "Quit and Reopen";

fn permissions_dialog_font() -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(BUTTON_FONT_SIZE, BUTTON_FONT_WEIGHT)
}

#[derive(Debug)]
pub struct PermissionsDialog {
    window: Retained<NSWindow>,
    summary_text_field: Retained<NSTextField>,
    accessibility_button: Retained<NSButton>,
    microphone_button: Retained<NSButton>,
    completion_button: Retained<NSButton>,
}

impl PermissionsDialog {
    pub fn new(target: &AnyObject, mtm: MainThreadMarker) -> Self {
        // The content view's constraints size the window: its width from
        // `CONTENT_WIDTH`, its height from the wrapped summary.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::ZERO,
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("Application Permissions"));

        let title_text_field = NSTextField::labelWithString(
            ns_string!("simple-ptt needs permissions for global shortcuts"),
            mtm,
        );
        title_text_field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
            TITLE_FONT_SIZE,
            TITLE_FONT_WEIGHT,
        )));

        let summary_text_field = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        configure_wrapping_label(&summary_text_field, BODY_FONT_SIZE);
        // An explicit wrapping width gives the label its wrapped height before
        // the first layout pass, so the window fits the summary when it opens.
        summary_text_field.setPreferredMaxLayoutWidth(CONTENT_WIDTH);

        let accessibility_button = permissions_dialog_button(
            mtm,
            target,
            "Request Accessibility",
            ApplicationAction::RequestAccessibility,
        );
        let microphone_button = permissions_dialog_button(
            mtm,
            target,
            "Request Microphone",
            ApplicationAction::RequestMicrophone,
        );
        let reset_button = permissions_dialog_button(
            mtm,
            target,
            RESET_BUTTON_TITLE,
            ApplicationAction::ResetPermissions,
        );
        let recheck_button = permissions_dialog_button(
            mtm,
            target,
            RECHECK_BUTTON_TITLE,
            ApplicationAction::RecheckPermissions,
        );
        let quit_button =
            permissions_dialog_button(mtm, target, "Quit", ApplicationAction::CompletePermissions);
        quit_button.setKeyEquivalent(ns_string!("q"));
        quit_button.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);

        let permission_buttons = button_row(mtm, &[&accessibility_button, &microphone_button]);
        let completion_buttons = button_row(mtm, &[&reset_button, &recheck_button, &quit_button]);

        let content_view = NSStackView::new(mtm);
        content_view.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        content_view.setAlignment(NSLayoutAttribute::Leading);
        content_view.setEdgeInsets(NSEdgeInsets {
            top: TOP_PADDING,
            left: HORIZONTAL_PADDING,
            bottom: BOTTOM_PADDING,
            right: HORIZONTAL_PADDING,
        });
        // Hugging above `NSLayoutPriorityWindowSizeStayPut` lets the content
        // shrink the window as well as grow it when the summary changes.
        for orientation in [
            NSLayoutConstraintOrientation::Horizontal,
            NSLayoutConstraintOrientation::Vertical,
        ] {
            content_view
                .setHuggingPriority_forOrientation(NSLayoutPriorityDefaultHigh, orientation);
        }
        for view in [
            &title_text_field as &NSView,
            &summary_text_field,
            &permission_buttons,
            &completion_buttons,
        ] {
            content_view.addArrangedSubview(view);
        }
        content_view.setCustomSpacing_afterView(TITLE_TO_SUMMARY_SPACING, &title_text_field);
        content_view.setCustomSpacing_afterView(SUMMARY_TO_BUTTONS_SPACING, &summary_text_field);
        content_view.setCustomSpacing_afterView(BUTTON_ROW_SPACING, &permission_buttons);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            summary_text_field
                .widthAnchor()
                .constraintEqualToConstant(CONTENT_WIDTH),
            permission_buttons
                .widthAnchor()
                .constraintEqualToAnchor(&summary_text_field.widthAnchor()),
            completion_buttons
                .widthAnchor()
                .constraintEqualToAnchor(&summary_text_field.widthAnchor()),
        ]));

        window.setContentView(Some(&content_view));

        let dialog = Self {
            window,
            summary_text_field,
            accessibility_button,
            microphone_button,
            completion_button: quit_button,
        };
        dialog.sync(&GlobalHotkeyPermissionFlow::unknown());
        dialog
    }

    pub fn show(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
        self.center_if_hidden();
        self.window.makeKeyAndOrderFront(None);
    }

    pub fn show_startup(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
        self.center_if_hidden();
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
    }

    /// Centers the window at the size its current summary needs, so a long
    /// summary does not grow it below the centre. A window already on screen
    /// keeps its position.
    fn center_if_hidden(&self) {
        if !self.window.isVisible() {
            self.window.layoutIfNeeded();
            self.window.center();
        }
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
        return format!(
            "Permissions are granted. Click {RELAUNCH_BUTTON_TITLE} to restart simple-ptt with working global shortcuts and paste automation. \
             Use {RESET_BUTTON_TITLE} only if macOS gets stuck or stops showing the app in Privacy & Security."
        );
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
        return format!(
            "Finish the remaining grants in macOS, then click {RECHECK_BUTTON_TITLE}. \
             If a visible permission button switches to Open, macOS now requires that privacy pane instead of another prompt. \
             Microphone access can start working in the current app session after {RECHECK_BUTTON_TITLE}. Global shortcut access may still require quitting and reopening simple-ptt after macOS updates the app trust state. \
             If you leave this window, reopen it from the menu bar with Application Permissions…."
        );
    }

    if flow.all_granted() {
        return "Application permissions are already granted for this app bundle.".to_owned();
    }

    format!(
        "simple-ptt needs Accessibility and Input Monitoring for its global shortcuts, and Accessibility also drives the synthetic paste shortcut. \
         Input Monitoring is requested automatically. Microphone access is needed to capture audio. \
         If macOS stops prompting or a grant looks stale, use {RESET_BUTTON_TITLE} to reset permissions and restart simple-ptt. \
         If you leave this window, reopen it from the menu bar with Application Permissions…."
    )
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
    let granted = matches!(
        state,
        GlobalHotkeyPermissionState::Granted | GlobalHotkeyPermissionState::NeedsRelaunch
    );

    button.setEnabled(!granted && !matches!(state, GlobalHotkeyPermissionState::Unknown));
    if granted {
        style_button_bezel_and_text(
            button,
            &title,
            Some(NSColor::systemGreenColor()),
            Some(NSColor::whiteColor()),
        );
    } else {
        style_button_bezel_and_text(button, &title, None, None);
    }
}

fn permissions_completion_button_title(flow: &GlobalHotkeyPermissionFlow) -> &'static str {
    if flow.relaunch_required() {
        return RELAUNCH_BUTTON_TITLE;
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

fn permissions_dialog_button(
    mtm: MainThreadMarker,
    target: &AnyObject,
    title: &str,
    action: ApplicationAction,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action.selector()),
            mtm,
        )
    };
    button.setFont(Some(&permissions_dialog_font()));
    button
}

/// A row of buttons that share its width equally.
fn button_row(mtm: MainThreadMarker, buttons: &[&NSButton]) -> Retained<NSStackView> {
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setDistribution(NSStackViewDistribution::FillEqually);
    row.setSpacing(BUTTON_GAP);
    // A row as tall as its buttons leaves no slack that would keep the window
    // from shrinking to a shorter summary.
    row.setHuggingPriority_forOrientation(
        NSLayoutPriorityDefaultHigh,
        NSLayoutConstraintOrientation::Vertical,
    );
    for button in buttons {
        row.addArrangedSubview(button);
    }
    row
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
            input_monitoring_state: GlobalHotkeyPermissionState::Requested,
            microphone_state: GlobalHotkeyPermissionState::Requested,
        });

        assert!(text.contains("Re-check"));
        assert!(text.contains("Microphone access can start working in the current app session"));
    }

    #[test]
    fn permissions_dialog_summary_names_both_grants_the_global_shortcuts_need() {
        let text = permissions_dialog_summary_text(&GlobalHotkeyPermissionFlow {
            permissions: GlobalHotkeyPermissions {
                accessibility_granted: false,
                input_monitoring_granted: false,
                microphone_granted: false,
            },
            accessibility_state: GlobalHotkeyPermissionState::Missing,
            input_monitoring_state: GlobalHotkeyPermissionState::Missing,
            microphone_state: GlobalHotkeyPermissionState::Missing,
        });

        assert!(text.contains("needs Accessibility and Input Monitoring for its global shortcuts"));
        assert!(text.contains("Accessibility also drives the synthetic paste shortcut"));
        assert!(text.contains("Input Monitoring is requested automatically"));
        assert!(!text.contains("anymore"));
        assert!(text.contains("Reset and Relaunch"));
        assert!(!text.contains("Reset Permissions"));
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
        assert!(text.contains("Reset and Relaunch"));
        assert!(!text.contains("Reset Permissions"));
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
