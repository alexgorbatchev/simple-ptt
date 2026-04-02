mod app;
mod audio;
mod billing;
mod config;
mod hotkey;
mod icon;
mod overlay;
mod state;
mod transcription;

use std::path::Path;

use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;

fn main() {
    env_logger::init();

    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [flag] if flag == "--list-devices" => {
            audio::print_input_devices().unwrap_or_else(|error| {
                eprintln!("{}", error);
                std::process::exit(1);
            });
            return;
        }
        [flag, output_dir] if flag == "--write-app-iconset" => {
            icon::write_application_iconset(Path::new(output_dir)).unwrap_or_else(|error| {
                eprintln!("{}", error);
                std::process::exit(1);
            });
            return;
        }
        _ => {}
    }

    log::info!("simple-ptt starting");

    let config = config::load_config();
    log::info!(
        "config loaded (ui.hotkey={}, mic.audio_device={:?}, mic.sample_rate={}Hz, mic.gain={}, mic.hold_ms={}, ui.font_name={:?}, ui.font_size={}, ui.footer_font_size={:?}, ui.meter_style={:?}, deepgram.endpointing_ms={}, deepgram.utterance_end_ms={}, deepgram.model={}, deepgram.language={}, api_key_configured={}, project_id_configured={})",
        config.ui.hotkey,
        config.mic.audio_device,
        config.mic.sample_rate,
        config.mic.gain,
        config.mic.hold_ms,
        config.ui.font_name,
        config.ui.font_size,
        config.ui.footer_font_size,
        config.ui.meter_style,
        config.deepgram.endpointing_ms,
        config.deepgram.utterance_end_ms,
        config.deepgram.model,
        config.deepgram.language,
        config.deepgram.api_key.as_deref().map(str::is_empty).map(|is_empty| !is_empty).unwrap_or(false)
            || std::env::var("DEEPGRAM_API_KEY").ok().as_deref().map(str::trim).map(|value| !value.is_empty()).unwrap_or(false),
        config.deepgram.project_id.as_deref().map(str::is_empty).map(|is_empty| !is_empty).unwrap_or(false)
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
            model: config.deepgram.model.clone(),
            language: config.deepgram.language.clone(),
            endpointing_ms: config.deepgram.endpointing_ms,
            utterance_end_ms: config.deepgram.utterance_end_ms,
        },
    );

    let (_audio_stream, actual_rate) = audio::build_input_stream(
        shared_state.clone(),
        transcription_controller.clone(),
        config.mic.sample_rate,
        config.mic.gain,
        config.mic.audio_device.as_deref(),
    );
    transcription_controller.set_sample_rate(actual_rate);

    hotkey::spawn_hotkey_thread(
        shared_state.clone(),
        billing_controller,
        transcription_controller,
        &config.ui.hotkey,
        config.mic.hold_ms,
    );

    let overlay_font_size = if config.ui.font_size.is_finite() && config.ui.font_size > 0.0 {
        config.ui.font_size
    } else {
        log::warn!(
            "ui.font_size {} is invalid; falling back to 12.0",
            config.ui.font_size
        );
        12.0
    };
    let overlay_footer_font_size = match config.ui.footer_font_size {
        Some(footer_font_size) if footer_font_size.is_finite() && footer_font_size > 0.0 => {
            footer_font_size
        }
        Some(footer_font_size) => {
            let fallback_footer_font_size = overlay_font_size * 0.6;
            log::warn!(
                "ui.footer_font_size {} is invalid; falling back to {}",
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
            font_name: config.ui.font_name.clone(),
            font_size: overlay_font_size,
            footer_font_size: overlay_footer_font_size,
            meter_style: config.ui.meter_style,
        },
    );
    ns_app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    app::setup_status_polling(delegate.clone(), shared_state);

    log::info!("starting NSApplication run loop");
    ns_app.run();
}
