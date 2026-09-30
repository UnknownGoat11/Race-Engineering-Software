//! Tauri commands: the complete, explicit IPC surface of the WebView.
//!
//! Rules: one typed command per operation, no generic "execute"/"invoke anything" entry point, no shell or
//! filesystem access for the frontend. Every command must also be listed in `build.rs` (`COMMANDS`) and granted
//! in `capabilities/default.json`, and mirrored in `src/lib/ipc.ts`.

use serde::Serialize;
use tauri::State;

use crate::app_info::AppInfo;
use crate::config::{ConfigError, Mode};
use crate::logging::component;
use crate::state::{AppState, ConfigSnapshot};

/// Error payload returned to the frontend: a stable machine-readable `code` and a human-readable `message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl From<ConfigError> for CommandError {
    fn from(error: ConfigError) -> Self {
        let code = match error {
            ConfigError::Invalid(_) => "config_invalid",
            ConfigError::Serialize(_) => "config_serialize_failed",
            ConfigError::Io { .. } => "config_write_failed",
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

/// Application name, version, build profile and build environment.
#[tauri::command]
pub fn get_app_info(state: State<'_, AppState>) -> AppInfo {
    state.app_info()
}

/// Current configuration, its path, and any start-up load issue.
#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> ConfigSnapshot {
    state.snapshot()
}

/// Persist the operating mode. Runs off the main thread (file write + fsync).
#[tauri::command(async)]
pub fn set_mode(state: State<'_, AppState>, mode: Mode) -> Result<ConfigSnapshot, CommandError> {
    state.set_mode(mode).map_err(|error| {
        tracing::error!(component = component::IPC, command = "set_mode", mode = mode.as_str(), error = %error, "command failed");
        CommandError::from(error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FieldError;

    #[test]
    fn command_error_serializes_code_and_message() {
        let error = CommandError::from(ConfigError::Invalid(vec![FieldError {
            field: "network.relay_port".to_owned(),
            message: "must be between 1 and 65535 (got 0)".to_owned(),
        }]));
        let value = serde_json::to_value(&error).expect("serialize");
        assert_eq!(value["code"], "config_invalid");
        assert!(
            value["message"]
                .as_str()
                .is_some_and(|m| m.contains("network.relay_port"))
        );
    }
}
