//! The overlay's Liquid Glass surfaces.
//!
//! The main text and the correction text sit on two `NSGlassEffectView`s in
//! one window, inside an `NSGlassEffectContainerView`, which renders them
//! together and merges them when they come within its spacing; at rest they
//! are `STACK_GAP` apart and read as two surfaces. The correction glass
//! appears above the main glass with a `CorrectionEffect` and disappears
//! with the same effect in reverse. Around them, a halo
//! blurs the screen: at full radius against the panels, easing to no blur at
//! the window edge (a progressive blur, through the private `variableBlur`
//! filter; see `private_effects`). Where that filter is missing, the halo is
//! a masked sheet of glass instead. The window pops in when it appears and out
//! when it goes. How all of it looks is set by a `GlassTuning`.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::{RcBlock, StackBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{define_class, msg_send, AllocAnyThread, MainThreadOnly};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSAppearance, NSAppearanceCustomization,
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSBezierPath, NSBitmapImageRep, NSBox, NSBoxType, NSColor, NSCompositingOperation, NSDeviceRGBColorSpace,
    NSFloatingWindowLevel, NSGlassEffectContainerView, NSGlassEffectView, NSGlassEffectViewStyle,
    NSGraphicsContext, NSImage, NSPanel, NSResponder, NSScreen, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_graphics::CGImage;
use objc2_foundation::{
    MainThreadMarker, NSObject, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSTimer,
};
use objc2_quartz_core::{CALayer, CAMediaTimingFunction, CATransaction};

use super::private_effects::{
    apply_glass_internals, read_glass_internals, GlassInternals, VariableBlur,
};

/// Vertical gap between the main glass and the correction glass above it.
pub const STACK_GAP: f64 = 10.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlassStyle {
    /// Frosted and adaptive: stays legible over anything.
    Regular,
    /// Transparent with refraction at the rim; Apple pairs it with a dimming
    /// layer (a dark `tint`) for legibility.
    Clear,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayAppearance {
    System,
    Light,
    Dark,
}

/// How the correction glass appears (and, in reverse, disappears). Glass
/// inside the container ignores opacity (measured on macOS 26.6), so every
/// effect starts from a shape small or hidden enough not to show a jump, and
/// only the correction text fades.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrectionEffect {
    /// A small pill merged into the main glass's top edge stretches into the
    /// panel; the container shows the liquid bridge as they part.
    Emerge,
    /// Grows upward from a hairline above the main glass.
    Unfold,
    /// Grows from `pop_shrink` smaller about its centre, like the window.
    Pop,
    /// Slides up from under the main glass.
    Slide,
    /// No second glass: the main glass grows at its top edge to hold the
    /// correction text above the transcript, with a divider between them.
    Expand,
}

/// Timing of the correction glass's motion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrectionEasing {
    Smooth,
    /// Overshoots a little when appearing, then settles.
    Spring,
}

/// Width, as a share of the panel, and height of the pill `Emerge` starts as.
const EMERGE_WIDTH_FRACTION: f64 = 0.3;
const EMERGE_HEIGHT: f64 = 20.0;

/// The frame the correction glass starts from with `effect` when it appears
/// at `resting` (window coordinates), and ends at when it disappears.
pub fn correction_start_frame(effect: CorrectionEffect, resting: NSRect, pop_shrink: f64) -> NSRect {
    let main_top = resting.origin.y - STACK_GAP;
    match effect {
        CorrectionEffect::Emerge => {
            let width = resting.size.width * EMERGE_WIDTH_FRACTION;
            let height = EMERGE_HEIGHT.min(resting.size.height);
            NSRect::new(
                NSPoint::new(
                    resting.origin.x + ((resting.size.width - width) / 2.0),
                    main_top - (height / 2.0),
                ),
                NSSize::new(width, height),
            )
        }
        CorrectionEffect::Unfold => NSRect::new(resting.origin, NSSize::new(resting.size.width, 1.0)),
        CorrectionEffect::Pop => popped_frame(resting, pop_shrink),
        // The correction glass does not show; the main glass grows instead.
        CorrectionEffect::Expand => resting,
        CorrectionEffect::Slide => NSRect::new(
            NSPoint::new(resting.origin.x, main_top - resting.size.height),
            resting.size,
        ),
    }
}

/// Opacity the window starts its pop in from and ends its pop out at. With
/// Reduce Motion the pop is only a fade, so it fades from and to nothing.
pub fn pop_edge_opacity(tuning: &GlassTuning, reduce_motion: bool) -> f64 {
    if reduce_motion {
        0.0
    } else {
        tuning.pop_opacity.clamp(0.0, 1.0)
    }
}

/// The main glass at `main_frame` grown at its top edge to also cover the
/// correction glass's resting frame (window coordinates).
pub fn expanded_main_frame(main_frame: NSRect, resting: NSRect) -> NSRect {
    NSRect::new(
        main_frame.origin,
        NSSize::new(
            main_frame.size.width,
            resting.origin.y + resting.size.height - main_frame.origin.y,
        ),
    )
}

/// Where the correction text area sits inside the main glass while it is
/// `glass_height` tall (in the main glass's own coordinates): the gap,
/// holding the divider, then the text, against the glass's top edge. It rides
/// that edge as the glass grows and shrinks, and is level with the correction
/// glass's `resting` frame once the glass has grown to cover it.
pub fn expansion_frame(resting: NSRect, margin: f64, glass_height: f64) -> NSRect {
    let height = resting.size.height + STACK_GAP;
    NSRect::new(
        NSPoint::new(resting.origin.x - margin, glass_height - height),
        NSSize::new(resting.size.width, height),
    )
}

/// Cubic Bézier timing control points `[x1, y1, x2, y2]` for the correction
/// glass appearing or disappearing.
pub fn correction_timing(easing: CorrectionEasing, appearing: bool) -> [f32; 4] {
    match (easing, appearing) {
        (CorrectionEasing::Spring, true) => [0.34, 1.36, 0.64, 1.0],
        _ => EASE_IN_OUT,
    }
}

/// Core Animation's ease-in-ease-out curve.
const EASE_IN_OUT: [f32; 4] = [0.42, 0.0, 0.58, 1.0];

/// Everything that decides how the overlay glass looks and moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassTuning {
    pub style: GlassStyle,
    /// Colour the glass is tinted toward, as hue, saturation, brightness,
    /// and alpha (alpha 0 leaves it untinted).
    pub tint_hue: f64,
    pub tint_saturation: f64,
    pub tint_brightness: f64,
    pub tint_alpha: f64,
    pub corner_radius: f64,
    /// Glass closer than this merges while the correction glass moves.
    pub merge_spacing: f64,
    pub appearance: OverlayAppearance,
    /// Whether a blur halo surrounds the panels.
    pub halo_enabled: bool,
    /// Width of the halo on every side of the panels.
    pub halo_margin: f64,
    pub halo_style: GlassStyle,
    /// How the halo's blur strength rises from the window edge to the panels.
    pub halo_curve: HaloCurve,
    /// Blur radius of the halo right against the panels.
    pub halo_blur_radius: f64,
    /// Whether the halo also blurs under the panels, as behind the Control
    /// Center tiles, rather than stopping at their edges.
    pub halo_under_panels: bool,
    /// Alpha of the black that darkens the halo, faded like its blur.
    pub halo_dim: f64,
    /// Whether `internals` replace the glass's own inner parameters.
    pub internals_enabled: bool,
    pub internals: GlassInternals,
    pub pop_seconds: f64,
    /// How much of its size the main glass is missing when it pops.
    pub pop_shrink: f64,
    /// Opacity the window pops in from and out to.
    pub pop_opacity: f64,
    pub correction_effect: CorrectionEffect,
    pub correction_easing: CorrectionEasing,
    pub correction_seconds: f64,
}

