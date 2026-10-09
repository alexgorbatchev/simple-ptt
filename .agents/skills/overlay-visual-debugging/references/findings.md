---
created_on: 2026-09-27 11:29
last_modified: 2026-10-09 13:23
status: current
---

These behaviors were measured on macOS 26.6 with probes, with later versions noted below. Each gives the behavior, then where the code handles it. Do not undo a handling without re-measuring.

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

- **Keyboard edits must keep their rendered baseline separate from the speech source** (macOS 26.6.2).
  - A native `insertText:replacementRange:` edit changed `hello` to `Hi` while `still coming in` remained interim. The following speech update displayed `Hi still coming in still coming in and working` at `t=2.113`; all 59 checked samples contained duplication.
  - The editor now publishes its edit relative to its last rendered text, and the session merges revisions against its preceding speech source under the text lock. Two native runs using the `similar` implementation each checked 59 samples with zero failures: `Hi still coming in and working` appeared at `t=2.113` and survived finalization. The transcript rectangle stayed unchanged across the keyboard and speech updates in each run.
  - Restoring the old prefix rebase reproduced the duplicated text at `t=2.113` and 59 failures out of 59 checks. The three session regressions failed again. Disabling the stale-editor merge separately lost a word received between UI ticks and failed its state regression.
  - The original duplication-fix probe also exposed unconditional caret movement from `{ location: 2, length: 0 }` after typing to `{ location: 31, length: 0 }` after the next result. The conditional following rule below now handles that behavior. No private API was added.

- **Narration follows only an idle caret already at the end** (macOS 26.6.2).
  - Before the change, a caret at offset 2 jumped to 15 on incoming narration at `t=2.303`. After a native `insertText:replacementRange:` at `t=3.001`, another result moved offset 20 to 26 at `t=3.204`. The probe failed 165 of 181 checked samples, including a selected range and a second keystroke restarting the pause.
  - `replace_working_text` captures the selection before replacing storage. It follows the new end only for a zero-length selection at the previous end and at least two seconds since the last `textDidChange:`. Otherwise it restores the existing selection, clamped to valid UTF-16 boundaries when a revision shortens or changes the text. The same rule applies to inline correction previews; scrolling to the new end happens only when following.
  - Two native runs each checked 210 samples with zero failures. In both, selection stayed at offset 2 through the `t=2.101` narration update, at offset 20 through the `t=3.204` update after typing, and at offset 27 through the `t=5.004` update after another keystroke. Moving the caret to the end without typing allowed the `t=6.109` update to follow to offset 37, over two seconds after the last keystroke. Moving it back into the text kept offset 2 through the next update. The transcript screen rectangle stayed `(1000.0, 674.8, 560.0, 120.19999999999999)` across these events in each run.
  - Disabling the two-second guard moved offset 20 to 26 again at `t=3.205`, failed 92 of 210 native checks, and failed the timing regression. Disabling the end-position guard separately failed the middle-caret and shortened-selection regressions. No private API was added.

- **Waiting feedback must update before the finishing layout hold** (macOS 26.6.2).
  - Updating the footer after the finishing early return left the shortcut hint displayed at `t=5.605` with `waiting=true, finishing=true`. The footer now changes before that return, while the hold still retains text, meter, and layout.
  - Two runs sampled every 12 ms and checked 542 samples each after selecting seven characters at `t=1.9`: transcript screen rect stayed `(584.0, 732.8, 560.0, 120.19999999999999)`, selection stayed `{ location: 0, length: 7 }`, glass height stayed `180.0`, and finishing scale stayed `0.8` after the transition. Both live and finishing notices appeared and cleared within the next UI update. Restoring the earlier return reproduced the missing finishing notice at `t=5.605` with the same geometry and selection.
  - Light and dark captures show the waiting footer at full recording size and uniformly scaled with finishing narration. The footer keeps `secondaryLabelColor`; no private API is added.
- **An unchanged overlay refresh must preserve text selection and scrolling** (macOS 26.6.2).
  - With seven error-text characters selected, the old `OverlayWindow::update` moved the selection from `{ location: 0, length: 7 }` to `{ location: 37, length: 0 }`. A subsequent native Command-C event left the pasteboard unchanged; the same event copied successfully before the refresh.
  - Changed working text now preserves the selection or follows the end according to the idle-caret rule above. `update` leaves selection and scrolling alone when the text is unchanged. In two probe runs, selection remained `{ location: 0, length: 7 }` through the refresh and Command-C wrote the selected text. Restoring the unconditional caret move reproduced the failure.
