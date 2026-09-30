---
created_on: 2026-09-30 11:51
last_modified: 2026-09-30 11:51
status: current
---

# The pill meter

This reference is for anyone changing the overlay's pill meter (`ui.meter_style = "pills"`). It records what the user asked for, in their words, and which part of the design answers each request, so that no part is "simplified" away without knowing what it protects. Keep it current with the code.

The pills are a picture of someone talking, not a waveform. Accuracy is not a goal. What matters is what the user sees: speech has to be obvious at a glance, and nothing else may look like speech.

## Requirements

Each requirement is quoted from the user and followed by what implements it.

### R1. Soft speech in a quiet office must fill the pills

> "im in an office and i speak softly, the transpription works fine, but pills are barely ticking up, we need some kind of adaptive system here maybe?"

`LoudnessRange` in `src/ui_meter/pill_levels.rs` adapts to the room and the speaker instead of using a fixed dBFS scale:

- **Noise floor:** the quietest of the last 50 pills that were not speech (10 s), starting from a -50 dBFS prior. It learns only from pills that were not speech, so a long monologue never becomes its own floor: the voice's softest syllables would otherwise become the "floor" after 10 s of talking, turning half the speech into dots.
- **Gate:** pills up to 6 dB above the floor (amplitude × 2) rest as dots.
- **Top of the range:** the 90th percentile of the last 50 speech pills, at least 12 dB above the gate. Pills map linearly in amplitude from the gate to the top.
- **No audio:** a stretch in which the microphone delivered no audio (`PillLoudness::NoAudio`) teaches the range nothing. Counted as silence it would drop the floor to 0, and every sound would then clear the gate.

### R2. Pills already drawn never change

> "we need to make sure that painted pills remain stable, eg if i start softly but then get louder"
>
> "when speech is detected the pills that were already in view all of a sudden expand from 0 to 100, users shouldnt see this expansion effect, the pills sliding in from the side should already have the right height"

- A pill's height is set once, from its own 0.2 s, and never recomputed when the range changes later.
- A pill needs its loudness and its speech result before it can be painted. The speech classifier only answers for 0.5 s windows every 0.25 s, so a pill can wait up to about 0.45 s after its time begins.
- The strip is drawn `ENTRY_DELAY_SECONDS` (0.7 s) to the right of the column (`right_anchor` in `src/ui_meter/pill_strip.rs`), so a pill enters the column 0.7 s after its time begins. The pool of recycled layers covers that extra distance (`pool_size`).
- A pill not painted by `LATEST_PAINT_SECONDS` (0.6 s) rests as a dot for good, even if its result arrives later. The 0.1 s between the two is longer than the overlay's 75 ms between updates, so the height a pill enters with is already on screen.
- The cost is that bars appear about 0.7 s after the sound.

Evidence: a probe of the debug tuner sampled every pill layer every 4 ms. In each of four runs, all 27 height changes happened while the pill was still right of the column, and none inside it, the fade included. With the entry delay disabled, all 27 happened inside the column, the growth the user reported (for example a dot growing from 4.75 to 8.34 pt at x 492.9 of 514).

### R3. Pills move at the pace of speech, smoothly

> "mooving too fast imo, i want bars to somewhat match the pace of speach"
>
> "the pills movement is very jerky because we essentialy keep pills static and just resize to advance... lets make it so that the pill stripe visually moves at the speech pace from right to left, on right and left we should have a fadeout effect of maybe 25px... make this is implemented efficiently, dont make an image 100000px wide and move it"

- One pill stands for 0.2 s (`PILL_SECONDS`), about one syllable: conversational English runs at 4 to 5 syllables a second.
- The strip moves by one linear Core Animation translation that lasts 24 hours and has no timing function, so the render server moves it at the display's rate and the app does nothing per frame. Even the linear `CAMediaTimingFunction` put the strip up to 28 pt off (see `findings.md` in the `overlay-visual-debugging` skill).
- Only enough pill layers to cover the column and the way in exist. A layer that leaves on the left becomes the next pill on the right.
- The column fades out over 25 pt at both ends (`FADE_WIDTH`), through a gradient mask.
- With Reduce Motion on, the strip steps one pill spacing at a time instead of gliding.