impl Default for GlassTuning {
    fn default() -> Self {
        Self {
            style: GlassStyle::Regular,
            tint_hue: 0.0,
            tint_saturation: 0.0,
            tint_brightness: 0.0,
            tint_alpha: 0.0,
            corner_radius: 16.0,
            merge_spacing: STACK_GAP - 2.0,
            appearance: OverlayAppearance::System,
            halo_enabled: true,
            halo_margin: 40.0,
            halo_style: GlassStyle::Clear,
            halo_curve: HaloCurve {
                start: (0.18, 0.0),
                control1: (0.48, 0.0),
                control2: (0.75, 0.04),
                end: (1.0, 0.04),
            },
            halo_blur_radius: 24.0,
            halo_under_panels: true,
            halo_dim: 0.0,
            internals_enabled: false,
            internals: GlassInternals::default(),
            pop_seconds: 0.24,
            pop_shrink: 0.08,
            pop_opacity: 0.15,
            correction_effect: CorrectionEffect::Emerge,
            correction_easing: CorrectionEasing::Spring,
            correction_seconds: 0.36,
        }
    }
}

impl GlassTuning {
    /// Room kept around the panels in the window: the halo, when there is
    /// one.
    pub fn margin(&self) -> f64 {
        if self.halo_enabled {
            self.halo_margin
        } else {
            0.0
        }
    }
}

/// How the halo's blur strength changes across the halo. x runs from the
/// halo's outer edge (0) to the panel edge (1); y is the blur strength. A
/// cubic Bézier runs from `start` to `end` through the pull of `control1` and
/// `control2`; before `start` the strength holds at `start`'s, and past `end`
/// at `end`'s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HaloCurve {
    pub start: (f64, f64),
    pub control1: (f64, f64),
    pub control2: (f64, f64),
    pub end: (f64, f64),
}

impl HaloCurve {
    pub const LINEAR: Self = Self::with_controls((0.25, 0.25), (0.75, 0.75));
    pub const EASE_IN: Self = Self::with_controls((0.42, 0.0), (1.0, 1.0));
    pub const EASE_OUT: Self = Self::with_controls((0.0, 0.0), (0.58, 1.0));
    pub const EASE_IN_OUT: Self = Self::with_controls((0.42, 0.0), (0.58, 1.0));

    /// A curve from (0, 0) to (1, 1) with the given control points, as in CSS
    /// `cubic-bezier(x1, y1, x2, y2)`.
    pub const fn with_controls(control1: (f64, f64), control2: (f64, f64)) -> Self {
        Self {
            start: (0.0, 0.0),
            control1,
            control2,
            end: (1.0, 1.0),
        }
    }

    /// The strength at `x`, clamped to 0..=1. Between the end points, the
    /// control points' x are held between the end points' x, which keeps x
    /// rising along the curve, so bisection on the curve parameter finds the
    /// one point with that x.
    pub fn strength_at(&self, x: f64) -> f64 {
        let (x0, y0) = self.start;
        let (x3, y3) = self.end;
        let x = x.clamp(0.0, 1.0);
        if x <= x0 || x3 <= x0 {
            return y0.clamp(0.0, 1.0);
        }
        if x >= x3 {
            return y3.clamp(0.0, 1.0);
        }
        let x1 = self.control1.0.clamp(x0, x3);
        let x2 = self.control2.0.clamp(x0, x3);
        let (mut low, mut high) = (0.0, 1.0);
        for _ in 0..48 {
            let middle = (low + high) / 2.0;
            if cubic(middle, x0, x1, x2, x3) < x {
                low = middle;
            } else {
                high = middle;
            }
        }
        cubic((low + high) / 2.0, y0, self.control1.1, self.control2.1, y3).clamp(0.0, 1.0)
    }
}

/// One coordinate of the cubic Bézier with values `p0`..`p3` at `t`.
fn cubic(t: f64, p0: f64, p1: f64, p2: f64, p3: f64) -> f64 {
    let u = 1.0 - t;
    (u * u * u * p0) + (3.0 * u * u * t * p1) + (3.0 * u * t * t * p2) + (t * t * t * p3)
}

/// Blur strength of the halo `step` steps in from the window edge, out of
/// `steps` to the panel edge, following `curve`.
pub fn halo_strength(step: usize, steps: usize, curve: HaloCurve) -> f64 {
    if steps == 0 {
        return curve.strength_at(1.0);
    }
    curve.strength_at(step as f64 / steps as f64)
}

/// Where the correction glass is in its show/hide motion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrectionPhase {
    Hidden,
    Shown,
    /// Disappearing; the window keeps its room until the motion ends.
    Disappearing,
}

/// How the correction glass gets from one phase to the next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrectionMotion {
    None,
    /// Place it at its resting frame without motion.
    Place,
    /// Appear with the tuned `CorrectionEffect`.
    Appear,
    /// Disappear with the effect in reverse, then hide.
    Disappear,
    /// Hide at once.
    Remove,
}

/// Plans the correction glass motion when the overlay asks for it to be
/// `visible`. With Reduce Motion on, it appears and disappears in place.
pub fn plan_correction_motion(
    phase: CorrectionPhase,
    visible: bool,
    reduce_motion: bool,
) -> (CorrectionPhase, CorrectionMotion) {
    match (phase, visible) {
        (CorrectionPhase::Hidden, false) => (CorrectionPhase::Hidden, CorrectionMotion::None),
        (CorrectionPhase::Shown, true) => (CorrectionPhase::Shown, CorrectionMotion::Place),
        (CorrectionPhase::Disappearing, false) => {
            (CorrectionPhase::Disappearing, CorrectionMotion::None)
        }
        (CorrectionPhase::Hidden | CorrectionPhase::Disappearing, true) if reduce_motion => {
            (CorrectionPhase::Shown, CorrectionMotion::Place)
        }
        (CorrectionPhase::Hidden | CorrectionPhase::Disappearing, true) => {
            (CorrectionPhase::Shown, CorrectionMotion::Appear)
        }
        (CorrectionPhase::Shown, false) if reduce_motion => {
            (CorrectionPhase::Hidden, CorrectionMotion::Remove)
        }
        (CorrectionPhase::Shown, false) => {
            (CorrectionPhase::Disappearing, CorrectionMotion::Disappear)
        }
    }
}

/// Frame of the window holding a main glass of `main_frame` and, when given,
/// a correction glass of `correction_frame` above it (both in screen
/// coordinates), with `margin` around them.
pub fn window_frame(main_frame: NSRect, correction_frame: Option<NSRect>, margin: f64) -> NSRect {
    let top = correction_frame
        .map(|frame| frame.origin.y + frame.size.height)
        .unwrap_or(main_frame.origin.y + main_frame.size.height);
    NSRect::new(
        NSPoint::new(main_frame.origin.x - margin, main_frame.origin.y - margin),
        NSSize::new(
            main_frame.size.width + (margin * 2.0),
            top - main_frame.origin.y + (margin * 2.0),
        ),
    )
}

