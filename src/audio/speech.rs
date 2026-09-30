//! Whether the microphone hears speech: Apple's SoundAnalysis built-in sound
//! classifier, run on its own thread over the captured audio, so the meter
//! can tell speech from typing, coughs and other sounds.
//!
//! The analysis is not safe on the real-time audio thread, so the audio
//! callback hands each block over through a bounded queue. Results arrive
//! for 0.5 s windows (the shortest the classifier takes), one every 0.25 s,
//! and are published to `AppState` on the media clock
//! (`CACurrentMediaTime`), the clock the pill strip moves by.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_avf_audio::{AVAudioFormat, AVAudioPCMBuffer};
use objc2_core_media::CMTime;
use objc2_foundation::ns_string;
use objc2_sound_analysis::{
    SNAudioStreamAnalyzer, SNClassificationResult, SNClassifierIdentifierVersion1,
    SNClassifySoundRequest, SNRequest, SNResult, SNResultsObserving,
};

use crate::state::{AppState, SpeechWindow};

/// The analysis window: the shortest the built-in classifier takes.
const WINDOW_SECONDS: f64 = 0.5;
/// How much successive windows overlap: a result every 0.25 s.
const WINDOW_OVERLAP: f64 = 0.5;
/// A window whose "speech" confidence reaches this is speech.
const SPEECH_CONFIDENCE: f64 = 0.5;
/// Blocks the analysis thread may fall behind by before blocks are dropped:
/// about two seconds of audio at the usual callback size.
const QUEUED_BLOCKS: usize = 64;

/// Captured audio for the analysis: mono samples from -1 to 1, and when the
/// block arrived on the media clock.
struct AudioBlock {
    samples: Vec<f32>,
    arrived_at: f64,
}

/// Hands captured audio to the speech analysis thread. Dropping it ends the
/// thread.
pub struct SpeechAnalysis {
    blocks: SyncSender<AudioBlock>,
}

impl SpeechAnalysis {
    /// Starts analysing audio at `sample_rate`, publishing results to
    /// `state`.
    pub fn start(sample_rate: u32, state: Arc<AppState>) -> Self {
        let (blocks, received) = sync_channel(QUEUED_BLOCKS);
        let thread_state = state.clone();
        let spawned = std::thread::Builder::new()
            .name("speech-analysis".into())
            .spawn(move || run(sample_rate, &thread_state, &received));
        if let Err(error) = spawned {
            log::error!("failed to start the speech analysis thread: {}", error);
            state.set_speech_analysis_available(false);
        }
        Self { blocks }
    }

    /// Queues `samples`, which arrived `arrived_at` on the media clock. Never
    /// waits: when the analysis has fallen behind, the block is dropped.
    pub fn analyze(&self, samples: Vec<f32>, arrived_at: f64) {
        if let Err(TrySendError::Full(_)) = self.blocks.try_send(AudioBlock { samples, arrived_at }) {
            log::debug!("speech analysis is behind; dropping an audio block");
        }
    }
}

/// Where the analyzer's audio sits on the media clock: the frame position
/// where the latest block ended, and when it arrived.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FrameClock {
    end_frame: i64,
    end_media: f64,
    sample_rate: f64,
}

impl FrameClock {
    /// The media time of `stream_seconds` into the analyzed audio (a frame
    /// position divided by the sample rate), counting back from the latest
    /// block.
    fn media_time(&self, stream_seconds: f64) -> f64 {
        self.end_media - ((self.end_frame as f64 / self.sample_rate) - stream_seconds)
    }
}

struct ObserverIvars {
    state: Arc<AppState>,
    clock: Arc<Mutex<FrameClock>>,
}

define_class!(
    /// Receives the classifier's results and publishes whether each window
    /// was speech.
    #[unsafe(super(NSObject))]
    #[name = "SimplePttSpeechObserver"]
    #[ivars = ObserverIvars]
    struct SpeechObserver;

    unsafe impl NSObjectProtocol for SpeechObserver {}

    unsafe impl SNResultsObserving for SpeechObserver {
        #[unsafe(method(request:didProduceResult:))]
        fn request_did_produce_result(
            &self,
            _request: &ProtocolObject<dyn SNRequest>,
            result: &ProtocolObject<dyn SNResult>,
        ) {
            self.record(result);
        }
    }
);

impl SpeechObserver {
    fn new(state: Arc<AppState>, clock: Arc<Mutex<FrameClock>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ObserverIvars { state, clock });
        unsafe { msg_send![super(this), init] }
    }

    fn record(&self, result: &ProtocolObject<dyn SNResult>) {
        let object: &objc2::runtime::AnyObject = result.as_ref();
        let Some(classification) = object.downcast_ref::<SNClassificationResult>() else {
            return;
        };
        // SAFETY: plain getters of a result the analyzer handed over.
        let (range, confidence) = unsafe {
            let range = classification.timeRange();
            let confidence = classification
                .classificationForIdentifier(ns_string!("speech"))
                .map_or(0.0, |speech| speech.confidence());
            (range, confidence)
        };
        let Ok(clock) = self.ivars().clock.lock().map(|clock| *clock) else {
            return;
        };
        // SAFETY: CMTimeGetSeconds of the result's own times.
        let (start, duration) = unsafe { (range.start.seconds(), range.duration.seconds()) };
        self.ivars().state.record_speech_window(SpeechWindow {
            start: clock.media_time(start),
            end: clock.media_time(start + duration),
            speech: confidence >= SPEECH_CONFIDENCE,
        });
    }
}

