# simple-ptt

Rust/AppKit menu bar push-to-talk app for macOS on Apple Silicon. This is a single-crate repo; keep this file focused on repo-wide rules.

## Commands
- Build (debug): `cargo build --locked`
- Build (release): `cargo build --locked --release`
- Test: `cargo test --locked`
- Build sanity/typecheck: `cargo check --message-format=short`
- CI gate (Apple Silicon macOS, `.github/workflows/ci.yml`): run `RUSTFLAGS="-D warnings" cargo check --locked --all-targets`, `RUSTFLAGS="-D warnings" cargo test --locked`, and `RUSTFLAGS="-D warnings" cargo build --locked --release --target aarch64-apple-darwin`. Keep the same `RUSTFLAGS` for all three commands; changing it rebuilds Cargo artifacts, so run this gate as a separate sequence from ordinary development checks.
- Run with the repo-local dev config: `just setup` then `just run` (both create the gitignored `./config.toml` from `config.example.toml` when it is missing)
- Prepare the default dev config without starting the app: `just prepare-default-config`
- Run with an explicit config file: `just run-config path/to/config.toml`
- Run with normal XDG/home config lookup: `just run-xdg`
- List audio input devices from an installed app bundle: `just list-devices`
- Test the Sparkle update flow: `just test-update` (stops the running app, rebuilds `dist/simple-ptt.app` with an older test version, and launches it; run only when app launches are authorized).
- Run with a simulated transcription error: `just run-simulated-error "error message"` (uses the repo-local config and launches the app; begin a recording to trigger the error).
- Build the `.app` bundle: `just bundle-release`
- Build the DMG: `just bundle-dmg`
- Build a Developer ID signed and notarized DMG: `just bundle-notarized <unused-dmg-path> <new-evidence-directory>` with the credentials described in `docs/internal/references/apple-signing.md`. Local bundle/install recipes default to ad-hoc development signing.
- Install to `~/Applications` and launch: `just install-app && just start`
- Stop the app, reset macOS permissions, and reinstall without launching: `just reinstall-and-reset` (uses `scripts/clear-macos-permissions.sh` and `just install-app`; accepts an optional install directory, defaulting to `~/Applications`).
- Overlay debug mode (for the user to run; it blocks): `just debug-overlay` (`simple-ptt --debug`) shows the overlay with built-in default config and a window of live controls for every `GlassTuning` and `PillTuning` value, the meter style, the voice that drives the meter (a synthetic voice that alternates soft and loud with a 1.5 s silence every 6 s, queuing its spectrum as the audio callback does, or the microphone from the user's `[mic]` config through the app's own audio path), a readout of the pills' range and the latest loudest band, the correction, and a narration loop. "Copy values" copies the tuned values for the user to paste back.
- Capture every overlay state over light and dark backdrops, then exit: `cargo run -- --overlay-snapshot .tmp/snapshots` (needs Screen Recording permission for the terminal).
- Record the README demo video (overlay, transcript, correction): follow the `readme-demo` skill in `.agents/skills/readme-demo/`; `bun .agents/skills/readme-demo/scripts/record-demo.ts record`, then `encode`.

## Setup
- `rust-toolchain.toml` pins the compiler, rustfmt, Clippy, and Apple Silicon target for local builds, CI, and releases. Run `rustup toolchain install --no-self-update` from the repository to install that declared toolchain; do not change the machine-wide Rust default.
- Runtime and release packaging are macOS-only and currently target Apple Silicon (`aarch64-apple-darwin` in `.github/workflows/release.yml`). The app requires macOS 26 or later (`LSMinimumSystemVersion` in `scripts/build-macos-app.sh`) because the overlay uses Liquid Glass (`NSGlassEffectView`); keep each release's `sparkle:minimumSystemVersion` in `appcast.xml` at the same version.
- Normal app launches should use `~/.config/simple-ptt/config.toml`. `SIMPLE_PTT_CONFIG` is for Terminal-driven dev runs only.
- Keep secrets out of the repo. Use placeholders in `config.example.toml`; do not commit real Deepgram or LLM API keys.