/// Frame of the main glass inside the window.
pub fn main_glass_frame(width: f64, main_height: f64, margin: f64) -> NSRect {
    NSRect::new(NSPoint::new(margin, margin), NSSize::new(width, main_height))
}

/// Resting frame of the correction glass inside the window, above a main
/// glass of `main_height`.
pub fn correction_resting_frame(
    main_height: f64,
    width: f64,
    correction_height: f64,
    margin: f64,
) -> NSRect {
    NSRect::new(
        NSPoint::new(margin, margin + main_height + STACK_GAP),
        NSSize::new(width, correction_height),
    )
}

/// `frame` shrunk by `shrink` of its size about its center: where the
/// glass pops in from and out to.
pub fn popped_frame(frame: NSRect, shrink: f64) -> NSRect {
    let inset_x = frame.size.width * shrink / 2.0;
    let inset_y = frame.size.height * shrink / 2.0;
    NSRect::new(
        NSPoint::new(frame.origin.x + inset_x, frame.origin.y + inset_y),
        NSSize::new(frame.size.width - (inset_x * 2.0), frame.size.height - (inset_y * 2.0)),
    )
}

define_class!(
    /// Correction glass. The correction text is read-only and the separate
    /// correction window it replaces ignored the mouse, so clicks on it do
    /// not reach its text view.
    #[unsafe(super(NSGlassEffectView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SimplePttPassiveGlassEffectView"]
    struct PassiveGlassEffectView;

    impl PassiveGlassEffectView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "OverlayPanel"]
    struct OverlayPanel;

    impl OverlayPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        /// Keeps the overlay's Liquid Glass in its active look. AppKit draws
        /// glass in a window without active appearance with inactive
        /// parameters (for Clear glass: blur radius 10 instead of 1, no outer
        /// refraction, a milky face), and this panel stops being key whenever
        /// another window takes focus while the overlay is up. Private
        /// `NSWindow` method, measured on macOS 26.6; see `private_effects`.
        #[unsafe(method(_hasActiveAppearance))]
        fn has_active_appearance(&self) -> bool {
            true
        }
    }
);

#[derive(Debug)]
pub struct OverlayGlass {
    pub panel: Retained<NSPanel>,
    /// The progressive blur, when this macOS has the filter for it.
    variable_blur: Option<VariableBlur>,
    /// The effect view whose blur layer runs `variable_blur`.
    blur_view: Retained<NSVisualEffectView>,
    /// The mask `variable_blur` last ran with, and what it was drawn for.
    blur_mask: RefCell<Option<(BlurMaskKey, Retained<CGImage>)>>,
    /// Darkens the halo; its mask is the blur mask.
    dim_view: Retained<NSView>,
    dim_mask: Retained<CALayer>,
    /// The glass's own `GlassInternals`, saved before the tuned ones
    /// replaced them, so turning the tuning off restores them. `None` while
    /// the glass runs its own values.
    internals_saved: Cell<Option<GlassInternals>>,
    /// The halo where `variable_blur` is missing: a sheet of glass whose
    /// layer mask fades its blur out.
    halo_view: Retained<NSView>,
    halo_glass: Retained<NSGlassEffectView>,
    halo_mask: Retained<CALayer>,
    container: Retained<NSGlassEffectContainerView>,
    main_glass: Retained<NSGlassEffectView>,
    correction_glass: Retained<NSGlassEffectView>,
    /// Holds the main text, meter, and footer, at the bottom of the main
    /// glass; resized only by `set_frames`.
    pub main_content_view: Retained<NSView>,
    /// Holds the correction text.
    pub correction_content_view: Retained<NSView>,
    /// Area at the top of the main glass holding the correction text while
    /// `CorrectionEffect::Expand` has grown the main glass.
    pub correction_expansion_view: Retained<NSView>,
    /// How much the main glass is grown at its top for `Expand`.
    main_extension: Cell<f64>,
    /// Where the main glass rests, `Expand` growth included. While it moves
    /// (a pop, an expansion) its own frame is somewhere in between, and
    /// setting a frame then cuts the motion short, so motions start from and
    /// frame changes compare with this instead.
    main_resting_frame: Rc<Cell<NSRect>>,
    /// Puts the main glass exactly at `main_resting_frame` once a motion
    /// ends; see `settle_main_glass_after`.
    main_settle_timer: RefCell<Option<Retained<NSTimer>>>,
    tuning: Cell<GlassTuning>,
    correction_phase: Rc<Cell<CorrectionPhase>>,
    /// Hides the correction glass once it has popped out; see
    /// `schedule_after_motion`.
    correction_out_timer: RefCell<Option<Retained<NSTimer>>>,
    /// Orders the window out once it has popped out.
    pop_out_timer: RefCell<Option<Retained<NSTimer>>>,
}

