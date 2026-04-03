# simple-ptt

A fast, minimal push-to-talk app for macOS with live Deepgram transcription and optional LLM cleanup before paste.

![simple-ptt demo](./screen.gif)

`simple-ptt` is intentionally small: menu bar app, global hotkey, live on-screen transcript, fast paste into the currently focused app. In normal use it aims to stay around **35 MB of RAM**. The goal is not to be feature-rich. The goal is to stay fast, understandable, and out of the way.

## At a glance

- **Platform:** macOS on **Apple Silicon (arm64)**
- **Transcription:** Deepgram real-time streaming
- **Optional cleanup:** transform buffered text with an LLM before pasting
- **UI:** menu bar app with a live transcript overlay
- **Memory target:** roughly **35 MB RAM** in normal use
- **Input model:** tap-to-toggle or hold-to-talk with a configurable threshold
- **Permissions:** Microphone plus **Accessibility** and/or **Input Monitoring**
- **Signing:** ad-hoc signed for local distribution, **not** Developer ID-signed or notarized

## Data flow

This app is not offline-first.

- Audio is streamed to **Deepgram** for transcription.
- If transformation is enabled, buffered transcript text is sent to your configured LLM provider.
- If you need a fully local workflow, this project is not there today.

## Cost note

Deepgram usage for this kind of developer push-to-talk workflow is usually cheap. A full workday typically lands around **$0.50 to $0.75 USD** in transcription cost, but that is only a rough estimate. Actual cost depends on how long you dictate, which Deepgram plan you are on, which model you use, and Deepgram's current pricing. Check the official [Deepgram pricing page](https://deepgram.com/pricing) before treating that number as current.

## Quick start

### Option 1: Install a release build

1. Open the latest GitHub release.
2. Download the `simple-ptt-vX.Y.Z-macos-arm64.dmg` asset.
3. Open the disk image.
4. Drag `simple-ptt.app` into `/Applications`.
5. Launch it:

```bash
open -g /Applications/simple-ptt.app
```

Do **not** launch the app directly from the mounted disk image. Copy it into `Applications` first.

On first launch, `simple-ptt` opens **Settings** if it does not find a usable config yet. Enter your Deepgram API key there and click **Save and Apply**. The app creates `~/.config/simple-ptt/config.toml` for you.

### First run on macOS

Expect the usual macOS prompts for:

- **Microphone**
- **Accessibility** and/or **Input Monitoring** for the global hotkey and synthetic paste workflow

The bundled app is ad-hoc signed but **not notarized**. If macOS blocks it on first launch, either allow it in **System Settings > Privacy & Security** or remove quarantine manually:

```bash
xattr -dr com.apple.quarantine /Applications/simple-ptt.app
```

If the app launches without a configured Deepgram API key, it opens **Settings** and tells you what is missing.

## How it works

### Default workflow

- Press the record hotkey (`F5` by default) to start listening.
- Speak and watch the live transcript overlay update in real time.
- Stop recording to paste the buffered text into the focused app.
- If transformation is configured and enabled, the app can clean up the transcript before pasting.

### Additional controls

- **Tap vs hold:** short press behaves like toggle; holding past `mic.hold_ms` turns the same hotkey into hold-to-talk.
- **Transform hotkey (`F6` by default):** transform the current transcript without auto-pasting it.
- **`Escape`:** abort recording, cancel background work, or discard a ready buffer.
- **`Cmd+V` while recording:** splice the current plain-text clipboard contents into the active transcript.

## Configuration

`simple-ptt` looks for config in this order:

1. `SIMPLE_PTT_CONFIG`
2. `$XDG_CONFIG_HOME/simple-ptt/config.toml`
3. `~/.config/simple-ptt/config.toml`

If no config file is found, defaults are used where possible and the app opens **Settings** so you can create one. For normal app launches, `~/.config/simple-ptt/config.toml` is the correct default. Relying on shell environment variables for LaunchServices-launched apps is fragile.

The native **Settings…** window is now the primary setup path. It creates the config file for you and lets you change the common options without editing TOML by hand. Manual editing is still supported if you want it.


### Minimal config

If you prefer to edit the file by hand, this is enough to get transcription working:

```toml
[deepgram]
api_key = "YOUR_DEEPGRAM_API_KEY"
```

### Common sections

- `[ui]`: record hotkey, overlay font sizes, mic meter style
- `[mic]`: input device, sample rate, gain, hold threshold
- `[deepgram]`: API key, language, model, endpoint timing
- `[transformation]`: transform hotkey, provider, model, prompt override

See [`config.example.toml`](./config.example.toml) for a complete manual-edit example.

### Audio input devices

To list available input devices from an installed app bundle:

```bash
~/Applications/simple-ptt.app/Contents/MacOS/simple-ptt --list-devices
```

## Build from source

### Requirements

- Rust toolchain
- macOS on **Apple Silicon (arm64)**

Build the release binary and bundle it as a macOS app:

```bash
cargo build --locked --release
./scripts/build-macos-app.sh target/release/simple-ptt dist/simple-ptt.app
```

Or use the checked-in helpers:

```bash
just bundle-release
just bundle-dmg
```

That creates:

```text
dist/simple-ptt.app
dist/simple-ptt.dmg
```

If you want it installed into `~/Applications` and launched with the same non-blocking workflow:

```bash
just install-app
just start
```

## Development

For the checked-in development config in this repository:

```bash
just run
```

Useful helper targets:

```bash
just run-config path/to/config.toml
just run-xdg
just bundle-release
just bundle-dmg
just install-app
just start
just list-devices
```

## License

MIT. See [LICENSE](./LICENSE).
