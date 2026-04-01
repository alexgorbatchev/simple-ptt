# Native macOS Menu Bar Dictation App

## Overview

A minimal Rust-based macOS menu bar application that provides push-to-talk and hold-to-talk voice input. It streams microphone audio to Deepgram, writes the final transcript to the system clipboard, and synthesizes a native paste shortcut into the currently focused application. The app lives in the system menu bar as an `NSStatusItem` and targets the smallest practical RAM footprint.

## Platform

- **Language**: Rust
- **Target**: macOS (Apple Silicon + Intel)
- **Build**: `cargo build` — no Xcode IDE required (only Xcode Command Line Tools)
- **Packaging**: `.app` bundle (required for microphone permission via macOS TCC)

## Core Requirements

### 1. Menu Bar Presence

- Appears as an `NSStatusItem` in the macOS menu bar
- Displays a small icon (microphone or similar) and/or text indicator
- `LSUIElement = true` in Info.plist (no Dock icon, no main window)
- Menu items:
  - Status indicator (listening / transcribing / idle / error)
  - Quit

### 1.1 Live Transcription Overlay

- When recording starts, show a small floating overlay centered on the screen that currently contains the mouse pointer
- The overlay must remain visible while recording and while final transcription is still being resolved
- The overlay content must be left-aligned and line-wrapped
- The overlay must include a small gray footer that can display project month-to-date Deepgram spend
- Long transcripts must remain readable without growing into a full-screen surface; a fixed-size scrolling text area is acceptable
- The overlay must ignore mouse interaction so it does not steal focus from the target application

### 2. Global Hotkeys

- **Push-to-talk (toggle)**: Press a key once to start recording, press again to stop
- **Hold-to-talk**: Hold a key to record, release to stop
- **Abort**: Press `Escape` during recording or transcription finalization to discard the current utterance and prevent paste
- Both modes use `CGEventTap` (via `rdev` crate with `unstable_grab` feature) for:
  - keyDown / keyUp detection
  - Ability to swallow key events so they don't leak to other apps
- Requires Accessibility permission for the `rdev`/`CGEventTap` hotkey path

### 3. Microphone Capture

- Capture audio from the configured input device using `cpal` (CoreAudio backend)
- Stream mono PCM (`linear16`) audio to Deepgram over WebSocket for live transcription
- Requires `NSMicrophoneUsageDescription` in Info.plist

### 4. Deepgram Transcription + Paste Flow

- Open a Deepgram live transcription stream when recording starts
- Send live microphone audio while the hotkey is active
- Finalize and close the stream when recording stops
- Collect final transcript segments, join them into insertion text, copy that text to the macOS general pasteboard, and trigger a native paste shortcut into the focused application

### 5. Configuration

- Read configuration from `$SIMPLE_PTT_CONFIG` when explicitly set
- Otherwise read configuration from `$XDG_CONFIG_HOME/simple-ptt/config.yaml`
- If `XDG_CONFIG_HOME` is unset, fall back to `~/.config/simple-ptt/config.yaml`
- Configuration fields:
  - `hotkey`: key name string (for example `"F5"` or `"RightAlt"`)
  - `deepgram_api_key`: optional plaintext API key in config; `DEEPGRAM_API_KEY` is used as a fallback
  - `deepgram_project_id`: optional Deepgram project ID in config; `DEEPGRAM_PROJECT_ID` is used as a fallback
  - `deepgram_language`: default `"en-US"`
  - `deepgram_model`: default `"nova-3"`
  - `audio_device`: optional input device name or numeric index; default input device if omitted
  - `sample_rate`: preferred sample rate; falls back to a supported device rate if needed
  - `gain`: input gain multiplier before PCM encoding
  - `hold_ms`: threshold that distinguishes hold-to-talk from tap-to-toggle behavior
  - `overlay_font_name`: optional AppKit font name for the overlay text; system font if omitted or invalid
  - `overlay_font_size`: overlay text size in points
  - `overlay_footer_font_size`: optional footer text size in points; defaults to 60% of `overlay_font_size`
  - `endpointing_ms`: Deepgram endpointing window in milliseconds
  - `utterance_end_ms`: Deepgram utterance-end timeout in milliseconds
- Key names map to keycodes in code — no GUI needed for configuration

### 6. RAM Target

- Idle: ~3-8 MB RSS
- During recording: as low as possible (streaming audio, not buffering entire recordings)

## Non-Requirements (Explicitly Out of Scope)

- No settings GUI window (config file only)
- No Dock icon
- No Mac App Store distribution
- No auto-update mechanism
- No built-in local STT/TTS engine (speech recognition is handled by Deepgram)

## Dependencies (Rust Crates)

- `objc2` + `objc2-app-kit` + `objc2-foundation` — typed AppKit bindings (preferred over deprecated `cocoa`/`objc`)
- `cpal` — cross-platform audio input (CoreAudio on macOS)
- `rdev` (with `unstable_grab`) — global keyboard event capture with keyDown/keyUp
- `serde` + `serde_yaml` — config file parsing
- `deepgram` + `tokio` — live transcription over WebSocket
- Minimal other dependencies

## Build & Packaging

```bash
# Build
cargo build --release

# Assemble .app bundle (script provided)
./scripts/bundle.sh

# Ad-hoc code sign for local use
codesign --force --sign - --entitlements entitlements.plist \
  target/release/bundle/JarvisMenuBar.app
```

### .app Bundle Structure

```
JarvisMenuBar.app/
  Contents/
    Info.plist
    MacOS/
      jarvis-native       # compiled binary
    Resources/
      icon.png            # menu bar icon
```

### Info.plist Keys

- `CFBundleIdentifier`: `com.jarvis.native`
- `CFBundleName`: `Jarvis`
- `CFBundleExecutable`: `jarvis-native`
- `LSUIElement`: `true`
- `NSMicrophoneUsageDescription`: `"Jarvis needs microphone access for voice commands"`

### Entitlements

- `com.apple.security.device.audio-input`: `true`

## Permissions Required

1. **Accessibility** — for `rdev` global hotkey capture and synthetic paste events
2. **Microphone** — for audio capture

Both are prompted by macOS automatically on first use when the app is properly bundled and signed.
