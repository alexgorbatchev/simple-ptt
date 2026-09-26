//! The dictation and correction prompt editors at the bottom of the
//! Transformation pane. Both prompts go only to the transformation LLM: the
//! dictation prompt is the system prompt of a transformation, and the
//! correction prompt replaces it for correction runs.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSButton, NSLayoutAttribute, NSStackView, NSStackViewGravity, NSTextView,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSRange, NSString};

use super::activate;
use crate::config::PromptResets;
use crate::settings_window::actions::SettingsAction;
use crate::settings_window::controls::{
    for_auto_layout, prompt_editor, push_button, section_header,
};
use crate::settings_window::form::{Prompt, PromptsForm};

/// Lines of text each editor shows at the window's minimum size, unless a
/// hint row above the editors takes that height.
const EDITOR_MINIMUM_VISIBLE_LINES: usize = 5;
const LABEL_TO_EDITOR_SPACING: f64 = 6.0;
const RESET_BUTTON_TITLE: &str = "Reset to Default";

/// A header row and an editor for each prompt, added to the bottom of a
/// vertical stack. Both editors fill the stack width and share its remaining
/// height equally. Each header row has a button that puts the built-in
/// default prompt back.
#[derive(Debug)]
pub struct PromptEditors {
    system_prompt_view: Retained<NSTextView>,
    correction_system_prompt_view: Retained<NSTextView>,
    /// Prompts reset since the last `load`; see `PromptsForm::resets`.
    resets: Cell<PromptResets>,
}

impl PromptEditors {
    /// Adds the prompt headers and editors to the bottom of `stack`, a
    /// vertical stack whose spacing separates them from the views above and
    /// from each other.
    pub fn new(mtm: MainThreadMarker, target: &AnyObject, stack: &NSStackView) -> Self {
        let system_prompt_header = header_row(
            mtm,
            "Dictation prompt",
            &reset_button(
                mtm,
                target,
                SettingsAction::ResetDictationPrompt,
                "Replace the dictation prompt with the built-in default",
            ),
        );
        let (system_prompt_scroll_view, system_prompt_view) =
            prompt_editor(mtm, EDITOR_MINIMUM_VISIBLE_LINES);
        let correction_prompt_header = header_row(
            mtm,
            "Correction prompt",
            &reset_button(
                mtm,
                target,
                SettingsAction::ResetCorrectionPrompt,
                "Replace the correction prompt with the built-in default",
            ),
        );
        let (correction_prompt_scroll_view, correction_system_prompt_view) =
            prompt_editor(mtm, EDITOR_MINIMUM_VISIBLE_LINES);

        let mut constraints = vec![correction_prompt_scroll_view
            .heightAnchor()
            .constraintEqualToAnchor(&system_prompt_scroll_view.heightAnchor())];
        for view in [
            &system_prompt_header as &NSView,
            &system_prompt_scroll_view,
            &correction_prompt_header,
            &correction_prompt_scroll_view,
        ] {
            stack.addArrangedSubview(view);
            constraints.push(
                view.widthAnchor()
                    .constraintEqualToAnchor(&stack.widthAnchor()),
            );
        }
        stack.setCustomSpacing_afterView(LABEL_TO_EDITOR_SPACING, &system_prompt_header);
        stack.setCustomSpacing_afterView(LABEL_TO_EDITOR_SPACING, &correction_prompt_header);
        activate(&constraints);

        Self {
            system_prompt_view,
            correction_system_prompt_view,
            resets: Cell::new(PromptResets::default()),
        }
    }

    pub fn load(&self, form: &PromptsForm) {
        self.system_prompt_view
            .setString(&NSString::from_str(&form.system_prompt));
        self.correction_system_prompt_view
            .setString(&NSString::from_str(&form.correction_system_prompt));
        self.resets.set(form.resets);
    }

    pub fn read(&self) -> PromptsForm {
        PromptsForm {
            system_prompt: self.system_prompt_view.string().to_string(),
            correction_system_prompt: self.correction_system_prompt_view.string().to_string(),
            resets: self.resets.get(),
        }
    }

    /// Puts the default text in `prompt`'s editor as a user edit and records
    /// the reset for the next Save.
    pub fn reset_to_default(&self, prompt: Prompt) {
        let mut form = self.read();
        form.reset_to_default(prompt);
        if replace_text_as_user_edit(self.editor(prompt), form.text(prompt)) {
            self.resets.set(form.resets);
        }
    }

    fn editor(&self, prompt: Prompt) -> &NSTextView {
        match prompt {
            Prompt::Dictation => &self.system_prompt_view,
            Prompt::Correction => &self.correction_system_prompt_view,
        }
    }
}

/// "Reset to Default" button; both editors have one, so the tooltip names
/// the prompt it resets.
fn reset_button(
    mtm: MainThreadMarker,
    target: &AnyObject,
    action: SettingsAction,
    tool_tip: &str,
) -> Retained<NSButton> {
    let button = push_button(mtm, RESET_BUTTON_TITLE, target, action);
    button.setToolTip(Some(&NSString::from_str(tool_tip)));
    button
}

/// Section header on the leading side with `button` on the trailing side,
/// aligned on their first baselines.
fn header_row(mtm: MainThreadMarker, title: &str, button: &NSButton) -> Retained<NSStackView> {
    let row = for_auto_layout(NSStackView::new(mtm));
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::FirstBaseline);
    row.addView_inGravity(&section_header(mtm, title), NSStackViewGravity::Leading);
    row.addView_inGravity(button, NSStackViewGravity::Trailing);
    row
}

/// Replaces the whole text of `text_view` the way a user edit does, unlike
/// `setString`: `shouldChangeTextInRange:replacementString:` lets the text
/// view (and its delegate) refuse the change and records it for undo when the
/// view allows undo, and `didChangeText` posts the change notification.
/// Returns whether the text was replaced.
fn replace_text_as_user_edit(text_view: &NSTextView, text: &str) -> bool {
    let text = NSString::from_str(text);
    let whole_text = NSRange::new(0, text_view.string().length());
    if !text_view.shouldChangeTextInRange_replacementString(whole_text, Some(&text)) {
        return false;
    }
    text_view.replaceCharactersInRange_withString(whole_text, &text);
    text_view.didChangeText();
    true
}
