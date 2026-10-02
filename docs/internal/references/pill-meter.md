---
created_on: 2026-09-30 11:51
last_modified: 2026-10-02 14:17
status: current
---

# The pill meter

This reference is for anyone changing the overlay's pill meter (`ui.meter_style = "pills"`). It records what the user asked for, in their words, and which part of the design answers each request, so that no part is "simplified" away without knowing what it protects. Keep it current with the code.

The pills are a picture of someone talking, not a waveform. Accuracy is not a goal. What matters is what the user sees: the pills respond at once to the voice, speech reads as a lively, varied up-and-down, and the room's noise stays quiet and still.

## Requirements

Each requirement is quoted from the user and followed by what implements it.

### R1. The pills respond at once, like Codex's voice meter

> "the only issue is that the pills indicating speach feel delayed by noticable amount, why?"
>
> "lets drop VAD all together implement codex algo"

Codex's meter (`openai/codex`, `codex-rs`) samples its microphone every 100 ms (`MICROPHONE_METER_INTERVAL` in `tui/src/chatwidget/realtime.rs`) and draws each sample at once, with no speech detection. The pills follow its pace and its immediacy:

- One pill stands for 100 ms (`PILL_SECONDS`), Codex's interval. The user chose it over the earlier 0.2 s ("100 ms like Codex").
- The audio callback in `src/audio/stream.rs` queues each block's loudness and time span (`LevelBlock`) through a bounded channel with `try_send`, which never waits, so the real-time thread stays real-time; a full queue drops blocks. The overlay drains it on each update (`AppState::level_blocks` in `src/state.rs`), keeping 2 s.
- There is no speech classifier. A pill is decided as soon as its audio is in, at most `LATEST_PAINT_SECONDS` (0.225 s) after its time begins: its own 0.1 s, 50 ms for the last block of its time to arrive, and one overlay update (75 ms). With the speech classifier the pills entered the column 0.7 s after their time (see Rejected approaches).
- The 50 ms for the last block covers a callback block of up to 800 frames at 16 kHz. The capture uses the device's default buffer size, which has not been measured; a later block makes its pill count as quiet rather than change in view (R3).

### R2. Soft speech in a quiet office fills the pills

> "im in an office and i speak softly, the transpription works fine, but pills are barely ticking up, we need some kind of adaptive system here maybe?"

`LoudnessRange` in `src/ui_meter/pill_levels.rs` adapts to the room and the speaker instead of a fixed scale such as Codex's (a peak of 512 to 8192 in 65535, -42 to -18 dBFS), which reads a soft voice low. The user chose to keep it ("Codex peak, adaptive range"):

- **Noise floor:** the quietest pill of the last 10 s (`floor_seconds`), starting from -50 dBFS as a prior. At 100 ms a pill, the gaps between words reach the room's noise, so the floor stays at the room however long someone talks, and follows the room within 10 s when it gets louder.
- **Gate:** pills up to 6 dB above the floor (`noise_gate_db`, about twice its amplitude) are quiet. A voice that starts before the room has been heard shows once it is 6 dB above the -50 dBFS prior; softer speech shows from the first gap between words.
- **Top of the range:** the 90th percentile (`top_quantile`) of the pills above the gate in the last 10 s (`top_seconds`), at least 12 dB above the gate (`min_span_db`). Pills map linearly in amplitude from the gate to the top, so one loud word does not shrink the speech around it.
- **No audio:** a stretch in which the microphone delivered nothing, and digital silence (below -90 dBFS RMS, a device delivering zeros), teach the range nothing (`PillLoudness::NoAudio`). Counted as a silent room, either would drop the floor so far that every sound would clear the gate.

### R3. Pills already drawn never change

> "we need to make sure that painted pills remain stable, eg if i start softly but then get louder"
>
> "when speech is detected the pills that were already in view all of a sudden expand from 0 to 100, users shouldnt see this expansion effect, the pills sliding in from the side should already have the right height"

- A decided pill takes the strip's next slot (R5) and its height is never recomputed, whatever the range does later.
- Slot `s` enters the column when the strip's clock reaches `s × PILL_SECONDS + ENTRY_DELAY_SECONDS` (0.255 s): the strip is drawn that far to the right of the column (`right_anchor` in `src/ui_meter/pill_strip.rs`), and the pool of recycled layers covers the extra distance (`pool_size`).
- A pill is put only in a slot that is not in view yet (`push_slot`). If the strip has already brought the next slot into view, that slot rests as a dot and the pill takes the first slot out of view, so nothing grows on screen.

