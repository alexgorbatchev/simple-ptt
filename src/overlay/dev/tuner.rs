//! Dev tool for this branch: `simple-ptt --debug` shows the overlay over the
//! desktop, driven like the app, with a window of live controls for every
//! `GlassTuning` value and a button that copies the values to the clipboard.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton, NSControlStateValueOn,
    NSFloatingWindowLevel, NSGridView, NSPasteboard, NSPasteboardTypeString, NSPopUpButton, NSScreen,
    NSSlider, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer};

use crate::app::overlay_style_from_config;
use crate::config::Config;
use crate::overlay::glass::{
    CorrectionEasing, CorrectionEffect, GlassStyle, GlassTuning, HaloCurve, OverlayAppearance,
};

const EFFECTS: [CorrectionEffect; 5] = [
    CorrectionEffect::Emerge,
    CorrectionEffect::Unfold,
    CorrectionEffect::Pop,
    CorrectionEffect::Slide,
    CorrectionEffect::Expand,
];
const EFFECT_TITLES: [&str; 5] = ["Emerge", "Unfold", "Pop", "Slide", "Expand"];
const EASINGS: [CorrectionEasing; 2] = [CorrectionEasing::Smooth, CorrectionEasing::Spring];
const EASING_TITLES: [&str; 2] = ["Smooth", "Spring"];

use super::curve_editor::{self, CurveEditor};

const CURVE_PRESETS: [(&str, HaloCurve); 4] = [
    ("Linear", HaloCurve::LINEAR),
    ("Ease in", HaloCurve::EASE_IN),
    ("Ease out", HaloCurve::EASE_OUT),
    ("Ease in-out", HaloCurve::EASE_IN_OUT),
];
use crate::overlay::OverlayWindow;
use crate::state::{
    AppState, DeepgramApiKeyFingerprint, DeepgramConnectionStatus, MicMeterSnapshot, OverlayText,
    STATE_BUFFER_READY, STATE_ERROR, STATE_RECORDING, STATE_TRANSFORMING,
};

const SCENARIOS: [&str; 4] = ["Recording", "Transforming", "Buffer ready", "Error"];
const SHOW_CORRECTION: &str = "Show correction";
const HIDE_CORRECTION: &str = "Hide correction";
const STYLES: [GlassStyle; 2] = [GlassStyle::Regular, GlassStyle::Clear];
const APPEARANCES: [OverlayAppearance; 3] = [OverlayAppearance::System, OverlayAppearance::Light, OverlayAppearance::Dark];
const STYLE_TITLES: [&str; 2] = ["Regular", "Clear"];
const APPEARANCE_TITLES: [&str; 3] = ["System", "Light", "Dark"];

/// A `GlassTuning` choice shown as a pop-up.
struct ChoiceField {
    name: &'static str,
    titles: &'static [&'static str],
    get: fn(&GlassTuning) -> usize,
    set: fn(&mut GlassTuning, usize),
}

/// A `GlassTuning` switch shown as a checkbox.
struct SwitchField {
    name: &'static str,
    get: fn(&GlassTuning) -> bool,
    set: fn(&mut GlassTuning, bool),
}

/// A numeric `GlassTuning` value shown as a slider.
struct NumberField {
    name: &'static str,
    min: f64,
    max: f64,
    get: fn(&GlassTuning) -> f64,
    set: fn(&mut GlassTuning, f64),
}

macro_rules! number {
    ($name:literal, $min:expr, $max:expr, $($path:ident).+) => {
        NumberField { name: $name, min: $min, max: $max, get: |t| t.$($path).+, set: |t, v| t.$($path).+ = v }
    };
}

macro_rules! switch {
    ($name:literal, $($path:ident).+) => {
        SwitchField { name: $name, get: |t| t.$($path).+, set: |t, v| t.$($path).+ = v }
    };
}

