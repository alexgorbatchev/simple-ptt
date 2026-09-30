---
name: overlay-visual-debugging
description: ALWAYS USE when debugging, tuning, or fixing how the simple-ptt overlay looks or moves — Liquid Glass (`NSGlassEffectView`), halo blur, pop and correction animations (Emerge, Unfold, Pop, Slide, Expand), text that shifts, jumps, jitters, reflows, or stutters, or `GlassTuning` values — in `src/overlay/` (`glass.rs`, `mod.rs`, `private_effects.rs`, `text_effects.rs`, `dev/`) or the meter (`src/ui_meter.rs`, `src/ui_meter/`, including the scrolling pill strip), or when using `simple-ptt --debug` or `--overlay-snapshot`. macOS 26 glass and AppKit animation behave differently from what training data says; MUST READ before changing overlay code. Do NOT use for the settings window (`src/settings_window/`).
author: alexgorbatchev
metadata:
  created_on: 2026-09-27 11:30
  last_modified: 2026-09-30 11:51
  status: current
---

## Tools

- `simple-ptt --debug` (`just debug-overlay`, `src/overlay/dev/tuner.rs`): the overlay pinned near the top of the screen, plus a window with a control for every `GlassTuning` value, a meter style picker (the `meter_style` values Settings offers) fed by a synthetic voice that alternates about 8 seconds soft and 8 seconds 15 dB louder (`voice_level_db`), so the pills' adaptive range and their stable history can be watched, a correction toggle, a "Loop narration" checkbox (narrates the transcript a word at a time, or the correction while it is shown), "Pop again", and "Copy values", which copies the values for the user to paste. Controls that have no effect under the current values are disabled.
- `simple-ptt --overlay-snapshot <dir>` (`src/overlay/dev/snapshot.rs`): captures every overlay state over light and dark backdrops as `<dir>/<theme>-<step>.png`, then exits. Use `.tmp/snapshots/` as `<dir>`. It needs Screen Recording permission for the terminal. Captures that show only the desktop wallpaper or solid black mean the display is locked or asleep, or that permission changed; stop and tell the user rather than measuring them.
- `assets/view_probe.rs`: a module for scripted, self-exiting runs that samples view geometry and prints who changes a frame. Read [references/probes.md](references/probes.md) before using it.
- `scripts/contrast.ts`: WCAG contrast of the text against the glass behind it, measured on captures. It is for text colour and legibility bugs; see "Measuring legibility on captures" in [references/probes.md](references/probes.md).
- lldb and the SDK headers, for private AppKit behavior. The recipes are in [references/probes.md](references/probes.md).

The tuner-only `OverlayWindow` methods (`pin_to_top`, `glass_tuning`, `set_glass_tuning`, `halo_is_progressive`, `glass_internals_now`) exist for `src/overlay/dev/`; do not call them from product code. Values the user settles on in the tuner become the `GlassTuning` defaults in `src/overlay/glass.rs`, with the tests there pinning them.

## Rules

- Every `simple-ptt` command you run must contain `--overlay-snapshot` or `VIEW_PROBE=1` (with a `run_script` end time). Any other launch (plain, or `--debug` without a probe) blocks the session and is prohibited. For interactive checks, ask the user to run `just debug-overlay`, and to paste the "Copy values" output or describe what they see.
- Measure before you change code. A claim such as "the text moves" or "the animation is cut short" needs samples: `t=` timestamps with screen rects, frames, or presentation heights, printed by a probe run. Do not diagnose from reasoning, code reading, or screenshots alone.
- Treat every nonzero deviation as a bug: a 0.2pt drift or a single-frame blip is 1px on a Retina display. Prohibited dismissals: "invisible", "one frame only", "rounding", "AppKit quirk, can't fix".
- Before editing any overlay code, read [references/findings.md](references/findings.md). It lists measured macOS 26.6 behaviors that contradict the obvious fix. Re-verify a finding with a probe before relying on it after a macOS update.
- Fix the cause the probe shows. Prohibited:
  - delays or extra timers that race AppKit;
  - re-setting a frame to hide a jump;
  - reading a glass view's `frame()` as its target;
  - "close enough" tolerances in comparisons that hide a sub-point drift.
- Every private API goes in `src/overlay/private_effects.rs`, looked up at run time (`respondsToSelector` or `AnyClass::get`). Its doc comment names the measured behavior and the macOS version. Tell the user about each new private API in your final message. Before using one, check the SDK headers for a public replacement.
- Keep probes out of commits. Remove `view_probe.rs`, its `mod` line, and the `VIEW_PROBE` block before committing, and confirm with `rg -n "view_probe|VIEW_PROBE" src/` returning nothing.

## Workflow

1. **Reproduce.** Get the tuner values from the user ("Copy values") and the exact steps, such as the effect, the scenario, and show or hide. Put the values into the probe run as a `GlassTuning`.
2. **Write the invariant as numbers,** for example "the transcript's screen y stays at 798.0 through expand and collapse", or "the glass height falls monotonically from 282 to 180 over `correction_seconds`".
3. **Sample.** Install `view_probe`, script the timeline (show at 2.0s, hide at 3.5s, exit at 4.6s), and sample every 0.012s: `screen_rect` of the views that must stay still, `presentation_height` of the glass that moves, and `ancestry` of the view that misbehaves. Collapse repeated lines with `awk` so only changes print. Run 2–3 times; one-frame blips vary between runs.
4. **Find the writer.** Call `watch_frame` on the view that changes, with a predicate that matches only the bad frame. Match the printed stack against the table in [references/probes.md](references/probes.md). When the stack ends in private AppKit, disassemble it (lldb recipes) to see which selectors it reads, then look for a switch or a public API.
5. **Fix and prove it.** Keep the "before" samples. Re-run after the fix and show that the invariant holds in every sample of every run. Then disable the fix (for example, comment out the call), re-run, and show that the failure returns; if it does not, the fix is not the cause. Pure geometry goes in a `glass.rs` function with a unit test written first, and that test must fail first.
6. **Clean up and verify.** Remove the probe wiring, then run `cargo test --locked`, `cargo check --message-format=short`, and `cargo build --locked --release`, each with zero warnings. For changes the user can see, take `--overlay-snapshot` captures and look at them.

## Report

End with these five items. If any one is missing, the fix is unverified; say so instead of claiming it works.

1. The invariant from step 2.
2. A "before" sample excerpt that breaks it.
3. An "after" excerpt from each run, showing that it holds.
4. The excerpt from the run with the fix disabled, showing the failure again.
5. The findings.md entries that applied, and the new private APIs (or "none"). Add every newly measured behavior to findings.md in the same change.
