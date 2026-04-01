mod app;
mod audio;
mod config;
mod hotkey;
mod state;
mod transcription;

use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;

fn main() {
    env_logger::init();
    log::info!("jarvis-native starting");

    let config = config::load_config();
    log::info!(
        "config loaded (hotkey={}, audio_device={:?}, sample_rate={}Hz, gain={}, hold_ms={}, endpointing_ms={}, utterance_end_ms={}, deepgram_model={}, deepgram_language={}, api_key_configured={})",
        config.hotkey,
        config.audio_device,
        config.sample_rate,
        config.gain,
        config.hold_ms,
        config.endpointing_ms,
        config.utterance_end_ms,
        config.deepgram_model,
        config.deepgram_language,
        config.deepgram_api_key.as_deref().map(str::is_empty).map(|is_empty| !is_empty).unwrap_or(false)
            || std::env::var("DEEPGRAM_API_KEY").ok().as_deref().map(str::trim).map(|value| !value.is_empty()).unwrap_or(false)
    );

    let deepgram_api_key = config
        .resolve_deepgram_api_key()
        .unwrap_or_else(|error| panic!("{}", error));

    let shared_state = state::AppState::new();

    let transcription_controller = transcription::spawn_transcription_thread(
        shared_state.clone(),
        transcription::DeepgramConfig {
            api_key: deepgram_api_key,
            model: config.deepgram_model.clone(),
            language: config.deepgram_language.clone(),
            endpointing_ms: config.endpointing_ms,
            utterance_end_ms: config.utterance_end_ms,
        },
    );

    let (_audio_stream, actual_rate) = audio::build_input_stream(
        shared_state.clone(),
        transcription_controller.clone(),
        config.sample_rate,
        config.gain,
        config.audio_device.as_deref(),
    );
    transcription_controller.set_sample_rate(actual_rate);

    hotkey::spawn_hotkey_thread(
        shared_state.clone(),
        transcription_controller,
        &config.hotkey,
        config.hold_ms,
    );

    let mtm = MainThreadMarker::new().expect("must run on main thread");
    let ns_app = NSApplication::sharedApplication(mtm);
    let delegate = app::AppDelegate::new(mtm);
    ns_app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    app::setup_status_polling(delegate.clone(), shared_state);

    log::info!("starting NSApplication run loop");
    ns_app.run();
}