const CHOICES: [ChoiceField; 5] = [
    ChoiceField {
        name: "style",
        titles: &STYLE_TITLES,
        get: |t| STYLES.iter().position(|v| *v == t.style).unwrap_or(0),
        set: |t, i| t.style = STYLES[i],
    },
    ChoiceField {
        name: "appearance",
        titles: &APPEARANCE_TITLES,
        get: |t| APPEARANCES.iter().position(|v| *v == t.appearance).unwrap_or(0),
        set: |t, i| t.appearance = APPEARANCES[i],
    },
    ChoiceField {
        name: "correction_effect",
        titles: &EFFECT_TITLES,
        get: |t| EFFECTS.iter().position(|v| *v == t.correction_effect).unwrap_or(0),
        set: |t, i| t.correction_effect = EFFECTS[i],
    },
    ChoiceField {
        name: "correction_easing",
        titles: &EASING_TITLES,
        get: |t| EASINGS.iter().position(|v| *v == t.correction_easing).unwrap_or(0),
        set: |t, i| t.correction_easing = EASINGS[i],
    },
    ChoiceField {
        name: "halo_style (fallback)",
        titles: &STYLE_TITLES,
        get: |t| STYLES.iter().position(|v| *v == t.halo_style).unwrap_or(0),
        set: |t, i| t.halo_style = STYLES[i],
    },
];

const SWITCHES: [SwitchField; 3] = [
    switch!("halo_enabled", halo_enabled),
    switch!("halo_under_panels", halo_under_panels),
    switch!("internals_enabled", internals_enabled),
];

static GENERAL: [NumberField; 13] = [
    number!("corner_radius", 0.0, 48.0, corner_radius),
    number!("merge_spacing", 0.0, 40.0, merge_spacing),
    number!("tint_hue", 0.0, 1.0, tint_hue),
    number!("tint_saturation", 0.0, 1.0, tint_saturation),
    number!("tint_brightness", 0.0, 1.0, tint_brightness),
    number!("tint_alpha", 0.0, 1.0, tint_alpha),
    number!("halo_margin", 0.0, 160.0, halo_margin),
    number!("halo_blur_radius", 0.0, 80.0, halo_blur_radius),
    number!("halo_dim", 0.0, 0.9, halo_dim),
    number!("pop_seconds", 0.0, 1.5, pop_seconds),
    number!("pop_shrink", 0.0, 0.5, pop_shrink),
    number!("pop_opacity", 0.0, 1.0, pop_opacity),
    number!("correction_seconds", 0.05, 1.5, correction_seconds),
];

static INTERNALS: [NumberField; 12] = [
    number!("blur_radius", 0.0, 60.0, internals.blur_radius),
    number!("refraction_amount", -300.0, 300.0, internals.refraction_amount),
    number!("refraction_height", 0.0, 120.0, internals.refraction_height),
    number!("face_opacity", 0.0, 1.0, internals.face_opacity),
    number!("face_white", 0.0, 1.5, internals.face_white),
    number!("face_black", 0.0, 1.0, internals.face_black),
    number!("face_saturation", 0.0, 3.0, internals.face_saturation),
    number!("shadow_opacity", 0.0, 1.0, internals.shadow_opacity),
    number!("highlight_key_amount", 0.0, 2.0, internals.highlight_key_amount),
    number!("highlight_fill_amount", 0.0, 2.0, internals.highlight_fill_amount),
    number!("highlight_spread", 0.0, 6.0, internals.highlight_spread),
    number!("highlight_curvature", 0.0, 2.0, internals.highlight_curvature),
];

