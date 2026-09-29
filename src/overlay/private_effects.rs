//! Undocumented AppKit and Core Animation pieces the overlay uses, all looked
//! up at run time: the `variableBlur` filter that gives the halo its
//! progressive blur, the inner parameters of Liquid Glass (its
//! `glassBackground` filter and its rim highlight), and the switch that keeps
//! a scroll view from insetting its text away from the glass's corners.
//! Nothing here is public API. Every lookup can fail on a later macOS;
//! callers then keep the documented behavior.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2::{msg_send, sel, Message};
use objc2_app_kit::{NSScrollView, NSView, NSVisualEffectView};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSNumber, NSString};
use objc2_quartz_core::CALayer;

/// The glass's inner parameters as tuned. The glass's own values depend on
/// its style and on the window's active appearance; read them with
/// `read_glass_internals`. `Default` is what macOS 26.6 uses for Clear glass
/// with active appearance (measured).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassInternals {
    pub blur_radius: f64,
    pub refraction_amount: f64,
    pub refraction_height: f64,
    pub face_opacity: f64,
    pub face_white: f64,
    pub face_black: f64,
    pub face_saturation: f64,
    pub shadow_opacity: f64,
    /// Strength of the rim highlight's key light (the bright edge).
    pub highlight_key_amount: f64,
    /// Strength of the rim highlight's fill light (the opposite edge).
    pub highlight_fill_amount: f64,
    pub highlight_spread: f64,
    pub highlight_curvature: f64,
}

impl Default for GlassInternals {
    fn default() -> Self {
        Self {
            blur_radius: 1.0,
            refraction_amount: -60.0,
            refraction_height: 20.0,
            face_opacity: 1.0,
            face_white: 1.15,
            face_black: 0.075,
            face_saturation: 1.06,
            shadow_opacity: 0.0,
            highlight_key_amount: 0.5,
            highlight_fill_amount: 0.5,
            highlight_spread: 2.055_335_325_932_132,
            highlight_curvature: 0.7,
        }
    }
}

/// The private `variableBlur` filter: blurs what is behind a visual effect
/// view with a radius that follows a mask (alpha 1 is the full radius, alpha
/// 0 no blur).
#[derive(Debug)]
pub struct VariableBlur {
    filter: Retained<NSObject>,
}

impl VariableBlur {
    /// `None` when this macOS has no `CAFilter` class or no `variableBlur`
    /// filter.
    pub fn new() -> Option<Self> {
        let class = AnyClass::get(c"CAFilter")?;
        // SAFETY: `+[CAFilter filterWithType:]` takes an NSString and returns
        // an autoreleased filter object or nil.
        let filter: Option<Retained<NSObject>> =
            unsafe { msg_send![class, filterWithType: &*NSString::from_str("variableBlur")] };
        filter.map(|filter| Self { filter })
    }

    /// Whether `effect_view`'s blur layer is running this filter now. AppKit
    /// may rebuild that layer (for example when the window's appearance
    /// changes), which drops the filter.
    pub fn is_applied(&self, effect_view: &NSVisualEffectView) -> bool {
        backdrop_layer(effect_view)
            .and_then(|layer| layer.filters())
            .is_some_and(|filters| {
                filters.count() == 1
                    && filter_name(&filters.objectAtIndex(0)).as_deref() == Some("variableBlur")
            })
    }

    /// Puts the filter on `effect_view`'s blur layer in place of the
    /// material's own blur, and hides the material's tint layers. Returns
    /// false while the effect view has not built its layers yet.
    pub fn apply(
        &self,
        effect_view: &NSVisualEffectView,
        radius: f64,
        mask: &CGImage,
        scale: f64,
    ) -> bool {
        let Some(backdrop) = backdrop_layer(effect_view) else {
            return false;
        };
        if self.is_applied(effect_view) {
            // The layer holds its own copy of the filter, and assigning an
            // edited filter again does not re-render (measured on macOS
            // 26.6); a key path through the named filter does.
            set_value_for_key_path(
                &backdrop,
                "filters.variableBlur.inputRadius",
                &NSNumber::new_f64(radius),
            );
            set_value_for_key_path(
                &backdrop,
                "filters.variableBlur.inputMaskImage",
                cg_image_object(mask),
            );
        } else {
            set_value(&self.filter, "inputRadius", &NSNumber::new_f64(radius));
            set_value(&self.filter, "inputMaskImage", cg_image_object(mask));
            set_value(&self.filter, "inputNormalizeEdges", &NSNumber::new_bool(true));
            let filters = NSArray::from_slice(&[&*self.filter as &AnyObject]);
            // SAFETY: `filters` holds one CAFilter, the type `CALayer.filters`
            // takes; the layer array copies it.
            unsafe { backdrop.setFilters(Some(&filters)) };
        }
        // Without the display scale the blur renders at 1x and shows a
        // pixelated tile.
        set_value(&backdrop, "scale", &NSNumber::new_f64(scale));
        if let Some(parent) = backdrop.superlayer() {
            // SAFETY: reading the sublayer array of a live layer on the main
            // thread.
            if let Some(siblings) = unsafe { parent.sublayers() } {
                for sibling in siblings.iter() {
                    if !std::ptr::eq(&*sibling, &*backdrop) {
                        sibling.setHidden(true);
                    }
                }
            }
        }
        true
    }
}

