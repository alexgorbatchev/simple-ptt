//! Motion and styling of the overlay text: interim words drawn in the
//! secondary label color, a short crossfade whenever the text changes (so
//! interim words settle into final ones and transformed text replaces the
//! original softly), and a shimmer sweeping over text the transformation
//! model is rewriting.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSTextView, NSView,
    NSWorkspace,
};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{
    ns_string, NSArray, NSAttributedStringKey, NSDictionary, NSMutableAttributedString, NSNumber,
    NSRange, NSString,
};
use objc2_quartz_core::{
    kCAMediaTimingFunctionEaseInEaseOut, kCATransitionFade, CABasicAnimation, CAGradientLayer,
    CALayer, CAMediaTiming, CAMediaTimingFunction, CATransition,
};

const TEXT_CROSSFADE_SECONDS: f64 = 0.22;
const SHIMMER_SWEEP_SECONDS: f64 = 1.8;
/// Opacity of text outside the shimmer's bright band.
const SHIMMER_RESTING_ALPHA: f64 = 0.62;
const SHIMMER_ANIMATION_KEY: &str = "simple-ptt.shimmer";
const CROSSFADE_ANIMATION_KEY: &str = "simple-ptt.crossfade";

/// Whether the user turned on Reduce Motion. Read at the moment motion would
/// start, so a change applies without relaunching.
pub fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// UTF-16 range (location, length) of the provisional tail of `text` that
/// starts at byte `provisional_start`, clamped to `text`. `None` when there is
/// no provisional tail inside `text`.
pub fn provisional_utf16_range(text: &str, provisional_start: Option<usize>) -> Option<(usize, usize)> {
    let start = provisional_start.filter(|&start| start < text.len() && text.is_char_boundary(start))?;
    let location = text[..start].encode_utf16().count();
    let length = text[start..].encode_utf16().count();
    Some((location, length))
}

/// Attributes of plain overlay text: `font` in `color`.
pub fn text_attributes(
    font: &NSFont,
    color: &NSColor,
) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    // SAFETY: both keys are immutable AppKit constants.
    let keys: [&NSAttributedStringKey; 2] =
        unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
    let values: [&AnyObject; 2] = [font.as_ref(), color.as_ref()];
    NSDictionary::from_slices(&keys, &values)
}

/// `text` with `attributes`, and its provisional tail in the secondary label
/// color.
pub fn attributed_text(
    text: &str,
    provisional_start: Option<usize>,
    attributes: &NSDictionary<NSAttributedStringKey, AnyObject>,
) -> Retained<NSMutableAttributedString> {
    let ns_text = NSString::from_str(text);
    // SAFETY: `attributes` maps attribute keys to values of their documented
    // types (an `NSFont` and an `NSColor`).
    let base = unsafe {
        objc2_foundation::NSAttributedString::new_with_attributes(&ns_text, attributes)
    };
    let attributed_text = NSMutableAttributedString::from_attributed_nsstring(&base);
    if let Some((location, length)) = provisional_utf16_range(text, provisional_start) {
        mark_provisional(&attributed_text, NSRange::new(location, length));
    }
    attributed_text
}

/// Draws the whole of `text_view` in `color`, then its `provisional` range in
/// the secondary label color, without replacing the text or moving the
/// selection.
pub fn restyle_provisional(text_view: &NSTextView, color: &NSColor, provisional: Option<NSRange>) {
    // SAFETY: the text view's storage is accessed on the main thread that
    // owns the view, inside a begin/end editing pair.
    let Some(text_storage) = (unsafe { text_view.textStorage() }) else {
        return;
    };
    let length = text_storage.length();
    text_storage.beginEditing();
    // SAFETY: the key is an immutable AppKit constant and the value an NSColor.
    unsafe {
        text_storage.addAttribute_value_range(
            NSForegroundColorAttributeName,
            color.as_ref(),
            NSRange::new(0, length),
        );
    }
    if let Some(range) = provisional {
        let location = range.location.min(length);
        let clamped = NSRange::new(location, range.length.min(length - location));
        if clamped.length > 0 {
            mark_provisional(&text_storage, clamped);
        }
    }
    text_storage.endEditing();
}

fn mark_provisional(text: &NSMutableAttributedString, range: NSRange) {
    let provisional_color = NSColor::secondaryLabelColor();
    // SAFETY: the key is an immutable AppKit constant and the value an NSColor.
    unsafe {
        text.addAttribute_value_range(
            NSForegroundColorAttributeName,
            provisional_color.as_ref(),
            range,
        );
    }
}

