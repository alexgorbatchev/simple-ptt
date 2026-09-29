## Contents

- Installing `view_probe`
- A scripted run
- Reading `watch_frame` stacks
- Finding the overlay's views
- Measuring legibility on captures
- lldb and SDK recipes

## Installing `view_probe`

1. `cp .agents/skills/overlay-visual-debugging/assets/view_probe.rs src/overlay/dev/view_probe.rs`
2. Append `pub mod view_probe;` to `src/overlay/dev/mod.rs`.
3. Add a `VIEW_PROBE` block to `src/overlay/dev/tuner.rs` in `run()`, just before `let _keep = (tuner, window, timer);`, as shown below.
4. Run `cargo build --message-format=short`, then `VIEW_PROBE=1 ./target/debug/simple-ptt --debug > .tmp/probe.txt 2>&1`. The run exits by itself at the script's end time.

The module provides:

| Function | Returns / does |
| --- | --- |
| `overlay_panel()` | The `OverlayPanel` window. |
| `find_views(root, class)` | Views of that exact class, depth first. |
| `screen_rect(view)` | Where the view is on screen. Use this, not `frame()`, to judge "did it move". |
| `ancestry(view)` | Frame and bounds of the view and every ancestor. It shows which level moved: a clip view's bounds origin means scrolling or insets; a glass origin means glass motion. |
| `presentation_height(view)` | The layer height as drawn now. |
| `watch_frame(view, label, predicate)` | Prints the call stack of each frame change the predicate accepts. |
| `run_script(events, every, end, sample)` | Runs the timed events, samples every `every` seconds, and exits at `end`. |
| `elapsed()` | Seconds since the script started. |

## A scripted run

This is a tested example (macOS 26.6): Expand, then show and hide the correction, sample the transcript, and report any transcript shrink.

```rust
if std::env::var("VIEW_PROBE").is_ok() {
    use super::view_probe::{ancestry, find_views, overlay_panel, presentation_height, run_script, screen_rect, watch_frame};
    let mut tuning = state.overlay.glass_tuning();
    tuning.correction_effect = CorrectionEffect::Expand;
    state.show_tuning(&tuning);
    state.overlay.set_glass_tuning(tuning);
    let (show, hide) = (state.clone(), state.clone());
    let events: Vec<(f64, Box<dyn Fn()>)> = vec![
        (1.9, Box::new(|| {
            let Some(root) = overlay_panel().and_then(|panel| panel.contentView()) else { return };
            if let Some(text) = find_views(&root, "NSTextView").first() {
                watch_frame(text, "transcript", |frame| frame.size.height < 120.0);
            }
        })),
        (2.0, Box::new(move || show.correction_shown.set(true))),
        (3.5, Box::new(move || hide.correction_shown.set(false))),
    ];
    run_script(events, 0.012, 4.6, |t| {
        let Some(root) = overlay_panel().and_then(|panel| panel.contentView()) else { return };
        let glass_height = find_views(&root, "NSGlassEffectView").first().and_then(|glass| presentation_height(glass));
        let texts = find_views(&root, "NSTextView");
        let Some(transcript) = texts.first() else { return };
        eprintln!("t={t:.3} glass_h={glass_height:?} transcript={:?}", screen_rect(transcript));
        eprintln!("    {}", ancestry(transcript));
    });
}
```

- The tuner redraws the overlay every 0.075s from `TunerState::tick`. `correction_shown` and `show_tuning` are `TunerState` members, reachable because the block sits in `tuner.rs`.
- Before 2.0s the overlay is still popping in. Keep events at or after about 1.9s, or the samples mix in the pop.
- Print only lines that differ from the previous one, for example `awk '{k=$0; sub(/^t=[0-9.]+ /,"",k); if (k!=prev) print; prev=k}'`.

## Reading `watch_frame` stacks

| Frames under `_postFrameChangeNotification` | Meaning |
| --- | --- |
| `setFrameSize:`/`setFrameOrigin:` ← `setValue:forKeyPath:` ← `CABasicAnimation applyForTime:` ← `NSAnimationManager` | An `animator()` animation is stepping the model frame. |
| `… NSAnimationManagerStopAnimation` | That animation's end. Compare its last value with the target: glass stops short. |
| `setContentInsets:` ← `-[NSScrollView _applyContentAreaLayout:]` ← `tile` ← `layout` | The scroll view changed its clip insets (the corner insets in findings.md). |
| `setPostsFrameChangedNotifications:` ← KVO `didChangeValueForKey:` | The frame changed indirectly; read further down the stack for the source. |
| `simple_ptt::…` frames | Our code; the symbol names the function. |

## Finding the overlay's views

