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

## Quick start

> [!IMPORTANT]
> The bundled app is ad-hoc signed but **not notarized**. If macOS blocks it on first launch, either allow it in **System Settings > Privacy & Security** or remove quarantine manually:
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/simple-ptt.app
> ```

Expect the usual macOS prompts for: **Microphone** and **Accessibility** for the global hotkey and synthetic paste workflow.

If the app launches but the global shortcuts do nothing, check **Privacy & Security > Input Monitoring** and **Privacy & Security > Accessibility** and make sure `/Applications/simple-ptt.app` is enabled in both places. If you replaced the app bundle with a new ad-hoc-signed build, remove and re-add it there before relaunching; macOS TCC can treat rebuilt ad-hoc-signed bundles as a new identity.

## Cost note

Deepgram usage for this kind of developer push-to-talk workflow is usually cheap. A full workday typically lands around **$0.50 USD** in transcription cost, but that is only a rough estimate. Actual cost depends on how long you dictate, which Deepgram plan you are on, which model you use, and Deepgram's current pricing. Check the official [Deepgram pricing page](https://deepgram.com/pricing) before treating that number as current.

## How it works

### Data flow

- Audio is streamed to **Deepgram** for transcription.
- If transformation is enabled, buffered transcript text is sent to your configured LLM provider.

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

If no config file is found, defaults are used where possible and the app opens **Settings** so you can create one. For normal app launches, `~/.config/simple-ptt/config.toml` is the correct default.

### Minimal config

If you prefer to edit the file by hand, this is enough to get transcription working. See [`config.example.toml`](./config.example.toml) for all available options.

```toml
[deepgram]
api_key = "YOUR_DEEPGRAM_API_KEY"
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