type NumberControl = (&'static NumberField, Retained<NSSlider>, Retained<NSTextField>);

struct Controls {
    scenario: Retained<NSPopUpButton>,
    choices: Vec<Retained<NSPopUpButton>>,
    switches: Vec<Retained<NSButton>>,
    numbers: Vec<NumberControl>,
    /// Row name labels, by control name, greyed out with their control.
    names: Vec<(&'static str, Retained<NSTextField>)>,
    curve_editor: Retained<CurveEditor>,
    curve_range: Retained<NSSlider>,
    curve_range_value: Retained<NSTextField>,
    curve_values: Retained<NSTextField>,
    curve_presets: Vec<Retained<NSButton>>,
}

/// Whether the control called `name` has any effect under `tuning`.
fn applies(name: &str, tuning: &GlassTuning, progressive: bool, reduce_motion: bool) -> bool {
    let halo = tuning.halo_enabled;
    let tinted = tuning.tint_alpha > 0.0;
    match name {
        "halo_margin" | "halo_curve" => halo,
        "halo_under_panels" | "halo_blur_radius" | "halo_dim" => halo && progressive,
        "halo_style (fallback)" => halo && !progressive,
        "tint_brightness" => tinted,
        "tint_saturation" => tinted && tuning.tint_brightness > 0.0,
        "tint_hue" => tinted && tuning.tint_brightness > 0.0 && tuning.tint_saturation > 0.0,
        "pop_shrink" | "pop_opacity" => !reduce_motion,
        "correction_effect" | "correction_easing" | "correction_seconds" => !reduce_motion,
        _ if INTERNALS.iter().any(|field| field.name == name) => tuning.internals_enabled,
        _ => true,
    }
}

pub struct TunerState {
    overlay: OverlayWindow,
    app_state: Arc<AppState>,
    controls: RefCell<Option<Controls>>,
    /// The overlay stays hidden until then, so "Pop again" shows both motions.
    hidden_until: Cell<Option<Instant>>,
    /// When the last control change happened, while its re-pop is pending.
    /// Glass picks up some changes (corner radius) only when the window pops
    /// in again, so a pause in changes triggers one.
    repop_after: Cell<Option<Instant>>,
    ticks: Cell<u64>,
    /// Whether the correction glass is shown, over any scenario.
    correction_shown: Cell<bool>,
}

fn format_value(value: f64) -> String {
    format!("{value:.2}")
}

fn format_curve(curve: HaloCurve) -> String {
    let point = |(x, y): (f64, f64)| format!("({x:.2}, {y:.3})");
    format!(
        "{} {} {} {}",
        point(curve.start),
        point(curve.control1),
        point(curve.control2),
        point(curve.end)
    )
}

impl TunerState {
    fn read_tuning(&self) -> GlassTuning {
        let controls = self.controls.borrow();
        let controls = controls.as_ref().expect("controls built");
        let mut tuning = self.overlay.glass_tuning();
        for (field, popup) in CHOICES.iter().zip(&controls.choices) {
            (field.set)(&mut tuning, popup.indexOfSelectedItem().max(0) as usize);
        }
        for (field, checkbox) in SWITCHES.iter().zip(&controls.switches) {
            (field.set)(&mut tuning, checkbox.state() == NSControlStateValueOn);
        }
        for (field, slider, label) in &controls.numbers {
            let value = (slider.doubleValue() * 100.0).round() / 100.0;
            (field.set)(&mut tuning, value);
            label.setStringValue(&NSString::from_str(&format_value(value)));
        }
        tuning.halo_curve = controls.curve_editor.curve();
        let was_enabled = self.overlay.glass_tuning().internals_enabled;
        if tuning.internals_enabled && !was_enabled {
            if let Some(own) = self.overlay.glass_internals_now() {
                tuning.internals = own;
                for (field, slider, label) in &controls.numbers {
                    if INTERNALS.iter().any(|internal| std::ptr::eq(internal, *field)) {
                        let value = (field.get)(&tuning);
                        slider.setDoubleValue(value);
                        label.setStringValue(&NSString::from_str(&format_value(value)));
                    }
                }
            }
        }
        controls.curve_values.setStringValue(&NSString::from_str(&format_curve(tuning.halo_curve)));
        self.refresh_enabled(&tuning);
        tuning
    }

    /// Enables only the controls that change something under `tuning`.
    fn refresh_enabled(&self, tuning: &GlassTuning) {
        let controls = self.controls.borrow();
        let controls = controls.as_ref().expect("controls built");
        let progressive = self.overlay.halo_is_progressive();
        let reduce_motion =
            objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        let enabled = |name: &str| applies(name, tuning, progressive, reduce_motion);
        for (field, popup) in CHOICES.iter().zip(&controls.choices) {
            popup.setEnabled(enabled(field.name));
        }
        for (field, checkbox) in SWITCHES.iter().zip(&controls.switches) {
            checkbox.setEnabled(enabled(field.name));
        }
        for (field, slider, value_label) in &controls.numbers {
            let on = enabled(field.name);
            slider.setEnabled(on);
            value_label.setEnabled(on);
        }
        let curve_on = enabled("halo_curve");
        controls.curve_editor.set_enabled(curve_on);
        controls.curve_range.setEnabled(curve_on);
        controls.curve_range_value.setEnabled(curve_on);
        for preset in &controls.curve_presets {
            preset.setEnabled(curve_on);
        }
        for (name, label) in &controls.names {
            let color = if enabled(name) {
                objc2_app_kit::NSColor::labelColor()
            } else {
                objc2_app_kit::NSColor::disabledControlTextColor()
            };
            label.setTextColor(Some(&color));
        }
    }

    fn show_tuning(&self, tuning: &GlassTuning) {
        let controls = self.controls.borrow();
        let controls = controls.as_ref().expect("controls built");
        for (field, popup) in CHOICES.iter().zip(&controls.choices) {
            popup.selectItemAtIndex((field.get)(tuning) as isize);
        }
        for (field, checkbox) in SWITCHES.iter().zip(&controls.switches) {
            checkbox.setState(if (field.get)(tuning) { 1 } else { 0 });
        }
        for (field, slider, label) in &controls.numbers {
            let value = (field.get)(tuning);
            slider.setDoubleValue(value);
            label.setStringValue(&NSString::from_str(&format_value(value)));
        }
        controls.curve_editor.set_curve(tuning.halo_curve);
        controls.curve_values.setStringValue(&NSString::from_str(&format_curve(tuning.halo_curve)));
        self.refresh_enabled(tuning);
    }

    fn tick(&self) {
        let tick = self.ticks.get() + 1;
        self.ticks.set(tick);
        if let Some(changed_at) = self.repop_after.get() {
            if changed_at.elapsed() >= Duration::from_millis(300) {
                self.repop_after.set(None);
                let pop_out = Duration::from_secs_f64(self.overlay.glass_tuning().pop_seconds);
                self.hidden_until.set(Some(Instant::now() + pop_out + Duration::from_millis(60)));
            }
        }
        if let Some(until) = self.hidden_until.get() {
            if Instant::now() < until {
                self.overlay.hide();
                return;
            }
            self.hidden_until.set(None);
        }

        let scenario = self
            .controls
            .borrow()
            .as_ref()
            .map(|controls| controls.scenario.indexOfSelectedItem().max(0) as usize)
            .unwrap_or(0);
        let final_text = "Let's move the standup to Thursday afternoon so the design review has a full morning, and ask Priya to share the ";
        let interim = "updated mockups before lunch ";
        let live = OverlayText::default();
        let texts = AppState::new();
        texts.set_live_overlay_text(format!("{final_text}{interim}"), Some(final_text.len()));
        let main_text = texts.overlay_text_snapshot();
        texts.set_live_overlay_correction_text("make it Friday instead and ", Some(23));
        let correction_text = texts.overlay_correction_text_snapshot();
        texts.set_overlay_text(format!("{final_text}{interim}").replace("Thursday", "Friday"));
        let preview_text = texts.overlay_text_snapshot();

        let level = (128.0 + 110.0 * ((tick as f64) * 0.35).sin()) as u8;
        let meter = MicMeterSnapshot { clip_event_counter: 0, level, peak: level.saturating_add(30), mic_active: true };
        let quiet = MicMeterSnapshot::default();
        let connection = self.app_state.deepgram_connection();
        let (state, main, error, scenario_correction, mic) = match scenario {
            0 => (STATE_RECORDING, &main_text, "", &live, meter),
            1 => (STATE_TRANSFORMING, &main_text, "", &preview_text, quiet),
            2 => (STATE_BUFFER_READY, &preview_text, "", &live, quiet),
            _ => (STATE_ERROR, &live, "Deepgram connection failed: 401 Unauthorized", &live, quiet),
        };
        let correction_active = self.correction_shown.get();
        let correction = if correction_active { &correction_text } else { scenario_correction };
        let mtm = MainThreadMarker::new().expect("main thread");
        self.overlay.update(mtm, state, connection, false, main, error, correction, correction_active, 1.0, "", mic);
    }
}

pub struct TunerIvars {
    state: Rc<TunerState>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SimplePttOverlayTuner"]
    #[ivars = TunerIvars]
    pub struct OverlayTuner;

    impl OverlayTuner {
        #[unsafe(method(controlChanged:))]
        fn control_changed(&self, _sender: Option<&AnyObject>) {
            let state = &self.ivars().state;
            let tuning = state.read_tuning();
            state.overlay.set_glass_tuning(tuning);
            state.repop_after.set(Some(Instant::now()));
        }

        #[unsafe(method(scenarioChanged:))]
        fn scenario_changed(&self, _sender: Option<&AnyObject>) {}

        #[unsafe(method(curveRangeChanged:))]
        fn curve_range_changed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            let value: f64 = unsafe { msg_send![sender, doubleValue] };
            let value = (value * 100.0).round() / 100.0;
            let state = &self.ivars().state;
            if let Some(controls) = state.controls.borrow().as_ref() {
                controls.curve_editor.set_y_max(value);
                controls.curve_range_value.setStringValue(&NSString::from_str(&format_value(value)));
            }
        }

        #[unsafe(method(curvePreset:))]
        fn curve_preset(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            let tag: isize = unsafe { msg_send![sender, tag] };
            let Some((_, curve)) = CURVE_PRESETS.get(tag.max(0) as usize) else { return };
            let state = &self.ivars().state;
            if let Some(controls) = state.controls.borrow().as_ref() {
                let y_max = controls.curve_editor.y_max();
                controls.curve_editor.set_curve(curve_editor::fitted(*curve, y_max));
            }
            let tuning = state.read_tuning();
            state.overlay.set_glass_tuning(tuning);
        }

        #[unsafe(method(copyValues:))]
        fn copy_values(&self, _sender: Option<&AnyObject>) {
            let tuning = self.ivars().state.overlay.glass_tuning();
            let text = format!("{tuning:#?}");
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            pasteboard.setString_forType(&NSString::from_str(&text), unsafe { NSPasteboardTypeString });
            println!("{text}");
        }

        #[unsafe(method(toggleCorrection:))]
        fn toggle_correction(&self, sender: Option<&AnyObject>) {
            let state = &self.ivars().state;
            let shown = !state.correction_shown.get();
            state.correction_shown.set(shown);
            if let Some(sender) = sender {
                let title = NSString::from_str(if shown { HIDE_CORRECTION } else { SHOW_CORRECTION });
                let _: () = unsafe { msg_send![sender, setTitle: &*title] };
            }
        }

        #[unsafe(method(popAgain:))]
        fn pop_again(&self, _sender: Option<&AnyObject>) {
            self.ivars().state.hidden_until.set(Some(Instant::now() + Duration::from_millis(900)));
        }

        #[unsafe(method(resetDefaults:))]
        fn reset_defaults(&self, _sender: Option<&AnyObject>) {
            let state = &self.ivars().state;
            state.show_tuning(&GlassTuning::default());
            state.overlay.set_glass_tuning(GlassTuning::default());
            state.repop_after.set(Some(Instant::now()));
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let mtm = MainThreadMarker::new().expect("main thread");
            NSApplication::sharedApplication(mtm).terminate(None);
        }
    }
);

fn view<T: objc2::Message + objc2::ClassType>(value: &Retained<T>) -> Retained<NSView> {
    // SAFETY: only called with NSView subclasses.
    unsafe { Retained::cast_unchecked(value.clone()) }
}

fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSView> {
    view(&name_label(mtm, text))
}

fn name_label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    small(&label);
    label
}

