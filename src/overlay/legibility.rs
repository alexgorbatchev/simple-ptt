//! Which appearance the overlay's text takes so it stays legible on tinted
//! glass. `NSGlassEffectView.tintColor` tints the glass but leaves its
//! content in the window's appearance, so a strong tint can match the
//! semantic label color: black text on a black tint read at 1.15:1 (measured
//! on captures). The text takes whichever appearance's label color contrasts
//! more with the glass behind it; semantic colors (label, secondary label,
//! system red) then resolve for that appearance.

use std::cell::Cell;

use block2::StackBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSColorSpace,
};
use objc2_foundation::NSArray;

/// An sRGB color, components from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Srgb {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

impl Srgb {
    /// This color drawn over `below`, which is taken as opaque.
    pub fn over(self, below: Srgb) -> Srgb {
        let blend = |top: f64, bottom: f64| (top * self.alpha) + (bottom * (1.0 - self.alpha));
        Srgb {
            red: blend(self.red, below.red),
            green: blend(self.green, below.green),
            blue: blend(self.blue, below.blue),
            alpha: 1.0,
        }
    }

    /// WCAG 2 relative luminance, taking the color as opaque.
    pub fn luminance(self) -> f64 {
        let linear = |value: f64| {
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        (0.2126 * linear(self.red)) + (0.7152 * linear(self.green)) + (0.0722 * linear(self.blue))
    }
}

/// WCAG 2 contrast ratio of two opaque colors, from 1 to 21.
pub fn contrast_ratio(first: Srgb, second: Srgb) -> f64 {
    let (first, second) = (first.luminance(), second.luminance());
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextAppearance {
    Light,
    Dark,
}

/// An appearance's `windowBackgroundColor` and `labelColor`, in sRGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppearanceColors {
    pub background: Srgb,
    pub label: Srgb,
}

/// The appearance for text on glass drawn in the `glass` appearance and
/// tinted toward `tint` by the tint's alpha. The glass behind the text is
/// taken as that tint over the glass appearance's window background; on
/// captures, half black over light glass measured 0.19 to 0.24 luminance
/// against 0.21 from this, and untinted light glass 0.72 to 0.88 against 1.0.
/// The text takes the appearance whose label color contrasts more with it;
/// a tie keeps the glass's appearance.
pub fn text_appearance(
    glass: TextAppearance,
    tint: Option<Srgb>,
    light: AppearanceColors,
    dark: AppearanceColors,
) -> TextAppearance {
    let base = match glass {
        TextAppearance::Light => light.background,
        TextAppearance::Dark => dark.background,
    };
    let behind = tint.map_or(base, |tint| tint.over(base));
    let contrast = |colors: AppearanceColors| contrast_ratio(colors.label.over(behind), behind);
    let (light_contrast, dark_contrast) = (contrast(light), contrast(dark));
    match glass {
        TextAppearance::Light if dark_contrast > light_contrast => TextAppearance::Dark,
        TextAppearance::Dark if light_contrast > dark_contrast => TextAppearance::Light,
        _ => glass,
    }
}

/// `color` in sRGB, as the current drawing appearance resolves it.
pub fn srgb(color: &NSColor) -> Option<Srgb> {
    let color = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    Some(Srgb {
        red: color.redComponent(),
        green: color.greenComponent(),
        blue: color.blueComponent(),
        alpha: color.alphaComponent(),
    })
}

/// `appearance`'s window background and label colors.
pub fn appearance_colors(appearance: &NSAppearance) -> Option<AppearanceColors> {
    let resolved = Cell::new(None);
    let resolve = StackBlock::new(|| {
        let background = srgb(&NSColor::windowBackgroundColor());
        let label = srgb(&NSColor::labelColor());
        resolved.set(background.zip(label).map(|(background, label)| AppearanceColors { background, label }));
    });
    appearance.performAsCurrentDrawingAppearance(&resolve);
    resolved.get()
}

/// The system appearance for `text`.
pub fn named_appearance(text: TextAppearance) -> Option<Retained<NSAppearance>> {
    // SAFETY: both names are immutable AppKit constants.
    let name = unsafe {
        match text {
            TextAppearance::Light => NSAppearanceNameAqua,
            TextAppearance::Dark => NSAppearanceNameDarkAqua,
        }
    };
    NSAppearance::appearanceNamed(name)
}

/// Whether `appearance` is light or dark.
pub fn text_appearance_of(appearance: &NSAppearance) -> TextAppearance {
    // SAFETY: both names are immutable AppKit constants.
    let (aqua, dark_aqua) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let best = appearance.bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[aqua, dark_aqua]));
    if best.is_some_and(|name| &*name == dark_aqua) {
        TextAppearance::Dark
    } else {
        TextAppearance::Light
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(value: f64, alpha: f64) -> Srgb {
        Srgb { red: value, green: value, blue: value, alpha }
    }

    // As AppKit resolves them on macOS 26.6 (`appkit_resolves_both_appearances`).
    fn light() -> AppearanceColors {
        AppearanceColors { background: grey(1.0, 1.0), label: grey(0.0, 0.847) }
    }

    fn dark() -> AppearanceColors {
        AppearanceColors { background: grey(0.118, 1.0), label: grey(1.0, 0.847) }
    }

    fn choose(glass: TextAppearance, tint: Option<Srgb>) -> TextAppearance {
        text_appearance(glass, tint, light(), dark())
    }

    #[test]
    fn appkit_resolves_both_appearances() {
        let colors = |text| {
            let appearance = named_appearance(text).expect("system appearance");
            assert_eq!(text_appearance_of(&appearance), text);
            appearance_colors(&appearance).expect("colors resolve")
        };
        let (light_colors, dark_colors) = (colors(TextAppearance::Light), colors(TextAppearance::Dark));
        eprintln!("RESOLVED light={light_colors:?} dark={dark_colors:?}");
        assert!(light_colors.background.luminance() > 0.5 && dark_colors.background.luminance() < 0.1);
        assert!(light_colors.label.red < 0.1 && dark_colors.label.red > 0.9);
    }

    #[test]
    fn contrast_ratio_follows_wcag() {
        assert!((contrast_ratio(grey(0.0, 1.0), grey(1.0, 1.0)) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(grey(0.5, 1.0), grey(0.5, 1.0)) - 1.0).abs() < 1e-9);
        // sRGB 0.5 is relative luminance 0.214 (WCAG 2 formula).
        assert!((grey(0.5, 1.0).luminance() - 0.2140).abs() < 1e-4);
    }

    #[test]
    fn a_colour_over_a_background_blends_by_its_alpha() {
        assert_eq!(grey(0.0, 0.5).over(grey(1.0, 1.0)), grey(0.5, 1.0));
        assert_eq!(grey(0.2, 1.0).over(grey(0.9, 1.0)), grey(0.2, 1.0));
        assert_eq!(grey(0.2, 0.0).over(grey(0.9, 1.0)), grey(0.9, 1.0));
    }

    #[test]
    fn untinted_glass_keeps_its_own_appearance() {
        assert_eq!(choose(TextAppearance::Light, None), TextAppearance::Light);
        assert_eq!(choose(TextAppearance::Dark, None), TextAppearance::Dark);
        let clear = Some(grey(0.0, 0.0));
        assert_eq!(choose(TextAppearance::Light, clear), TextAppearance::Light);
        assert_eq!(choose(TextAppearance::Dark, clear), TextAppearance::Dark);
    }

    #[test]
    fn opaque_tints_get_the_text_that_reads_on_them() {
        // Measured on captures: black text on these read at 1.15 to 1.95.
        let black = Some(grey(0.0, 1.0));
        let blue = Some(Srgb { red: 0.0, green: 0.294, blue: 0.7, alpha: 1.0 });
        assert_eq!(choose(TextAppearance::Light, black), TextAppearance::Dark);
        assert_eq!(choose(TextAppearance::Light, blue), TextAppearance::Dark);
        // And white text on these read at 1.07 to 1.22.
        let white = Some(grey(1.0, 1.0));
        let yellow = Some(Srgb { red: 0.9, green: 1.0, blue: 0.0, alpha: 1.0 });
        assert_eq!(choose(TextAppearance::Dark, white), TextAppearance::Light);
        assert_eq!(choose(TextAppearance::Dark, yellow), TextAppearance::Light);
        // Mid grey: black text measured 5.3, white text 3.4.
        let mid = Some(grey(0.5, 1.0));
        assert_eq!(choose(TextAppearance::Light, mid), TextAppearance::Light);
        assert_eq!(choose(TextAppearance::Dark, mid), TextAppearance::Light);
    }

    #[test]
    fn partial_tints_blend_with_the_glass_appearance() {
        // Half white over dark glass measured 0.25 to 0.34 luminance, where
        // white text read at 2.4 to 3.0; black text reads better.
        assert_eq!(choose(TextAppearance::Dark, Some(grey(1.0, 0.5))), TextAppearance::Light);
        // Half black over light glass: black text still reads better.
        assert_eq!(choose(TextAppearance::Light, Some(grey(0.0, 0.5))), TextAppearance::Light);
        // A light tint on light glass, or a dark one on dark glass, changes nothing.
        assert_eq!(choose(TextAppearance::Light, Some(grey(1.0, 0.8))), TextAppearance::Light);
        assert_eq!(choose(TextAppearance::Dark, Some(grey(0.0, 0.8))), TextAppearance::Dark);
    }
}
