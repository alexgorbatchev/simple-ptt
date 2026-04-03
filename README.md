# simple-ptt

![simple-ptt demo](./screen.gif)

A very basic push-to-talk utility for macOS.

The goal here is not to be feature-rich. The goal is to stay small, stay fast, and stay understandable. In normal use it aims to stay under roughly 35 MB of RAM, uses the Deepgram API for real-time transcription, and shows the live transcription on screen while you are still talking. If configured, it can also run an LLM cleanup pass on the buffered transcript before pasting.

I built this because I did not like the existing push-to-talk implementations. Everything felt too slow to respond to a hotkey, too slow to process and paste, and none that I tried showed transcription in real time.

## What it does

- Menu bar app with a global push-to-talk hotkey
- Real-time transcription with Deepgram
- On-screen overlay that updates while you speak
- Tap-to-toggle and hold-to-talk behavior with a configurable hold threshold
- Synthetic paste into the currently focused app
- Optional LLM cleanup pass before pasting buffered text
- Separate transformation hotkey for transform-without-paste workflows
- Buffered text state so you can transform, paste, or discard later
- Optional month-to-date Deepgram billing display when `deepgram.project_id` is configured
- Native settings window from the menu bar
- Configurable audio input device, overlay font, and overlay mic meter style
- File-backed settings save with TOML comment/unknown-key preservation
- Live apply for most settings without relaunch
- CLI helper to list available input devices

## Platform

This project currently targets **macOS on Apple Silicon (arm64)**.

## Install

### Option 1: Download the disk image from a release

1. Open the latest GitHub release.
2. Download the `simple-ptt-vX.Y.Z-macos-arm64.dmg` asset.
3. Open the disk image.
4. Drag `simple-ptt.app` into `Applications`.

Do not launch the app directly from the mounted disk image. Copy it into `Applications` first, then launch it.

Launch it from Terminal without blocking your shell:

```bash
open -g /Applications/simple-ptt.app
```

### Option 2: Build from source

Requirements:

- Rust toolchain
- macOS

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

If you want to install it into `~/Applications` and keep the non-blocking Terminal launch workflow:

```bash
just install-app
just start
```

## First run on macOS

The provided app-bundling script applies **ad-hoc codesigning** by default, but the app is **not Developer ID-signed or notarized**.

If macOS blocks it on first launch, use one of the following:

- run it once, then allow it in **System Settings > Privacy & Security**
- or remove the quarantine attribute manually:

```bash
xattr -dr com.apple.quarantine /Applications/simple-ptt.app
```

You should also expect macOS permission prompts for:

- **Microphone**
- **Accessibility** and/or **Input Monitoring** for the global push-to-talk hotkey and synthetic paste workflow

If the app launches without a configured Deepgram API key, it now opens the **Settings** window and shows an alert explaining what to add.

## How it behaves

### Record hotkey (`ui.hotkey`, default `F5`)

The record hotkey is not just a simple toggle. It supports both tap-to-toggle and hold-to-talk behavior:

- From **idle** or **error** state, pressing the record hotkey starts recording.
- If you **hold** the hotkey for at least `mic.hold_ms` and then release it, recording stops immediately on release.
- If you **tap** the hotkey and release it before `mic.hold_ms`, recording stays active and the next press stops it.
- If a transcript buffer is ready, pressing the record hotkey pastes the buffered text.
- While background work is already running, presses are ignored.

What happens when recording stops depends on transformation configuration:

- If transformation is configured and `transformation.auto = true`, stopping recording runs the transformation step and pastes the transformed result.
- Otherwise, stopping recording pastes the raw transcript.

### Transformation hotkey (`transformation.hotkey`, default `F6`)

The transformation hotkey is only registered when transformation is configured successfully.

- While **recording**, pressing the transformation hotkey stops recording and transforms the current transcript **without auto-pasting it**.
- While a **buffer is ready**, pressing the transformation hotkey transforms the current buffered text in place.
- After a transformed buffer is ready, press the record hotkey to paste it.