/// Small control size and font, to keep the tuner window compact.
fn small(control: &objc2_app_kit::NSControl) {
    control.setControlSize(objc2_app_kit::NSControlSize::Small);
    control.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(
        objc2_app_kit::NSFont::smallSystemFontSize(),
    )));
}

fn popup(mtm: MainThreadMarker, titles: &[&str], target: &AnyObject, action: objc2::runtime::Sel) -> Retained<NSPopUpButton> {
    let popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 26.0)),
        false,
    );
    small(&popup);
    for title in titles {
        popup.addItemWithTitle(&NSString::from_str(title));
    }
    unsafe {
        popup.setTarget(Some(target));
        popup.setAction(Some(action));
    }
    popup
}

fn slider_rows(
    mtm: MainThreadMarker,
    fields: &'static [NumberField],
    target: &AnyObject,
    rows: &mut Vec<Retained<NSArray<NSView>>>,
    numbers: &mut Vec<NumberControl>,
    names: &mut Vec<(&'static str, Retained<NSTextField>)>,
) {
    for field in fields {
        let slider = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                field.min,
                field.min,
                field.max,
                Some(target),
                Some(sel!(controlChanged:)),
                mtm,
            )
        };
        slider.setContinuous(true);
        small(&slider);
        slider.widthAnchor().constraintEqualToConstant(150.0).setActive(true);
        let value_label = name_label(mtm, "0.00");
        let name = name_label(mtm, field.name);
        rows.push(NSArray::from_retained_slice(&[view(&name), view(&slider), view(&value_label)]));
        numbers.push((field, slider, value_label));
        names.push((field.name, name));
    }
}

