//! LMU Race Engineering desktop application (Tauri 2 shell).
//!
//! This crate is intentionally thin: window/IPC wiring, configuration, and logging. Acquisition, engines,
//! transport and recording live in the workspace crates (`crates/*`) and are orchestrated via `app-core`.

pub mod app_info;
pub mod commands;
pub mod config;
pub mod logging;
pub mod state;

use tauri::{Manager, RunEvent};

use crate::config::ConfigStore;
use crate::logging::{LoggingGuard, component};
use crate::state::{AppState, Identity};

/// Build and run the application. Returns only if the Tauri runtime could not be created.
pub fn run() -> Result<(), tauri::Error> {
    let app = tauri::Builder::default()
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
            app.manage(AppState::new(store, loaded, environment, identity));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_config,
            commands::set_mode
        ])
        .build(tauri::generate_context!())?;

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            tracing::info!(component = component::APP, "exiting");
            if let Some(guard) = handle.try_state::<LoggingGuard>() {
                guard.flush_and_close();
            }
        }
    });
    Ok(())
}
