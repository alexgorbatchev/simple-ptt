//! Dev tool: `simple-ptt --overlay-snapshot <dir>` renders
//! every overlay state over light and dark backdrops and captures each with
//! `screencapture`.

use std::process::Command;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFloatingWindowLevel,
    NSFont, NSScreen, NSTextAlignment, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSDate, NSPoint, NSRect, NSRunLoop, NSSize, NSString};
use objc2_quartz_core::CAGradientLayer;

use crate::app::overlay_style_from_config;
use crate::config::Config;
use crate::overlay::OverlayWindow;
use crate::state::{
    AppState, MicMeterSnapshot, OverlayText,
    STATE_BUFFER_READY, STATE_ERROR, STATE_RECORDING, STATE_TRANSFORMING,
};

fn pump(seconds: f64) {
    NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(seconds));
}

fn text(text: &str, provisional: Option<&str>) -> OverlayText {
    let state = AppState::new();
    let start = provisional.and_then(|tail| text.find(tail));
    state.set_live_overlay_text(text, start);
    state.overlay_text_snapshot()
}

/// Size of the backdrop: room for the overlay, its halo, its correction, and
/// the capture's padding.
const BACKDROP_SIZE: NSSize = NSSize::new(1100.0, 760.0);
/// Distance between the backdrop's rows of text.
const BACKDROP_ROW_PITCH: f64 = 64.0;

/// A backdrop for the overlay: a gradient with rows of text, centred on the
/// screen's visible frame like the overlay, so the text runs under the glass.
fn backdrop(mtm: MainThreadMarker, visible: NSRect, dark: bool) -> Retained<NSWindow> {
    let frame = NSRect::new(
        NSPoint::new(
            visible.origin.x + ((visible.size.width - BACKDROP_SIZE.width) / 2.0),
            visible.origin.y + ((visible.size.height - BACKDROP_SIZE.height) / 2.0),
        ),
        BACKDROP_SIZE,
    );
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setLevel(NSFloatingWindowLevel - 1);
    let view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), frame.size));
    view.setWantsLayer(true);
    let gradient = CAGradientLayer::new();
    gradient.setFrame(view.bounds());
    let colors: Vec<Retained<NSColor>> = if dark {
        vec![
            NSColor::colorWithSRGBRed_green_blue_alpha(0.05, 0.07, 0.16, 1.0),
            NSColor::colorWithSRGBRed_green_blue_alpha(0.22, 0.08, 0.30, 1.0),
            NSColor::colorWithSRGBRed_green_blue_alpha(0.02, 0.20, 0.22, 1.0),
        ]
    } else {
        vec![
            NSColor::colorWithSRGBRed_green_blue_alpha(0.98, 0.93, 0.80, 1.0),
            NSColor::colorWithSRGBRed_green_blue_alpha(0.75, 0.88, 0.99, 1.0),
            NSColor::colorWithSRGBRed_green_blue_alpha(0.96, 0.80, 0.90, 1.0),
        ]
    };
    let cg: Vec<_> = colors.iter().map(|color| color.CGColor()).collect();
    let refs: Vec<&AnyObject> = cg.iter().map(|color| color.as_ref() as &AnyObject).collect();
    unsafe { gradient.setColors(Some(&NSArray::from_slice(&refs))) };
    gradient.setStartPoint(CGPoint::new(0.0, 0.0));
    gradient.setEndPoint(CGPoint::new(1.0, 1.0));
    view.layer().unwrap().addSublayer(&gradient);
    // Busy content behind the glass so refraction and legibility show: rows
    // of text centred across the backdrop, and as a block down it.
    let rows = (frame.size.height / BACKDROP_ROW_PITCH).floor() as usize;
    let first_row_y = (frame.size.height - (rows as f64 * BACKDROP_ROW_PITCH)) / 2.0;
    for row in 0..rows {
        let label = NSTextField::labelWithString(
            &NSString::from_str("The quick brown fox jumps over the lazy dog — backdrop text 0123456789"),
            mtm,
        );
        label.setFont(Some(&NSFont::boldSystemFontOfSize(26.0)));
        let label_color = if dark {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.9, 0.9, 1.0, 0.55)
        } else {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.1, 0.1, 0.2, 0.55)
        };
        label.setTextColor(Some(&label_color));
        label.setAlignment(NSTextAlignment::Center);
        label.setFrame(NSRect::new(
            NSPoint::new(0.0, first_row_y + (row as f64 * BACKDROP_ROW_PITCH)),
            NSSize::new(frame.size.width, 36.0),
        ));
        view.addSubview(&label);
    }
    window.setContentView(Some(&view));
    window.orderFrontRegardless();
    window
}

