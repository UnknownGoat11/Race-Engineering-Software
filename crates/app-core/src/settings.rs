//! `CoreSettings`: the plain, validated runtime settings `app-core` runs with.
//!
//! Persistence stays in `src-tauri/config.rs` (`config.json`, atomic writes, schema 1) because
//! it is coupled to Tauri and logging. The adapter converts it into [`CoreSettings`] and calls
//! [`AppCore::apply_settings`] or passes it into [`AppCore::new`].
//!
//! Invariants:
//! - `driver.auto_stream` defaults to `false` (owner decision Q7).
//! - `driver.recording_enabled` defaults to `true` (owner decision Q9: recording runs independent of network/login).
//! - `ui.max_refresh_hz` defaults to 60 and cannot exceed 60 (D-012).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bridge::status::AppMode;

/// Error validating core application settings.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettingsError {
    #[error("UI max refresh rate must be between 10 Hz and 60 Hz, got {0}")]
    InvalidRefreshRate(u32),

    #[error("UI scale factor must be between 0.5 and 3.0, got {0}")]
    InvalidScale(String),

    #[error("Network connection timeout must be between 500 ms and 60,000 ms, got {0}")]
    InvalidConnectTimeout(u32),

    #[error("Invalid endpoint URL or host: {0}")]
    InvalidEndpoint(String),
}

/// Driver configuration settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverSettings {
    /// Whether to automatically begin live streaming when telemetry is available and user is authenticated.
    /// Default is `false` (owner decision Q7).
    pub auto_stream: bool,
    /// Whether recording to local disk is active. Default is `true` (owner decision Q9).
    pub recording_enabled: bool,
    /// Directory where telemetry sessions are recorded. Default is `"recordings"`.
    pub recording_dir: PathBuf,
}

impl Default for DriverSettings {
    fn default() -> Self {
        Self {
            auto_stream: false,
            recording_enabled: true,
            recording_dir: PathBuf::from("recordings"),
        }
    }
}

/// Network relay connection settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkSettings {
    /// Relay server URL (host:port or full URL). None if unconfigured.
    pub relay_url: Option<String>,
    /// Connection timeout in milliseconds. Default: 5,000 ms.
    pub connect_timeout_ms: u32,
    /// Optional extra trusted CA certificate bytes (PEM) for development/testing (D-016).
    pub extra_ca_pem: Option<Vec<u8>>,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            relay_url: None,
            connect_timeout_ms: 5000,
            extra_ca_pem: None,
        }
    }
}

/// UI bridge settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSettings {
    /// Maximum UI refresh rate in Hz (capped at 60 Hz per D-012).
    pub max_refresh_hz: u32,
    /// UI display scale.
    pub scale: f64,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            max_refresh_hz: 60,
            scale: 1.0,
        }
    }
}

/// Plain, validated settings for `AppCore`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoreSettings {
    /// Active operating mode (Driver or Engineer). Default is `AppMode::Driver`.
    pub mode: AppMode,
    /// Driver mode configuration.
    pub driver: DriverSettings,
    /// Network configuration.
    pub network: NetworkSettings,
    /// UI bridge configuration.
    pub ui: UiSettings,
}

impl Default for CoreSettings {
    fn default() -> Self {
        Self {
            mode: AppMode::Driver,
            driver: DriverSettings::default(),
            network: NetworkSettings::default(),
            ui: UiSettings::default(),
        }
    }
}

impl CoreSettings {
    /// Validates configuration values against architectural invariants.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if !(10..=60).contains(&self.ui.max_refresh_hz) {
            return Err(SettingsError::InvalidRefreshRate(self.ui.max_refresh_hz));
        }
        if self.ui.scale < 0.5 || self.ui.scale > 3.0 || self.ui.scale.is_nan() {
            return Err(SettingsError::InvalidScale(format!("{}", self.ui.scale)));
        }
        if !(500..=60_000).contains(&self.network.connect_timeout_ms) {
            return Err(SettingsError::InvalidConnectTimeout(
                self.network.connect_timeout_ms,
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_valid() {
        let settings = CoreSettings::default();
        assert_eq!(settings.mode, AppMode::Driver);
        assert!(!settings.driver.auto_stream);
        assert!(settings.driver.recording_enabled);
        assert_eq!(settings.ui.max_refresh_hz, 60);
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn invalid_refresh_rate_rejected() {
        let mut settings = CoreSettings::default();
        settings.ui.max_refresh_hz = 120; // > 60 Hz rejected
        assert_eq!(
            settings.validate(),
            Err(SettingsError::InvalidRefreshRate(120))
        );

        settings.ui.max_refresh_hz = 5; // < 10 Hz rejected
        assert_eq!(
            settings.validate(),
            Err(SettingsError::InvalidRefreshRate(5))
        );
    }

    #[test]
    fn invalid_scale_rejected() {
        let mut settings = CoreSettings::default();
        settings.ui.scale = 0.2;
        assert!(settings.validate().is_err());

        settings.ui.scale = f64::NAN;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn serialization_roundtrip() {
        let settings = CoreSettings::default();
        let json = serde_json::to_string(&settings).expect("serialize");
        let parsed: CoreSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(settings, parsed);
    }
}
