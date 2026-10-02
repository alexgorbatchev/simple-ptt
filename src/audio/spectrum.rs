//! The live spectrum of the captured audio for the pill meter: an FFT of the
//! last 64 ms, summed into `SPECTRUM_BANDS` mel-spaced bands from 100 Hz up to
//! 8 kHz (or half the sample rate, if lower), low pitches first.
//!
//! It runs on the real-time audio thread, so every buffer is made when the
//! stream is built: taking in samples and analysing them allocate nothing
//! (`RealToComplex::process_with_scratch`).

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::state::SPECTRUM_BANDS;

/// How much audio each analysis covers: 1024 samples at 16 kHz, so the lowest
/// bands still span a few FFT bins.
const WINDOW_SECONDS: f64 = 0.064;
/// The lowest band's lower edge: below the voice's fundamental.
const LOWEST_HZ: f64 = 100.0;
/// The highest band's upper edge, unless half the sample rate is lower.
const HIGHEST_HZ: f64 = 8_000.0;
/// Band levels are floored here: digital silence.
const SILENCE_DB: f32 = -140.0;

/// One band: the FFT bins it sums, and its centre frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Band {
    bins: (usize, usize),
    centre_hz: f32,
}

pub struct SpectrumAnalyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    /// The latest samples, as a ring: `history[write]` is the oldest once
    /// `filled` reaches its length.
    history: Vec<f32>,
    write: usize,
    filled: usize,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    bands: [Band; SPECTRUM_BANDS],
    /// Scales a band's summed power so a full-scale sine reads about 0 dB.
    power_scale: f32,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        let length = (((f64::from(sample_rate) * WINDOW_SECONDS) / 2.0).round() as usize * 2).max(64);
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(length);
        // Hann window.
        let window: Vec<f32> = (0..length)
            .map(|index| {
                let phase = std::f32::consts::TAU * index as f32 / length as f32;
                0.5 - (0.5 * phase.cos())
            })
            .collect();
        let window_sum: f32 = window.iter().sum();
        Self {
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            history: vec![0.0; length],
            write: 0,
            filled: 0,
            bands: mel_bands(f64::from(sample_rate), length),
            // A sine's power, (amplitude × window sum / 2)², spreads over the Hann
            // window's equivalent noise bandwidth of 1.5 bins.
            power_scale: 4.0 / (window_sum * window_sum * 1.5),
            window,
            fft,
        }
    }

    /// Takes in 16-bit little-endian mono samples (the dictation audio).
    pub fn push_pcm16(&mut self, bytes: &[u8]) {
        for pair in bytes.chunks_exact(2) {
            self.push(f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.0);
        }
    }

    /// Forgets the samples taken in, so the next analysis waits for a whole
    /// window of new audio.
    pub fn reset(&mut self) {
        self.filled = 0;
    }

    fn push(&mut self, sample: f32) {
        self.history[self.write] = sample;
        self.write = (self.write + 1) % self.history.len();
        self.filled = (self.filled + 1).min(self.history.len());
    }

    /// Each band's level in dB, the power summed over its bins, low pitches
    /// first, once a whole window has been taken in.
    pub fn analyze(&mut self) -> Option<[f32; SPECTRUM_BANDS]> {
        let length = self.history.len();
        if self.filled < length {
            return None;
        }
        for (offset, (input, weight)) in self.input.iter_mut().zip(&self.window).enumerate() {
            *input = self.history[(self.write + offset) % length] * weight;
        }
        self.fft.process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch).ok()?;
        Some(std::array::from_fn(|index| {
            let (low, high) = self.bands[index].bins;
            let power: f32 = self.output[low..high].iter().map(Complex::norm_sqr).sum();
            (10.0 * (power * self.power_scale).max(1e-14).log10()).max(SILENCE_DB)
        }))
    }

    /// Each band's centre frequency, low pitches first.
    pub fn centres_hz(&self) -> [f32; SPECTRUM_BANDS] {
        std::array::from_fn(|index| self.bands[index].centre_hz)
    }
}

fn mel(hz: f64) -> f64 {
    2595.0 * (1.0 + (hz / 700.0)).log10()
}

fn hz(mel: f64) -> f64 {
    700.0 * (10f64.powf(mel / 2595.0) - 1.0)
}

