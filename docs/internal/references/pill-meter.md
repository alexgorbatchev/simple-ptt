---
created_on: 2026-09-30 11:51
last_modified: 2026-10-03 08:24
status: current
---

# The pill meter

This reference is for anyone changing the overlay's pill meter (`ui.meter_style = "pills"`). It records what the user asked for, in their words, and which part of the design answers each request, so that no part is "simplified" away without knowing what it protects. Keep it current with the code.

The pills are a live picture of the voice, not a measurement. What matters is what the user sees: the pills respond at once, show the voice's changing spectrum, and stay empty while the room is quiet.

When F5 finishes nonempty dictation for pasting, audio capture stops and the overlay retains its last displayed text, meter, and layout while transformation runs. The whole presentation scales to 80% and fades together; removing the meter during that wait would move the text. An empty overlay fades out immediately without that hold. F6 continues capturing audio and updating the spectrum at full overlay size while transformation runs.

## Requirements

Each requirement is quoted from the user and followed by what implements it.

### R1. A centred, live spectrum of the voice

> "Center the pills. The pills should expand up and down dynamically and responsively. The height of the pills should change from left to right, creating a spectrograph-like movie effect that responds to the voice."

The user chose a real frequency spectrum, low pitches on the left ("Frequency spectrum"), in a centred cluster ("Centered cluster"), replacing the scrolling strip ("Replace it"):

