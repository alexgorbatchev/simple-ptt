mod app;
mod audio;
mod auto_launch;
mod billing;
mod config;
mod deepgram_api;
mod deepgram_connection;
mod hotkey;
mod hotkey_binding;
mod hotkey_capture;
mod icon;
mod overlay;
mod permissions;
mod permissions_dialog;
mod settings;
mod settings_window;
mod state;
mod transcription;
mod transformation;
mod transformation_models;

use std::any::Any;
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

    match std::panic::catch_unwind(run_graphical_application) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            report_startup_error(&error);
            std::process::exit(1);
        }
        Err(panic_payload) => {
            report_startup_error(&panic_payload_message(panic_payload));
            std::process::exit(1);
        }
    }
}

fn run_graphical_application() -> Result<(), String> {
    log::info!("simple-ptt starting");

    let loaded_config = config::load_config();
    let runtime_config = config::materialize_runtime_config(&loaded_config);
    let config_path = config::config_path()?;

    crate::auto_launch::apply_auto_launch_config(runtime_config.ui.start_on_login);

    log::info!(
        "config loaded (ui.start_on_login={}, ui.hotkey={}, mic.audio_device={:?}, mic.sample_rate={}Hz, mic.gain={}, mic.hold_ms={}, ui.font_name={:?}, ui.font_size={}, ui.footer_font_size={:?}, ui.meter_style={:?}, deepgram.endpointing_ms={}, deepgram.utterance_end_ms={}, deepgram.model={}, deepgram.language={}, deepgram.api_key_configured={}, deepgram.project_id_configured={}, transformation.enabled={}, transformation.hotkey={:?}, transformation.auto={}, transformation.provider={:?}, transformation.model={:?}, transformation.api_key_configured={}, transformation.system_prompt_configured={})",
        runtime_config.ui.start_on_login,
        runtime_config.ui.hotkey,
        runtime_config.mic.audio_device,
        runtime_config.mic.sample_rate,
        runtime_config.mic.gain,
        runtime_config.mic.hold_ms,
        runtime_config.ui.font_name,
        runtime_config.ui.font_size,
        runtime_config.ui.footer_font_size,
        runtime_config.ui.meter_style,
        runtime_config.deepgram.endpointing_ms,
        runtime_config.deepgram.utterance_end_ms,
        runtime_config.deepgram.model,
        runtime_config.deepgram.language,
        runtime_config.deepgram.api_key.as_deref().map(str::trim).map(|value| !value.is_empty()).unwrap_or(false),
        runtime_config.deepgram.project_id.as_deref().map(str::trim).map(|value| !value.is_empty()).unwrap_or(false),
        runtime_config.resolve_transformation_config().is_ok(),
        Some(runtime_config.transformation.hotkey.as_str()),
        runtime_config.transformation.auto,
        runtime_config.transformation.provider,
        &runtime_config.transformation.model,
        runtime_config.transformation.api_key.as_deref().map(str::trim).map(|value| !value.is_empty()).unwrap_or(false),
        !runtime_config.transformation.system_prompt.trim().is_empty(),
    );

    let config_store =
        settings::LiveConfigStore::new(loaded_config.clone(), runtime_config.clone(), config_path);
    let startup_hotkey_permissions = permissions::GlobalHotkeyPermissions::current();

    let hotkey_capture_controller = hotkey_capture::HotkeyCaptureController::new();
    let transformation_models_controller =
        transformation_models::TransformationModelsController::new();
    let deepgram_connection_controller = deepgram_connection::DeepgramConnectionController::new();
    let shared_state = state::AppState::new();

    let billing_controller =
        billing::BillingController::new(shared_state.clone(), config_store.clone());
    let transcription_controller =
        transcription::spawn_transcription_thread(shared_state.clone(), config_store.clone());
    let (audio_controller, initial_audio_error) = if startup_hotkey_permissions
        .hotkey_permissions_granted()
        && startup_hotkey_permissions.microphone_granted
    {
        audio::AudioController::new(
            shared_state.clone(),
            transcription_controller.clone(),
            config_store.clone(),
        )
    } else {
        (
            audio::AudioController::inactive(
                shared_state.clone(),
                transcription_controller.clone(),
                config_store.clone(),
            ),
            None,
        )
    };

    if startup_hotkey_permissions.hotkey_permissions_granted() {
        hotkey::spawn_hotkey_thread(
            shared_state.clone(),
            billing_controller.clone(),
            transcription_controller.clone(),
            config_store.clone(),
            hotkey_capture_controller.clone(),
        );
    } else {
        log::warn!(
            "global hotkey thread not started because Accessibility or Input Monitoring is missing at launch; relaunch after granting both permissions"
        );
    }

    let mtm = MainThreadMarker::new().expect("must run on main thread");
    let ns_app = NSApplication::sharedApplication(mtm);
    let delegate = app::AppDelegate::new(
        mtm,
        app::overlay_style_from_config(&runtime_config),
        config_store,
        startup_hotkey_permissions,
        initial_audio_error,
        hotkey_capture_controller.clone(),
        transformation_models_controller.clone(),
        deepgram_connection_controller.clone(),
        billing_controller,
        audio_controller,
        shared_state.clone(),
    );
    ns_app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    app::setup_status_polling(
        delegate.clone(),
        shared_state,
        hotkey_capture_controller,
        transformation_models_controller,
        deepgram_connection_controller,
    );

    log::info!("starting NSApplication run loop");
    ns_app.run();
    Ok(())
}

fn report_startup_error(error: &str) {
    log::error!("startup failed: {}", error);
    app::show_startup_error_dialog(
        "simple-ptt couldn't start",
        &startup_error_instructions(error),
    );
}

fn startup_error_instructions(error: &str) -> String {
    format!(
        concat!(
            "{}\n\n",
            "What to check:\n",
            "- ~/.config/simple-ptt/config.toml exists and is valid TOML\n",
            "- if mic.audio_device is configured, it matches a real input device\n",
            "- if startup fails before the menu appears, run the bundled binary directly from Terminal for the exact error\n\n",
            "For more detail, run this in Terminal:\n",
            "/Applications/simple-ptt.app/Contents/MacOS/simple-ptt\n\n",
            "Note: simple-ptt is a menu bar app. On successful launch it appears in the menu bar, not in the Dock."
        ),
        error
    )
}

fn panic_payload_message(panic_payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = panic_payload.downcast_ref::<String>() {
        return message.clone();
    }

    if let Some(message) = panic_payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }

    "simple-ptt panicked during startup".to_owned()
}

#[cfg(test)]
mod tests {
    use super::startup_error_instructions;

    #[test]
    fn startup_error_instructions_include_terminal_debugging_path() {
        let instructions =
            startup_error_instructions("configured audio_device 'Missing' was not found");

        assert!(instructions.contains("/Applications/simple-ptt.app/Contents/MacOS/simple-ptt"));
        assert!(instructions.contains("menu bar"));
    }
}