- **Transformation feedback needs readable text through both processing states** (macOS 26.6.2).
  - The old record/transform handoff set text opacity to `0.02`; after twelve UI updates the text view's measured alpha was `0.0196078431372549`. The shimmer ran only in `STATE_TRANSFORMING`, leaving the final transcription wait without it.
  - Both transform handoffs in `hotkey.rs` retain full text opacity. `OverlayWindow::update` runs the existing shimmer in `STATE_PROCESSING` and `STATE_TRANSFORMING`. Two probe runs measured text alpha `1.0` in both states and advancing presentation-layer gradient locations; the mask was absent in recording and error states. Disabling the changes reproduced the low alpha and missing processing shimmer. Reduce Motion still suppresses the shimmer through `Shimmer::start`.
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

- **A key nonactivating overlay can report AppKit activation without process activation** (macOS 26.6.2).
  - While recording, the native probe measured `NSApplication::isActive=true` and `NSRunningApplication::currentApplication().isActive=false` with the overlay as key window. Opening Settings from that state produced `active=false, frontmost=false, settings=true, hotkeys_blocked=true, key=None` at `t=2.110`: the Settings window was inaccessible and recording hotkeys were blocked.
  - The default-mode turn requires both activation flags. `NSRunningApplication::activateWithOptions(empty())` rejects the request even when Settings is ordered front. `NSApplication::activate()` also leaves the process inactive. Keeping Settings hidden while awaiting activation leaves Cmd+, at `pending=AwaitingActivation, settings=false, overlay=false`; waiting for the overlay fade to end does not resolve it.
  - The delegate orders Settings front without making it key, then calls the public `NSApplication::activateIgnoringOtherApps(true)` for the explicit Settings request. `applicationDidBecomeActive:` makes Settings key and enables its keyboard handling. With no overlay, the explicit activation call alone also leaves Settings hidden; ordering the window first supplies the activation target. If activation is declined, the window stays visible with recording hotkeys enabled until activation or closure.
  - Native probes sampled every 12ms, forced the overlay to be key immediately before Cmd+, and required actual process activation and a key Settings window after the overlay hid. Replacing the activation call with `NSRunningApplication::activateWithOptions(empty())` left `active=false, frontmost=false, settings=true, pending=AwaitingActivation, overlay=false, key=None` at `t=1.827` and failed. A separate no-overlay probe failed with the window-ordering step disabled and passed with it restored.
  - Two restored runs reached `active=true, frontmost=true, settings=true, pending=Idle, overlay=false, key=Some("Settings")` at `t=1.824` in each run. Both exited successfully with Settings visible and the overlay hidden. Moving focus to another application afterward is allowed; the invariant requires that Settings actually acquired foreground key focus after the overlay hid.
  - The successful-activation path needs a native probe: pure state-transition tests cannot validate macOS granting foreground focus. `activateIgnoringOtherApps` is deprecated, but its documented explicit foreground behavior is required by these measurements; it is a direct call for this user action, not a fallback. No private API is added.

- **F5 fades the meter without releasing its layout space** (macOS 26.6.2).
  - The finishing early return previously retained meter opacity at `1.0` throughout processing and transformation. `fade_finishing_meter` now animates the meter container's public `alphaValue` to zero over 0.16 s, alongside the finishing shrink. Its target is set once so status ticks do not restart the fade; the fast-result dismissal path uses it too.
  - Two native runs sampled every 12 ms checked 125 finishing samples each with zero failures: meter opacity decreased in 9 and 10 sampled steps, reached exactly `0.0`, and stayed there through transformation. The transcript's screen rectangle stayed `(1000.0, 1058.8, 560.0, 120.19999999999999)`; root scale held exactly `0.8` after the transition. The next recording restored meter opacity to `1.0`.
  - Disabling the fade restored opacity `1.0`, with 100 failing samples out of 125. No private API is added.

- **An empty F5 handoff dismisses instead of holding a placeholder** (macOS 26.6.2).
  - Before the change, empty narration held at scale `0.8`, panel alpha `1.0`, with shimmer throughout processing and transformation. The finishing handoff now marks only nonempty narration for that hold; empty or whitespace-only narration sets the existing dismissal flag before processing starts.
  - Two native runs sampled every 12ms, each checking 204 samples with zero failures: empty F5 faded immediately through the normal 0.24s dismissal, remained hidden through late final text, and nonempty F5 still held at `0.8` with shimmer. Disabling the handoff change restored 154 failures in 203 samples and the failing hotkey regression test.
  - The recording hotkey restores visibility when starting the next recording. The queued transcription start does not restore it, so a delayed start cannot undo a subsequent empty F5 dismissal. Errors still restore the overlay through `report_error`. No private API was added.
