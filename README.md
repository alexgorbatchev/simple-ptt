# simple-ptt

![simple-ptt demo](./screen.gif)

A very basic push-to-talk utility for macOS.

The goal here is not to be feature-rich. The goal is to stay small, stay fast, and stay understandable. In normal use it aims to stay under roughly 25 MB of RAM, uses the Deepgram API for real-time transcription, and shows the live transcription on screen while you are still talking. If configured, you can also press a key to clean up the spoken text with an LLM pass.

I built this because I did not like the existing push-to-talk implementations. Everything felt too slow to respond to a hotkey, too slow to process and paste and none that I tried showed transcription in "real time". 

## What it does

- Global push-to-talk hotkey
- Real-time transcription with Deepgram
- On-screen overlay that updates while you speak
- Optional LLM cleanup pass before pasting buffered text
- Paste-oriented workflow for dropping transcribed text into whatever app you are using
- Minimal configuration, minimal runtime footprint

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
cargo build --release
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

This app bundle is not Apple-signed or notarized.

If macOS blocks it on first launch, use one of the following:

- run it once, then allow it in **System Settings > Privacy & Security**
- or remove the quarantine attribute manually:

```bash
xattr -dr com.apple.quarantine /path/to/simple-ptt.app
```

You should also expect macOS permission prompts for:

- **Microphone**
- **Accessibility** and/or **Input Monitoring** for the global push-to-talk hotkey and synthetic paste workflow

## Configuration

`simple-ptt` looks for configuration in this order:

1. `SIMPLE_PTT_CONFIG`
2. `$XDG_CONFIG_HOME/simple-ptt/config.toml`
3. `~/.config/simple-ptt/config.toml`

If no config file is found, defaults are used where possible. However, you still need to provide a Deepgram API key either in the config file or through the `DEEPGRAM_API_KEY` environment variable.

Start from the example file in this repository:

```bash
mkdir -p ~/.config/simple-ptt
cp config.example.toml ~/.config/simple-ptt/config.toml
```

### Example config

Typical flow with transformation enabled:

If `transformation.auto = true` (the default):

1. Press `F5` to start dictation.
2. Press `F5` again to stop dictation, transform the transcript, and paste the transformed result.

Optional shortcut:

- Press `F6` while recording to stop dictation and transform the current transcript without auto-pasting it. After that, press `F5` to paste the transformed buffer.

If `transformation.auto = false` or transformation is not configured:

1. Press `F5` to start dictation.
2. Press `F5` again to stop dictation and paste the raw transcript.

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

### Config values

#### `[ui]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `hotkey` | No | `F5` | Global push-to-talk key. |
| `font_name` | No | system default | Overlay font family name. |
| `font_size` | No | `12.0` | Main overlay font size. |
| `footer_font_size` | No | derived from `font_size` | Footer text font size. |
| `meter_style` | No | `animated-color` | Overlay mic meter style. Supported values: `animated-color`, `animated-height`, and `none`. |

#### `[mic]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `audio_device` | No | default input device | Exact input device name or numeric index. Use `simple-ptt --list-devices` to print the available inputs and their indices. |
| `sample_rate` | No | `16000` | Requested audio sample rate in Hz. |
| `gain` | No | `4.0` | Input gain multiplier. |
| `hold_ms` | No | `300` | Minimum hold duration for the push-to-talk hotkey in milliseconds. |

#### `[deepgram]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `api_key` | Yes* | none | Deepgram API key. Can also be provided via `DEEPGRAM_API_KEY`. |
| `project_id` | No | none | Optional Deepgram project ID. Can also be provided via `DEEPGRAM_PROJECT_ID`. |
| `language` | No | `en-US` | Deepgram language code. |
| `model` | No | `nova-3` | Deepgram transcription model. |
| `endpointing_ms` | No | `300` | Endpointing delay for transcription finalization. |
| `utterance_end_ms` | No | `1000` | Utterance end timeout in milliseconds. |

#### `[transformation]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `hotkey` | No | `F6` | Global key used to stop dictation and transform the current transcript before the final paste step. |
| `auto` | No | `true` | When `true` and transformation is configured, pressing the record hotkey while dictation is active stops dictation, transforms the transcript, and pastes the transformed result. When `false`, the record hotkey keeps the raw paste behavior. |
| `provider` | No | none | Rig provider name for the transformation request. See the canonical supported values below. |
| `api_key` | No | none | API key used for the transformation provider. If omitted, the app falls back to the provider-specific environment variables listed below. |
| `model` | No | `gpt-5.4-mini` | Model name used for the transformation request. |
| `system_prompt` | No | built-in prompt | Optional override for the built-in transformation prompt. |

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
- `ollama` → no API key, but the current Rig-based client path expects `OLLAMA_API_BASE_URL`

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

To print the available audio input devices and their numeric indices from the installed app bundle:

```bash
~/Applications/simple-ptt.app/Contents/MacOS/simple-ptt --list-devices
```

## Development

For the checked-in development config in this repository:

```bash
just run
```

For the bundled app workflow used by releases:

```bash
just bundle-release
just bundle-dmg
open -g dist/simple-ptt.app
```

## License

MIT. See [LICENSE](./LICENSE).