- `SpectrumAnalyzer` in `src/audio/spectrum.rs` runs on the real-time audio thread. On every captured block it takes a Hann-windowed FFT (`realfft`, added with the user's approval) of the last 64 ms: 1024 samples at 16 kHz. It sums the power into `SPECTRUM_BANDS` (40) bands evenly spaced in mel from 100 Hz to 8 kHz, or to half the sample rate if lower, each at least one FFT bin and none overlapping. A band's level is its summed power in dB, scaled so a full-scale sine reads about 0 dB (the Hann window's equivalent noise bandwidth is 1.5 bins). Every buffer is made when the stream is built, and `process_with_scratch` allocates nothing, so the callback stays real-time.
- The callback queues each `SpectrumFrame` (the bands' levels and centre frequencies) through a bounded channel with `try_send`, which never waits; a full queue drops frames. The overlay takes the frames queued since its last update (`AppState::take_spectrum_frames`).
- `PillCluster` in `src/ui_meter/pill_cluster.rs` draws one capsule per band, 3.325 pt wide and 2 pt apart (closing up if the meter is narrower than the 211 pt they need), centred in the meter's cluster, growing up and down from the middle line (`centred_bar_frame`).
- Each update moves every pill to its new height with Core Animation's implicit animation over the time since the last update (linear timing), so the pills glide between the overlay's 75 ms updates. With Reduce Motion on, or after a gap of 0.15 s or more, they move at once.

Evidence: a probe of the debug tuner's synthetic voice, sampling the pill layers every 20 ms, found the 40 pills centred (23.26 pt on each side of a 257.52 pt meter). Each band took 30 to 72 distinct heights in 2.4 s of speech, against 32 updates in that time, and the spectrum's shape followed the voice's drifting formants:

```
3.20s ▄▄▄▅▅▅▇██▇▇▆▅▅▄▄▃▃▃▃▃▃▃▃▂▂▂▂▂▂▂▂▂▂▂▁▂▁▁▁
4.40s ▅▅▅▆▆▆▇██▇▆▅▅▅▅▅▆▆▆▆▅▅▅▄▄▄▃▃▃▃▃▃▃▃▃▃▂▂▂▂
7.60s ▅▅▅▅▆▇██▇▇▇▆▆▅▄▄▄▄▄▄▄▃▃▃▃▃▃▃▃▃▃▃▂▃▂▂▂▂▂▂
```

### R2. The pills respond at once

> "the only issue is that the pills indicating speach feel delayed by noticable amount, why?"
>
> "lets drop VAD all together implement codex algo"

- There is no speech classifier: every sound above the room's noise shows, as in Codex's voice meter, which draws its microphone level every 100 ms with no speech detection.
- A pill rises at once to its band's level and falls at `fall_per_second` (2.5 heights a second), like an equalizer's (`PillLevels::update`).
- From sound to pill: up to 64 ms for the FFT window to fill with it, the callback block, and the overlay's next update (75 ms), then the glide over the next 75 ms.

### R3. Soft speech in a quiet office fills the pills

> "im in an office and i speak softly, the transpription works fine, but pills are barely ticking up, we need some kind of adaptive system here maybe?"

`PillLevels` in `src/ui_meter/pill_levels.rs` draws each band against a range that adapts to the room and the speaker:

- **Each band's noise floor** is its quietest level of the last 10 s (`floor_seconds`). The room's hum and hiss differ by band, so each band learns its own.
- **Each band's gate** is 12 dB above its floor (`noise_gate_db`).
- **The top**, shared by every band so the voice's spectral shape shows, is the 98th percentile (`top_quantile`) of the band levels above their gates over the last 10 s (`top_seconds`), at least 24 dB above each band's gate (`min_span_db`).
- **Each pill rests 30 dB below the top** (`range_db`), or at its gate if that is higher, and maps linearly in dB between. A soft voice and a loud one thus fill the pills alike, as long as the voice's range clears the gate.
- **A tilt** of 3 dB per octave above 1 kHz, taken away below (`tilt_db_per_octave`), lifts the voice's quieter high pitches.

### R4. The room stays quiet, and the row is empty before speech

> "it's showing ambient noise currently too , can we add controls for it to the debug window so i can tune?"
>
> "lets not show the leading empty line before any speech starts"

- Each update averages the power of the frames since the last one, usually 2 or 3, before learning and drawing (`PillLevels::averaged`). The narrow low bands cover a few FFT bins each, and their noise jumps by up to 9 dB from frame to frame; averaged, a single-frame spike stays under the gate.
- A band's floor has no starting value: it is the quietest level heard, so the first frames of the room set it. The cost is that a voice from the very first frame shows only from its first gap, a syllable or two in.
- Digital silence (a device delivering zeros, -130 dB or below) teaches the range nothing.
- A pill fades in over the first tenth of its height, so it grows in rather than popping up, and a faint flicker stays faint. A pill at rest is hidden, so the row is empty in silence.

Evidence, from 8 s of speech recorded with `say`, after 3 s of -58 dBFS white noise (an office's) and before 2 s more, run through the analyzer and the pills in 512-sample blocks, updating every 75 ms:

| Setting | Tallest pill in the room | Tallest pill after speech | Speech pills at full height | Mean speech pill |
| --- | --- | --- | --- | --- |
| Each update's loudest frame, gate 6 dB, 30 dB below the 90th percentile, a -90 dB starting floor | 0.58 | — | 25% | 0.69 |
| Frames averaged, gate 6 dB, 30 dB below the 90th percentile | 0.26 | 0.00 | 13% | 0.58 |
| Frames averaged, gate 12 dB, 30 dB below the 98th percentile (the defaults) | 0.01 | 0.00 | 4% | 0.38 |

Real room noise read -78 to -66 dB a band, louder in the wider high bands, so the -90 dB starting floor let the room light every pill for its first 10 s. With the defaults, speech kept a clear, changing shape:

```
▃█▆▁█▆▂█▃▇▄▅▂▅▅▅▆█▇▆▅▃▁▁▁▃▃▂▂▅▅▃▂▂▃▃▃▃▂·
▅█▅▅█▆█▇▆▇▄▅▆▄▇▄▄▄▅▃▁···▂▄▃▁▂▄▄▃▁▁▃▄▃▄▃▃
```

In the tuner's probe, no pill showed in 50 samples of its synthetic voice's 1.5 s silence.

### R5. The values can be tuned against the real room

> "it's showing ambient noise currently too , can we add controls for it to the debug window so i can tune?"

- Every value above is a field of `PillTuning` in `src/ui_meter/pill_tuning.rs`, and its `Default` holds the values in use: `noise_gate_db`, `min_span_db`, `range_db`, `floor_seconds`, `top_quantile`, `top_seconds`, `tilt_db_per_octave`, `fall_per_second`.
- Debug mode (`just debug-overlay`) has a slider for each, in its pills column. Its voice picker drives the meter from a synthetic voice, whose spectrum has two formants that drift (`voice_spectrum_db`) and a 1.5 s silence every 6 s, or from the microphone in the user's `[mic]` config, captured through the app's own `AudioController`, so the room's real noise and the real spectrum drive the pills. An idle transcription worker takes the dictation audio and drops it, because no session starts.
- A readout shows the bands' average noise floor and gate, the top, and the latest loudest band. Its digits have one width and its width is fixed, so its changing numbers do not move the controls.
- "Copy values" copies the pill values with the glass values. The values the user settles on become the `PillTuning` defaults.

## Superseded requirements

The spectrum replaced a scrolling strip of pills, one per 100 ms of the recording's loudness. The user asked for these while it existed; they no longer apply:

> "lets make it so that the pill stripe visually moves at the speech pace from right to left, on right and left we should have a fadeout effect of maybe 25px"
>
> "the pills sliding in from the side should already have the right height"
>
> "i would like there to be a strong visual indication in the pill wave of speech (eg up/down segments close by together, not just a continuous highs in a single block)"
>
> "maybe instead of animating the pills all the time, we only animate the side movement when there's speech"

The strip moved by one 24-hour linear Core Animation translation, paused in silence (Apple's Technical Q&A QA1673). Each pill was final before it slid into the column, and the pills alternated tall and short. Its measured Core Animation behaviours are in `findings.md` in the `overlay-visual-debugging` skill.

## Where the parts live

| Part | File |
| --- | --- |
| The FFT and the bands | `src/audio/spectrum.rs` (`SpectrumAnalyzer`) |
| Analysing each block in the audio callback | `src/audio/stream.rs` |
| The spectrum queue | `src/state.rs` (`SpectrumFrame`, `spectrum_sender`, `take_spectrum_frames`) |
| The range, rise and fall, and heights | `src/ui_meter/pill_levels.rs` (`PillLevels`) |
| Drawing and gliding the pills | `src/ui_meter/pill_cluster.rs` (`PillCluster`) |
| The tunable values and their defaults | `src/ui_meter/pill_tuning.rs` (`PillTuning`) |
| Synthetic voice and live microphone for debug mode | `src/overlay/dev/tuner.rs` (`voice_spectrum_db`, `publish_voice`, `Microphone`) |

## Rejected approaches

- **A speech classifier gating the pills.** Apple's SoundAnalysis classifier (`SNClassifySoundRequest`, 0.5 s windows at 50% overlap) told speech from other sound well (speech from `say` in 30 of 30 windows; loud noise, quiet hiss, keyboard-like clicks and hum in 0 of 15 each; about 0.7% of one core), but a pill had to wait for a window covering it, so pills entered the strip 0.7 s after their time. The user found it "delayed by noticable amount".
- **WebRTC VAD.** It judged speech from `say` 98% voiced, but also loud white noise 100% and keyboard-like clicks 60%.
- **A band's mean power per FFT bin.** It dilutes a tone across the band's bins (a half-scale sine read -12 dB instead of about -6) and turns the wide high bands down; the summed power is the band's energy.
- **Each update's loudest frame, and a -90 dB starting floor.** Room noise lit the pills (see R4's table).
- **Codex's fixed scale.** A peak of 512 to 8192 in 65535 (-42 to -18 dBFS) reads a soft voice in a quiet office low, the first complaint (R3).