/// Crossfades the next change of `view`'s content: call it right before the
/// change. Unchanged glyphs look the same in both frames, so only new or
/// restyled words visibly fade. A fade is not motion, so it runs with Reduce
/// Motion on too.
pub fn crossfade_next_change(view: &NSView) {
    let Some(layer) = view.layer() else {
        return;
    };
    let transition = CATransition::new();
    // SAFETY: both names are immutable Core Animation constants.
    unsafe {
        transition.setType(kCATransitionFade);
        transition.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(
            kCAMediaTimingFunctionEaseInEaseOut,
        )));
    }
    transition.setDuration(TEXT_CROSSFADE_SECONDS);
    layer.addAnimation_forKey(&transition, Some(&NSString::from_str(CROSSFADE_ANIMATION_KEY)));
}

/// A bright band sweeping across text while the transformation model
/// rewrites it: a gradient mask on the text's scroll view layer, so it lights
/// the glyphs and never the glass behind them.
#[derive(Debug)]
pub struct Shimmer {
    mask: Retained<CAGradientLayer>,
}

impl Shimmer {
    pub fn new() -> Self {
        let mask = CAGradientLayer::new();
        let dim = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, SHIMMER_RESTING_ALPHA);
        let bright = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 1.0);
        let dim = dim.CGColor();
        let bright = bright.CGColor();
        let colors: [&AnyObject; 3] = [dim.as_ref(), bright.as_ref(), dim.as_ref()];
        // SAFETY: `colors` holds CGColors, the type `CAGradientLayer.colors`
        // documents.
        unsafe { mask.setColors(Some(&NSArray::from_slice(&colors))) };
        mask.setLocations(Some(&shimmer_locations(-0.45)));
        mask.setStartPoint(CGPoint::new(0.0, 0.35));
        mask.setEndPoint(CGPoint::new(1.0, 0.65));
        Self { mask }
    }

    /// Starts the sweep over `view`, or keeps it running. Does nothing with
    /// Reduce Motion on; the glass tint still shows the state.
    pub fn start(&self, view: &NSView) {
        let Some(layer) = view.layer() else {
            return;
        };
        self.mask.setFrame(layer.bounds());
        if is_mask_of(&layer, &self.mask) || reduce_motion() {
            return;
        }

        let key_path = ns_string!("locations");
        let sweep = CABasicAnimation::animationWithKeyPath(Some(key_path));
        // SAFETY: `locations` animates between arrays of NSNumber.
        unsafe {
            sweep.setFromValue(Some(&shimmer_locations(-0.45)));
            sweep.setToValue(Some(&shimmer_locations(1.0)));
        }
        sweep.setDuration(SHIMMER_SWEEP_SECONDS);
        sweep.setRepeatCount(f32::INFINITY);
        self.mask
            .addAnimation_forKey(&sweep, Some(&NSString::from_str(SHIMMER_ANIMATION_KEY)));
        // SAFETY: the mask is a standalone layer owned by this `Shimmer`,
        // not part of any other layer tree.
        unsafe { layer.setMask(Some(&self.mask)) };
    }

    /// Keeps the mask matched to `view` after it is resized.
    pub fn track_bounds(&self, view: &NSView) {
        if let Some(layer) = view.layer() {
            if is_mask_of(&layer, &self.mask) {
                self.mask.setFrame(layer.bounds());
            }
        }
    }

    pub fn stop(&self, view: &NSView) {
        if let Some(layer) = view.layer() {
            if is_mask_of(&layer, &self.mask) {
                // SAFETY: removing the mask restores the layer's own drawing.
                unsafe { layer.setMask(None) };
            }
        }
        self.mask
            .removeAnimationForKey(&NSString::from_str(SHIMMER_ANIMATION_KEY));
    }
}

fn is_mask_of(layer: &CALayer, mask: &CAGradientLayer) -> bool {
    layer
        .mask()
        .is_some_and(|current| std::ptr::eq(&*current, &**mask))
}

/// Gradient stops of the bright band starting at `leading`.
fn shimmer_locations(leading: f64) -> Retained<NSArray<NSNumber>> {
    let stops = [leading, leading + 0.22, leading + 0.45].map(NSNumber::new_f64);
    NSArray::from_retained_slice(&stops)
}

#[cfg(test)]
mod tests {
    use super::provisional_utf16_range;

    #[test]
    fn provisional_range_covers_the_interim_tail_in_utf16_units() {
        let text = "naïve words 👋 still talking ";
        let start = text.find("still").unwrap();

        assert_eq!(
            provisional_utf16_range(text, Some(start)),
            Some((15, "still talking ".len()))
        );
    }

    #[test]
    fn provisional_range_is_absent_without_a_valid_start() {
        assert_eq!(provisional_utf16_range("hello", None), None);
        assert_eq!(provisional_utf16_range("hello", Some(5)), None);
        assert_eq!(provisional_utf16_range("héllo", Some(2)), None);
    }
}