### Escape key

`Escape` is an abort/discard control:

- while **recording**, it aborts the current session and discards the transcript
- while **processing** or **transforming**, it requests abort
- while a **buffer is ready**, it discards the buffer

## Overlay and menu bar UI

### Overlay

The on-screen overlay shows:

- live interim and final transcript text
- default status text when there is no transcript yet:
  - `Listening…`
  - `Transcribing…`
  - `Ready to paste…`
  - `Transforming…`
- optional footer text for billing information
- optional footer hint text showing the transform/paste hotkeys
- a live mic meter while recording
- a clip warning border on the mic meter when the input clips

`ui.meter_style = "none"` disables the mic meter and the clip warning indicator.

### Menu bar item

The app runs as a menu bar accessory app.

The status item:

- shows an idle icon when inactive
- shows an active icon while recording, processing, transforming, or waiting with a buffered transcript

The menu contains:

- a version item that opens the GitHub repository
- a **Settings…** item
- an optional billing line when month-to-date spend is available
- an app termination item (`⌘Q`)

### Settings window

The settings window is opened from the menu bar.

It edits the resolved config file path shown at the top of the window and saves back to TOML while preserving unrelated keys and comments. The font family control is a native drop-down populated from available macOS font families plus a system-default option, the meter style control is also a native drop-down, the Deepgram model uses a curated model drop-down, and the transformation provider uses a supported-provider drop-down. **Cancel** closes the window without saving.

The record and transform hotkey rows include **Capture…** buttons. Click one, then press a supported key or key chord to fill the field automatically. The hotkey field updates live as you press or release keys during capture. Press `Escape` with no modifiers to cancel capture. Duplicate record/transform bindings are rejected immediately. While the settings window is open, the main global hotkeys are suspended so function keys and chords can be edited safely.

Apply behavior:

- overlay styling, hotkeys, `mic.gain`, and `mic.hold_ms` apply immediately
- microphone device/sample-rate changes apply immediately when idle, or after the current recording stops if a recording is active
- Deepgram and transformation provider/model/API settings apply to the next recording or transformation request
- no relaunch is required for normal settings changes

## Configuration

`simple-ptt` looks for configuration in this order:

1. `SIMPLE_PTT_CONFIG`
2. `$XDG_CONFIG_HOME/simple-ptt/config.toml`
3. `~/.config/simple-ptt/config.toml`

If no config file is found, defaults are used where possible and the app opens Settings automatically on launch so you can create `config.toml` with **Save and Apply**. However, you still need to provide a Deepgram API key either in the config file or through the `DEEPGRAM_API_KEY` environment variable.

Start from the example file in this repository:

```bash
mkdir -p ~/.config/simple-ptt
cp config.example.toml ~/.config/simple-ptt/config.toml
```

### Example config

```toml
[ui]
hotkey = "F5"
# font_name = "Menlo"
font_size = 12.0
footer_font_size = 10.0
meter_style = "animated-color"
# meter_style = "animated-height"
# meter_style = "none"

[mic]
# Optional: exact input device name or numeric index from the host input device list.
# audio_device = "MacBook Pro Microphone"
sample_rate = 16000
gain = 4.0
hold_ms = 300

[deepgram]
api_key = "YOUR_DEEPGRAM_API_KEY"
# project_id = "98bf0e8b-23f6-4c01-b672-604008a47504"
language = "en-US"
model = "nova-3"
endpointing_ms = 300
utterance_end_ms = 1000

[transformation]
hotkey = "F6"
auto = true
provider = "openai"
api_key = "YOUR_LLM_API_KEY"
model = "gpt-5.4-mini"
# Optional override; omit to use the built-in prompt.
# system_prompt = "..."
```

### Supported hotkey names

Hotkeys accept either a **single key** or a **modifier chord** such as `Shift+Cmd+Z`.

Supported primary keys are:

