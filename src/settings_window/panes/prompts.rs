use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSLayoutAttribute, NSStackView, NSTextView, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::NSString;

use super::{activate, pane_view, PaneContentHeight};
use crate::settings_window::controls::{for_auto_layout, prompt_editor, section_header};
use crate::settings_window::form::PromptsForm;

const EDITOR_MIN_HEIGHT: f64 = 120.0;
const LABEL_TO_EDITOR_SPACING: f64 = 6.0;
const SECTION_SPACING: f64 = 16.0;

/// The dictation and correction prompt editors, stacked vertically. Both
/// editors fill the pane width and share its height equally.
#[derive(Debug)]
pub struct PromptsPane {
    system_prompt_view: Retained<NSTextView>,
    correction_system_prompt_view: Retained<NSTextView>,
}

impl PromptsPane {
    pub fn new(mtm: MainThreadMarker) -> (Self, Retained<NSView>) {
        let system_prompt_label = section_header(mtm, "Dictation prompt");
        let (system_prompt_scroll_view, system_prompt_view) = prompt_editor(mtm);
        let correction_prompt_label = section_header(mtm, "Correction prompt");
        let (correction_prompt_scroll_view, correction_system_prompt_view) = prompt_editor(mtm);

        let stack = for_auto_layout(NSStackView::new(mtm));
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        for view in [
            &system_prompt_label as &NSView,
            &system_prompt_scroll_view,
            &correction_prompt_label,
            &correction_prompt_scroll_view,
        ] {
            stack.addArrangedSubview(view);
        }
        stack.setCustomSpacing_afterView(LABEL_TO_EDITOR_SPACING, &system_prompt_label);
        stack.setCustomSpacing_afterView(SECTION_SPACING, &system_prompt_scroll_view);
        stack.setCustomSpacing_afterView(LABEL_TO_EDITOR_SPACING, &correction_prompt_label);

        activate(&[
            system_prompt_scroll_view
                .widthAnchor()
                .constraintEqualToAnchor(&stack.widthAnchor()),
            correction_prompt_scroll_view
                .widthAnchor()
                .constraintEqualToAnchor(&stack.widthAnchor()),
            system_prompt_scroll_view
                .heightAnchor()
                .constraintGreaterThanOrEqualToConstant(EDITOR_MIN_HEIGHT),
            correction_prompt_scroll_view
                .heightAnchor()
                .constraintEqualToAnchor(&system_prompt_scroll_view.heightAnchor()),
        ]);

        let pane = Self {
            system_prompt_view,
            correction_system_prompt_view,
        };
        let view = pane_view(mtm, &stack, PaneContentHeight::Fill);
        (pane, view)
    }

    pub fn load(&self, form: &PromptsForm) {
        self.system_prompt_view
            .setString(&NSString::from_str(&form.system_prompt));
        self.correction_system_prompt_view
            .setString(&NSString::from_str(&form.correction_system_prompt));
    }

    pub fn read(&self) -> PromptsForm {
        PromptsForm {
            system_prompt: self.system_prompt_view.string().to_string(),
            correction_system_prompt: self.correction_system_prompt_view.string().to_string(),
        }
    }
}