/// Sets `internals` on the `glassBackground` filter and rim highlight under
/// `glass`: a glass view, or an `NSGlassEffectContainerView`, which renders all
/// the glass inside it together, so its layers hold the filter rather than
/// each glass view's. Returns false when those layers are not built (yet).
pub fn apply_glass_internals(glass: &NSView, internals: &GlassInternals) -> bool {
    let Some(root) = glass.layer() else {
        return false;
    };
    let Some(background) = find_layer(&root, &|layer| {
        layer
            .filters()
            .is_some_and(|filters| filters.iter().any(|filter| filter_name(&filter).as_deref() == Some("glassBackground")))
    }) else {
        return false;
    };
    let inputs = [
        ("inputBlurRadius", internals.blur_radius),
        ("inputInnerRefractionAmount", internals.refraction_amount),
        ("inputInnerRefractionHeight", internals.refraction_height),
        ("inputFaceOpacity", internals.face_opacity),
        ("inputFaceColorMatrixWhite", internals.face_white),
        ("inputFaceColorMatrixBlack", internals.face_black),
        ("inputFaceColorMatrixSaturation", internals.face_saturation),
        ("inputShadowOpacity", internals.shadow_opacity),
    ];
    for (key, value) in inputs {
        // A key path through the named filter makes Core Animation re-render;
        // changing the filter object in place would not.
        set_value_for_key_path(
            &background,
            &format!("filters.glassBackground.{key}"),
            &NSNumber::new_f64(value),
        );
    }

    let Some(highlight_class) = AnyClass::get(c"CASDFKeyFillHighlightEffect") else {
        return true;
    };
    for_each_layer(&root, &mut |layer| {
        let Some(effect) = value_for_key(layer, "effect") else {
            return;
        };
        if !is_kind_of(&effect, highlight_class) {
            return;
        }
        let settings = [
            ("keyAmount", internals.highlight_key_amount),
            ("fillAmount", internals.highlight_fill_amount),
            ("keySpread", internals.highlight_spread),
            ("fillSpread", internals.highlight_spread),
            ("curvature", internals.highlight_curvature),
        ];
        for (key, value) in settings {
            set_value(&effect, key, &NSNumber::new_f64(value));
        }
        // The layer re-renders when its effect is assigned again.
        set_value(layer, "effect", &effect);
    });
    true
}

/// The glass's own inner parameters as AppKit set them, or `None` when the
/// layers under `glass` (see `apply_glass_internals`) are not built (yet). They change with the glass style
/// and with the window's active appearance, so they are read, not assumed.
pub fn read_glass_internals(glass: &NSView) -> Option<GlassInternals> {
    let root = glass.layer()?;
    let background = find_layer(&root, &|layer| {
        layer
            .filters()
            .is_some_and(|filters| filters.iter().any(|filter| filter_name(&filter).as_deref() == Some("glassBackground")))
    })?;
    let input = |key: &str| -> Option<f64> {
        let value = value_for_key_path(&background, &format!("filters.glassBackground.{key}"))?;
        Some(value.downcast::<NSNumber>().ok()?.doubleValue())
    };
    let mut internals = GlassInternals {
        blur_radius: input("inputBlurRadius")?,
        refraction_amount: input("inputInnerRefractionAmount")?,
        refraction_height: input("inputInnerRefractionHeight")?,
        face_opacity: input("inputFaceOpacity")?,
        face_white: input("inputFaceColorMatrixWhite")?,
        face_black: input("inputFaceColorMatrixBlack")?,
        face_saturation: input("inputFaceColorMatrixSaturation")?,
        shadow_opacity: input("inputShadowOpacity")?,
        ..GlassInternals::default()
    };
    if let Some(highlight_class) = AnyClass::get(c"CASDFKeyFillHighlightEffect") {
        let mut highlight = None;
        for_each_layer(&root, &mut |layer| {
            if highlight.is_some() {
                return;
            }
            if let Some(effect) = value_for_key(layer, "effect") {
                if is_kind_of(&effect, highlight_class) {
                    highlight = Some(effect);
                }
            }
        });
        if let Some(effect) = highlight {
            let number = |key: &str| {
                value_for_key(&effect, key)
                    .and_then(|value| value.downcast::<NSNumber>().ok())
                    .map(|value| value.doubleValue())
            };
            internals.highlight_key_amount = number("keyAmount").unwrap_or(internals.highlight_key_amount);
            internals.highlight_fill_amount = number("fillAmount").unwrap_or(internals.highlight_fill_amount);
            internals.highlight_spread = number("keySpread").unwrap_or(internals.highlight_spread);
            internals.highlight_curvature = number("curvature").unwrap_or(internals.highlight_curvature);
        }
    }
    Some(internals)
}

