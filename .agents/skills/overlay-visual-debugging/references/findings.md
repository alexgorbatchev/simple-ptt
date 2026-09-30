These behaviors were measured on macOS 26.6 with probes. Each gives the behavior, then where the code handles it. Do not undo a handling without re-measuring.

## Liquid Glass (`NSGlassEffectView`)

- **Glass ignores `alphaValue` and layer opacity** inside an `NSGlassEffectContainerView`: it cannot fade. The correction effects move or scale the glass and fade only its content (`CorrectionEffect` in `glass.rs`); the whole-window pop fades the `NSPanel` instead.
- **A new corner radius takes effect only when the glass is shown again.** That is why the tuner pops the overlay again about 300ms after a change.
- **An inactive window draws inactive, frosted glass.** `OverlayPanel` overrides the private `-_hasActiveAppearance` to return YES.
- **The container renders all its glass together.** Its layers, not each glass view's, hold the `glassBackground` filter; `apply_glass_internals` and `read_glass_internals` take the container.

- **Which tuning changes need a re-pop.** Captured live and after a re-pop, with the overlay updated every 75ms as in the tuner and the app:
  - `corner_radius` differed (the corners keep the old radius) and so did the glass `internals`; `needs_repop` in `dev/tuner.rs` re-pops for those.
  - Every other value applied live (0 pixels different), including `style` and `appearance`. The `pop_*` values show only in a pop.
- **An appearance change drops the progressive blur.** The halo shows as a hard-edged box until the next overlay update re-applies `variableBlur` (`refresh_progressive_halo`). The app updates the overlay only on state changes, or every 20 status ticks (about 1.5s), so while the overlay is idle a system light/dark switch can show the box that long. This is not fixed.

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

## Text colour on tinted glass

- **Glass does nothing for the legibility of its content.** `NSGlassEffectView.tintColor` tints the glass, but the content keeps the window's appearance. The SDK header and docs promise nothing more.
  - Measured on captures before the fix: black or blue at alpha 1 with Aqua text read 1.15–1.95:1, and white or yellow at alpha 1 with Dark Aqua text 1.07–1.22:1.
  - `update_text_appearance` (see `legibility.rs`) sets Aqua or Dark Aqua on the glass content views, whichever `labelColor` contrasts more with the tint over the glass appearance's `windowBackgroundColor`, and `nil` when that is the glass's own appearance. It runs from `apply_look` and from `viewDidChangeEffectiveAppearance` of the container's stack view (`AppearanceTrackingView`), which covers system light/dark switches.
  - After the fix, body text measured at least 4.29:1 across 32 tint, appearance, and backdrop cases.
- **On a mid-luminance background no label colour reaches 4.5:1.** Grey at alpha 1, or black at alpha 0.5 over light glass, measured 4.29–4.57. `labelColor` is 85% black or 85% white. The footer uses `secondaryLabelColor`, which measured 2.50–6.23 (lowest on mid-grey).
- **Clear glass does not adapt its content to what is behind it; Regular does.**
  - In dark mode over a light backdrop, Regular glass rendered dark (0.092 luminance) and its white text read at 5.86:1.
  - Clear glass stayed see-through (0.924), and its white text read at 1.06:1, the same with or without `update_text_appearance`.
  - Apple's `Glass.clear` docs: "ensure content remains legible by adding a dimming layer or other treatment beneath the glass", such as transparent black.
- **The dimming layer (`GlassTuning::dim`) keeps Clear glass legible.** The defaults now use no dim (0) and a black tint at 69% instead, which darkens the glass itself; the figures below were measured for the dim, and the tinted default has not been measured.
  - It is an `NSBox` under each glass, filled with `windowBackgroundColor` in the text's appearance. `sync_dims` keeps it on its glass; a probe found frame and visibility equal to the glass in all 863 samples through pops, Emerge, and Expand.
  - Measured with the paired method over busy backdrop text, the hardest case is dark mode (white text) over a light backdrop: 1.89:1 with no dim, 3.67 at 0.5, 4.32 at 0.6, and 4.71 at 0.65. Every other case was already at 4.26 or more with no dim.
  - The tint and footer figures above came from the old full-screen backdrop, whose text did not reach under the glass, so they measure a flat backdrop only.
- **AppKit's resolved values on macOS 26.6:** `windowBackgroundColor` is 1.0 (Aqua) and 0.118 (Dark Aqua); `labelColor` is black or white at alpha 0.847.
- **Only text and the pills use semantic colours.** The pills are `labelColor` resolved in their view's appearance (`cg_color_in` in `ui_meter.rs`); the other meter styles' bars and the shimmer mask are fixed sRGB and do not adapt.

## Window and halo

