//! LMU Race Engineering desktop application (Tauri 2 shell).
//!
//! This crate is intentionally thin: window/IPC wiring, configuration, and logging. Acquisition, engines,
//! transport and recording live in the workspace crates (`crates/*`) and are orchestrated via `app-core`.

pub mod app_info;
pub mod commands;
pub mod config;
pub mod core_bridge;
pub mod lifecycle;
pub mod logging;
pub mod state;

use std::sync::Arc;

use tauri::{Manager, RunEvent};

use app_core::bridge::sink::UiSink;
use app_core::bridge::status::{AppMode, ErrorDto, UiEvent};
use transport::credentials::{CredentialStore, KeyringCredentialStore};

use crate::config::{ConfigStore, Mode};
use crate::core_bridge::TauriUiSink;
use crate::lifecycle::CloseGuard;
use crate::logging::{LoggingGuard, component};
use crate::state::{AppState, Identity};

/// Sub-directory of the per-user app data directory that holds recordings by default.
const RECORDINGS_DIR: &str = "recordings";

/// Build and run the application. Returns only if the Tauri runtime could not be created.
pub fn run() -> Result<(), tauri::Error> {
    let app = tauri::Builder::default()
        // Must be the first plugin: a second launch exits before anything else starts (owner decision Q6).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            lifecycle::focus_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(CloseGuard::default())
        .on_window_event(lifecycle::on_window_event)
        .setup(|app| {
            let environment = app_info::build_environment();

            let log_dir = app.path().app_log_dir().ok();
            let logging = logging::init(log_dir.as_deref());
            logging::install_panic_hook();
            let log_dir_text = logging.log_dir.as_ref().map(|dir| dir.display().to_string());
            tracing::info!(
                component = component::APP,
                version = %app.package_info().version,
                environment = environment.as_str(),
                build_profile = if cfg!(debug_assertions) { "debug" } else { "release" },
                log_dir = log_dir_text.as_deref(),
                "starting"
            );
            if let Some(error) = &logging.error {
                tracing::warn!(component = component::APP, error = %error, "file logging unavailable");
            }
            app.manage(logging.guard);

            let store = ConfigStore::new(app.path().app_config_dir()?);
            let loaded = store.load(environment);
            let identity = Identity {
                version: app.package_info().version.to_string(),
                identifier: app.config().identifier.clone(),
                log_dir: logging.log_dir,
            };

            let recordings = app.path().app_data_dir()?.join(RECORDINGS_DIR);
            let sink = Arc::new(TauriUiSink::new(app.handle().clone()));
            let credentials: Arc<dyn CredentialStore> = Arc::new(KeyringCredentialStore::default());
            let state = AppState::build(
                store,
                loaded,
                environment,
                identity,
                recordings,
                Arc::clone(&sink),
                credentials,
            )
            .map_err(|e| tauri::Error::Io(std::io::Error::other(e.to_string())))?;

            // Start the saved mode's services (driver: LMU reader + automatic recording, Q9). Before the first
            // mode choice nothing runs. A failure never blocks start-up; it is logged and reported to the UI.
            if let Some(mode) = state.mode() {
                let core_mode = match mode {
                    Mode::Driver => AppMode::Driver,
                    Mode::Engineer => AppMode::Engineer,
                };
                if let Err(error) = state.core().activate_mode(core_mode) {
                    tracing::error!(component = component::APP, mode = mode.as_str(), error = %error, "core services did not start");
                    sink.emit(UiEvent::Error(ErrorDto {
                        category: "core".to_owned(),
                        message: format!("The {mode} services did not start: {error}"),
                        fatal: false,
                    }));
                }
            }

            if let Some(window) = app.get_webview_window(lifecycle::MAIN_WINDOW) {
                lifecycle::restore_window_geometry(&window, &state.window_geometry());
            }
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_config,
            commands::set_mode,
            commands::update_settings,
            commands::get_core_status,
            commands::test_relay_connection,
            commands::log_ui_error,
            commands::login,
            commands::register_account,
            commands::logout,
            commands::get_login_state,
            commands::driver_start_streaming,
            commands::driver_stop_streaming,
            commands::driver_end_session,
            commands::driver_new_code,
            commands::driver_kick_engineer,
            commands::driver_set_recording,
            commands::driver_add_marker,
            commands::driver_ack_alert,
            commands::engineer_join,
            commands::engineer_confirm,
            commands::engineer_cancel,
            commands::engineer_disconnect,
            commands::engineer_request_snapshot,
            commands::demo_start,
            commands::demo_stop,
            commands::bridge_subscribe,
            commands::bridge_unsubscribe,
        ])
        .build(tauri::generate_context!())?;

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            tracing::info!(component = component::APP, "exiting");
            if let Some(state) = handle.try_state::<AppState>()
                && let Err(error) = state.core().shutdown()
            {
                tracing::error!(component = component::APP, error = %error, "core shutdown failed");
            }
            if let Some(guard) = handle.try_state::<LoggingGuard>() {
                guard.flush_and_close();
            }
        }
    });
    Ok(())
}
