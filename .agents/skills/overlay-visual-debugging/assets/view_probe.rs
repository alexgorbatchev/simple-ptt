//! Temporary probes for overlay motion and layout bugs. Copy to
//! `src/overlay/dev/view_probe.rs`, add `pub mod view_probe;` to
//! `src/overlay/dev/mod.rs`, and call it from `tuner::run()`. Delete the file,
//! the `mod` line, and the call before committing.

use std::cell::Cell;
use std::ptr::NonNull;
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, MainThreadMarker, Message};
use objc2_app_kit::{NSApplication, NSView, NSViewFrameDidChangeNotification, NSWindow};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSRect, NSRunLoop, NSRunLoopCommonModes, NSString,
    NSThread, NSTimer,
};
use objc2_quartz_core::CALayer;

thread_local! {
    static START: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Seconds since `run_script` started (0 before).
pub fn elapsed() -> f64 {
    START.with(|start| start.get().map_or(0.0, |start| start.elapsed().as_secs_f64()))
}

pub fn class_name(object: &AnyObject) -> String {
    // SAFETY: every Objective-C object answers `className`.
    let name: Retained<NSString> = unsafe { msg_send![object, className] };
    name.to_string()
}

/// The overlay's panel (`OverlayPanel` in `src/overlay/glass.rs`).
pub fn overlay_panel() -> Option<Retained<NSWindow>> {
    let app = NSApplication::sharedApplication(MainThreadMarker::new()?);
    app.windows().iter().find(|window| class_name(window) == "OverlayPanel")
}

/// Views of class `class` under `root`, depth first: in the overlay the
/// transcript's views come before the correction's.
pub fn find_views(root: &NSView, class: &str) -> Vec<Retained<NSView>> {
    let mut found = Vec::new();
    fn walk(view: &NSView, class: &str, found: &mut Vec<Retained<NSView>>) {
        if class_name(view) == class {
            found.push(view.retain());
        }
        for subview in view.subviews().iter() {
            walk(&subview, class, found);
        }
    }
    walk(root, class, &mut found);
    found
}

/// `view`'s bounds in screen coordinates: where it really is on screen.
pub fn screen_rect(view: &NSView) -> NSRect {
    let in_window = view.convertRect_toView(view.bounds(), None);
    view.window().map_or(in_window, |window| window.convertRectToScreen(in_window))
}

/// Frame and bounds of `view` and each of its ancestors, innermost first.
pub fn ancestry(view: &NSView) -> String {
    let mut parts = Vec::new();
    let mut cursor = Some(view.retain());
    while let Some(current) = cursor {
        let (f, b) = (current.frame(), current.bounds());
        parts.push(format!(
            "{}[f={:.2},{:.2} {:.1}x{:.1} b={:.2},{:.2}]",
            class_name(&current),
            f.origin.x,
            f.origin.y,
            f.size.width,
            f.size.height,
            b.origin.x,
            b.origin.y,
        ));
        // SAFETY: reading the superview of a live view on the main thread.
        cursor = unsafe { current.superview() };
    }
    parts.join(" < ")
}

/// Height of `view`'s layer as drawn now. It differs from `frame()` while a
/// Core Animation animation runs; for AppKit `animator()` frame animations
/// the two move together.
pub fn presentation_height(view: &NSView) -> Option<f64> {
    let layer = view.layer()?;
    // SAFETY: `presentationLayer` returns nil or a copy of the layer.
    let presentation: Option<Retained<CALayer>> = unsafe { msg_send![&*layer, presentationLayer] };
    presentation.map(|layer| layer.bounds().size.height)
}

/// Prints the AppKit call stack of every frame change of `view` that
/// `report` accepts. It finds who moves or resizes a view when breakpoints
/// on `setFrame:` catch nothing (AppKit's animation manager and layout
/// helpers call `setFrameOrigin:` and `setFrameSize:` directly).
pub fn watch_frame(view: &NSView, label: &'static str, report: impl Fn(NSRect) -> bool + 'static) {
    let observed = view.retain();
    let block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
        let frame = observed.frame();
        if !report(frame) {
            return;
        }
        eprintln!("FRAME {label} t={:.3} {frame:?}", elapsed());
        let symbols = NSThread::callStackSymbols();
        let symbols: Vec<String> = symbols.iter().map(|symbol| symbol.to_string()).collect();
        // Skip the probe's own frames and the notification center's.
        let first = symbols
            .iter()
            .position(|symbol| symbol.contains("_postFrameChangeNotification"))
            .unwrap_or(0);
        for symbol in symbols.iter().skip(first).take(24) {
            eprintln!("    {symbol}");
        }
    });
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: the block only reads a main-thread view, and the notification
    // is posted on the main thread that changes the frame.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSViewFrameDidChangeNotification),
            Some(view),
            None,
            &block,
        )
    };
    // The probe lives as long as the process.
    std::mem::forget(token);
}

/// Runs `events` (seconds from now, action) once each, calls `sample` every
/// `sample_every` seconds with the elapsed time, and exits the process at
/// `end` seconds, so a scripted run needs no one to close the app.
pub fn run_script(
    events: Vec<(f64, Box<dyn Fn()>)>,
    sample_every: f64,
    end: f64,
    sample: impl Fn(f64) + 'static,
) {
    START.with(|start| start.set(Some(Instant::now())));
    let pending = Cell::new(events);
    let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        let now = elapsed();
        let mut events = pending.take();
        events.retain(|(at, action)| {
            if *at <= now {
                eprintln!("-- t={now:.3} event scheduled at {at:.3}");
                action();
                false
            } else {
                true
            }
        });
        pending.set(events);
        sample(now);
        if now >= end {
            std::process::exit(0);
        }
    });
    // SAFETY: the timer is added to the main run loop, so the block runs on
    // the main thread.
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(sample_every, true, &block) };
    // SAFETY: main thread; `NSRunLoopCommonModes` is a Foundation constant.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    std::mem::forget(timer);
}
