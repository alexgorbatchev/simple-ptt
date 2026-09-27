These behaviors were measured on macOS 26.6 with probes. Each gives the behavior, then where the code handles it. Do not undo a handling without re-measuring.

## Liquid Glass (`NSGlassEffectView`)

- **Glass ignores `alphaValue` and layer opacity** inside an `NSGlassEffectContainerView`: it cannot fade. The correction effects move or scale the glass and fade only its content (`CorrectionEffect` in `glass.rs`); the whole-window pop fades the `NSPanel` instead.
- **A new corner radius takes effect only when the glass is shown again.** That is why the tuner pops the overlay again about 300ms after a change.
- **An inactive window draws inactive, frosted glass.** `OverlayPanel` overrides the private `-_hasActiveAppearance` to return YES.
- **The container renders all its glass together.** Its layers, not each glass view's, hold the `glassBackground` filter; `apply_glass_internals` and `read_glass_internals` take the container.

## AppKit animation of glass frames

- **`NSAnimationContext`'s completion handler never fires** for a group that animates an `NSGlassEffectView`. Motion ends use a one-shot `NSTimer` (`schedule_after_motion`).
- **`animator().setFrame(_:)` on glass steps the model frame** on each display-link tick, so `frame()` returns in-between values.
  - Setting any frame mid-motion cancels the animation. This was the cause of the "abrupt" Expand collapse (274 → 180pt in one frame).
  - Compare with and start motions from `main_resting_frame`, never from `main_glass.frame()`.
- **A glass frame animation stops at its last displayed step, not its target.**
  - A pop to origin (40, 40) ended at (39.9, 40.2). Later animations then jittered ±0.2pt around it, and the transcript with them.
  - `settle_main_glass_after` sets the exact resting frame when a motion ends.
  - Expand animates only `setFrameSize:`, because its origin does not change.
- **The same cut-short bug affects the correction glass.** `update_correction`'s `Place` branch compares `correction_glass.frame()` with the target every tick. In Emerge its width jumped from 317 to 560pt mid-show. This is not yet fixed.

## Scroll views and text

- **The scroll view pulls its content in near a container's rounded corner.**
  - On macOS 26 an `NSScrollView` insets its clip view (`contentInsets`) while a rounded ancestor corner is near its edge (`_NSScrollViewLayoutHelper updateLayoutWithMinimumDocumentFrameSize:` → `_applyContentAreaLayout:`). The text view shrinks and scrolls by the inset. The Expand collapse moved the transcript 6pt for about 50ms.
  - `automaticallyAdjustsContentInsets` does **not** control this.
  - The switch is the private `-[NSScrollView _setAllowsAdditionalContentInsetsForCornerRadii:]`. `-[NSTextView viewDidMoveToSuperview]` sets it back to YES, so set it after `setDocumentView:` (`set_text_document` in `overlay/mod.rs`).
  - The public replacement, `NSView.cornerConfiguration`, is macOS 27.
- **A vertically resizable `NSTextView` shrinks between updates.** Text views use `setVerticallyResizable(false)`, and `resize_text_view` sets their height and scrolls back to the top when the text fits.
- **The Expand correction area rides the glass's top edge.**
  - Place it for the glass's *current* height (`expansion_frame(resting, margin, glass_height)`); a frame computed for the grown glass sits up to 130pt off mid-motion.
  - The correction scroll view's autoresizing mask depends on its host: `ViewMaxYMargin` in the expansion area (above the divider), `ViewMinYMargin` in the correction glass.

## Private Core Animation (halo)

- **Changes to an installed `variableBlur` filter must go through key paths**: `filters.variableBlur.inputMaskImage` / `.inputRadius`. Assigning `filters` again does not re-render. The filter's layer `scale` must be the display scale.
- **Glass internals are set through key paths too**: `filters.glassBackground.<input>`. The rim highlight is a `CASDFKeyFillHighlightEffect`; assign the layer's `effect` again to redraw it.
