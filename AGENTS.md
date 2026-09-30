# simple-ptt

Rust/AppKit menu bar push-to-talk app for macOS on Apple Silicon. This is a single-crate repo; keep this file focused on repo-wide rules.

## Commands
- Build (debug): `cargo build --locked`
- Build (release): `cargo build --locked --release`
- Test: `cargo test --locked`
- Build sanity/typecheck: `cargo check --message-format=short`
- Run with the repo-local dev config: `just run` (creates the gitignored `./config.toml` from `config.example.toml` when it is missing)
- Run with an explicit config file: `just run-config path/to/config.toml`
- Run with normal XDG/home config lookup: `just run-xdg`
- List audio input devices from an installed app bundle: `just list-devices`
- Build the `.app` bundle: `just bundle-release`
- Build the DMG: `just bundle-dmg`
- Install to `~/Applications` and launch: `just install-app && just start`
- Overlay debug mode (for the user to run; it blocks): `just debug-overlay` (`simple-ptt --debug`) shows the overlay with built-in default config and a window of live controls for every `GlassTuning` value, the meter style (fed a synthetic voice that alternates soft and loud), the correction, and a narration loop. "Copy values" copies the tuned values for the user to paste back.
- Capture every overlay state over light and dark backdrops, then exit: `cargo run -- --overlay-snapshot .tmp/snapshots` (needs Screen Recording permission for the terminal).

## Setup
- Runtime and release packaging are macOS-only and currently target Apple Silicon (`aarch64-apple-darwin` in `.github/workflows/release.yml`). The app requires macOS 26 or later (`LSMinimumSystemVersion` in `scripts/build-macos-app.sh`) because the overlay uses Liquid Glass (`NSGlassEffectView`); keep each release's `sparkle:minimumSystemVersion` in `appcast.xml` at the same version.
- Normal app launches should use `~/.config/simple-ptt/config.toml`. `SIMPLE_PTT_CONFIG` is for Terminal-driven dev runs only.
- Keep secrets out of the repo. Use placeholders in `config.example.toml`; do not commit real Deepgram or LLM API keys.