- letters: `A` through `Z`
- digits: `0` through `9`
- function keys: `F1` through `F12`
- `Escape` / `Esc`
- `Space`
- `Tab`
- `CapsLock`
- `LeftShift`, `RightShift`, `LShift`, `RShift`
- `LeftControl`, `RightControl`, `LCtrl`, `RCtrl`
- `LeftAlt`, `RightAlt`, `LAlt`, `RAlt`, `LeftOption`, `RightOption`
- `LeftMeta`, `RightMeta`, `LeftCommand`, `RightCommand`, `LCmd`, `RCmd`
- `Return`, `Enter`
- `Backspace`, `Delete`, `ForwardDelete`
- `Home`, `End`, `PageUp`, `PageDown`
- `UpArrow`, `DownArrow`, `LeftArrow`, `RightArrow`, and their short forms `Up`, `Down`, `Left`, `Right`

Supported chord modifiers are:

- `Shift`
- `Ctrl` / `Control`
- `Alt` / `Option`
- `Cmd` / `Command` / `Meta`

A multi-key hotkey must contain exactly one non-modifier primary key.

### Config values

#### `[ui]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `hotkey` | No | `F5` | Global record key. See the record-hotkey behavior above. Accepts either a single key or a modifier chord such as `Shift+Cmd+Z`. |
| `font_name` | No | system default | Overlay font family name. In the settings window this is chosen from a native drop-down of available macOS font families plus the system-default option. The legacy alias `overlay_font_family` is also accepted. |
| `font_size` | No | `12.0` | Main overlay font size. |
| `footer_font_size` | No | derived from `font_size` | Footer text font size. |
| `meter_style` | No | `animated-color` | Overlay mic meter style. Supported values: `animated-color`, `animated-height`, and `none`. `none` hides the meter and clip indicator entirely. |

#### `[mic]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `audio_device` | No | default input device | Exact input device name or numeric index. Use `simple-ptt --list-devices` to print the available inputs and their indices. |
| `sample_rate` | No | `16000` | Requested audio sample rate in Hz. If the device does not support it, the app falls back to the device default input config. |
| `gain` | No | `4.0` | Input gain multiplier applied before encoding and meter calculation. |
| `hold_ms` | No | `300` | Hold threshold in milliseconds for the record hotkey. Releasing after at least this long acts like hold-to-talk; releasing sooner leaves recording running until the next press. |

#### `[deepgram]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `api_key` | Yes* | none | Deepgram API key. Can also be provided via `DEEPGRAM_API_KEY`. |
| `project_id` | No | none | Optional Deepgram project ID. When set, the app refreshes month-to-date Deepgram spend when recording starts and shows it in the overlay footer. If a dollar amount is available, it is also shown in the menu bar. `DEEPGRAM_PROJECT_ID` is also accepted. |
| `language` | No | `en-US` | Deepgram language code. |
| `model` | No | `nova-3` | Deepgram transcription model. |
| `endpointing_ms` | No | `300` | Endpointing delay for transcription finalization. |
| `utterance_end_ms` | No | `1000` | Utterance end timeout in milliseconds. |

Billing notes for `deepgram.project_id`:

- billing display is disabled completely when no project ID is configured
- billing refresh happens when recording starts
- the Deepgram billing API may require an admin- or owner-level project API key; otherwise the overlay footer shows a permission warning instead of a dollar amount

#### `[transformation]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `hotkey` | No | `F6` | Global transform key. While recording, it stops and transforms without auto-pasting. While a buffer is ready, it transforms the buffered text in place. Accepts either a single key or a modifier chord such as `Shift+Cmd+Z`. |
| `auto` | No | `true` | When `true` and transformation is configured, stopping recording with the record hotkey transforms the transcript and pastes the transformed result. When `false`, the record hotkey keeps the raw-paste behavior. |
| `provider` | No | none | Rig provider name for the transformation request. See the canonical supported values below. |
| `api_key` | No | none | API key used for the transformation provider. If omitted, the app falls back to the provider-specific environment variables listed below. |
| `model` | No | `gpt-5.4-mini` | Model name used for the transformation request. |
| `system_prompt` | No | built-in prompt | Optional override for the built-in transformation prompt. The built-in prompt rewrites raw dictation into clean instructions, fixes punctuation and obvious transcription errors, preserves technical terms, and can convert spoken symbol words such as “underscore” or “slash” into their literal characters when clearly intended. |