impl OverlayGlass {
    pub fn new(mtm: MainThreadMarker, width: f64, main_height: f64, tuning: GlassTuning) -> Self {
        let margin = tuning.margin();
        let main_frame = main_glass_frame(width, main_height, margin);
        let window_rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width + (margin * 2.0), main_height + (margin * 2.0)),
        );
        let panel = make_panel(mtm, window_rect);
        let fill = NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable;

        let root_view = NSView::initWithFrame(NSView::alloc(mtm), window_rect);
        root_view.setAutoresizingMask(fill);
        let halo_view = NSView::initWithFrame(NSView::alloc(mtm), window_rect);
        halo_view.setAutoresizingMask(fill);
        halo_view.setWantsLayer(true);
        // Clicks on the halo reach nothing, like clicks on the correction glass.
        let halo_glass: Retained<PassiveGlassEffectView> =
            unsafe { msg_send![PassiveGlassEffectView::alloc(mtm), initWithFrame: window_rect] };
        let halo_glass: Retained<NSGlassEffectView> = Retained::into_super(halo_glass);
        halo_glass.setAutoresizingMask(fill);
        halo_glass.setCornerRadius(0.0);
        halo_view.addSubview(&halo_glass);
        let halo_mask = CALayer::new();
        if let Some(layer) = halo_view.layer() {
            // SAFETY: the mask is a standalone layer owned by `OverlayGlass`.
            unsafe { layer.setMask(Some(&halo_mask)) };
        }

        let blur_view = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), window_rect);
        blur_view.setAutoresizingMask(fill);
        blur_view.setMaterial(NSVisualEffectMaterial::HUDWindow);
        blur_view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        // The overlay's app is never the active app, and an inactive effect
        // view does not blur.
        blur_view.setState(NSVisualEffectState::Active);
        let dim_view = NSView::initWithFrame(NSView::alloc(mtm), window_rect);
        dim_view.setAutoresizingMask(fill);
        dim_view.setWantsLayer(true);
        let dim_mask = CALayer::new();
        if let Some(layer) = dim_view.layer() {
            layer.setBackgroundColor(Some(&NSColor::blackColor().CGColor()));
            // SAFETY: the mask is a standalone layer owned by `OverlayGlass`.
            unsafe { layer.setMask(Some(&dim_mask)) };
        }
        let variable_blur = VariableBlur::new();
        if variable_blur.is_none() {
            log::warn!("the variableBlur filter is unavailable; the overlay halo uses masked glass");
        }

        let container = NSGlassEffectContainerView::initWithFrame(
            NSGlassEffectContainerView::alloc(mtm),
            window_rect,
        );
        container.setAutoresizingMask(fill);
        let stack_view = NSView::initWithFrame(NSView::alloc(mtm), window_rect);
        stack_view.setAutoresizingMask(fill);

        let main_glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), main_frame);
        let main_glass_content = make_glass_content(mtm, &main_glass, main_frame);
        // The transcript, meter, and footer: sized by `set_frames` only, so
        // they stay still while the glass grows at its top (`Expand`) and
        // keep their size, centred, while the window pops.
        let main_content_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), main_frame.size),
        );
        main_content_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewMaxXMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        main_content_view.setWantsLayer(true);
        main_glass_content.addSubview(&main_content_view);

        let correction_glass: Retained<PassiveGlassEffectView> =
            unsafe { msg_send![PassiveGlassEffectView::alloc(mtm), initWithFrame: main_frame] };
        let correction_glass: Retained<NSGlassEffectView> = Retained::into_super(correction_glass);
        let correction_content_view = make_glass_content(mtm, &correction_glass, main_frame);
        correction_glass.setHidden(true);

        let correction_expansion_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, main_height), NSSize::new(width, STACK_GAP)),
        );
        // Stays at the top of the main glass as it grows and shrinks.
        correction_expansion_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        correction_expansion_view.setWantsLayer(true);
        correction_expansion_view.setHidden(true);
        let divider = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, STACK_GAP / 2.0), NSSize::new(width, 1.0)),
        );
        divider.setBoxType(NSBoxType::Separator);
        divider.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        correction_expansion_view.addSubview(&divider);
        main_glass_content.addSubview(&correction_expansion_view);

        stack_view.addSubview(&main_glass);
        stack_view.addSubview(&correction_glass);
        container.setContentView(Some(&stack_view));
        // Outside the container, so the panels never merge into the halo.
        root_view.addSubview(&blur_view);
        root_view.addSubview(&dim_view);
        root_view.addSubview(&halo_view);
        root_view.addSubview(&container);
        panel.setContentView(Some(&root_view));

        let glass = Self {
            panel,
            variable_blur,
            blur_view,
            blur_mask: RefCell::new(None),
            dim_view,
            dim_mask,
            internals_saved: Cell::new(None),
            halo_view,
            halo_glass,
            halo_mask,
            container,
            main_glass,
            correction_glass,
            main_content_view,
            correction_content_view,
            correction_expansion_view,
            main_extension: Cell::new(0.0),
            main_resting_frame: Rc::new(Cell::new(main_frame)),
            main_settle_timer: RefCell::new(None),
            tuning: Cell::new(tuning),
            correction_phase: Rc::new(Cell::new(CorrectionPhase::Hidden)),
            correction_out_timer: RefCell::new(None),
            pop_out_timer: RefCell::new(None),
        };
        glass.apply_look(tuning);
        glass
    }

    pub fn tuning(&self) -> GlassTuning {
        self.tuning.get()
    }

    /// Whether the halo is the progressive blur rather than masked glass.
    pub fn halo_is_progressive(&self) -> bool {
        self.variable_blur.is_some()
    }

    /// Applies `tuning` to the glass and window. Geometry that depends on it
    /// (the halo margin) takes effect at the next `set_frames`.
    pub fn set_tuning(&self, tuning: GlassTuning) {
        if self.tuning.replace(tuning) != tuning {
            self.apply_look(tuning);
        }
    }

    fn apply_look(&self, tuning: GlassTuning) {
        let style = match tuning.style {
            GlassStyle::Regular => NSGlassEffectViewStyle::Regular,
            GlassStyle::Clear => NSGlassEffectViewStyle::Clear,
        };
        let tint = (tuning.tint_alpha > 0.0).then(|| {
            NSColor::colorWithHue_saturation_brightness_alpha(
                tuning.tint_hue,
                tuning.tint_saturation,
                tuning.tint_brightness,
                tuning.tint_alpha,
            )
        });
        for glass in [&self.main_glass, &self.correction_glass] {
            glass.setStyle(style);
            glass.setTintColor(tint.as_deref());
            glass.setCornerRadius(tuning.corner_radius);
        }
        self.container.setSpacing(tuning.merge_spacing);

        // SAFETY: both names are immutable AppKit constants.
        let appearance = match tuning.appearance {
            OverlayAppearance::System => None,
            OverlayAppearance::Light => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }),
            OverlayAppearance::Dark => {
                NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua })
            }
        };
        self.panel.setAppearance(appearance.as_deref());

        let progressive = self.variable_blur.is_some();
        self.blur_view.setHidden(!tuning.halo_enabled || !progressive);
        self.dim_view.setHidden(!tuning.halo_enabled || !progressive || tuning.halo_dim <= 0.0);
        self.dim_view.setAlphaValue(tuning.halo_dim);
        self.halo_view.setHidden(!tuning.halo_enabled || progressive);
        self.halo_glass.setStyle(match tuning.halo_style {
            GlassStyle::Regular => NSGlassEffectViewStyle::Regular,
            GlassStyle::Clear => NSGlassEffectViewStyle::Clear,
        });
        let (mask_image, contents_center) =
            halo_mask_image(tuning.halo_margin, tuning.corner_radius, tuning.halo_curve);
        // SAFETY: an NSImage is a documented type for `CALayer.contents`.
        unsafe { self.halo_mask.setContents(Some(mask_image.as_ref() as &AnyObject)) };
        self.halo_mask.setContentsCenter(contents_center);
        fit_halo_mask(&self.halo_view, &self.halo_mask);
        self.refresh_progressive_halo();
        self.refresh_internals();
    }

    /// Keeps the progressive blur running with a mask drawn for the current
    /// window size and tuning. Cheap when nothing changed; AppKit may have
    /// rebuilt the blur layer, so it checks the filter is still there.
    fn refresh_progressive_halo(&self) {
        let Some(variable_blur) = &self.variable_blur else {
            return;
        };
        let tuning = self.tuning.get();
        if !tuning.halo_enabled {
            return;
        }
        let scale = self.panel.backingScaleFactor();
        let size = self.panel.frame().size;
        let key = BlurMaskKey {
            width_px: (size.width * scale).round() as usize,
            height_px: (size.height * scale).round() as usize,
            margin_px: (tuning.halo_margin * scale).round() as usize,
            corner_px: tuning.corner_radius * scale,
            curve: tuning.halo_curve,
            under_panels: tuning.halo_under_panels,
            radius: tuning.halo_blur_radius,
        };
        let cached = self.blur_mask.borrow().as_ref().map(|(cached_key, _)| *cached_key);
        if cached != Some(key) {
            let Some(mask) = blur_mask_image(&key) else {
                return;
            };
            // SAFETY: a CGImage is a documented type for `CALayer.contents`,
            // and CF objects are valid Objective-C objects.
            unsafe {
                self.dim_mask
                    .setContents(Some(&*(&*mask as *const CGImage as *const AnyObject)));
            }
            self.blur_mask.replace(Some((key, mask)));
        } else if variable_blur.is_applied(&self.blur_view) {
            return;
        }
        fit_halo_mask(&self.dim_view, &self.dim_mask);
        if let Some((_, mask)) = self.blur_mask.borrow().as_ref() {
            variable_blur.apply(&self.blur_view, tuning.halo_blur_radius, mask, scale);
        }
    }

    /// Applies the tuned glass internals, saving the glass's own values
    /// first; once they are turned off, puts the saved values back.
    /// The container renders the main and correction glass together, so the
    /// internals live in its layers and apply to both.
    fn refresh_internals(&self) {
        if self.tuning.get().internals_enabled {
            if self.internals_saved.get().is_none() {
                self.internals_saved.set(read_glass_internals(&self.container));
            }
            apply_glass_internals(&self.container, &self.tuning.get().internals);
        } else if let Some(own) = self.internals_saved.take() {
            apply_glass_internals(&self.container, &own);
        }
    }

    /// The main glass's inner parameters as drawn now.
    pub fn glass_internals_now(&self) -> Option<GlassInternals> {
        read_glass_internals(&self.container)
    }


    pub fn correction_phase(&self) -> CorrectionPhase {
        self.correction_phase.get()
    }

    /// Screen y of the main glass's bottom edge.
    pub fn main_origin_y(&self) -> f64 {
        self.panel.frame().origin.y + self.main_resting_frame.get().origin.y
    }

    /// Sizes the window to `window_frame` (screen coordinates) and places the
    /// main glass in it.
    pub fn set_frames(&self, window_frame: NSRect, main_height: f64) {
        if self.panel.frame() != window_frame {
            self.panel.setFrame_display(window_frame, true);
            fit_halo_mask(&self.halo_view, &self.halo_mask);
            self.refresh_internals();
        }
        self.refresh_progressive_halo();
        let margin = self.tuning.get().margin();
        let main_frame = main_glass_frame(
            window_frame.size.width - (margin * 2.0),
            main_height + self.main_extension.get(),
            margin,
        );
        if self.main_resting_frame.replace(main_frame) != main_frame {
            self.main_glass.setFrame(main_frame);
        }
        let content_frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(main_frame.size.width, main_height),
        );
        if self.main_content_view.frame() != content_frame {
            self.main_content_view.setFrame(content_frame);
        }
    }

    /// Pops the window in, or back in if it is popping out. Reduce Motion
    /// fades it in without the pop.
    pub fn pop_in(&self, reduce_motion: bool) {
        let tuning = self.tuning.get();
        let was_popping_out = cancel_timer(&self.pop_out_timer);
        let main_frame = self.main_resting_frame.get();
        if !was_popping_out {
            self.panel.setAlphaValue(pop_edge_opacity(&tuning, reduce_motion));
            if !reduce_motion {
                self.main_glass
                    .setFrame(popped_frame(main_frame, tuning.pop_shrink));
            }
            self.panel.orderFrontRegardless();
            self.refresh_internals();
        }
        let panel = &self.panel;
        let main_glass = &self.main_glass;
        animate(tuning.pop_seconds, || {
            panel.animator().setAlphaValue(1.0);
            if !reduce_motion {
                main_glass.animator().setFrame(main_frame);
            }
        });
        if !reduce_motion {
            self.settle_main_glass_after(tuning.pop_seconds);
        }
    }

    /// Pops the window out and orders it out when the motion ends. Reduce
    /// Motion fades it out without the pop.
    pub fn pop_out(&self, reduce_motion: bool) {
        let tuning = self.tuning.get();
        cancel_timer(&self.correction_out_timer);
        // The pop out's end puts the glass back where it rests.
        cancel_timer(&self.main_settle_timer);
        let main_frame = self.main_resting_frame.get();
        let panel = &self.panel;
        let main_glass = &self.main_glass;
        let end_opacity = pop_edge_opacity(&tuning, reduce_motion);
        animate(tuning.pop_seconds, || {
            panel.animator().setAlphaValue(end_opacity);
            if !reduce_motion {
                main_glass
                    .animator()
                    .setFrame(popped_frame(main_frame, tuning.pop_shrink));
            }
        });

        let panel = self.panel.clone();
        let main_glass = self.main_glass.clone();
        let correction_glass = self.correction_glass.clone();
        let correction_phase = self.correction_phase.clone();
        let main_resting_frame = self.main_resting_frame.clone();
        schedule_after_motion(&self.pop_out_timer, tuning.pop_seconds, move || {
            panel.orderOut(None);
            panel.setAlphaValue(1.0);
            main_glass.setFrame(main_resting_frame.get());
            correction_phase.set(CorrectionPhase::Hidden);
            correction_glass.setHidden(true);
        });
    }

    /// Shows, moves, or hides the correction glass. `resting_frame` is in
    /// window coordinates, or `None` when the correction glass should go
    /// away. It moves with the tuned `CorrectionEffect` while its text
    /// fades; see `CorrectionEffect` for why the glass itself cannot fade.
    pub fn update_correction(&self, resting_frame: Option<NSRect>, reduce_motion: bool) {
        let tuning = self.tuning.get();
        let (phase, motion) = plan_correction_motion(
            self.correction_phase.get(),
            resting_frame.is_some(),
            reduce_motion,
        );
        self.correction_phase.set(phase);
        if tuning.correction_effect == CorrectionEffect::Expand {
            self.update_expansion(motion, resting_frame);
            return;
        }
        if self.main_extension.replace(0.0) > 0.0 {
            // Another effect was picked while the main glass was grown; the
            // next `set_frames` gives it its own height back.
            self.correction_expansion_view.setHidden(true);
        }
        let correction_glass = &self.correction_glass;

        match (motion, resting_frame) {
            (CorrectionMotion::Place, Some(frame)) => {
                cancel_timer(&self.correction_out_timer);
                // Setting the frame it already has would cut a pop short.
                if correction_glass.isHidden() || correction_glass.frame() != frame {
                    correction_glass.setFrame(frame);
                }
                self.correction_content_view.setAlphaValue(1.0);
                correction_glass.setHidden(false);
            }
            (CorrectionMotion::Appear, Some(frame)) => {
                cancel_timer(&self.correction_out_timer);
                let content = &self.correction_content_view;
                if correction_glass.isHidden() {
                    correction_glass.setFrame(correction_start_frame(
                        tuning.correction_effect,
                        frame,
                        tuning.pop_shrink,
                    ));
                    content.setAlphaValue(0.0);
                    correction_glass.setHidden(false);
                }
                let timing = correction_timing(tuning.correction_easing, true);
                animate_with(tuning.correction_seconds, timing, || {
                    correction_glass.animator().setFrame(frame);
                    content.animator().setAlphaValue(1.0);
                });
            }
            (CorrectionMotion::Disappear, _) => {
                let content = &self.correction_content_view;
                let frame = correction_glass.frame();
                let end = correction_start_frame(tuning.correction_effect, frame, tuning.pop_shrink);
                let timing = correction_timing(tuning.correction_easing, false);
                animate_with(tuning.correction_seconds, timing, || {
                    correction_glass.animator().setFrame(end);
                    content.animator().setAlphaValue(0.0);
                });

                self.finish_disappearing_after(tuning.correction_seconds);
            }
            (CorrectionMotion::Remove, _) => {
                cancel_timer(&self.correction_out_timer);
                correction_glass.setHidden(true);
            }
            (CorrectionMotion::None, _)
            | (CorrectionMotion::Place | CorrectionMotion::Appear, None) => {}
        }
    }

    /// `update_correction` for `CorrectionEffect::Expand`: grows the main
    /// glass at its top to take the correction glass's room and fades the
    /// correction text in there; shrinks it back to disappear.
    fn update_expansion(&self, motion: CorrectionMotion, resting_frame: Option<NSRect>) {
        let tuning = self.tuning.get();
        let main_glass = &self.main_glass;
        let expansion = &self.correction_expansion_view;
        self.correction_glass.setHidden(true);
        let resting = self.main_resting_frame.get();
        let base = NSRect::new(
            resting.origin,
            NSSize::new(resting.size.width, resting.size.height - self.main_extension.get()),
        );
        match (motion, resting_frame) {
            (CorrectionMotion::Place | CorrectionMotion::Appear, Some(frame)) => {
                cancel_timer(&self.correction_out_timer);
                let grown = expanded_main_frame(base, frame);
                self.main_extension.set(grown.size.height - base.size.height);
                self.main_resting_frame.set(grown);
                if motion == CorrectionMotion::Place && resting != grown {
                    main_glass.setFrame(grown);
                }
                // Placed for the glass's height now, which is mid-motion
                // while it grows or shrinks; autoresizing keeps it at the top
                // edge from there.
                expansion.setFrame(expansion_frame(
                    frame,
                    tuning.margin(),
                    main_glass.frame().size.height,
                ));
                if motion == CorrectionMotion::Place {
                    expansion.setAlphaValue(1.0);
                    expansion.setHidden(false);
                    return;
                }
                if expansion.isHidden() {
                    expansion.setAlphaValue(0.0);
                    expansion.setHidden(false);
                }
                // Only the size moves: the glass grows at its top, and an
                // animated origin could leave it off its resting origin.
                let timing = correction_timing(tuning.correction_easing, true);
                animate_with(tuning.correction_seconds, timing, || {
                    main_glass.animator().setFrameSize(grown.size);
                    expansion.animator().setAlphaValue(1.0);
                });
                self.settle_main_glass_after(tuning.correction_seconds);
            }
            (CorrectionMotion::Disappear, _) => {
                self.main_extension.set(0.0);
                self.main_resting_frame.set(base);
                let timing = correction_timing(tuning.correction_easing, false);
                animate_with(tuning.correction_seconds, timing, || {
                    main_glass.animator().setFrameSize(base.size);
                    expansion.animator().setAlphaValue(0.0);
                });
                self.settle_main_glass_after(tuning.correction_seconds);
                self.finish_disappearing_after(tuning.correction_seconds);
            }
            (CorrectionMotion::Remove, _) => {
                cancel_timer(&self.correction_out_timer);
                self.main_extension.set(0.0);
                self.main_resting_frame.set(base);
                expansion.setHidden(true);
                main_glass.setFrame(base);
            }
            (CorrectionMotion::None, _)
            | (CorrectionMotion::Place | CorrectionMotion::Appear, None) => {}
        }
    }

    /// Puts the main glass exactly where it rests once its motion ends,
    /// `seconds` from now. An animated frame of an `NSGlassEffectView` stops
    /// at its last displayed step instead of its target: a pop in to an
    /// origin of (40, 40) was measured to end at (39.9, 40.2) on macOS 26.6,
    /// and later motions then start from, and wobble around, that origin.
    fn settle_main_glass_after(&self, seconds: f64) {
        let main_glass = self.main_glass.clone();
        let main_resting_frame = self.main_resting_frame.clone();
        schedule_after_motion(&self.main_settle_timer, seconds, move || {
            let resting = main_resting_frame.get();
            if main_glass.frame() != resting {
                main_glass.setFrame(resting);
            }
        });
    }

    /// Once the correction has disappeared (`seconds` from now), hides it
    /// and gives its room in the window back.
    fn finish_disappearing_after(&self, seconds: f64) {
        let correction_phase = self.correction_phase.clone();
        let correction_glass = self.correction_glass.clone();
        let expansion = self.correction_expansion_view.clone();
        let panel = self.panel.clone();
        let main_resting_frame = self.main_resting_frame.clone();
        let halo_view = self.halo_view.clone();
        let halo_mask = self.halo_mask.clone();
        let margin = self.tuning.get().margin();
        schedule_after_motion(&self.correction_out_timer, seconds, move || {
            correction_phase.set(CorrectionPhase::Hidden);
            correction_glass.setHidden(true);
            expansion.setHidden(true);
            let window_frame = panel.frame();
            panel.setFrame_display(
                NSRect::new(
                    window_frame.origin,
                    NSSize::new(
                        window_frame.size.width,
                        main_resting_frame.get().size.height + (margin * 2.0),
                    ),
                ),
                true,
            );
            fit_halo_mask(&halo_view, &halo_mask);
        });
    }
}