## Conventions
- Keep AppKit work on the main thread. Follow the `MainThreadMarker` and AppDelegate patterns in `src/main.rs` and `src/app/mod.rs`; do not move Cocoa/AppKit calls onto worker threads.
- Each Deepgram session owns its Tokio runtime so all session tasks and sockets are released when it ends. Keep shutdown bounded and abortable in `src/transcription/session.rs`, and settle cancellation through `finish_session` in `src/transcription/mod.rs` before pasting, transforming, or resuming. Terminal metadata ends transcription without waiting for transport EOF.
- Capture the editable annotation before publishing `STATE_RECORDING` and include it in the dictation start command; the worker must not infer resume intent from the later state. An empty correction resumes an interrupted dictation without invoking the transformation model. Keep audio playing and defer rebuilds while `AppState::is_capturing_audio()` holds, including correction completion and resume.
- F5 automatic transformation reuses successful manual F6 output only when the finalized annotation still matches it, ignoring segment-edge whitespace from transcription. Track this in the worker's `TransformationHistory`, compare after `finish_session`, and settle cancellation even on reuse. New dictations clear the history; explicit F6 requests always run.
- Ready-buffer F6 refreshes the worker buffer from the editable `AppState` text, including deletions. Empty input returns to idle without an LLM request. Preserve that input in the overlay on transformation failure.
- Transformation streams require a terminal record before returning pasteable text. Reject output-limit and content-filter cutoffs even when they contain text. Anthropic requests set an explicit output budget instead of depending on rig's model-prefix defaults.
- The default correction shortcut is `Alt+Cmd` (Option+Command on macOS). Modifier-only correction chords accept either side and press order; releasing either required modifier group ends correction. Use native event flags for the modifiers remaining after release, including when the other side of the same modifier stays held. Command alone leaves copy and paste shortcuts available.
- Audio callback health uses one monotonic origin per `StreamActivity`, with a fresh first-callback deadline after play or resume. A replacement stream acknowledges only the hardware notification generation captured before its rebuild, so reconnects during the rebuild stay pending. See `docs/internal/references/audio-recovery.md`.
- Deepgram uses the user's SDK fork pinned to a reviewed commit. Read `docs/internal/references/deepgram-sessions.md` before changing its dependency, task ownership, cancellation, deadlines, or waiting indication; a dropped Tokio JoinHandle detaches its task, and a keep-alive receives no server reply.
- The settings window is AppKit with Auto Layout through the `objc2-app-kit` crate (decision in #7): a toolbar-style `NSTabViewController` with one pane per area in `src/settings_window/panes/`, each pane an `NSGridView` form built with `FormGrid` in `src/settings_window/grid.rs`. The panes are General, Microphone, Deepgram, and Transformation; the Transformation pane also holds the dictation and correction prompt editors (`src/settings_window/panes/prompt_editors.rs`) below its form, because both prompts go only to the transformation LLM. Do not position settings views with frames or computed offsets, do not draw layer borders on native controls, use system font variants only (no monospaced or custom families; the monospaced-digit system font is fine for changing numbers), and do not add SwiftUI, another GUI toolkit, or new FFI for settings UI.
- When adding or changing a setting, update all layers together: the pane in `src/settings_window/panes/` (control load/read), its field in `SettingsForm` in `src/settings_window/form.rs` (`from_config`/`to_config`, with round-trip tests there), `src/config/mod.rs` (defaults, resolution, persistence), and `config::problems` in `src/config/problems.rs`, which Save, Settings load, and startup all report. Numeric settings use `number_field` in `src/settings_window/controls.rs` so an `NSNumberFormatter` rejects invalid text at entry.
- Every control targeting `AppDelegate` must use a typed action: `SettingsAction` from the `settings_actions!` table in `src/settings_window/actions.rs` for Settings, or `ApplicationAction` from the `application_actions!` table in `src/app/actions.rs` for application menus and permission-dialog buttons. Define each action once in its table and implement the matching `#[unsafe(method(...))]` on `AppDelegate` in `src/app/mod.rs`. Do not pass raw `sel!` selectors to these controls. The `app_delegate_implements_every_settings_window_action` and `app_delegate_implements_every_application_action` tests fail with the missing selector names. Standard responder-chain actions and Sparkle's controller-owned update action keep their native targets.
- Open the settings window only through `AppDelegate::present_settings_window` in `src/app/mod.rs`. A key nonactivating overlay can make `NSApplication::isActive` true while another process is frontmost, so the default-mode turn also checks `NSRunningApplication::currentApplication().isActive()`. When inactive, order Settings front without making it key, then call `NSApplication::activateIgnoringOtherApps(true)` for this explicit user request. On macOS 26.6.2, `activate()` leaves it inactive and `NSRunningApplication::activateWithOptions` rejects the request; this deprecated public call and its narrowly scoped `#[allow(deprecated)]` are explicitly approved. Before replacing it, use a native probe to verify that Cmd+, hides the overlay and opens Settings with both process activation and key focus; pure Rust tests cannot verify macOS granting activation. Settings takes key focus and enables its keyboard handling only on `applicationDidBecomeActive:` (decided by `src/app/settings_presentation.rs`). A key window in an inactive app does not open its pop-up menus (#16). If activation is declined, keep the window visible and the request pending with recording hotkeys enabled until activation or window closure. Apart from the launch-time activation request in `applicationDidFinishLaunching:`, do not activate the app for Settings or order the Settings window front anywhere else. The windows opened at launch, and their order, are planned in `src/app/startup_windows.rs`.
- Preserve user config comments and unknown TOML sections by writing through `config::save_config` in `src/config/mod.rs`. It intentionally uses `toml_edit`; do not replace it with a lossy serializer.
- Permission changes are stateful and may require relaunch after grant. Follow the `NeedsRelaunch` flow in `src/permissions.rs` and `src/permissions_dialog.rs` instead of shortcutting it.
- Keep build and packaging changes aligned across `rust-toolchain.toml`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `scripts/build-macos-app.sh`, and `scripts/build-macos-dmg.sh`.
- Release packaging must use `scripts/build-notarized-dmg.sh`, preserve Sparkle's downloader entitlements, sign nested code inside out, and pass stapling, strict signature, and Gatekeeper checks on the final mounted artifact. Reuse submission IDs after notarization timeouts. Never use `codesign --deep` for signing or replace an asset already referenced by the update feed. See `docs/internal/references/apple-signing.md`.
- Before changing how the overlay looks or moves (`src/overlay/`, `src/ui_meter.rs`, `src/ui_meter/`), load the `overlay-visual-debugging` skill in `.agents/skills/`; it holds the probe workflow and measured macOS 26 glass behaviors. Values the user settles on in debug mode become the `GlassTuning` defaults in `src/overlay/glass.rs`, pinned by its tests. Debug mode lives in `src/overlay/dev/`; product code must not call the `OverlayWindow` methods only it uses (`pin_to_top`, `glass_tuning`, `set_glass_tuning`, `halo_is_progressive`, `glass_internals_now`, `pill_tuning`, `set_pill_tuning`, `pill_range_now`). Values the user settles on for the pills become the `PillTuning` defaults in `src/ui_meter/pill_tuning.rs`. Debug mode is a developer tool: keep it out of `README.md`.
- The pill meter's requirements, and why each part of its design exists, are in `docs/internal/references/pill-meter.md`. Read it before changing the pills (`src/ui_meter/pill_levels.rs`, `src/ui_meter/pill_cluster.rs`, the spectrum in `src/audio/spectrum.rs` and its plumbing in `src/audio/stream.rs` and `src/state.rs`), and update it with any change to what they do.

## Releases & Versioning
- **SemVer:** Automatically determine the next best SemVer release version based on the git history (e.g. `feat:` for minor, `fix:` for patch). Always confirm the proposed next version with the user before committing bumps or creating tags.
- **Release Notes:** Automatically generate and provide comprehensive release notes based on the git history and implemented features/fixes when preparing a release.
- **Version Authority:** The ultimate authority on the current version depends on the deployment destination. For projects with external registries (like NPM or Crates.io), the published registry is the authority, not just GitHub tags. Since this app compiles binaries directly to GitHub Releases with no external registry, **GitHub Releases are the absolute authority** for its version.
- **Failed Releases:** If a GitHub release action fails to compile or attach binaries, it is acceptable to delete the tag/release and republish the exact same version number to retry the process.

The release workflow uploads the DMG; complete Sparkle feed publication manually using the exact commands and failure handling in `docs/internal/references/sparkle-publication.md`:

1. Confirm the approved tag is published and download its actual DMG with `gh release download`; never sign a local rebuild instead.
2. Inspect the DMG read-only without launching the app. Verify its version, macOS minimum, bundled framework, feed URL, and public key against packaging metadata. Compare the key with Sparkle's `generate_keys --account <existing-account> -p`; stop if the matching private key is unavailable. Do not generate a replacement key.
3. Copy the current feed to staging and run the matching Sparkle version's `generate_appcast --account <existing-account>` with the published download URL prefix, `--maximum-versions 0`, and `--maximum-deltas 0`.
4. Require the generated entry's URL, versions, and minimum OS to match the release/bundle, its length to match both downloaded bytes and GitHub asset metadata, and `sign_update --account <existing-account> --verify <downloaded-DMG> <entry-signature>` to succeed.
5. Independently review and publish only the verified `appcast.xml` change to current `main`. It goes live immediately; never publish an entry before its asset exists or replace an asset already referenced by the feed.
6. Fetch the live feed and verify that its entry matches the reviewed version, URL, length, and signature before marking the release complete. Retain verification evidence and remove only reviewed staging files.

## Gotchas
- LaunchServices-launched apps do not reliably inherit shell environment variables. For real app runs, prefer file-backed config in `~/.config/simple-ptt/config.toml`.
- `just run` sets `SIMPLE_PTT_CONFIG=./config.toml`; `just run-xdg` does not. Use the right command when reproducing config-loading bugs.
- macOS TCC state can become stale after rebuilding or replacing the ad-hoc-signed app bundle. Use the in-app permissions flow or `scripts/clear-macos-permissions.sh`, then relaunch.
- Do not start the application yourself, that's a blocking process and user doesn't expect it. This includes `just debug-overlay`; `--overlay-snapshot` and the `readme-demo` skill's `record-demo.ts record` are the exceptions because they exit on their own.
- Every settings pane must fit the window's minimum content size: `SettingsWindow::new` sizes it from the largest pane's `fittingSize`, measured again after a layout pass so wrapping hints count at their wrapped height. Hint rows hidden while empty (the API key environment hints) are not counted; the Transformation pane's prompt editors give up that height instead of the window growing, because their minimum height has a priority below `NSLayoutPriorityWindowSizeStayPut`. `cargo test` cannot check AppKit layout, so verify layout changes with an ad hoc, uncommitted off-screen harness under `.tmp/` that builds the window on the main thread and checks `hasAmbiguousLayout`, first-baseline alignment, and snapshots; do not launch the app.
- **Overlay UI Keybindings:** Do not introduce explicit keyboard actions (like Enter, Esc, etc.) inside the overlay's text editor. The entire dictation, editing, and pasting sequence is driven purely by the system-wide record/transform hotkeys (e.g., F5/F6) captured by the CGEventTap in `src/hotkey_macos.rs` and dispatched in `src/hotkey.rs`. Releasing the recording hotkey acts as the trigger to finish and paste.

## Boundaries
- Always: there MUST be ZERO errors AND ZERO WARNINGS when the application is checked or built (`cargo check`, `cargo test`, `cargo build`). A successful build that emits warnings is strictly unacceptable and considered a build failure.
- Always: `.github/workflows/ci.yml` enforces the zero-warnings rule on pull requests and pushes to `main`: it checks all Cargo targets, runs tests, and builds the Apple Silicon release binary using the pinned toolchain and `RUSTFLAGS="-D warnings"`. Reproduce it with the CI gate commands above.
- Always: after Rust or packaging-script changes, run `cargo test --locked`, `cargo check --message-format=short`, and `cargo build --locked --release`. Verify that output is 100% clean with zero warnings.
- Ask first: changes to `Cargo.toml`, `.github/workflows/release.yml`, bundle metadata/signing in `scripts/build-macos-app.sh`, or the permission architecture in `src/permissions*.rs`.
- Never: commit secrets in config files, hand-edit generated output under `dist/` or `target/`, bypass `config::save_config` with a destructive config rewrite, or introduce fallbacks, degraded functionality, or secondary alternative execution paths unless explicitly requested by the user.

## References
- `README.md`
- `config.example.toml`
- `src/main.rs`
- `src/app/`
- `src/audio/`
- `src/config/`
- `src/overlay/`
- `src/settings_window/`
- `src/transcription/`
- `src/permissions.rs`
- `docs/internal/references/pill-meter.md`
- `docs/internal/references/deepgram-sessions.md`
- `docs/internal/references/audio-recovery.md`
- `.agents/skills/overlay-visual-debugging/`
- `.agents/skills/readme-demo/`
- `scripts/build-macos-app.sh`
- `.github/workflows/release.yml`
- `.github/workflows/ci.yml`
- `rust-toolchain.toml`