- **Uniform overlay scaling belongs on the root rendering layer** (macOS 26.6.2).
  - Animating the main glass's frame changed its width during dismissal (560 → 559.5pt in the first step) while the text view stayed 560pt wide. It resized the glass rather than scaling the whole overlay.
  - `OverlayMotionView` uses public Core Animation transforms from its AppKit `updateLayer` override. The window, glass, root bounds, and text layout stay fixed. Two repeated runs each checked 125 samples with no invariant failures: F5 held root scale `0.8` through processing and transformation, and F6 held `1.0`; both shimmered. Captures show the text, footer, glass, and halo scaling together.
  - Entrance keyframes go through `1.1` and settle at `1.0`. Sampled entrance peaks were `1.1` and `1.0998241941560991`; the display sampling can miss the exact peak between keyframes. Reduce Motion uses an unscaled fade.
  - `NSView::convertRect_toView` continues to report the unscaled layout rectangle through the root layer's transform. For composited motion, measure the root presentation transform as well as layout bounds and inspect captures.
  - Exercise the real capture flag when probing F5. Removing the meter on the recording-to-processing transition changed the text view's height from `120.2` to `151.0` and its screen y from `511.8` to `481.0`, even with the correct root scale. F5's finishing update retains the displayed text and layout instead of rebuilding them; the meter now fades within its retained space as measured above. With the real capture transition, two runs again checked 125 samples each with zero failures; F6 continues its live meter updates.
- **Dismissal must retain its displayed text and report actual window visibility** (macOS 26.6.2).
  - The old `hide()` cleared 84 characters to zero while the panel was still visible and fading. Keeping the view's content until its next visible update retains narration throughout the fade; transformation's `PreserveOverlay` mode collects F5's result without replacing that narration. F6 keeps `ReplaceOverlay`.
  - Marking the overlay hidden at the start of its fade gave `visible=true, reported_visible=false` at `t=2.017`, allowing the paste handoff to proceed early. `pop_out` now calls the owner's visibility callback after `orderOut`; repeated probes keep both visibility values equal through dismissal.
  - A result can finish between UI ticks. The finishing marker survives `STATE_IDLE` until the actual hidden callback clears it, and `hide` applies the finishing scale even when no processing tick was rendered. The fast-result probe reached scale `0.8` during its fade; disabling marker retention restores the failing lifecycle test and an 8% dismissal shrink.
  - Disabling the rendering transforms, narration retention, and finishing marker produced 91 failures in 125 samples and restored the two failing finishing regression tests. No private API was added.
- **The glass content must sit on whole points.** The glass sits `margin()` in from the window edge. With a fractional margin (90.71), AppKit snapped the text view to the pixel grid but not the meter's layers, which ended up 0.21 pt apart. `margin()` rounds to whole points, and every margin use goes through it.
- **The halo fades out at the edge of its own views** (blur, dim, fallback glass), not at the window's edge. `layout_halo` sizes them.
  - With Expand they end a margin above the main glass's current top, so the blur grows with the glass. Before, it took the grown size the moment the correction showed.
  - `layout_halo` runs on every `NSViewFrameDidChangeNotification` of the main glass. The notification fires on each animation step, in the same display tick.
- **Window frames are whole points; views are not.** Moving the window's top edge with the glass put the halo 0.5pt off the glass on about 1 step in 10. That is why the window takes its grown height at once (it is transparent there) and only the halo views follow.
- **Drawing the progressive-blur mask ring by ring at full size takes 14–22ms** (release, 1280×520–724px), longer than a display frame. `blur_mask_image` stretches a small `blur_mask_cap` in about 1.7ms, and a test proves the result is byte-identical. The first image draw in a process costs about 100ms of one-time warm-up.
- **`NSImageResizingModeStretch` is 0 on macOS** (1 is the iOS value; see `NSImage.h`). objc2-app-kit declares the type without its constants.

## The pills (Core Animation)