fn grid(mtm: MainThreadMarker, rows: &[Retained<NSArray<NSView>>]) -> Retained<NSGridView> {
    let grid = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(rows), mtm);
    grid.setRowSpacing(2.0);
    grid.setColumnSpacing(8.0);
    // Keep each column at its own size instead of stretching to fill.
    for orientation in [
        objc2_app_kit::NSLayoutConstraintOrientation::Vertical,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    ] {
        grid.setContentHuggingPriority_forOrientation(999.0, orientation);
    }
    grid
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();

    let app_state = AppState::new();
    let api_key = DeepgramApiKeyFingerprint::of("debug-key");
    app_state.set_deepgram_connection_status(DeepgramConnectionStatus::Connected, api_key);
    let mut style = overlay_style_from_config(&Config::default());
    style.deepgram_api_key = Some(api_key);
    let overlay = OverlayWindow::new(mtm, &style, app_state.clone());
    // Near the top of the screen, leaving room for the controls below it.
    overlay.pin_to_top(Some(24.0));

    let state = Rc::new(TunerState {
        overlay,
        app_state,
        controls: RefCell::new(None),
        hidden_until: Cell::new(None),
        repop_after: Cell::new(None),
        ticks: Cell::new(0),
        correction_shown: Cell::new(false),
    });
    let tuner = OverlayTuner::alloc(mtm).set_ivars(TunerIvars { state: state.clone() });
    let tuner: Retained<OverlayTuner> = unsafe { msg_send![super(tuner), init] };
    let target: &AnyObject = &tuner;

    let scenario = popup(mtm, &SCENARIOS, target, sel!(scenarioChanged:));
    let mut left_rows = vec![NSArray::from_retained_slice(&[label(mtm, "scenario"), view(&scenario), label(mtm, "")])];
    let mut choices = Vec::new();
    let mut names = Vec::new();
    for field in &CHOICES {
        let choice = popup(mtm, field.titles, target, sel!(controlChanged:));
        let name = name_label(mtm, field.name);
        left_rows.push(NSArray::from_retained_slice(&[view(&name), view(&choice), label(mtm, "")]));
        choices.push(choice);
        names.push((field.name, name));
    }
    let mut switches = Vec::new();
    for field in &SWITCHES {
        let checkbox = unsafe {
            NSButton::checkboxWithTitle_target_action(&NSString::from_str(field.name), Some(target), Some(sel!(controlChanged:)), mtm)
        };
        small(&checkbox);
        switches.push(checkbox);
    }
    left_rows.push(NSArray::from_retained_slice(&[label(mtm, ""), view(&switches[0]), label(mtm, "")]));
    left_rows.push(NSArray::from_retained_slice(&[label(mtm, ""), view(&switches[1]), label(mtm, "")]));
    let mut numbers = Vec::new();
    slider_rows(mtm, &GENERAL, target, &mut left_rows, &mut numbers, &mut names);

    let mut right_rows = vec![NSArray::from_retained_slice(&[label(mtm, "glass internals"), view(&switches[2]), label(mtm, "")])];
    slider_rows(mtm, &INTERNALS, target, &mut right_rows, &mut numbers, &mut names);

    let weak_state = Rc::downgrade(&state);
    let curve_editor = CurveEditor::new(mtm, move |_curve| {
        if let Some(state) = weak_state.upgrade() {
            let tuning = state.read_tuning();
            state.overlay.set_glass_tuning(tuning);
        }
    });
    for orientation in [
        objc2_app_kit::NSLayoutConstraintOrientation::Vertical,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    ] {
        curve_editor.setContentCompressionResistancePriority_forOrientation(1000.0, orientation);
    }
    let curve_values = name_label(mtm, "");
    let curve_name = name_label(mtm, "halo_curve");
    names.push(("halo_curve", curve_name.clone()));
    let curve_presets: Vec<Retained<NSButton>> = CURVE_PRESETS
        .iter()
        .enumerate()
        .map(|(index, (title, _))| {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str(title), Some(target), Some(sel!(curvePreset:)), mtm)
            };
            button.setTag(index as isize);
            small(&button);
            button
        })
        .collect();
    let presets_row = NSStackView::stackViewWithViews(
        &NSArray::from_retained_slice(&curve_presets.iter().map(view).collect::<Vec<_>>()),
        mtm,
    );
    let (range_min, range_max) = curve_editor::Y_MAX_RANGE;
    let curve_range = unsafe {
        NSSlider::sliderWithValue_minValue_maxValue_target_action(
            range_max,
            range_min,
            range_max,
            Some(target),
            Some(sel!(curveRangeChanged:)),
            mtm,
        )
    };
    curve_range.setContinuous(true);
    small(&curve_range);
    curve_range.widthAnchor().constraintEqualToConstant(150.0).setActive(true);
    let curve_range_value = name_label(mtm, &format_value(range_max));
    let curve_range_name = name_label(mtm, "curve_y_max");
    names.push(("halo_curve", curve_range_name.clone()));
    right_rows.push(NSArray::from_retained_slice(&[
        view(&curve_range_name),
        view(&curve_range),
        view(&curve_range_value),
    ]));
    right_rows.push(NSArray::from_retained_slice(&[view(&curve_name), view(&curve_editor), label(mtm, "")]));
    right_rows.push(NSArray::from_retained_slice(&[label(mtm, "x: halo edge → panel"), view(&curve_values), label(mtm, "")]));
    right_rows.push(NSArray::from_retained_slice(&[label(mtm, "y: blur strength"), view(&presets_row), label(mtm, "")]));

    let right_grid = grid(mtm, &right_rows);
    // The curve editor, its readout, and the preset buttons span the slider
    // and value columns, so they do not push the values away from their
    // sliders.
    let rows = right_grid.numberOfRows();
    for row in [rows - 3, rows - 2, rows - 1] {
        right_grid
            .rowAtIndex(row)
            .mergeCellsInRange(objc2_foundation::NSRange::new(1, 2));
    }
    let columns = NSStackView::stackViewWithViews(
        &NSArray::from_retained_slice(&[view(&grid(mtm, &left_rows)), view(&right_grid)]),
        mtm,
    );
    columns.setSpacing(24.0);
    columns.setAlignment(objc2_app_kit::NSLayoutAttribute::Top);

    let button = |title: &str, action| {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str(title), Some(target), Some(action), mtm)
        };
        small(&button);
        view(&button)
    };
    let buttons = NSStackView::stackViewWithViews(
        &NSArray::from_retained_slice(&[
            button("Copy values", sel!(copyValues:)),
            button(SHOW_CORRECTION, sel!(toggleCorrection:)),
            button("Pop again", sel!(popAgain:)),
            button("Reset defaults", sel!(resetDefaults:)),
            button("Quit", sel!(quit:)),
        ]),
        mtm,
    );
    let content = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&[view(&columns), view(&buttons)]), mtm);
    content.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    content.setSpacing(10.0);
    let container = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)));
    content.setTranslatesAutoresizingMaskIntoConstraints(false);
    container.addSubview(&content);
    const PADDING: f64 = 20.0;
    objc2_app_kit::NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        content.leadingAnchor().constraintEqualToAnchor_constant(&container.leadingAnchor(), PADDING),
        container.trailingAnchor().constraintEqualToAnchor_constant(&content.trailingAnchor(), PADDING),
        content.topAnchor().constraintEqualToAnchor_constant(&container.topAnchor(), PADDING),
        container.bottomAnchor().constraintEqualToAnchor_constant(&content.bottomAnchor(), PADDING),
    ]));

    state.controls.replace(Some(Controls {
        scenario,
        choices,
        switches,
        numbers,
        names,
        curve_editor,
        curve_range,
        curve_range_value,
        curve_values,
        curve_presets,
    }));
    state.show_tuning(&GlassTuning::default());

    let size = container.fittingSize();
    let screen = NSScreen::mainScreen(mtm).expect("screen").visibleFrame();
    let frame = NSRect::new(
        NSPoint::new(screen.origin.x + (screen.size.width - size.width) / 2.0, screen.origin.y + 8.0),
        size,
    );
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Overlay tuner"));
    // Stays above the terminal it is launched from, next to the overlay.
    window.setLevel(NSFloatingWindowLevel);
    window.setContentView(Some(&container));
    window.makeKeyAndOrderFront(None);
    app.activate();

    let tick_state = state.clone();
    let tick = RcBlock::new(move |_timer: NonNull<NSTimer>| tick_state.tick());
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(0.075, true, &tick) };
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };

    let _keep = (tuner, window, timer);
    app.run();
}