Canonical supported values for `transformation.provider`:

- `anthropic`
- `cohere`
- `deepseek`
- `galadriel`
- `gemini`
- `groq`
- `huggingface`
- `hyperbolic`
- `mira`
- `mistral`
- `moonshot`
- `ollama`
- `openai`
- `openrouter`
- `perplexity`
- `together`
- `xai`

Environment variable fallback for `[transformation]`:

- `anthropic` → `ANTHROPIC_API_KEY`
- `cohere` → `COHERE_API_KEY`
- `deepseek` → `DEEPSEEK_API_KEY`
- `galadriel` → `GALADRIEL_API_KEY`
- `gemini` → `GEMINI_API_KEY` (also accepts `GOOGLE_API_KEY`)
- `groq` → `GROQ_API_KEY`
- `huggingface` → `HUGGINGFACE_API_KEY` (also accepts `HF_TOKEN`)
- `hyperbolic` → `HYPERBOLIC_API_KEY`
- `mira` → `MIRA_API_KEY`
- `mistral` → `MISTRAL_API_KEY`
- `moonshot` → `MOONSHOT_API_KEY`
- `openai` → `OPENAI_API_KEY`
- `openrouter` → `OPENROUTER_API_KEY`
- `perplexity` → `PERPLEXITY_API_KEY`
- `together` → `TOGETHER_API_KEY`
- `xai` → `XAI_API_KEY`
- `ollama` → no API key, but the current Rig client path expects `OLLAMA_API_BASE_URL`

`transformation.api_key` in the TOML takes precedence over the provider-specific API key environment variable.
There is currently no equivalent environment-variable fallback for `transformation.provider` or `transformation.model`.
If the `[transformation]` section is omitted or incomplete, the transformation feature is disabled, the transformation hotkey is not registered, and `transformation.auto` has no effect.

\* Required either in config or via environment.

## Run

For the non-blocking Terminal workflow, install the app bundle into `~/Applications` or `/Applications` and launch it with `open`:

```bash
open -g ~/Applications/simple-ptt.app
```

For app launches, keep configuration in `~/.config/simple-ptt/config.toml`.
Using shell environment variables such as `SIMPLE_PTT_CONFIG` or `DEEPGRAM_API_KEY` is fine for direct binary execution, but it is the wrong default for LaunchServices-based app launches because shell environment inheritance is not reliable there.

You can update settings from the app itself through the menu bar **Settings…** window. That window writes to the resolved config file path shown in the UI.

### CLI helpers

List available audio input devices from the installed app bundle:

```bash
~/Applications/simple-ptt.app/Contents/MacOS/simple-ptt --list-devices
```

There is also an internal helper used by the app-bundling script:

```bash
simple-ptt --write-app-iconset <output-dir>
```

That command is primarily for packaging and generates the iconset consumed by `scripts/build-macos-app.sh`.

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

What they do:

- `just run` / `just run-local`: runs `cargo run` with `SIMPLE_PTT_CONFIG=./config.toml`
- `just run-config <path>`: same, but with an explicit config path
- `just run-xdg`: runs without `SIMPLE_PTT_CONFIG`, so normal config-path lookup applies
- `just bundle-release`: builds the release binary and bundles `dist/simple-ptt.app`
- `just bundle-dmg`: builds `dist/simple-ptt.dmg`
- `just install-app`: installs the bundled app into `~/Applications` by default
- `just start`: launches the installed app with `open -g`
- `just list-devices`: runs `--list-devices` from the installed app bundle

## License

MIT. See [LICENSE](./LICENSE).