- **Narrower pills and tighter gaps retain the centred animated row** (macOS 26.6.2).
  - `PillCluster` uses 2.8 pt widths and 1.5 pt gaps, keeping its 1.4 pt corner radius. The 40-pill span falls from 211 pt to 170.5 pt.
  - Two native 12ms runs checked 247 and 283 samples with zero failures: model and presentation widths stayed at 2.8 pt, the row's centre offset stayed exactly zero, and x positions stayed fixed while presentation heights changed in 187 and 193 samples. The capture shows the compact pills in the overlay.
  - Restoring the previous widths and gaps makes the native compact-row check fail again. No private API was added.
- **Frequency bands spread from the centre for a balanced speech silhouette** (macOS 26.6.2).
  - With ascending frequencies placed from left to right, the synthetic voice's 284.7 Hz peak appeared 87.8625 pt left of centre; its height-weighted centre moved as far as 68.701156 pt from the middle of the 211 pt cluster.
  - `PillCluster::pill_x` now gives the lowest pair the two centre positions, and successive pairs alternate towards the edges. The peak above appeared 7.9875 pt right of centre. Two 12ms probe runs checked 342 and 448 samples without failure: every sampled peak was in the middle half, x positions stayed identical through the runs, and the height-weighted centre stayed within 3.421995 and 3.167551 pt respectively. This is a fixed visual arrangement of the bands; their input data remains in ascending frequency order.
  - Disabling that column arrangement restored the left-hand peak, all 448 native checks failed, and the new frequency-placement unit test failed while both spacing tests still passed. No private API was added. The screenshot contained only wallpaper, so capture attempts stopped under the skill's rule; this change's screen appearance is not visually verified.
- **Implicit animations within each update glide the spectrum pills between updates.** `PillCluster::render` (`ui_meter/pill_cluster.rs`) sets every pill's frame and opacity in a `CATransaction` whose duration is the time since the last update (the overlay updates every 75 ms) with linear timing. Sampling the presentation layers every 20 ms over the tuner's synthetic voice, each band took 30 to 72 distinct heights in 2.4 s of speech (32 updates), and no pill showed in 50 samples of its silence. The requirements behind the pills are in `docs/internal/references/pill-meter.md`.

The scrolling pill strip that came before the spectrum measured these; no code relies on them now, but they hold for any long Core Animation motion:

- **A timing function wrecks a long linear animation, even `kCAMediaTimingFunctionLinear`.** A `transform.translation.x` animation lasting 24 hours is at about 1e-5 of its progress a few seconds in.
  - With the linear `CAMediaTimingFunction`, the presentation layer's translation crawled at 0.4 pt/s, then jumped 42 and 21 pt, then ran 13% slow. It sat 13–28 pt off the expected line in the first 5 seconds.
  - A timing function is a cubic Bézier that Core Animation solves numerically; near 0 the linear one is x ≈ 3t², so the solver's tolerance shows as tens of points over a 2.9 million-point travel.
  - With no timing function (`nil` is linear pacing), the translation stayed within one display frame of the expected line (0–0.56 pt at 33.75 pt/s) in all 875 samples. Leave `timingFunction` unset on long animations.
- **Pausing a layer's timing stops and resumes its animations in place** (Apple's Technical Q&A QA1673: `speed` 0 with the paused local time in `timeOffset`, then `speed` 1 with `beginTime` taking up the pause). Over a 1.5 s pause the translation held still, and moved at 53.22 and 53.25 pt/s (53.25 expected) before and after it, with no jump.
- **The presentation layer advances once per display frame,** so a sample of it lags the ideal value by up to one frame of motion. Judge smoothness by the step between frames, not by an exact match with `CACurrentMediaTime`. Sampled every 4 ms, a per-sample speed comes out about double, since the pairs that show motion carry a whole frame's travel; measure speed over a second or more.
- **A layer's new model position shows only after the transaction commits.** A layer sampled a few milliseconds after its model moved was still drawn at its old position. Compare model and presentation only after the run loop has turned.
- **A fade does not hide a layer changing size.** When strip pills took their height while inside a 25 pt fade mask, the user saw them grow from dots.

## Private Core Animation (halo)

- **Changes to an installed `variableBlur` filter must go through key paths**: `filters.variableBlur.inputMaskImage` / `.inputRadius`. Assigning `filters` again does not re-render. The filter's layer `scale` must be the display scale.
- **Glass internals are set through key paths too**: `filters.glassBackground.<input>`. The rim highlight is a `CASDFKeyFillHighlightEffect`; assign the layer's `effect` again to redraw it.
