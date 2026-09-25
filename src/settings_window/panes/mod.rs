//! One module per settings pane. Each pane builds its view, loads its part of
//! `SettingsForm` into its controls, and reads the controls back into it.

pub mod deepgram;
pub mod general;
pub mod microphone;
pub mod prompts;
pub mod transformation;

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSLayoutConstraint, NSView};
use objc2_foundation::NSArray;

const PANE_MARGIN: f64 = 20.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaneContentHeight {
    /// The content keeps its fitting height at the top of the pane.
    Fitting,
    /// The content fills the height of the pane.
    Fill,
}

/// Wraps `content` in a pane view with standard margins. The content fills
/// the pane width; its height either fills the pane or stays at its fitting
/// height with free space below.
pub fn pane_view(
    mtm: MainThreadMarker,
    content: &NSView,
    height: PaneContentHeight,
) -> Retained<NSView> {
    let pane = NSView::new(mtm);
    pane.addSubview(content);

    let bottom = match height {
        PaneContentHeight::Fitting => content
            .bottomAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&pane.bottomAnchor(), -PANE_MARGIN),
        PaneContentHeight::Fill => content
            .bottomAnchor()
            .constraintEqualToAnchor_constant(&pane.bottomAnchor(), -PANE_MARGIN),
    };
    activate(&[
        content
            .topAnchor()
            .constraintEqualToAnchor_constant(&pane.topAnchor(), PANE_MARGIN),
        content
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&pane.leadingAnchor(), PANE_MARGIN),
        content
            .trailingAnchor()
            .constraintEqualToAnchor_constant(&pane.trailingAnchor(), -PANE_MARGIN),
        bottom,
    ]);
    pane
}

pub fn activate(constraints: &[Retained<NSLayoutConstraint>]) {
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(constraints));
}
