---
created_on: 2026-09-30 11:51
last_modified: 2026-09-30 14:20
status: current
---

# The pill meter

This reference is for anyone changing the overlay's pill meter (`ui.meter_style = "pills"`). It records what the user asked for, in their words, and which part of the design answers each request, so that no part is "simplified" away without knowing what it protects. Keep it current with the code.

The pills are a picture of someone talking, not a waveform. Accuracy is not a goal. What matters is what the user sees: the pills respond at once to the voice, speech is obvious at a glance, and the room's noise stays quiet.

## Requirements

Each requirement is quoted from the user and followed by what implements it.

### R1. The pills respond at once, like Codex's voice meter

> "the only issue is that the pills indicating speach feel delayed by noticable amount, why?"
>
> "lets drop VAD all together implement codex algo"

Codex's meter (`openai/codex`, `codex-rs`) keeps the loudest sample since its last read (`record_peak` in `voice-host/src/devices.rs`), reads and resets it every 100 ms (`take_microphone_peak` in `realtime-webrtc/src/session.rs`), and draws it at once. The pills do the same, with the adaptive range (R2) and texture (R4) the user chose to keep:

- One pill stands for 100 ms (`PILL_SECONDS`), Codex's interval. The user chose it over the earlier 0.2 s ("100 ms like Codex"), which doubles the strip's speed to 67.5 pt/s.
- A pill's loudness is the loudest sample of its time (`pill_loudness`): the peak of each captured block, which `encode_pcm_mono` in `src/audio/devices.rs` already measures after the gain.
- The audio callback in `src/audio/stream.rs` queues each block's peak and time span (`LevelBlock`) through a bounded channel with `try_send`, which never waits, so the real-time thread stays real-time; a full queue drops blocks. The overlay drains it on each update (`AppState::level_blocks` in `src/state.rs`), keeping 2 s.
- There is no speech classifier. A pill is painted as soon as its audio is in, at most `LATEST_PAINT_SECONDS` (0.225 s) after its time begins: its own 0.1 s, 50 ms for the last block of its time to arrive, and one overlay update (75 ms). It enters the column at `ENTRY_DELAY_SECONDS` (0.255 s), 30 ms later, for the paint to reach the screen. With the speech classifier the entry delay was 0.7 s (see Rejected approaches).
- The 50 ms for the last block covers a callback block of up to 800 frames at 16 kHz. The capture uses the device's default buffer size, which has not been measured; a longer block makes pills rest as dots rather than change in view (R3).

### R2. Soft speech in a quiet office fills the pills

> "im in an office and i speak softly, the transpription works fine, but pills are barely ticking up, we need some kind of adaptive system here maybe?"

`LoudnessRange` in `src/ui_meter/pill_levels.rs` adapts to the room and the speaker instead of Codex's fixed scale (a peak of 512 to 8192 in 65535, -42 to -18 dBFS), which reads a soft voice low. The user chose to keep it ("Codex peak, adaptive range"):

- **Noise floor:** the quietest pill of the last 10 s (`floor_seconds`), starting from Codex's noise floor (-42 dBFS) as a prior. At 100 ms a pill, the gaps between words reach the room's noise, so the floor stays at the room however long someone talks, and follows the room within 10 s when it gets louder.
- **Gate:** pills up to 6 dB above the floor (`noise_gate_db`, about twice its amplitude) rest as dots. A voice that starts before the room has been heard shows once it is 6 dB above the -42 dBFS prior; softer speech shows from the first gap between words.
- **Top of the range:** the 90th percentile (`top_quantile`) of the pills above the gate in the last 10 s (`top_seconds`), at least 12 dB above the gate (`min_span_db`). Pills map linearly in amplitude from the gate to the top, so one loud word does not shrink the speech around it.
- **No audio:** a stretch in which the microphone delivered nothing, and digital silence (peaks below -90 dBFS, a device delivering zeros), teach the range nothing (`PillLoudness::NoAudio`). Counted as a silent room, either would drop the floor so far that every sound would clear the gate.

### R3. Pills already drawn never change

> "we need to make sure that painted pills remain stable, eg if i start softly but then get louder"
>
> "when speech is detected the pills that were already in view all of a sudden expand from 0 to 100, users shouldnt see this expansion effect, the pills sliding in from the side should already have the right height"

- A pill's height is set once, from its own 100 ms, and never recomputed when the range changes later.
- The strip is drawn `ENTRY_DELAY_SECONDS` to the right of the column (`right_anchor` in `src/ui_meter/pill_strip.rs`), so a pill enters the column after its latest paint. The pool of recycled layers covers that extra distance (`pool_size`).
- A pill not painted by `LATEST_PAINT_SECONDS` rests as a dot for good, even if its audio arrives later.

Evidence: a probe of the debug tuner sampled every pill layer every 4 ms for 6.5 s. In each of two runs, all 55 height changes happened while the pill was still right of the column, and none inside it, the fade included. With the entry delay disabled, all 55 happened inside the column, the growth the user reported (for example a dot growing from 4.75 to 12.35 pt at x 498.6 of 514).

### R4. Speech reads as a strong up-and-down texture

> "i would like there to be a strong visual indication in the pill wave of speech (eg up/down segments close by together, not just a continuous highs in a single block), i dont specifically care about actual accuracy, this is visualisation only for the user, not wave form analysis if that makes sense"