/// Runs `changes` as one ease-in-ease-out animation group of `seconds`.
fn animate(seconds: f64, changes: impl Fn()) {
    animate_with(seconds, EASE_IN_OUT, changes);
}

/// Runs `changes` as one animation group of `seconds`, timed by the cubic
/// Bézier `[x1, y1, x2, y2]`.
fn animate_with(seconds: f64, timing: [f32; 4], changes: impl Fn()) {
    let changes: &dyn Fn() = &changes;
    let changes = StackBlock::new(move |context: NonNull<NSAnimationContext>| {
        // SAFETY: AppKit passes the current, live animation context.
        let context = unsafe { context.as_ref() };
        context.setDuration(seconds);
        context.setAllowsImplicitAnimation(true);
        let [x1, y1, x2, y2] = timing;
        let timing = CAMediaTimingFunction::functionWithControlPoints(x1, y1, x2, y2);
        context.setTimingFunction(Some(&timing));
        changes();
    });
    NSAnimationContext::runAnimationGroup(&changes);
}

/// Runs `finish` once, `seconds` from now, unless `slot` is cancelled first.
/// `NSAnimationContext` never calls the completion handler of a group that
/// animates an `NSGlassEffectView` (measured on macOS 26.6), so motions that
/// need an ending use a one-shot timer of the motion's length instead.
fn schedule_after_motion(
    slot: &RefCell<Option<Retained<NSTimer>>>,
    seconds: f64,
    finish: impl Fn() + 'static,
) {
    cancel_timer(slot);
    let block = RcBlock::new(move |_timer: NonNull<NSTimer>| finish());
    // SAFETY: the block only touches main-thread AppKit objects, and the
    // timer is added to the main run loop, so it fires on the main thread.
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(seconds, false, &block) };
    // SAFETY: this runs on the main thread, which owns the main run loop;
    // `NSRunLoopCommonModes` is an immutable Foundation constant. Common
    // modes keep the timer firing while a menu is tracked.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    slot.replace(Some(timer));
}