### R4. Only speech raises the pills

> "i wonder if there's a cheap way to recognize speech for the sound vis?"

- Apple's SoundAnalysis built-in classifier (`SNClassifySoundRequest`, `SNClassifierIdentifierVersion1`) runs on its own thread in `src/audio/speech.rs`, over 0.5 s windows at 50% overlap. A window counts as speech when its "speech" confidence is at least 0.5.
- A pill is speech if any speech window overlaps it, so the pills turn on as soon as a word starts. Pills that are not speech (typing, coughs, a door, the room) rest as dots.
- The same thread records each audio block's mean square (`LevelBlock` in `src/state.rs`), and the pills take their loudness from those blocks, not from the audio callback's smoothed meter level. It keeps doing so when the classifier cannot start; then every pill counts as speech, and only the gate separates sound from the room.

Evidence (the "Telling speech from other sound" section of `findings.md`):

| Detector | Speech from `say` judged speech | Loud noise, typing-like clicks judged speech |
| --- | --- | --- |
| WebRTC VAD (`webrtc-vad`, very aggressive) | 98% | 100% and 60% |
| SoundAnalysis | 30 of 30 windows | 0 of 15 each |

SoundAnalysis cost about 0.7% of one core.

### R5. Speech reads as a strong up-and-down texture

> "i would like there to be a strong visual indication in the pill wave of speech (eg up/down segments close by together, not just a continuous highs in a single block), i dont specifically care about actual accuracy, this is visualisation only for the user, not wave form analysis if that makes sense"

- A speech pill's height is `texture(index) × (0.5 + 0.5 × loudness)`. Loudness only scales the pattern; it does not set it.
- `texture` alternates by pill index between a tall band (0.75 to 1.0) and a short band (0.15 to 0.4), with a height within the band from a hash of the index. That way it is random-looking but the same every time the pill is drawn.
- Pauses between words, and everything that is not speech, are dots, which breaks the zigzag into word-length runs.

Evidence: before this change, a simulation of speech recorded with `say` put 85% of the speech pills at 95% of full height or more (the smoothed level's peak per pill, on a dB range whose top rose at once to the loudest speech). With per-pill RMS on the linear range above, 22% reached that height, and the spread held for speech 18 dB softer. On screen, the tuner's last probe showed `▆▃▆▃▅▃▆▃▆··▃▅▃▅▃▅▃▅▃▅··▄▆▃▆`: every pair of neighbouring speech pills differed by 2 pt or more.

## Where the parts live

| Part | File |
| --- | --- |
| Heights, range, texture, paint deadline | `src/ui_meter/pill_levels.rs` |
| Strip motion, entry delay, layer pool, fade | `src/ui_meter/pill_strip.rs` |
| Converting the audio timeline to the strip's clock | `src/ui_meter.rs` (`push_pill`) |
| Speech classification and block loudness | `src/audio/speech.rs` |
| Kept speech results (20 s) and loudness (2 s) | `src/state.rs` (`AudioTimeline`) |
| Synthetic voice for debug mode | `src/overlay/dev/tuner.rs` (`publish_voice`) |

## Rejected approaches

- **Growing pills in the right-hand fade.** An earlier version let pills get their height in the 25 pt fade, assuming the fade hid the change. It does not: the user saw pills grow from dots to full height.
- **The smoothed meter level in dBFS, peak per pill, with a top that rises at once to the loudest speech.** Most speech pills pinned at full height (85% at 95% or more), so speech read as one solid block.
- **Codex's meter (the peak sample of each interval, mapped linearly).** Its voice strip was the user's reference for the look. Taking each pill's peak sample on the adaptive linear range above left 35% of the pills from the same recording at 95% of full height or more. Codex's own scale is fixed (noise floor 512/65535 to full scale 8192/65535), which does not adapt to a soft voice.
- **WebRTC VAD.** It called loud noise and keyboard clicks speech (see R4).