Evidence: a probe of the debug tuner sampled every pill layer every 4 ms for 6.5 s, over a synthetic voice with a 1.5 s silence. In each of two runs, all height changes (44 and 47) happened while the slot was still right of the column, and none inside it, the fade included. With the strip anchored at the column's right edge instead, all 47 happened inside it, the growth the user reported.

### R4. Speech reads as a varied up-and-down texture

> "i would like there to be a strong visual indication in the pill wave of speech (eg up/down segments close by together, not just a continuous highs in a single block), i dont specifically care about actual accuracy, this is visualisation only for the user, not wave form analysis if that makes sense"
>
> "the pills dont looks good at all now, it's basically 3 sizes including 0 volume... so i only see 2 steps when there's speech"

- A pill's loudness is the RMS of its audio (`pill_loudness`), weighted by the length of each block whose middle falls in its time. The user chose it over Codex's loudest sample ("RMS + continuous texture").
- A pill above the gate is drawn `texture(slot) × (0.5 + 0.5 × loudness)` (`loudness_share`). `texture` alternates by slot between a tall band (0.6 to 1.0) and a short band (0.2 to 0.6), which meet, at a height within the band from a hash of the slot. That way it is random-looking but the same every time the slot is drawn.
- Gaps between words are dots, which breaks the zigzag into word-length runs. Without a speech classifier, any sound above the gate zigzags too, not only speech.

Evidence, from 8 s of speech recorded with `say` plus -58 dBFS noise, run through the pill model in 512-frame blocks:

| Measure | Pills in the top two tenths of the range | Spread of the drawn heights over tenths |
| --- | --- | --- |
| Loudest sample, bands 0.75–1.0 and 0.15–0.4 | 67% (loudness alone) | two clusters: the bands' tops |
| RMS, bands 0.6–1.0 and 0.2–0.6 | 46% (loudness alone) | 0, 3, 14, 11, 10, 10, 11, 11, 6, 2 (of 78) |

With the RMS and the meeting bands, 74 of 77 neighbouring speech pills differed by 0.1 of full height or more.

### R5. The strip moves only while there is sound, smoothly

> "the pills movement is very jerky because we essentialy keep pills static and just resize to advance... lets make it so that the pill stripe visually moves at the speech pace from right to left, on right and left we should have a fadeout effect of maybe 25px... make this is implemented efficiently, dont make an image 100000px wide and move it"
>
> "maybe instead of animating the pills all the time, we only animate the side movement when there's speech"

- The strip moves by one linear Core Animation translation that lasts 24 hours and has no timing function, so the render server moves it at the display's rate and the app does nothing per frame. Even the linear `CAMediaTimingFunction` put the strip up to 28 pt off (see `findings.md` in the `overlay-visual-debugging` skill).
- It moves only while there is sound (`PillLevels::moving`). A pill above the gate starts it; quiet pills scroll by as dots until `pause_after_seconds` (0.4 s, 4 pills) of them in a row stop it, and quiet pills are then skipped until the next sound. Long silences collapse to 4 dots; gaps between words stay. The user chose this ("Stop after a short pause").
- Before the first sound the strip is empty, not a row of dots ("lets not show the leading empty line before any speech starts"): a slot never filled has no pill (`slot_heights` gives `None`), and its layer is hidden. Dots show only for quiet pills the moving strip took in. A probe of the tuner found all 101 pill layers hidden until the synthetic voice's first sound, then one more shown per slot filled.
- Stopping and starting pause and resume the strip layer's timing as Apple's Technical Q&A QA1673 does (`PillStrip::set_moving`): `speed` 0 with the paused local time in `timeOffset`, then `speed` 1 with `beginTime` taking up the time spent paused. The strip's clock is its local time since it started, so it runs only while the strip moves, and its one animation carries on where it stopped.
- Each pill is 3.325 pt wide, 30% narrower than the first 4.75 pt ("lets make the pills 30% narrower too"), with its corner radius scaled the same (1.4 pt), 2 pt apart. One pill spacing per 100 ms makes the strip move at 53.25 pt/s.
- Only enough pill layers to cover the column and the way in exist. A layer that leaves on the left becomes the next slot on the right.
- The column fades pills out over 25 pt on the left (`EXIT_FADE_WIDTH`) and in over 8 pt on the right (`ENTRY_FADE_WIDTH`), through a gradient mask. The right-hand fade was 25 pt too, but pills enter final (R3), so it only delayed them; the user chose 8 pt ("8 pt and keep 25 pt -> sure").
- With Reduce Motion on, the strip steps one slot spacing at a time instead of gliding, and stops the same way.

