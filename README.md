# simple-ptt

A very basic push-to-talk utility for macOS.

The goal here is not to be feature-rich. The goal is to stay small, stay fast, and stay understandable. In normal use it aims to stay under roughly 25 MB of RAM, uses the Deepgram API for real-time transcription, and shows the live transcription on screen while you are still talking.

I built this because I did not like the existing push-to-talk implementations. The main differentiator here is simple: you can actually see what is being transcribed while you speak.

## What it does

- Global push-to-talk hotkey
- Real-time transcription with Deepgram
- On-screen overlay that updates while you speak
- Paste-oriented workflow for dropping transcribed text into whatever app you are using
- Minimal configuration, minimal runtime footprint

## Platform

This project currently targets **macOS on Apple Silicon (arm64)**.

## Install

### Option 1: Download a release binary

1. Open the latest GitHub release.
2. Download the `simple-ptt-vX.Y.Z-macos-arm64.zip` asset.
3. Unzip it.
4. Enter the extracted directory and move `simple-ptt` somewhere convenient, for example:

```bash
cd simple-ptt-vX.Y.Z-macos-arm64
mkdir -p ~/.local/bin
mv simple-ptt ~/.local/bin/
```

If `~/.local/bin` is not already on your `PATH`, add it in your shell profile.

### Option 2: Build from source

Requirements:

- Rust toolchain
- macOS

Build:

```bash
cargo build --release
```

The binary will be written to:

```text
target/release/simple-ptt
```

## First run on macOS

This binary is not Apple-signed or notarized.

If macOS blocks it on first launch, use one of the following:

- run it once, then allow it in **System Settings > Privacy & Security**
- or remove the quarantine attribute manually:

```bash
xattr -d com.apple.quarantine /path/to/simple-ptt
```

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

```toml
[ui]
hotkey = "F5"
# font_name = "Menlo"
font_size = 12.0
footer_font_size = 10.0

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
```

### Config values

#### `[ui]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `hotkey` | No | `F5` | Global push-to-talk key. |
| `font_name` | No | system default | Overlay font family name. |
| `font_size` | No | `12.0` | Main overlay font size. |
| `footer_font_size` | No | derived from `font_size` | Footer text font size. |

#### `[mic]`

| Key | Required | Default | Description |
| --- | --- | --- | --- |
| `audio_device` | No | default input device | Exact input device name or numeric index. |
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

\* Required either in config or via environment.

## Run

If you installed the binary onto your `PATH`:

```bash
simple-ptt
```

If you want to point at a custom config file:

```bash
SIMPLE_PTT_CONFIG=/path/to/config.toml simple-ptt
```

If you prefer environment variables for secrets:

```bash
DEEPGRAM_API_KEY=your_key_here simple-ptt
```

## Development

Run locally with the checked-in development helper commands:

```bash
just run
```

or:

```bash
cargo run
```

## License

MIT. See [LICENSE](./LICENSE).