/// `SPECTRUM_BANDS` bands evenly spaced in mel from `LOWEST_HZ` to
/// `HIGHEST_HZ` (or half of `sample_rate`), over the bins of a `length`-point
/// FFT: each at least one bin, none overlapping.
fn mel_bands(sample_rate: f64, length: usize) -> [Band; SPECTRUM_BANDS] {
    let top = HIGHEST_HZ.min(sample_rate / 2.0);
    let (low_mel, high_mel) = (mel(LOWEST_HZ), mel(top));
    let edge = |index: usize| hz(low_mel + ((high_mel - low_mel) * index as f64 / SPECTRUM_BANDS as f64));
    let last_bin = length / 2;
    let bin = |frequency: f64| ((frequency * length as f64 / sample_rate).round() as usize).clamp(1, last_bin);
    let mut next_low = bin(edge(0));
    std::array::from_fn(|index| {
        let low = next_low.min(last_bin);
        let high = bin(edge(index + 1)).max(low + 1).min(last_bin + 1);
        next_low = high;
        Band { bins: (low, high), centre_hz: (edge(index) * edge(index + 1)).sqrt() as f32 }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `seconds` of a sine at `frequency` and amplitude `amplitude` (1 is full
    /// scale), as 16-bit samples.
    fn sine(sample_rate: u32, frequency: f64, amplitude: f64, seconds: f64) -> Vec<u8> {
        (0..(f64::from(sample_rate) * seconds) as usize)
            .flat_map(|index| {
                let value = amplitude * (std::f64::consts::TAU * frequency * index as f64 / f64::from(sample_rate)).sin();
                ((value * 32767.0) as i16).to_le_bytes()
            })
            .collect()
    }

    fn band_of(analyzer: &SpectrumAnalyzer, frequency: f32) -> usize {
        let centres = analyzer.centres_hz();
        (0..SPECTRUM_BANDS)
            .min_by(|a, b| (centres[*a] / frequency).ln().abs().total_cmp(&(centres[*b] / frequency).ln().abs()))
            .unwrap()
    }

    #[test]
    fn bands_rise_from_100_hz_to_8_khz_each_with_its_own_bins() {
        for sample_rate in [16_000, 44_100, 48_000] {
            let analyzer = SpectrumAnalyzer::new(sample_rate);
            let bands = analyzer.bands;
            for pair in bands.windows(2) {
                assert!(pair[0].bins.1 <= pair[1].bins.0, "{sample_rate}: {pair:?}");
                assert!(pair[0].centre_hz < pair[1].centre_hz, "{sample_rate}: {pair:?}");
            }
            for band in &bands {
                assert!(band.bins.0 < band.bins.1, "{sample_rate}: {band:?}");
            }
            let bin_hz = sample_rate as f32 / analyzer.history.len() as f32;
            assert!((bands[0].bins.0 as f32 * bin_hz - 100.0).abs() <= bin_hz, "{sample_rate}: {:?}", bands[0]);
            let top = bands[SPECTRUM_BANDS - 1].bins.1 as f32 * bin_hz;
            assert!((top - 8_000.0).abs() <= 2.0 * bin_hz, "{sample_rate}: top {top}");
        }
    }

    #[test]
    fn a_tone_lights_its_own_band() {
        let mut analyzer = SpectrumAnalyzer::new(16_000);
        analyzer.push_pcm16(&sine(16_000, 1_000.0, 0.5, 0.1));
        let levels = analyzer.analyze().unwrap();
        let tone_band = band_of(&analyzer, 1_000.0);
        let loudest = (0..SPECTRUM_BANDS).max_by(|a, b| levels[*a].total_cmp(&levels[*b])).unwrap();
        assert_eq!(loudest, tone_band, "{levels:?}");
        // A half-scale sine is about -6 dBFS in its band, far above a band
        // two octaves up.
        assert!((levels[tone_band] + 6.0).abs() < 4.0, "{}", levels[tone_band]);
        assert!(levels[tone_band] - levels[band_of(&analyzer, 4_000.0)] > 40.0, "{levels:?}");
    }

    #[test]
    fn silence_reads_as_silence() {
        let mut analyzer = SpectrumAnalyzer::new(16_000);
        analyzer.push_pcm16(&vec![0; 16_000 * 2 / 10]);
        let levels = analyzer.analyze().unwrap();
        assert!(levels.iter().all(|level| *level <= SILENCE_DB + 1.0), "{levels:?}");
    }

    #[test]
    fn analysis_waits_for_a_whole_window_taken_in_any_pieces() {
        let mut analyzer = SpectrumAnalyzer::new(16_000);
        let tone = sine(16_000, 500.0, 0.5, 0.064);
        let (first, rest) = tone.split_at(700);
        analyzer.push_pcm16(first);
        assert_eq!(analyzer.analyze(), None);
        analyzer.push_pcm16(rest);
        let levels = analyzer.analyze().unwrap();
        let loudest = (0..SPECTRUM_BANDS).max_by(|a, b| levels[*a].total_cmp(&levels[*b])).unwrap();
        assert_eq!(loudest, band_of(&analyzer, 500.0));
    }

    #[test]
    fn the_window_holds_the_latest_audio_in_time_order() {
        // 2360 samples of a 2 kHz tone, then 400 of a 300 Hz one: in time
        // order the 2 kHz tone fills the Hann window's middle and the 300 Hz
        // one its fading end. (The ring stores the 300 Hz tone across its
        // middle, so analysing it in storage order would read 300 Hz.)
        let mut analyzer = SpectrumAnalyzer::new(16_000);
        analyzer.push_pcm16(&sine(16_000, 2_000.0, 0.5, 2_360.0 / 16_000.0));
        analyzer.push_pcm16(&sine(16_000, 300.0, 0.5, 400.0 / 16_000.0));
        let levels = analyzer.analyze().unwrap();
        assert!(
            levels[band_of(&analyzer, 2_000.0)] > levels[band_of(&analyzer, 300.0)] + 6.0,
            "{levels:?}"
        );
    }

    #[test]
    fn the_newest_audio_is_analysed() {
        // A 2 kHz tone, then a 300 Hz one filling the whole window.
        let mut analyzer = SpectrumAnalyzer::new(16_000);
        analyzer.push_pcm16(&sine(16_000, 2_000.0, 0.5, 0.2));
        analyzer.push_pcm16(&sine(16_000, 300.0, 0.5, 0.07));
        let levels = analyzer.analyze().unwrap();
        let loudest = (0..SPECTRUM_BANDS).max_by(|a, b| levels[*a].total_cmp(&levels[*b])).unwrap();
        assert_eq!(loudest, band_of(&analyzer, 300.0), "{levels:?}");
    }
}