/// Invalidates the timer in `slot`; returns whether it had yet to fire.
fn cancel_timer(slot: &RefCell<Option<Retained<NSTimer>>>) -> bool {
    match slot.take() {
        Some(timer) => {
            let was_pending = timer.isValid();
            timer.invalidate();
            was_pending
        }
        None => false,
    }
}

/// Sizes the halo mask to the halo, without an implicit animation.
fn fit_halo_mask(halo_view: &NSView, halo_mask: &CALayer) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    halo_mask.setFrame(halo_view.bounds());
    CATransaction::commit();
}

/// The halo's mask and the part of it that stretches (`contentsCenter`, in
/// unit coordinates): clear under the panel outline, opaque right outside it,
/// and fading to clear at the window edge by `halo_strength`. One small image
/// fits every window size because only its centre pixel stretches.
fn halo_mask_image(margin: f64, corner_radius: f64, curve: HaloCurve) -> (Retained<NSImage>, NSRect) {
    let cap = margin + corner_radius;
    let side = (cap * 2.0) + 1.0;
    let steps = margin.ceil().max(1.0) as usize;
    let draw = RcBlock::new(move |bounds: NSRect| -> Bool {
        let Some(context) = NSGraphicsContext::currentContext() else {
            return Bool::NO;
        };
        draw_halo_rings(&context, bounds, margin, corner_radius, curve, steps, true);
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(side, side), false, &draw);
    let centre = NSRect::new(
        NSPoint::new(cap / side, cap / side),
        NSSize::new(1.0 / side, 1.0 / side),
    );
    (image, centre)
}

/// Draws the halo's strength as alpha into `bounds` in the current context:
/// 0 at the edge, rising along `curve` to 1 at `margin` in, and inside
/// that either 1 or, with `hole`, 0 (where the panels' own glass is).
fn draw_halo_rings(
    context: &NSGraphicsContext,
    bounds: NSRect,
    margin: f64,
    corner_radius: f64,
    curve: HaloCurve,
    steps: usize,
    hole: bool,
) {
    context.saveGraphicsState();
    // Each ring replaces the one outside it rather than adding to it.
    context.setCompositingOperation(NSCompositingOperation::Copy);
    for step in 0..=steps {
        let inset = margin * step as f64 / steps as f64;
        let ring = NSRect::new(
            NSPoint::new(bounds.origin.x + inset, bounds.origin.y + inset),
            NSSize::new(bounds.size.width - (inset * 2.0), bounds.size.height - (inset * 2.0)),
        );
        let radius = corner_radius + margin - inset;
        NSColor::colorWithWhite_alpha(0.0, halo_strength(step, steps, curve)).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ring, radius, radius).fill();
    }
    if hole {
        context.setCompositingOperation(NSCompositingOperation::Clear);
        let panel_outline = NSRect::new(
            NSPoint::new(bounds.origin.x + margin, bounds.origin.y + margin),
            NSSize::new(bounds.size.width - (margin * 2.0), bounds.size.height - (margin * 2.0)),
        );
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            panel_outline,
            corner_radius,
            corner_radius,
        )
        .fill();
    }
    context.restoreGraphicsState();
}

