mod app;
mod audio;
mod billing;
mod config;
mod hotkey;
mod icon;
mod overlay;
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
        "config loaded (hotkey={}, audio_device={:?}, sample_rate={}Hz, gain={}, hold_ms={}, overlay_font_name={:?}, overlay_font_size={}, overlay_footer_font_size={:?}, endpointing_ms={}, utterance_end_ms={}, deepgram_model={}, deepgram_language={}, api_key_configured={}, project_id_configured={})",
        config.hotkey,
        config.audio_device,
        config.sample_rate,
        config.gain,
        config.hold_ms,
        config.overlay_font_name,
        config.overlay_font_size,
        config.overlay_footer_font_size,
        config.endpointing_ms,
        config.utterance_end_ms,
        config.deepgram_model,
        config.deepgram_language,
        config.deepgram_api_key.as_deref().map(str::is_empty).map(|is_empty| !is_empty).unwrap_or(false)
            || std::env::var("DEEPGRAM_API_KEY").ok().as_deref().map(str::trim).map(|value| !value.is_empty()).unwrap_or(false),
        config.deepgram_project_id.as_deref().map(str::is_empty).map(|is_empty| !is_empty).unwrap_or(false)
            || std::env::var(billing::deepgram_project_id_env_var()).ok().as_deref().map(str::trim).map(|value| !value.is_empty()).unwrap_or(false)
    );

    let deepgram_api_key = config
        .resolve_deepgram_api_key()
        .unwrap_or_else(|error| panic!("{}", error));

    let deepgram_project_id = config.resolve_deepgram_project_id();

    let shared_state = state::AppState::new();

    let billing_controller = billing::BillingController::new(
        shared_state.clone(),
        billing::BillingConfig {
            api_key: deepgram_api_key.clone(),
            project_id: deepgram_project_id,
        },
    );

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
        billing_controller,
        transcription_controller,
        &config.hotkey,
        config.hold_ms,
    );

    let overlay_font_size =
        if config.overlay_font_size.is_finite() && config.overlay_font_size > 0.0 {
            config.overlay_font_size
        } else {
            log::warn!(
                "overlay_font_size {} is invalid; falling back to 18.0",
                config.overlay_font_size
            );
            18.0
        };
    let overlay_footer_font_size = match config.overlay_footer_font_size {
        Some(footer_font_size) if footer_font_size.is_finite() && footer_font_size > 0.0 => {
            footer_font_size
        }
        Some(footer_font_size) => {
            let fallback_footer_font_size = overlay_font_size * 0.6;
            log::warn!(
                "overlay_footer_font_size {} is invalid; falling back to {}",
                footer_font_size,
                fallback_footer_font_size
            );
            fallback_footer_font_size
        }
        None => overlay_font_size * 0.6,
    };

    let mtm = MainThreadMarker::new().expect("must run on main thread");
    let ns_app = NSApplication::sharedApplication(mtm);
    let delegate = app::AppDelegate::new(
        mtm,
        overlay::OverlayStyle {
            font_name: config.overlay_font_name.clone(),
            font_size: overlay_font_size,
            footer_font_size: overlay_footer_font_size,
        },
    );
    ns_app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    app::setup_status_polling(delegate.clone(), shared_state);

    log::info!("starting NSApplication run loop");
    ns_app.run();
}
