//! Part of the `--debug` tuner: an editor for the halo's fade curve. Four
//! draggable handles: the start and end points (white, outlined) and the two
//! control points (blue and orange). The plot samples
//! `HaloCurve::strength_at`, so it shows exactly what the halo applies.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBezierPath, NSColor, NSEvent, NSResponder, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::overlay::glass::HaloCurve;

const INSET: f64 = 14.0;
const HANDLE_RADIUS: f64 = 6.0;
const SIZE: NSSize = NSSize::new(220.0, 130.0);
/// Range of the plot's top edge, in blur strength: the editor works on low
/// strengths, zoomed by the tuner's `curve_y_max` slider.
pub const Y_MAX_RANGE: (f64, f64) = (0.05, 0.25);
/// Least distance kept between the start and end points' x.
const MIN_SPAN: f64 = 0.02;

/// Which of the curve's four points a handle moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Handle {
    Start,
    Control1,
    Control2,
    End,
}

const HANDLES: [Handle; 4] = [Handle::Start, Handle::Control1, Handle::Control2, Handle::End];

fn point_of(curve: &HaloCurve, handle: Handle) -> (f64, f64) {
    match handle {
        Handle::Start => curve.start,
        Handle::Control1 => curve.control1,
        Handle::Control2 => curve.control2,
        Handle::End => curve.end,
    }
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// `curve` with every strength scaled by `y_max`: a 0..=1 preset fitted to
/// the plot.
pub fn fitted(curve: HaloCurve, y_max: f64) -> HaloCurve {
    let fit = |(x, y): (f64, f64)| (x, round(y * y_max));
    HaloCurve {
        start: fit(curve.start),
        control1: fit(curve.control1),
        control2: fit(curve.control2),
        end: fit(curve.end),
    }
}

/// `curve` with `handle` moved to (`x`, `y`). An end point keeps its x on its
/// side of the other end point, and takes its control point along with it.
fn moved(curve: HaloCurve, handle: Handle, x: f64, y: f64, y_max: f64) -> HaloCurve {
    let mut curve = curve;
    let shift = |point: (f64, f64), dx: f64, dy: f64| {
        (round((point.0 + dx).clamp(0.0, 1.0)), round((point.1 + dy).clamp(0.0, y_max)))
    };
    match handle {
        Handle::Start => {
            let x = x.min(curve.end.0 - MIN_SPAN).max(0.0);
            let (dx, dy) = (x - curve.start.0, y - curve.start.1);
            curve.control1 = shift(curve.control1, dx, dy);
            curve.start = (round(x), round(y));
        }
        Handle::End => {
            let x = x.max(curve.start.0 + MIN_SPAN).min(1.0);
            let (dx, dy) = (x - curve.end.0, y - curve.end.1);
            curve.control2 = shift(curve.control2, dx, dy);
            curve.end = (round(x), round(y));
        }
        Handle::Control1 => curve.control1 = (round(x), round(y)),
        Handle::Control2 => curve.control2 = (round(x), round(y)),
    }
    curve
}

pub struct CurveIvars {
    curve: Cell<HaloCurve>,
    dragging: Cell<Option<Handle>>,
    enabled: Cell<bool>,
    /// Blur strength at the top of the plot.
    y_max: Cell<f64>,
    on_change: RefCell<Option<Box<dyn Fn(HaloCurve)>>>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SimplePttCurveEditor"]
    #[ivars = CurveIvars]
    pub struct CurveEditor;

    impl CurveEditor {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_content_size(&self) -> NSSize {
            SIZE
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if !self.ivars().enabled.get() {
                return;
            }
            let point = self.local_point(event);
            let curve = self.ivars().curve.get();
            let distance = |handle: Handle| {
                let (x, y) = point_of(&curve, handle);
                let at = self.to_view(x, y);
                ((at.x - point.x).powi(2) + (at.y - point.y).powi(2)).sqrt()
            };
            let nearest = HANDLES
                .into_iter()
                .min_by(|a, b| distance(*a).total_cmp(&distance(*b)))
                .expect("four handles");
            self.ivars().dragging.set(Some(nearest));
            self.drag_to(point);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if self.ivars().dragging.get().is_some() {
                self.drag_to(self.local_point(event));
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().dragging.set(None);
        }
    }
);

impl CurveEditor {
    pub fn new(mtm: MainThreadMarker, on_change: impl Fn(HaloCurve) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CurveIvars {
            curve: Cell::new(HaloCurve::LINEAR),
            dragging: Cell::new(None),
            enabled: Cell::new(true),
            y_max: Cell::new(Y_MAX_RANGE.1),
            on_change: RefCell::new(Some(Box::new(on_change))),
        });
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), SIZE);
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn curve(&self) -> HaloCurve {
        self.ivars().curve.get()
    }

    pub fn set_curve(&self, curve: HaloCurve) {
        self.ivars().curve.set(curve);
        self.setNeedsDisplay(true);
    }

    pub fn y_max(&self) -> f64 {
        self.ivars().y_max.get()
    }

    /// Zooms the plot so its top edge is `y_max` blur strength. The curve
    /// keeps its values; points above the top are drawn at the top edge.
    pub fn set_y_max(&self, y_max: f64) {
        self.ivars().y_max.set(y_max.clamp(Y_MAX_RANGE.0, Y_MAX_RANGE.1));
        self.setNeedsDisplay(true);
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.ivars().enabled.set(enabled);
        self.setNeedsDisplay(true);
    }

    fn plot(&self) -> NSRect {
        let bounds = self.bounds();
        NSRect::new(
            NSPoint::new(bounds.origin.x + INSET, bounds.origin.y + INSET),
            NSSize::new(bounds.size.width - (INSET * 2.0), bounds.size.height - (INSET * 2.0)),
        )
    }

    fn to_view(&self, x: f64, y: f64) -> NSPoint {
        let plot = self.plot();
        let y = (y / self.y_max()).min(1.0);
        NSPoint::new(plot.origin.x + (x * plot.size.width), plot.origin.y + (y * plot.size.height))
    }

    fn local_point(&self, event: &NSEvent) -> NSPoint {
        self.convertPoint_fromView(event.locationInWindow(), None)
    }

    fn drag_to(&self, point: NSPoint) {
        let Some(handle) = self.ivars().dragging.get() else {
            return;
        };
        let plot = self.plot();
        let x = ((point.x - plot.origin.x) / plot.size.width).clamp(0.0, 1.0);
        let y_max = self.y_max();
        let y = ((point.y - plot.origin.y) / plot.size.height).clamp(0.0, 1.0) * y_max;
        let curve = moved(self.ivars().curve.get(), handle, x, y, y_max);
        self.set_curve(curve);
        if let Some(on_change) = self.ivars().on_change.borrow().as_ref() {
            on_change(curve);
        }
    }

    fn draw(&self) {
        let enabled = self.ivars().enabled.get();
        NSColor::controlBackgroundColor().setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), 8.0, 8.0).fill();

        // Quarter grid: a quarter of the top strength per row.
        let y_max = self.y_max();
        NSColor::separatorColor().setStroke();
        for quarter in 0..=4 {
            let t = quarter as f64 / 4.0;
            NSBezierPath::strokeLineFromPoint_toPoint(self.to_view(t, 0.0), self.to_view(t, y_max));
            NSBezierPath::strokeLineFromPoint_toPoint(
                self.to_view(0.0, t * y_max),
                self.to_view(1.0, t * y_max),
            );
        }

        let curve = self.ivars().curve.get();
        NSColor::secondaryLabelColor().setStroke();
        let line = |from: (f64, f64), to: (f64, f64)| {
            NSBezierPath::strokeLineFromPoint_toPoint(self.to_view(from.0, from.1), self.to_view(to.0, to.1));
        };
        line(curve.start, curve.control1);
        line(curve.end, curve.control2);

        let path = NSBezierPath::bezierPath();
        path.moveToPoint(self.to_view(0.0, curve.strength_at(0.0)));
        for sample in 1..=120 {
            let x = sample as f64 / 120.0;
            path.lineToPoint(self.to_view(x, curve.strength_at(x)));
        }
        path.setLineWidth(2.5);
        let stroke = if enabled { NSColor::labelColor() } else { NSColor::disabledControlTextColor() };
        stroke.setStroke();
        path.stroke();

        for handle in HANDLES {
            let (x, y) = point_of(&curve, handle);
            let centre = self.to_view(x, y);
            let dot = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                NSPoint::new(centre.x - HANDLE_RADIUS, centre.y - HANDLE_RADIUS),
                NSSize::new(HANDLE_RADIUS * 2.0, HANDLE_RADIUS * 2.0),
            ));
            let fill = match (enabled, handle) {
                (false, _) => NSColor::disabledControlTextColor(),
                (true, Handle::Control1) => NSColor::systemBlueColor(),
                (true, Handle::Control2) => NSColor::systemOrangeColor(),
                (true, Handle::Start | Handle::End) => NSColor::whiteColor(),
            };
            fill.setFill();
            dot.fill();
            if matches!(handle, Handle::Start | Handle::End) {
                stroke.setStroke();
                dot.setLineWidth(1.5);
                dot.stroke();
            }
        }
    }
}
