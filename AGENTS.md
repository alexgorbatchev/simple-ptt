# simple-ptt

Rust/AppKit menu bar push-to-talk app for macOS on Apple Silicon. This is a single-crate repo; keep this file focused on repo-wide rules.

## Commands
- Build (debug): `cargo build --locked`
- Build (release): `cargo build --locked --release`
- Test: `cargo test --locked`
- Build sanity/typecheck: `cargo check --message-format=short`
- Run with the repo-local dev config: `just run`
- Run with an explicit config file: `just run-config path/to/config.toml`
- Run with normal XDG/home config lookup: `just run-xdg`
- List audio input devices from an installed app bundle: `just list-devices`
- Build the `.app` bundle: `just bundle-release`
- Build the DMG: `just bundle-dmg`
- Install to `~/Applications` and launch: `just install-app && just start`

## Setup
- Runtime and release packaging are macOS-only and currently target Apple Silicon (`aarch64-apple-darwin` in `.github/workflows/release.yml`).
- Normal app launches should use `~/.config/simple-ptt/config.toml`. `SIMPLE_PTT_CONFIG` is for Terminal-driven dev runs only.
- Keep secrets out of the repo. Use placeholders in `config.example.toml`; do not commit real Deepgram or LLM API keys.

## Conventions
- Keep AppKit work on the main thread. Follow the `MainThreadMarker` and AppDelegate patterns in `src/main.rs` and `src/app.rs`; do not move Cocoa/AppKit calls onto worker threads.
- When adding or changing a setting, update all three layers together: `src/settings_window.rs` (UI read/write), `src/config.rs` (defaults, resolution, persistence), and `validate_settings_config` in `src/app.rs`.
- Preserve user config comments and unknown TOML sections by writing through `config::save_config` in `src/config.rs`. It intentionally uses `toml_edit`; do not replace it with a lossy serializer.
- Permission changes are stateful and may require relaunch after grant. Follow the `NeedsRelaunch` flow in `src/permissions.rs` and `src/permissions_dialog.rs` instead of shortcutting it.
- Keep packaging changes aligned across `scripts/build-macos-app.sh`, `scripts/build-macos-dmg.sh`, and `.github/workflows/release.yml`.

## Gotchas
- LaunchServices-launched apps do not reliably inherit shell environment variables. For real app runs, prefer file-backed config in `~/.config/simple-ptt/config.toml`.
- `just run` sets `SIMPLE_PTT_CONFIG=./config.toml`; `just run-xdg` does not. Use the right command when reproducing config-loading bugs.
- macOS TCC state can become stale after rebuilding or replacing the ad-hoc-signed app bundle. Use the in-app permissions flow or `scripts/clear-macos-permissions.sh`, then relaunch.
- Do not start the application yourself, that's a blocking process and user doesn't expect it.

## Boundaries
- Always: there must be no errors or warnings when the application is built. A successful build with warnings is not acceptable.
- Always: after Rust or packaging-script changes, run `cargo test --locked`, `cargo check --message-format=short`, and `cargo build --locked --release`.
- Ask first: changes to `Cargo.toml`, `.github/workflows/release.yml`, bundle metadata/signing in `scripts/build-macos-app.sh`, or the permission architecture in `src/permissions*.rs`.
- Never: commit secrets in config files, hand-edit generated output under `dist/` or `target/`, or bypass `config::save_config` with a destructive config rewrite.

## References
- `README.md`
- `config.example.toml`
- `src/main.rs`
- `src/app.rs`
- `src/config.rs`
- `src/settings_window.rs`
- `src/permissions.rs`
- `scripts/build-macos-app.sh`
- `.github/workflows/release.yml`