- **The glass content must sit on whole points.** The glass sits `margin()` in from the window edge. With a fractional margin (90.71), AppKit snapped the text view to the pixel grid but not the meter's layers, which ended up 0.21 pt apart. `margin()` rounds to whole points, and every margin use goes through it.
- **The halo fades out at the edge of its own views** (blur, dim, fallback glass), not at the window's edge. `layout_halo` sizes them.
  - With Expand they end a margin above the main glass's current top, so the blur grows with the glass. Before, it took the grown size the moment the correction showed.
  - `layout_halo` runs on every `NSViewFrameDidChangeNotification` of the main glass. The notification fires on each animation step, in the same display tick.
- **Window frames are whole points; views are not.** Moving the window's top edge with the glass put the halo 0.5pt off the glass on about 1 step in 10. That is why the window takes its grown height at once (it is transparent there) and only the halo views follow.
- **Drawing the progressive-blur mask ring by ring at full size takes 14–22ms** (release, 1280×520–724px), longer than a display frame. `blur_mask_image` stretches a small `blur_mask_cap` in about 1.7ms, and a test proves the result is byte-identical. The first image draw in a process costs about 100ms of one-time warm-up.
- **`NSImageResizingModeStretch` is 0 on macOS** (1 is the iOS value; see `NSImage.h`). objc2-app-kit declares the type without its constants.

## The pill strip (Core Animation)

- **A timing function wrecks a long linear animation, even `kCAMediaTimingFunctionLinear`.** The pill strip (`ui_meter/pill_strip.rs`) moves by one `transform.translation.x` animation lasting 24 hours, so a few seconds in its progress is about 1e-5.
  - With the linear `CAMediaTimingFunction`, the presentation layer's translation crawled at 0.4 pt/s, then jumped 42 and 21 pt, then ran 13% slow. It sat 13–28 pt off the expected line in the first 5 seconds.
  - A timing function is a cubic Bézier that Core Animation solves numerically; near 0 the linear one is x ≈ 3t², so the solver's tolerance shows as tens of points over a 2.9 million-point travel.
  - With no timing function (`nil` is linear pacing), the translation stayed within one display frame of the expected line (0–0.56 pt at 33.75 pt/s) in all 875 samples. Leave `timingFunction` unset on long animations.
- **The presentation layer advances once per display frame,** so a sample of it lags the ideal value by up to one frame of motion. Judge smoothness by the step between frames, not by an exact match with `CACurrentMediaTime`.
- **A layer's new model position shows only after the transaction commits.** A recycled pill sampled a few milliseconds after its model moved was still drawn at its old position (off the column on the left, so invisible). Compare model and presentation only after the run loop has turned.
- Measured with a probe sampling the strip's presentation layer every 4 ms: every pill layer only slid left with the strip or was recycled from beyond the left edge to the right edge (17 recycles in 3.5 s), and painted pills kept their height in all 66,424 checks.

## Telling speech from other sound (the pills)

- **WebRTC VAD does not reject typing or loud noise.** Run as the app would (libfvad through the `webrtc-vad` crate, 16 kHz, 20 ms frames, `VeryAggressive`) on 16 kHz test sounds, it judged speech from `say` 98% voiced, but also loud white noise 100%, and keyboard-like clicks (8 ms bursts every 150 ms over quiet hiss) 60%. Only quiet hiss (2%) and a 120 Hz hum (2%) were rejected, which the adaptive range's noise floor already handles.
- **SoundAnalysis's built-in classifier does.** `SNClassifySoundRequest` with `SNClassifierIdentifierVersion1`, 0.5 s windows at 50% overlap, "speech" confidence at or above 0.5: speech from `say` (and the same 18 dB softer) in 30 of 30 windows, the loud noise, quiet hiss, clicks and hum in 0 of 15 each, through the app's own `audio/speech.rs` pipeline as well as `SNAudioFileAnalyzer`. It cost about 0.7% of one core. Its shortest window is 0.5 s (`windowDurationConstraint`: 0.5 to 15 s).
- **The classification delay stays inside the right-hand fade.** A pill waits as a dot until a result covers its 0.2 s, at most about 0.3 s after it closes. A probe of the tuner (which publishes results for its synthetic voice) found all 27 height changes in 6.5 s inside the 25 pt fade, and none in the shown column.

## Private Core Animation (halo)

- **Changes to an installed `variableBlur` filter must go through key paths**: `filters.variableBlur.inputMaskImage` / `.inputRadius`. Assigning `filters` again does not re-render. The filter's layer `scale` must be the display scale.
- **Glass internals are set through key paths too**: `filters.glassBackground.<input>`. The rim highlight is a `CASDFKeyFillHighlightEffect`; assign the layer's `effect` again to redraw it.