Evidence: in the same probe the strip held still from 4.96 to 6.00 s and from 5.03 to 6.00 s in the two runs, for a silence from 4.5 to 6.0 s, and moved at 53.22 and 53.25 pt/s over the moving stretches before and after the pause (53.25 expected; the probe samples the screen once per display frame). With the pause disabled it never held still.

### R6. The values can be tuned against the real room

> "it's showing ambient noise currently too , can we add controls for it to the debug window so i can tune?"

- Every value above that decides what counts as noise, when the strip stops, and how the texture looks, is a field of `PillTuning` in `src/ui_meter/pill_tuning.rs`, and its `Default` holds the values in use: `noise_gate_db`, `min_span_db`, `floor_seconds`, `top_quantile`, `top_seconds`, `loudness_share`, `pause_after_seconds`, and the two bands (`tall_low`, `tall_high`, `short_low`, `short_high`).
- Debug mode (`just debug-overlay`) has a slider for each, in its pills column. A new value applies to the pills decided from then on; slots already filled keep their height (R3).
- Its voice picker drives the meter from the synthetic voice, which has a 1.5 s silence every 6 s for the strip to stop in, or from the microphone. The microphone is the one in the user's `[mic]` config, captured through the app's own `AudioController`, so the room's real noise drives the pills. An idle transcription worker takes the dictation audio and drops it, because no session starts.
- A readout shows the noise floor, the gate, the top of the range, and the latest block's level, in dBFS, so a sound that raises the pills can be compared with the gate. Its digits have one width and its width is fixed, so its changing numbers do not move the controls.
- "Copy values" copies the pill values with the glass values. The values the user settles on become the `PillTuning` defaults.

## Where the parts live

| Part | File |
| --- | --- |
| Loudness, range, texture, decisions, slots, when the strip moves | `src/ui_meter/pill_levels.rs` |
| Strip motion and its pause, entry delay, layer pool, fades | `src/ui_meter/pill_strip.rs` |
| The tunable values and their defaults | `src/ui_meter/pill_tuning.rs` (`PillTuning`) |
| The recording's clock, the strip's clock, and the loudness onto them | `src/ui_meter.rs` (`push_pill`, `render_pills`) |
| Each block's loudness, queued from the audio callback | `src/audio/stream.rs` |
| The loudness queue, and the loudness kept (2 s) | `src/state.rs` (`level_sender`, `level_blocks`) |
| Synthetic voice and live microphone for debug mode | `src/overlay/dev/tuner.rs` (`publish_voice`, `Microphone`) |

## Rejected approaches

- **A speech classifier gating the pills.** Apple's SoundAnalysis classifier (`SNClassifySoundRequest`, 0.5 s windows at 50% overlap) told speech from other sound well: speech from `say` in 30 of 30 windows, and loud noise, quiet hiss, keyboard-like clicks and hum in 0 of 15 each, at about 0.7% of one core. But a pill had to wait for a window covering its end, up to about 0.45 s, so pills entered the column 0.7 s after their time began. The user found it "delayed by noticable amount" and chose Codex's approach instead.
- **WebRTC VAD.** Run at 16 kHz on 20 ms frames, most aggressive, it judged speech from `say` 98% voiced, but also loud white noise 100% and keyboard-like clicks 60%.
- **Each pill's loudest sample (Codex's measure).** Through speech it barely changes from one 100 ms to the next: 67% of the speech pills sat in the top two tenths of the range, so the texture's two bands were all that showed (R4).
- **Growing pills in the right-hand fade.** An earlier version let pills get their height in the 25 pt fade, assuming the fade hid the change. It does not: the user saw pills grow from dots to full height.
- **The smoothed meter level in dBFS, peak per pill, with a top that rises at once to the loudest speech.** Most speech pills pinned at full height (85% at 95% or more in a simulation of speech from `say`), so speech read as one solid block.
- **Codex's fixed scale.** A peak of 512 to 8192 in 65535 (-42 to -18 dBFS) reads a soft voice in a quiet office low, the first complaint (R2).