/// The analyzer and the objects it needs alive: it only weakly retains the
/// observer.
struct Analyzer {
    analyzer: Retained<SNAudioStreamAnalyzer>,
    format: Retained<AVAudioFormat>,
    _request: Retained<SNClassifySoundRequest>,
    _observer: Retained<SpeechObserver>,
    clock: Arc<Mutex<FrameClock>>,
    position: i64,
}

impl Analyzer {
    fn new(sample_rate: u32, state: &Arc<AppState>) -> Result<Self, String> {
        let rate = f64::from(sample_rate);
        // SAFETY: the SoundAnalysis and AVFAudio calls below take and return
        // objects created here, with the arguments their headers document.
        unsafe {
            let format = AVAudioFormat::initStandardFormatWithSampleRate_channels(AVAudioFormat::alloc(), rate, 1)
                .ok_or_else(|| format!("no mono float audio format at {} Hz", sample_rate))?;
            let identifier = SNClassifierIdentifierVersion1
                .ok_or_else(|| "the built-in sound classifier is not available".to_owned())?;
            let request = SNClassifySoundRequest::initWithClassifierIdentifier_error(SNClassifySoundRequest::alloc(), identifier)
                .map_err(|error| error.localizedDescription().to_string())?;
            request.setWindowDuration(CMTime::with_seconds(WINDOW_SECONDS, 1000));
            request.setOverlapFactor(WINDOW_OVERLAP);
            let clock = Arc::new(Mutex::new(FrameClock { end_frame: 0, end_media: 0.0, sample_rate: rate }));
            let observer = SpeechObserver::new(state.clone(), clock.clone());
            let analyzer = SNAudioStreamAnalyzer::initWithFormat(SNAudioStreamAnalyzer::alloc(), &format);
            analyzer
                .addRequest_withObserver_error(ProtocolObject::from_ref(&*request), ProtocolObject::from_ref(&*observer))
                .map_err(|error| error.localizedDescription().to_string())?;
            Ok(Self { analyzer, format, _request: request, _observer: observer, clock, position: 0 })
        }
    }

    fn analyze(&mut self, block: &AudioBlock) {
        let Ok(frames) = u32::try_from(block.samples.len()) else {
            return;
        };
        if frames == 0 {
            return;
        }
        // SAFETY: the buffer holds `frames` frames of the analyzer's own
        // format (one float channel); exactly that many samples are written
        // into its one channel before its length is set.
        unsafe {
            let Some(buffer) = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(AVAudioPCMBuffer::alloc(), &self.format, frames) else {
                return;
            };
            let channels = buffer.floatChannelData();
            if channels.is_null() {
                return;
            }
            let channel = (*channels).as_ptr();
            std::ptr::copy_nonoverlapping(block.samples.as_ptr(), channel, block.samples.len());
            buffer.setFrameLength(frames);
            if let Ok(mut clock) = self.clock.lock() {
                clock.end_frame = self.position + i64::from(frames);
                clock.end_media = block.arrived_at;
            }
            self.analyzer.analyzeAudioBuffer_atAudioFramePosition(&buffer, self.position);
        }
        self.position += i64::from(frames);
    }
}

fn run(sample_rate: u32, state: &Arc<AppState>, received: &Receiver<AudioBlock>) {
    let mut analyzer = match Analyzer::new(sample_rate, state) {
        Ok(analyzer) => analyzer,
        Err(error) => {
            log::warn!("speech analysis unavailable; the meter shows every sound: {}", error);
            state.set_speech_analysis_available(false);
            return;
        }
    };
    state.set_speech_analysis_available(true);
    while let Ok(block) = received.recv() {
        analyzer.analyze(&block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysed_frames_map_onto_the_media_clock() {
        // 16 kHz; the latest block ended at frame 32000 (2 s in) and arrived
        // at media time 100 s.
        let clock = FrameClock { end_frame: 32_000, end_media: 100.0, sample_rate: 16_000.0 };
        assert!((clock.media_time(2.0) - 100.0).abs() < 1e-9);
        assert!((clock.media_time(1.5) - 99.5).abs() < 1e-9);
    }

    #[test]
    fn silence_is_analysed_as_not_speech_on_the_media_clock() {
        let state = AppState::new();
        let mut analyzer = Analyzer::new(16_000, &state).expect("the built-in classifier is available");
        // Two seconds of silence in 20 ms blocks, arriving from media time 50 s.
        for block in 0..100 {
            let arrived_at = 50.0 + ((block + 1) as f64 * 0.02);
            analyzer.analyze(&AudioBlock { samples: vec![0.0; 320], arrived_at });
        }
        let windows = state.speech_windows();
        assert!(windows.len() >= 3, "{windows:?}");
        assert!(windows.iter().all(|window| !window.speech), "{windows:?}");
        // The windows sit within the two seconds the audio arrived in, each
        // half a second long.
        for window in &windows {
            assert!(window.start >= 49.99 && window.end <= 52.01, "{window:?}");
            assert!((window.end - window.start - WINDOW_SECONDS).abs() < 0.01, "{window:?}");
        }
    }
}
