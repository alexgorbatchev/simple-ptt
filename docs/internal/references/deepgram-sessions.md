---
created_on: 2026-10-03 09:21
last_modified: 2026-10-05 11:14
status: current
---

# Deepgram sessions

This reference is for maintainers changing transcription, cancellation, or its waiting indication.

## Dependency

The app uses [alexgorbatchev/deepgram-rust-sdk](https://github.com/alexgorbatchev/deepgram-rust-sdk), based on SDK 0.11.0, pinned to [05dac8affb5ea68948da7b7fc989956e46f05907](https://github.com/alexgorbatchev/deepgram-rust-sdk/commit/05dac8affb5ea68948da7b7fc989956e46f05907). The user requested that the SDK fix live in their GitHub fork. Keep Cargo.toml and Cargo.lock pinned to the reviewed commit.

The SDK worker gates its keep-alive timer after CloseStream and continues reading final responses. Its command receiver is gated while closing, too, so a closed receiver cannot starve the socket. Dropping a transcription stream aborts its audio bridge; dropping the bridge's WebsocketHandle aborts the socket worker. Keep-alive messages go directly to the socket rather than through the worker's own bounded command queue.

## Ownership and completion

`ActiveSession` in `src/transcription/session.rs` owns the audio sender, transcription task, `SessionProgress`, and a single-worker Tokio runtime. Finishing drops the sender to initiate CloseStream, then waits for the task, cancellation, or the finishing deadline. Cancellation and timeout abort and await the task before returning. Dropping an active session aborts the task instead of detaching it; its owned runtime also releases independently spawned session tasks and sockets.

Runtime destruction happens outside the async execution context and uses Tokio's normal `Drop`. [Tokio documents that this waits for blocking work to stop](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#shutdown). The connection and finishing deadlines bound those waits, rather than the subsequent runtime teardown.

The transcript reader accepts the final metadata (`TerminalResponse`) as completion after finishing starts. It does not wait for socket EOF. EOF without final metadata and metadata received during active recording are errors, so an incomplete result does not enter the normal paste path. Deepgram documents final results followed by metadata on [CloseStream](https://developers.deepgram.com/docs/close-stream).

`AppState::wait_for_abort` registers its notification before checking the abort latch. The session does not consume the latch: `finish_session` and `settle_session_finish` in the transcription worker consume it before any paste, transformation, or resume. This also catches an abort racing a successful result or a timeout. Finishing cancellation discards the recording and returns to idle. Cancellation during connection startup clears dictation, or retains the base annotation when starting a correction. Both paths clear the waiting indication and retain an overlay dismissed by the hotkey.

## Waiting indication

`SessionLimits` in `src/transcription/progress.rs` gives connection and finishing waits a 15-second limit. Each displays an indication after 5 seconds. Finishing uses an absolute deadline; additional responses do not extend it.

During live recording, accepted Linear16 audio above the microphone meter's existing floor starts a pending-response interval. Results acknowledge audio through `start + duration`, including results with empty transcripts. Progress resets the interval; acknowledging all pending audible audio clears it. Digital silence and audio below that floor do not start the interval. A live notice already displayed remains visible when finishing begins.

Live waiting is informational and does not stop recording. Audio level is not a speech classifier, so background sound can trigger the notice. Deepgram explicitly [sends no response to KeepAlive](https://developers.deepgram.com/docs/audio-keep-alive); absence of a keep-alive reply cannot diagnose a failed connection.

The status poll compares `deepgram_waiting` as part of `UiSnapshot`. Recording and processing display **Waiting…** beside the menu bar icon and **Waiting for Deepgram…  <ESC> cancel** in the existing overlay footer. The menu bar title remains available when an empty recording's overlay has already dismissed. The footer updates before the finishing layout hold, preserving its text selection, meter, and root scale. Other app states suppress the notice. Session guards clear the flag on completion, failure, cancellation, and task abortion.

## Verification

- The fork's `tests/websocket_shutdown_local.rs` uses real local WebSockets to exercise delayed final responses past the three-second timer, disabled keep-alives, and dropping quiet handles and streams.
- `src/transcription/progress.rs` tests live delays, recovery, quiet audio, absolute finishing deadlines, connection timeout, and cancellation of concurrent waits.
- `src/transcription/session.rs` tests terminal metadata without EOF, incomplete disconnection, final audio draining, cancellation during finishing, runtime transport teardown after cancellation and timeout, and dropping a quiet session.
- `src/transcription/mod.rs` tests cancellation state and annotation ownership. `src/app/mod.rs` tests status polling and title changes, including a dismissed overlay.
- `--overlay-snapshot` captures live and finishing notices over light and dark backdrops. The overlay skill's `references/findings.md` records native geometry and selection measurements.
- A native status item using the app's active microphone icon expands from 34 to 96 points when the waiting title is assigned, then returns to 34 points when cleared. A bitmap cached from the native button verifies that the icon and title render together.