- The main glass is the first `NSGlassEffectView`. The halo and correction glass are `SimplePttPassiveGlassEffectView`, so they do not match that class name.
- `NSTextView`, depth first: the transcript first, then the correction text.
- The transcript's ancestry is: text view < clip view < scroll view < `main_content_view` < the glass's content wrapper < a private `ContentHolderView` < main glass < stack view < `NSGlassEffectContainerView` < root view.

## Measuring legibility on captures

For text colour or contrast bugs, measure pixels rather than reasoning about colours. The snapshot backdrop is centred on the overlay, with rows of text running under the glass, so captures include busy content behind the text.

Temporarily add a matrix loop at the top of the `for dark in [false, true]` loop in `src/overlay/dev/snapshot.rs`, after `pump(0.3);`. It sets tunings, redraws like the tuner, captures each case with and without the transcript (the `-notext` pair the paired method needs), then continues. This is the tested dim matrix; vary other `GlassTuning` fields the same way:

```rust
if std::env::var("DIM_MATRIX").is_ok() {
    use crate::overlay::glass::OverlayAppearance;
    let final_text = text(&full, None);
    for dim in [0.0, 0.35, 0.65] {
        for (appearance_name, appearance) in [("aqua", OverlayAppearance::Light), ("darkaqua", OverlayAppearance::Dark)] {
            let mut tuning = overlay.glass_tuning();
            tuning.dim = dim;
            tuning.appearance = appearance;
            overlay.set_glass_tuning(tuning);
            for (suffix, transcript) in [("", &final_text), ("-notext", &empty)] {
                for _ in 0..8 {
                    update(STATE_RECORDING, transcript, "", &empty, false, meter);
                    pump(0.075);
                }
                capture(mtm, dir, &format!("dim-{dim:.2}-{appearance_name}-{theme}{suffix}"));
            }
        }
    }
    backdrop.orderOut(None);
    pump(0.3);
    continue;
}
```

Run it with `cargo build --release && DIM_MATRIX=1 ./target/release/simple-ptt --overlay-snapshot .tmp/dim`, then `bun .agents/skills/overlay-visual-debugging/scripts/contrast.ts .tmp/dim dim-`. Pass the capture's `halo_margin` as the third argument when it is not the default.
- The body result compares each transcript glyph pixel with the same pixel in the `-notext` capture, which is what is behind that glyph. Without a pair the script falls back to the region median, marked `~`; that method is unreliable with backdrop text behind the glass, and the footer (which the pairs keep) is always measured that way.
- The regions in `scripts/contrast.ts` fit the recording state on a 2x display. Before trusting any numbers, draw them on one capture (the body region starts 40,35 pixels from the glass corner at `(40 + halo_margin) * 2`) and look at it.
- Remove the matrix block before committing (`rg -n _MATRIX src/` must print nothing).

## lldb and SDK recipes

Build first. Every command works on `./target/debug/simple-ptt` without a running process, except the breakpoint run.

```sh
# Which classes implement a selector (regex over AppKit symbols):
lldb --batch -o "image lookup -r -s 'contentViewInsets|_allowsAdditional'" ./target/debug/simple-ptt 2>&1 | rg -o '[-+]\[[^]]+\]' | sort -u
# What a private method reads and calls (selectors it sends):
lldb --batch -o "dis -n '-[NSScrollView _applyContentAreaLayout:]'" ./target/debug/simple-ptt 2>&1 | rg -o 'objc_msgSend\$[A-Za-z_:]+' | awk '!seen[$0]++'
# Who calls a setter, with its receiver and first argument (x2), in a self-exiting probe run:
VIEW_PROBE=1 lldb --batch -o "breakpoint set -n '-[NSScrollView _setAllowsAdditionalContentInsetsForCornerRadii:]' -C 'register read x0 x2' -C 'bt 6' -G true" -o "run --debug" ./target/debug/simple-ptt > .tmp/lldb.txt 2>&1
# Public API check (macOS 27 SDK headers name APIs not on 26):
rg -n -i "<term>" "$(xcrun --sdk macosx --show-sdk-path)/System/Library/Frameworks/AppKit.framework/Headers/"
```

- A 4-instruction getter or setter (`ldrb`/`strb` at an ivar offset) is a plain flag. Look for the code that writes it back, as `viewDidMoveToSuperview` does in findings.md.
- Symbolic breakpoints on `setFrame:` miss frame changes made through `setFrameOrigin:`/`setFrameSize:` or KVC. Use `watch_frame` instead.
- Apple's documentation pages render empty through WebFetch. Fetch the JSON instead: `https://developer.apple.com/tutorials/data/documentation/appkit/<class>/<member>.json` (lowercase path).