/// Keeps `scroll_view` from insetting its content away from the rounded
/// corners of the glass it sits in. On macOS 26 a scroll view insets its clip
/// view while an ancestor's rounded corner is near its edge
/// (`-[_NSScrollViewLayoutHelper updateLayoutWithMinimumDocumentFrameSize:]`):
/// as `Expand` shrinks the main glass, its top edge passes the transcript and
/// the text jumped down by up to 6 points (measured on macOS 26.6). The
/// overlay pads its text itself. The public `NSView.cornerConfiguration`
/// arrives in macOS 27. Returns false when the switch is missing.
pub fn disallow_corner_content_insets(scroll_view: &NSScrollView) -> bool {
    if !scroll_view.respondsToSelector(sel!(_setAllowsAdditionalContentInsetsForCornerRadii:)) {
        return false;
    }
    // SAFETY: the selector exists (checked above) and takes one BOOL.
    unsafe {
        let _: () = msg_send![scroll_view, _setAllowsAdditionalContentInsetsForCornerRadii: false];
    }
    true
}

fn value_for_key_path(object: &AnyObject, key_path: &str) -> Option<Retained<AnyObject>> {
    // SAFETY: key-value coding read through a key path Core Animation
    // resolves; returns nil for an unknown value.
    unsafe { msg_send![object, valueForKeyPath: &*NSString::from_str(key_path)] }
}

fn backdrop_layer(effect_view: &NSVisualEffectView) -> Option<Retained<CALayer>> {
    let class = AnyClass::get(c"CABackdropLayer")?;
    let root = effect_view.layer()?;
    find_layer(&root, &|layer| is_kind_of(layer, class))
}

fn is_kind_of(object: &AnyObject, class: &AnyClass) -> bool {
    // SAFETY: `-isKindOfClass:` is defined on every NSObject.
    unsafe { msg_send![object, isKindOfClass: class] }
}

fn find_layer(layer: &CALayer, matches: &dyn Fn(&CALayer) -> bool) -> Option<Retained<CALayer>> {
    if matches(layer) {
        return Some(layer.retain());
    }
    // SAFETY: reading the sublayer array of a live layer on the main thread.
    let sublayers = unsafe { layer.sublayers() }?;
    sublayers.iter().find_map(|sublayer| find_layer(&sublayer, matches))
}

fn for_each_layer(layer: &CALayer, visit: &mut dyn FnMut(&CALayer)) {
    visit(layer);
    // SAFETY: reading the sublayer array of a live layer on the main thread.
    if let Some(sublayers) = unsafe { layer.sublayers() } {
        for sublayer in sublayers.iter() {
            for_each_layer(&sublayer, visit);
        }
    }
}

fn filter_name(filter: &AnyObject) -> Option<String> {
    value_for_key(filter, "name").and_then(|name| name.downcast::<NSString>().ok()).map(|name| name.to_string())
}

fn value_for_key(object: &AnyObject, key: &str) -> Option<Retained<AnyObject>> {
    // SAFETY: key-value coding read of a key the object answers to; returns
    // nil for an unknown value.
    unsafe { msg_send![object, valueForKey: &*NSString::from_str(key)] }
}

fn set_value(object: &AnyObject, key: &str, value: &AnyObject) {
    // SAFETY: key-value coding write of a key the object is known to answer
    // to, with a value of that key's type.
    unsafe {
        let _: () = msg_send![object, setValue: value, forKey: &*NSString::from_str(key)];
    }
}

fn set_value_for_key_path(object: &AnyObject, key_path: &str, value: &AnyObject) {
    // SAFETY: as `set_value`, through a key path Core Animation resolves.
    unsafe {
        let _: () = msg_send![object, setValue: value, forKeyPath: &*NSString::from_str(key_path)];
    }
}

fn cg_image_object(image: &CGImage) -> &AnyObject {
    // SAFETY: CGImage is a Core Foundation type, and CF objects are valid
    // Objective-C objects (toll-free), so the reference can be passed as one.
    unsafe { &*(image as *const CGImage as *const AnyObject) }
}