- A pill above the gate is drawn `texture(index) × (0.5 + 0.5 × loudness)` (`loudness_share`). Loudness only scales the pattern; it does not set it. The user chose to keep the texture over Codex's plain levels ("Keep the texture").
- `texture` alternates by pill index between a tall band (0.75 to 1.0) and a short band (0.15 to 0.4), with a height within the band from a hash of the index. That way it is random-looking but the same every time the pill is drawn.
- Gaps between words are dots, which breaks the zigzag into word-length runs. Without a speech classifier, any sound above the gate zigzags too, not only speech.

Evidence: on screen, the tuner's last probe showed `▇▃▆▃·▃▆▃▆▃·▃▆▄▆▃·▃▆▄···▄▇▃█▄▅▄▆▄█▃▅▃█▄▇▄▅▃█▄···`: 56 of 58 pairs of neighbouring raised pills differed by 2 pt or more, and 3 of 65 raised pills reached full height.

### R5. Pills move smoothly, fading at the edges

> "the pills movement is very jerky because we essentialy keep pills static and just resize to advance... lets make it so that the pill stripe visually moves at the speech pace from right to left, on right and left we should have a fadeout effect of maybe 25px... make this is implemented efficiently, dont make an image 100000px wide and move it"

- The strip moves by one linear Core Animation translation that lasts 24 hours and has no timing function, so the render server moves it at the display's rate and the app does nothing per frame. Even the linear `CAMediaTimingFunction` put the strip up to 28 pt off (see `findings.md` in the `overlay-visual-debugging` skill).
- Only enough pill layers to cover the column and the way in exist. A layer that leaves on the left becomes the next pill on the right.
- The column fades pills out over 25 pt on the left (`EXIT_FADE_WIDTH`) and in over 8 pt on the right (`ENTRY_FADE_WIDTH`), through a gradient mask. The right-hand fade was 25 pt too, but pills enter final (R3), so it only delayed them. The user asked for 8 pt after the pills felt late ("8 pt and keep 25 pt -> sure"). At 67.5 pt/s, a pill is fully shown 0.19 s after it enters.
- With Reduce Motion on, the strip steps one pill spacing at a time instead of gliding.

### R6. The values can be tuned against the real room

> "it's showing ambient noise currently too , can we add controls for it to the debug window so i can tune?"

- Every value above that decides what counts as noise, and how the texture looks, is a field of `PillTuning` in `src/ui_meter/pill_tuning.rs`, and its `Default` holds the values in use: `noise_gate_db`, `min_span_db`, `floor_seconds`, `top_quantile`, `top_seconds`, `loudness_share`, and the two bands (`tall_low`, `tall_high`, `short_low`, `short_high`).
- Debug mode (`just debug-overlay`) has a slider for each, in its pills column. A new value applies to the pills painted from then on; pills already drawn keep their height (R3).
- Its voice picker drives the meter from the synthetic voice or from the microphone. The microphone is the one in the user's `[mic]` config, captured through the app's own `AudioController`, so the room's real noise drives the pills. An idle transcription worker takes the dictation audio and drops it, because no session starts.
- A readout shows the noise floor, the gate, the top of the range, and the latest block's peak, in dBFS, so a sound that raises the pills can be compared with the gate. Its digits have one width and its width is fixed, so its changing numbers do not move the controls.
- "Copy values" copies the pill values with the glass values. The values the user settles on become the `PillTuning` defaults.

## Where the parts live

| Part | File |
| --- | --- |
| Heights, range, texture, paint deadline | `src/ui_meter/pill_levels.rs` |
| Strip motion, entry delay, layer pool, fades | `src/ui_meter/pill_strip.rs` |
| The tunable values and their defaults | `src/ui_meter/pill_tuning.rs` (`PillTuning`) |
| Converting the loudness to the strip's clock | `src/ui_meter.rs` (`push_pill`) |
| Each block's peak, queued from the audio callback | `src/audio/stream.rs` |
| The loudness queue, and the loudness kept (2 s) | `src/state.rs` (`level_sender`, `level_blocks`) |
| Synthetic voice and live microphone for debug mode | `src/overlay/dev/tuner.rs` (`publish_voice`, `Microphone`) |

## Rejected approaches

- **A speech classifier gating the pills.** Apple's SoundAnalysis classifier (`SNClassifySoundRequest`, 0.5 s windows at 50% overlap) told speech from other sound well: speech from `say` in 30 of 30 windows, and loud noise, quiet hiss, keyboard-like clicks and hum in 0 of 15 each, at about 0.7% of one core. But a pill had to wait for a window covering its end, up to about 0.45 s, so pills entered the column 0.7 s after their time began. The user found it "delayed by noticable amount" and chose Codex's approach instead.
- **WebRTC VAD.** Run at 16 kHz on 20 ms frames, most aggressive, it judged speech from `say` 98% voiced, but also loud white noise 100% and keyboard-like clicks 60%.
- **Growing pills in the right-hand fade.** An earlier version let pills get their height in the 25 pt fade, assuming the fade hid the change. It does not: the user saw pills grow from dots to full height.
- **The smoothed meter level in dBFS, peak per pill, with a top that rises at once to the loudest speech.** Most speech pills pinned at full height (85% at 95% or more in a simulation of speech from `say`), so speech read as one solid block.
- **Codex's fixed scale.** A peak of 512 to 8192 in 65535 (-42 to -18 dBFS) reads a soft voice in a quiet office low, the first complaint (R2).
- **Loudness alone, without the texture.** Taking each pill's peak on the adaptive range left 35% of the pills (0.2 s each) from the same recording at 95% of full height or more, so continuous speech read as a block again (R4).