/// What a progressive-blur mask was drawn for, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BlurMaskKey {
    width_px: usize,
    height_px: usize,
    margin_px: usize,
    corner_px: f64,
    curve: HaloCurve,
    under_panels: bool,
    radius: f64,
}

/// The progressive blur's mask at the window's pixel size: the filter reads
/// alpha as blur strength and stretches the mask over the whole layer, so it
/// is drawn at full size rather than stretched from a small image.
fn blur_mask_image(key: &BlurMaskKey) -> Option<Retained<CGImage>> {
    if key.width_px == 0 || key.height_px == 0 {
        return None;
    }
    // SAFETY: null planes make the bitmap allocate its own pixel buffer;
    // the other arguments describe 8-bit RGBA, which AppKit supports.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            key.width_px as isize,
            key.height_px as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;
    let previous = NSGraphicsContext::currentContext();
    NSGraphicsContext::setCurrentContext(Some(&context));
    let bounds = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(key.width_px as f64, key.height_px as f64),
    );
    draw_halo_rings(
        &context,
        bounds,
        key.margin_px as f64,
        key.corner_px,
        key.curve,
        key.margin_px.max(1),
        !key.under_panels,
    );
    context.flushGraphics();
    NSGraphicsContext::setCurrentContext(previous.as_deref());
    bitmap.CGImage()
}

fn make_glass_content(
    mtm: MainThreadMarker,
    glass: &NSGlassEffectView,
    frame: NSRect,
) -> Retained<NSView> {
    glass.setAutoresizingMask(NSAutoresizingMaskOptions::ViewNotSizable);
    let content_view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), frame.size),
    );
    content_view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    content_view.setWantsLayer(true);
    glass.setContentView(Some(&content_view));
    content_view
}