fn overlay_frame(mtm: MainThreadMarker) -> Option<NSRect> {
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    (0..windows.count())
        .map(|index| windows.objectAtIndex(index))
        .find(|window| {
            // SAFETY: every Objective-C object answers `className`.
            let class: Retained<NSString> = unsafe { objc2::msg_send![&**window, className] };
            class.to_string() == "OverlayPanel" && window.isVisible()
        })
        .map(|window| window.frame())
}

fn capture(mtm: MainThreadMarker, dir: &str, name: &str) {
    let Some(frame) = overlay_frame(mtm) else {
        eprintln!("{name}: overlay not visible");
        return;
    };
    let screen_height = NSScreen::mainScreen(mtm).unwrap().frame().size.height;
    let pad = 40.0;
    let x = frame.origin.x - pad;
    let y = screen_height - (frame.origin.y + frame.size.height) - pad;
    let region = format!(
        "{},{},{},{}",
        x.round(),
        y.round(),
        (frame.size.width + pad * 2.0).round(),
        (frame.size.height + pad * 2.0).round()
    );
    let path = format!("{dir}/{name}.png");
    let full = format!("{dir}/full.png");
    Command::new("/usr/sbin/screencapture").args(["-x", &full]).status().expect("screencapture runs");
    let scale = NSScreen::mainScreen(mtm).unwrap().backingScaleFactor();
    let crop = format!(
        "{}x{}+{}+{}",
        ((frame.size.width + pad * 2.0) * scale).round(),
        ((frame.size.height + pad * 2.0) * scale).round(),
        (x * scale).round().max(0.0),
        (y * scale).round().max(0.0)
    );
    let status = Command::new("magick").args([&full, "-crop", &crop, "+repage", &path]).status().expect("magick runs");
    println!("{name}: {region} -> {path} ({status})");
}

pub fn run(dir: &str) {
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    std::fs::create_dir_all(dir).unwrap();

    let state = AppState::new();
    let style = overlay_style_from_config(&Config::default());
    let overlay = OverlayWindow::new(mtm, &style, state.clone());

    let dictated = "Let's move the standup to Thursday afternoon so the design review has a full morning, and ask Priya to share the";
    let interim_tail = "updated mockups before lunch";
    let full = format!("{dictated} {interim_tail} ");
    let meter = MicMeterSnapshot { clip_event_counter: 0, level: 150, peak: 200, level_db: -21.0, mic_active: true };
    let empty = text("", None);

    let screen = NSScreen::mainScreen(mtm).unwrap().visibleFrame();
    for dark in [false, true] {
        let theme = if dark { "dark" } else { "light" };
        let backdrop = backdrop(mtm, screen, dark);
        pump(0.3);

        let update = |state_value: u8,
                      main: &OverlayText,
                      error: &str,
                      correction: &OverlayText,
                      correction_active: bool,
                      mic: MicMeterSnapshot| {
            overlay.update(
                mtm, state_value, false, main, error, correction, correction_active, 1.0, mic,
            );
        };
        // Like the status poll: one update, then ~75 ms of run loop, and a
        // capture every other tick.
        let ticks = |name: &str, count: usize, tick: &dyn Fn()| {
            for index in 0..count {
                tick();
                pump(0.075);
                if index % 2 == 0 {
                    capture(mtm, dir, &format!("{theme}-{name}-{index:02}"));
                }
            }
        };

        let interim = text(&full, Some(interim_tail));
        ticks("1-pop-in", 8, &|| update(STATE_RECORDING, &interim, "", &empty, false, meter));
        capture(mtm, dir, &format!("{theme}-2-recording-interim"));

        let final_text = text(&full, None);
        let correction = text("make it Friday instead and ", Some("and "));
        ticks("3-correction-reveal", 8, &|| {
            update(STATE_RECORDING, &final_text, "", &correction, true, meter)
        });

        let preview = text(&full.replace("Thursday", "Friday"), None);
        ticks("4-correction-retract", 8, &|| {
            update(STATE_TRANSFORMING, &final_text, "", &preview, false, MicMeterSnapshot::default())
        });
        capture(mtm, dir, &format!("{theme}-5-transforming"));

        let done = text(&full.replace("Thursday", "Friday"), None);
        update(STATE_BUFFER_READY, &done, "", &empty, false, MicMeterSnapshot::default());
        pump(0.6);
        capture(mtm, dir, &format!("{theme}-6-buffer-ready"));

        update(STATE_ERROR, &empty, "Deepgram connection failed: 401 Unauthorized", &empty, false, MicMeterSnapshot::default());
        pump(0.6);
        capture(mtm, dir, &format!("{theme}-7-error"));

        overlay.hide();
        for index in 0..3 {
            pump(0.05);
            capture(mtm, dir, &format!("{theme}-8-pop-out-{index:02}"));
        }
        pump(0.5);
        backdrop.orderOut(None);
        pump(0.3);
    }
}
