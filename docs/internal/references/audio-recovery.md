---
created_on: 2026-10-08 11:52
last_modified: 2026-10-08 11:52
status: current
---

# Audio stream recovery

This reference describes microphone disconnection and reconnection for contributors working on `src/audio/`.

## Callback health

Each input stream owns a `StreamActivity`. Its callback timestamps and watchdog use the same monotonic `Instant` origin. Separate clock origins can hide a stalled stream by making its latest callback appear to be in the watchdog's future.

A successful play starts a 1.5-second callback deadline, including when no first callback arrives. Repeated play requests do not extend that deadline. Pausing disables the deadline; resuming starts a fresh one. The callback records activity using an atomic store without taking a lock.

`AudioController::sync_stream_state` checks health during recording as well as while idle. A missing, failed, or stalled stream interrupts recording with an overlay error, leaving the displayed transcript intact. Idle recovery can then rebuild the input and update the transcription sample rate. The record shortcut starts the next recording.

## Device notifications

Core Audio device-list and default-input notifications increment a shared generation counter. Each controller acknowledges the generation captured before it begins choosing and building a replacement stream, after installing that stream. A notification that arrives during the rebuild remains pending for the next check. Failed builds do not acknowledge notifications.

Device selection remains governed by `BuiltInPreference`: a lost input prefers the built-in microphone, a configured named input is used again when connected, and an unspecified input follows the system default subject to that preference.

## Verification

Unit tests cover callback loss, missing first callbacks, pause/resume deadlines, repeated play requests, a reconnect during a rebuild, and recording interruption without discarding the transcript. Temporarily disabling the corresponding fixes must make these regressions fail.

These tests do not physically unplug a microphone or exercise a native replacement stream. Verify headphones by disconnecting and reconnecting them while idle and during recording, with **Keep microphone connection open** both enabled and disabled. After reconnecting, the record shortcut must capture audio again without relaunching the app.