fn make_panel(mtm: MainThreadMarker, panel_rect: NSRect) -> Retained<NSPanel> {
    let panel: Retained<OverlayPanel> = unsafe {
        msg_send![
            OverlayPanel::alloc(mtm),
            initWithContentRect: panel_rect,
            styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            backing: NSBackingStoreType::Buffered,
            defer: false,
            screen: NSScreen::mainScreen(mtm).as_deref()
        ]
    };
    let panel: Retained<NSPanel> = Retained::into_super(panel);

    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(false);
    panel.setWorksWhenModal(true);
    panel.setLevel(NSFloatingWindowLevel);
    panel.setOpaque(false);
    panel.setHasShadow(false);
    panel.setHidesOnDeactivate(false);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::MoveToActiveSpace
            | NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correction_glass_appears_and_disappears_with_its_effect() {
        assert_eq!(
            plan_correction_motion(CorrectionPhase::Hidden, true, false),
            (CorrectionPhase::Shown, CorrectionMotion::Appear)
        );
        assert_eq!(
            plan_correction_motion(CorrectionPhase::Shown, true, false),
            (CorrectionPhase::Shown, CorrectionMotion::Place)
        );
        assert_eq!(
            plan_correction_motion(CorrectionPhase::Shown, false, false),
            (CorrectionPhase::Disappearing, CorrectionMotion::Disappear)
        );
        assert_eq!(
            plan_correction_motion(CorrectionPhase::Disappearing, false, false),
            (CorrectionPhase::Disappearing, CorrectionMotion::None)
        );
        assert_eq!(
            plan_correction_motion(CorrectionPhase::Disappearing, true, false),
            (CorrectionPhase::Shown, CorrectionMotion::Appear)
        );
        assert_eq!(
            plan_correction_motion(CorrectionPhase::Hidden, false, false),
            (CorrectionPhase::Hidden, CorrectionMotion::None)
        );
    }

    #[test]
    fn correction_glass_appears_and_disappears_in_place_with_reduce_motion() {
        assert_eq!(
            plan_correction_motion(CorrectionPhase::Hidden, true, true),
            (CorrectionPhase::Shown, CorrectionMotion::Place)
        );
        assert_eq!(
            plan_correction_motion(CorrectionPhase::Disappearing, true, true),
            (CorrectionPhase::Shown, CorrectionMotion::Place)
        );
        assert_eq!(
            plan_correction_motion(CorrectionPhase::Shown, false, true),
            (CorrectionPhase::Hidden, CorrectionMotion::Remove)
        );
    }

    #[test]
    fn correction_effects_start_from_their_own_frames() {
        let resting = NSRect::new(NSPoint::new(28.0, 218.0), NSSize::new(560.0, 92.0));
        assert_eq!(
            correction_start_frame(CorrectionEffect::Emerge, resting, 0.08),
            NSRect::new(NSPoint::new(224.0, 198.0), NSSize::new(168.0, 20.0))
        );
        assert_eq!(
            correction_start_frame(CorrectionEffect::Unfold, resting, 0.08),
            NSRect::new(NSPoint::new(28.0, 218.0), NSSize::new(560.0, 1.0))
        );
        assert_eq!(
            correction_start_frame(CorrectionEffect::Pop, resting, 0.08),
            popped_frame(resting, 0.08)
        );
        assert_eq!(
            correction_start_frame(CorrectionEffect::Slide, resting, 0.08),
            NSRect::new(NSPoint::new(28.0, 116.0), NSSize::new(560.0, 92.0))
        );
    }

    #[test]
    fn spring_easing_overshoots_on_the_way_in_only() {
        let [_, spring_in_y1, _, _] = correction_timing(CorrectionEasing::Spring, true);
        let [_, spring_out_y1, _, spring_out_y2] = correction_timing(CorrectionEasing::Spring, false);
        assert!(spring_in_y1 > 1.0);
        assert!(spring_out_y1 <= 1.0 && spring_out_y2 <= 1.0);
        for appearing in [true, false] {
            let [_, y1, _, y2] = correction_timing(CorrectionEasing::Smooth, appearing);
            assert!((0.0..=1.0).contains(&y1) && (0.0..=1.0).contains(&y2));
        }
    }

    #[test]
    fn window_pops_from_and_to_its_pop_opacity() {
        let tuning = GlassTuning {
            pop_opacity: 0.15,
            ..GlassTuning::default()
        };
        assert_eq!(pop_edge_opacity(&tuning, false), 0.15);
        // Reduce Motion replaces the pop with a plain fade from and to nothing.
        assert_eq!(pop_edge_opacity(&tuning, true), 0.0);
        assert_eq!(GlassTuning::default().pop_opacity, 0.15);
    }

    #[test]
    fn expanded_main_glass_takes_the_correction_glass_room() {
        let main_height = 180.0;
        let resting = correction_resting_frame(main_height, 560.0, 92.0, 28.0);
        assert_eq!(
            expanded_main_frame(main_glass_frame(560.0, main_height, 28.0), resting),
            NSRect::new(NSPoint::new(28.0, 28.0), NSSize::new(560.0, 282.0))
        );
        // In the main glass's own coordinates: the gap, then the correction text.
        assert_eq!(
            expansion_frame(resting, 28.0, 282.0),
            NSRect::new(NSPoint::new(0.0, 180.0), NSSize::new(560.0, 102.0))
        );
        // Mid-motion it hugs the glass's top edge, which it rides.
        assert_eq!(
            expansion_frame(resting, 28.0, 180.0),
            NSRect::new(NSPoint::new(0.0, 78.0), NSSize::new(560.0, 102.0))
        );
        assert_eq!(
            correction_start_frame(CorrectionEffect::Expand, resting, 0.08),
            resting
        );
    }

    #[test]
    fn window_spans_the_glass_and_the_margin_around_it() {
        let main_frame = NSRect::new(NSPoint::new(100.0, 200.0), NSSize::new(560.0, 180.0));
        let correction_frame =
            NSRect::new(NSPoint::new(100.0, 390.0), NSSize::new(560.0, 92.0));

        assert_eq!(
            window_frame(main_frame, None, 28.0),
            NSRect::new(NSPoint::new(72.0, 172.0), NSSize::new(616.0, 236.0))
        );
        assert_eq!(
            window_frame(main_frame, Some(correction_frame), 28.0),
            NSRect::new(NSPoint::new(72.0, 172.0), NSSize::new(616.0, 338.0))
        );
        assert_eq!(window_frame(main_frame, None, 0.0), main_frame);
    }

    #[test]
    fn glass_sits_inside_the_margin() {
        assert_eq!(
            main_glass_frame(560.0, 180.0, 28.0),
            NSRect::new(NSPoint::new(28.0, 28.0), NSSize::new(560.0, 180.0))
        );
        assert_eq!(
            correction_resting_frame(180.0, 560.0, 92.0, 28.0),
            NSRect::new(NSPoint::new(28.0, 218.0), NSSize::new(560.0, 92.0))
        );
    }

    #[test]
    fn window_keeps_no_margin_without_a_halo() {
        let tuning = GlassTuning {
            halo_enabled: false,
            ..GlassTuning::default()
        };

        assert_eq!(tuning.margin(), 0.0);
        assert_eq!(GlassTuning::default().margin(), GlassTuning::default().halo_margin);
    }

    #[test]
    fn halo_blur_fades_from_full_at_the_panels_to_none_at_the_window_edge() {
        let linear = HaloCurve::LINEAR;
        assert_eq!(halo_strength(0, 40, linear), 0.0);
        assert_eq!(halo_strength(40, 40, linear), 1.0);
        assert!((halo_strength(20, 40, linear) - 0.5).abs() < 1e-6);
        assert!((halo_strength(10, 40, linear) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn halo_curve_shapes_the_fade_between_the_edges() {
        let ease_in = HaloCurve::with_controls((0.42, 0.0), (1.0, 1.0));
        let ease_out = HaloCurve::with_controls((0.0, 0.0), (0.58, 1.0));
        assert!(halo_strength(20, 40, ease_in) < 0.4);
        assert!(halo_strength(20, 40, ease_out) > 0.6);
        for curve in [ease_in, ease_out, HaloCurve::EASE_IN_OUT] {
            assert_eq!(halo_strength(0, 40, curve), 0.0);
            assert_eq!(halo_strength(40, 40, curve), 1.0);
            let strengths: Vec<f64> = (0..=40).map(|step| halo_strength(step, 40, curve)).collect();
            assert!(strengths.windows(2).all(|pair| pair[0] <= pair[1] + 1e-9));
        }
    }

    #[test]
    fn halo_curve_keeps_strength_between_none_and_full() {
        let overshoot = HaloCurve::with_controls((0.2, 1.8), (0.8, -0.8));
        for step in 0..=40 {
            let strength = halo_strength(step, 40, overshoot);
            assert!((0.0..=1.0).contains(&strength), "step {step}: {strength}");
        }
    }

    #[test]
    fn halo_curve_holds_its_end_points_beyond_them() {
        let curve = HaloCurve {
            start: (0.2, 0.3),
            end: (0.8, 0.9),
            ..HaloCurve::LINEAR
        };
        assert!((curve.strength_at(0.0) - 0.3).abs() < 1e-9);
        assert!((curve.strength_at(0.1) - 0.3).abs() < 1e-9);
        assert!((curve.strength_at(0.2) - 0.3).abs() < 1e-6);
        assert!((curve.strength_at(0.8) - 0.9).abs() < 1e-6);
        assert!((curve.strength_at(0.95) - 0.9).abs() < 1e-9);
        assert!((curve.strength_at(1.0) - 0.9).abs() < 1e-9);
        let middle = curve.strength_at(0.5);
        assert!(middle > 0.3 && middle < 0.9, "{middle}");
    }

    #[test]
    fn halo_edges_take_the_end_point_strengths() {
        let curve = HaloCurve {
            start: (0.0, 0.25),
            end: (1.0, 0.75),
            ..HaloCurve::LINEAR
        };
        assert!((halo_strength(0, 40, curve) - 0.25).abs() < 1e-6);
        assert!((halo_strength(40, 40, curve) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn glass_pops_from_a_slightly_smaller_frame_about_its_center() {
        let frame = NSRect::new(NSPoint::new(28.0, 28.0), NSSize::new(500.0, 200.0));

        assert_eq!(
            popped_frame(frame, 0.08),
            NSRect::new(NSPoint::new(48.0, 36.0), NSSize::new(460.0, 184.0))
        );
    }
}