## Conventions
- Keep AppKit work on the main thread. Follow the `MainThreadMarker` and AppDelegate patterns in `src/main.rs` and `src/app/mod.rs`; do not move Cocoa/AppKit calls onto worker threads.
- The settings window is AppKit with Auto Layout through the `objc2-app-kit` crate (decision in #7): a toolbar-style `NSTabViewController` with one pane per area in `src/settings_window/panes/`, each pane an `NSGridView` form built with `FormGrid` in `src/settings_window/grid.rs`. The panes are General, Microphone, Deepgram, and Transformation; the Transformation pane also holds the dictation and correction prompt editors (`src/settings_window/panes/prompt_editors.rs`) below its form, because both prompts go only to the transformation LLM. Do not position settings views with frames or computed offsets, do not draw layer borders on native controls, use system font variants only (no monospaced or custom families; the monospaced-digit system font is fine for changing numbers), and do not add SwiftUI, another GUI toolkit, or new FFI for settings UI.
- When adding or changing a setting, update all layers together: the pane in `src/settings_window/panes/` (control load/read), its field in `SettingsForm` in `src/settings_window/form.rs` (`from_config`/`to_config`, with round-trip tests there), `src/config/mod.rs` (defaults, resolution, persistence), and `validate_settings_config` in `src/app/mod.rs`. Numeric settings use `number_field` in `src/settings_window/controls.rs` so an `NSNumberFormatter` rejects invalid text at entry.
- Settings window controls send target/action messages to `AppDelegate`. Define each action once as a row of the `settings_actions!` table in `src/settings_window/actions.rs`, and implement the matching `#[unsafe(method(...))]` on `AppDelegate` in `src/app/mod.rs`. Do not pass raw `sel!` selectors to settings controls; `app_delegate_implements_every_settings_window_action` fails when a selector is not implemented.
- Open the settings window only through `AppDelegate::present_settings_window` in `src/app/mod.rs`. `NSApplication::activate` is only a request, so the window is ordered front after `applicationDidBecomeActive:` (decided by `src/app/settings_presentation.rs`); a key window in an inactive app does not open its pop-up menus (#16). If macOS declines activation, the request stays pending until the app is next activated (for example from its Dock icon) or, when Settings is already on screen, until it is closed. Apart from the launch-time activation request in `applicationDidFinishLaunching:`, do not activate the app for Settings or order the Settings window front anywhere else. The windows opened at launch, and their order, are planned in `src/app/startup_windows.rs`.
- Preserve user config comments and unknown TOML sections by writing through `config::save_config` in `src/config/mod.rs`. It intentionally uses `toml_edit`; do not replace it with a lossy serializer.
- Permission changes are stateful and may require relaunch after grant. Follow the `NeedsRelaunch` flow in `src/permissions.rs` and `src/permissions_dialog.rs` instead of shortcutting it.
- Keep packaging changes aligned across `scripts/build-macos-app.sh`, `scripts/build-macos-dmg.sh`, and `.github/workflows/release.yml`.
- Before changing how the overlay looks or moves (`src/overlay/`, `src/ui_meter.rs`, `src/ui_meter/`), load the `overlay-visual-debugging` skill in `.agents/skills/`; it holds the probe workflow and measured macOS 26 glass behaviors. Values the user settles on in debug mode become the `GlassTuning` defaults in `src/overlay/glass.rs`, pinned by its tests. Debug mode lives in `src/overlay/dev/`; product code must not call the `OverlayWindow` methods only it uses (`pin_to_top`, `glass_tuning`, `set_glass_tuning`, `halo_is_progressive`, `glass_internals_now`). Debug mode is a developer tool: keep it out of `README.md`.

## Releases & Versioning
- **SemVer:** Automatically determine the next best SemVer release version based on the git history (e.g. `feat:` for minor, `fix:` for patch). Always confirm the proposed next version with the user before committing bumps or creating tags.
- **Release Notes:** Automatically generate and provide comprehensive release notes based on the git history and implemented features/fixes when preparing a release.
- **Version Authority:** The ultimate authority on the current version depends on the deployment destination. For projects with external registries (like NPM or Crates.io), the published registry is the authority, not just GitHub tags. Since this app compiles binaries directly to GitHub Releases with no external registry, **GitHub Releases are the absolute authority** for its version.
- **Failed Releases:** If a GitHub release action fails to compile or attach binaries, it is acceptable to delete the tag/release and republish the exact same version number to retry the process.

## Gotchas
- LaunchServices-launched apps do not reliably inherit shell environment variables. For real app runs, prefer file-backed config in `~/.config/simple-ptt/config.toml`.
- `just run` sets `SIMPLE_PTT_CONFIG=./config.toml`; `just run-xdg` does not. Use the right command when reproducing config-loading bugs.
- macOS TCC state can become stale after rebuilding or replacing the ad-hoc-signed app bundle. Use the in-app permissions flow or `scripts/clear-macos-permissions.sh`, then relaunch.
- Do not start the application yourself, that's a blocking process and user doesn't expect it. This includes `just debug-overlay`; `--overlay-snapshot` is the exception because it exits on its own.
- Every settings pane must fit the window's minimum content size: `SettingsWindow::new` sizes it from the largest pane's `fittingSize`, measured again after a layout pass so wrapping hints count at their wrapped height. Hint rows hidden while empty (the API key environment hints) are not counted; the Transformation pane's prompt editors give up that height instead of the window growing, because their minimum height has a priority below `NSLayoutPriorityWindowSizeStayPut`. `cargo test` cannot check AppKit layout, so verify layout changes with an ad hoc, uncommitted off-screen harness under `.tmp/` that builds the window on the main thread and checks `hasAmbiguousLayout`, first-baseline alignment, and snapshots; do not launch the app.
- **Overlay UI Keybindings:** Do not introduce explicit keyboard actions (like Enter, Esc, etc.) inside the overlay's text editor. The entire dictation, editing, and pasting sequence is driven purely by the system-wide record/transform hotkeys (e.g., F5/F6) captured by the CGEventTap in `src/hotkey_macos.rs` and dispatched in `src/hotkey.rs`. Releasing the recording hotkey acts as the trigger to finish and paste.

## Boundaries
- Always: there MUST be ZERO errors AND ZERO WARNINGS when the application is checked or built (`cargo check`, `cargo test`, `cargo build`). A successful build that emits warnings is strictly unacceptable and considered a build failure.
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
- `.agents/skills/overlay-visual-debugging/`
- `scripts/build-macos-app.sh`
- `.github/workflows/release.yml`
