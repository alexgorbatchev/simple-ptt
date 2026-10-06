mod app;
mod audio;
mod auto_launch;
mod config;
mod deepgram_api;
mod deepgram_connection;
mod hotkey;
mod hotkey_binding;
mod hotkey_capture;
mod icon;
mod key;
mod overlay;
mod permissions;
mod permissions_dialog;
mod settings;
mod settings_window;
mod state;
mod transcription;
mod transformation;
mod transformation_models;
mod ui_meter;
mod updater;

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
        [flag] if flag == "--debug" => {
            overlay::dev::tuner::run();
            return;
        }
        [flag, output_dir] if flag == "--overlay-snapshot" => {
            overlay::dev::snapshot::run(output_dir);
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
        "config loaded (ui.start_on_login={}, ui.hotkey={}, mic.audio_device={:?}, mic.sample_rate={}Hz, mic.gain={}, mic.hold_ms={}, ui.font_name={:?}, ui.font_size={}, ui.footer_font_size={:?}, ui.meter_style={:?}, deepgram.endpointing_ms={}, deepgram.utterance_end_ms={}, deepgram.model={}, deepgram.language={}, deepgram.api_key_configured={}, transformation.enabled={}, transformation.hotkey={:?}, transformation.auto={}, transformation.provider={:?}, transformation.model={:?}, transformation.api_key_configured={}, transformation.system_prompt_configured={}, transformation.correction_system_prompt_configured={})",
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
        runtime_config.resolve_transformation_config().is_ok(),
        Some(runtime_config.transformation.hotkey.as_str()),
        runtime_config.transformation.auto,
        runtime_config.transformation.provider,
        &runtime_config.transformation.model,
        runtime_config.transformation.api_key.as_deref().map(str::trim).map(|value| !value.is_empty()).unwrap_or(false),
        !runtime_config.transformation.system_prompt.trim().is_empty(),
        !runtime_config.transformation.correction_system_prompt.trim().is_empty(),
    );

    let config_store =
        settings::LiveConfigStore::new(loaded_config.clone(), runtime_config.clone(), config_path);
    let startup_hotkey_permissions = permissions::GlobalHotkeyPermissions::current();

    let hotkey_capture_controller = hotkey_capture::HotkeyCaptureController::new();
    let transformation_models_controller =
        transformation_models::TransformationModelsController::new();
    let deepgram_connection_controller = deepgram_connection::DeepgramConnectionController::new();
    let shared_state = state::AppState::new();

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
        hotkey_capture_controller,
        transformation_models_controller,
        deepgram_connection_controller,
        audio_controller,
        shared_state,
    );
    ns_app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    app::setup_status_polling(&delegate);

    log::info!("starting NSApplication run loop");
    ns_app.run();
    Ok(())
}

fn report_startup_error(error: &str) {
    log::error!("startup failed: {}", error);
    let config_path = config::config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|path_error| format!("unavailable ({path_error})"));
    let executable_path = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|path_error| format!("unavailable ({path_error})"));

    app::show_startup_error_dialog(
        "simple-ptt couldn't start",
        &startup_error_instructions(error, &config_path, &executable_path),
    );
}

fn startup_error_instructions(error: &str, config_path: &str, executable_path: &str) -> String {
    format!(
        concat!(
            "{}\n\n",
            "What to check:\n",
            "This config file must exist and contain valid TOML:\n{}\n",
            "If it is missing, create its parent directory if needed and copy config.example.toml from the repository to that path. If you meant to use the normal home config, unset SIMPLE_PTT_CONFIG.\n",
            "- If mic.audio_device is configured, it matches a real input device\n\n",
            "For more detail, run this executable from Terminal (quote the path if it contains spaces):\n{}\n\n",
            "Note: simple-ptt is a menu bar app. On successful launch it appears in the menu bar, not in the Dock."
        ),
        error, config_path, executable_path
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
    fn startup_error_instructions_include_resolved_paths() {
        let config_path = "/Users/example/Library/Application Support/simple-ptt/config.toml";
        let executable_path = "/Applications/Custom Simple PTT.app/Contents/MacOS/simple-ptt";
        let instructions = startup_error_instructions(
            "configured audio_device 'Missing' was not found",
            config_path,
            executable_path,
        );

        assert!(instructions.contains(config_path));
        assert!(instructions.contains(executable_path));
        assert!(instructions.contains("menu bar"));
    }

    #[test]
    fn startup_error_instructions_explain_config_recovery_without_fixed_paths() {
        let config_path = "/tmp/simple-ptt/custom-config.toml";
        let executable_path = "/tmp/simple-ptt/bin/simple-ptt";
        let instructions =
            startup_error_instructions("missing config", config_path, executable_path);

        assert!(instructions.contains("copy config.example.toml"));
        assert!(instructions.contains(config_path));
        assert!(instructions.contains(executable_path));
        assert!(!instructions.contains("~/.config/simple-ptt/config.toml"));
        assert!(!instructions.contains("/Applications/simple-ptt.app/Contents/MacOS/simple-ptt"));
    }
}
