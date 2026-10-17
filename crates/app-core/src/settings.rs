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

use race_engines::fuel::{FuelWindow, ReserveMode};

use crate::bridge::status::AppMode;
use crate::driver::engines::EngineHubConfig;

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

    /// The fuel-per-lap window is not one the fuel engine supports (1, 2, 3, 5 or 10 laps).
    #[error("fuel window of {0} laps is not supported (supported: 1, 2, 3, 5, 10)")]
    UnsupportedFuelWindow(u32),

    /// The fuel reserve is not a finite number of laps in the accepted range.
    #[error("fuel reserve must be between 0 and 10 laps, got {0}")]
    InvalidFuelReserve(String),

    /// The engine configuration was rejected by the race engines.
    #[error("engine configuration rejected: {0}")]
    InvalidEngineConfig(String),
}

/// Fuel windows (laps) supported by [`race_engines::fuel::FuelWindow`].
pub const SUPPORTED_FUEL_WINDOW_LAPS: [u32; 5] = [1, 2, 3, 5, 10];

/// Accepted fuel reserve range in laps (inclusive).
pub const FUEL_RESERVE_LAPS_RANGE: (f64, f64) = (0.0, 10.0);

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

/// Race-engine settings applied to the driver pipeline's `EngineHub`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineSettings {
    /// Evaluate the default alert rules (fuel/energy reserve, stream age, packet loss, latency).
    pub default_alerts_enabled: bool,
    /// Laps averaged for fuel per lap. Only [`SUPPORTED_FUEL_WINDOW_LAPS`] are accepted.
    pub fuel_window_laps: u32,
    /// Fuel reserve in laps: the fuel engine's reserve and the low-fuel alert threshold.
    pub fuel_reserve_laps: f64,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            default_alerts_enabled: true,
            fuel_window_laps: 5,
            fuel_reserve_laps: 1.0,
        }
    }
}

impl EngineSettings {
    /// The fuel engine window for `fuel_window_laps`, or a typed error for an unsupported size.
    pub fn fuel_window(&self) -> Result<FuelWindow, SettingsError> {
        match self.fuel_window_laps {
            1 => Ok(FuelWindow::Laps1),
            2 => Ok(FuelWindow::Laps2),
            3 => Ok(FuelWindow::Laps3),
            5 => Ok(FuelWindow::Laps5),
            10 => Ok(FuelWindow::Laps10),
            other => Err(SettingsError::UnsupportedFuelWindow(other)),
        }
    }

    /// Validates the values (window size and reserve range).
    pub fn validate(&self) -> Result<(), SettingsError> {
        self.fuel_window()?;
        let (min, max) = FUEL_RESERVE_LAPS_RANGE;
        if !self.fuel_reserve_laps.is_finite()
            || self.fuel_reserve_laps < min
            || self.fuel_reserve_laps > max
        {
            return Err(SettingsError::InvalidFuelReserve(format!(
                "{}",
                self.fuel_reserve_laps
            )));
        }
        Ok(())
    }

    /// The `EngineHub` configuration for these settings: built-in defaults with the fuel window,
    /// fuel reserve (fuel engine and low-fuel alert) and the default-alert switch applied.
    pub fn to_engine_config(&self) -> Result<EngineHubConfig, SettingsError> {
        self.validate()?;
        let mut config = EngineHubConfig::default();
        config.fuel.window = self.fuel_window()?;
        config.fuel.reserve = ReserveMode::Laps(self.fuel_reserve_laps);
        config.alert.include_default_rules = self.default_alerts_enabled;
        config.alert.fuel_reserve_laps = self.fuel_reserve_laps;
        config
            .alert
            .validate()
            .map_err(|e| SettingsError::InvalidEngineConfig(e.to_string()))?;
        Ok(config)
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
    /// Race-engine configuration (fuel window/reserve, default alerts).
    #[serde(default)]
    pub engines: EngineSettings,
}

impl Default for CoreSettings {
    fn default() -> Self {
        Self {
            mode: AppMode::Driver,
            driver: DriverSettings::default(),
            network: NetworkSettings::default(),
            ui: UiSettings::default(),
            engines: EngineSettings::default(),
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
        self.engines.validate()?;
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
    fn supported_fuel_windows_map_to_engine_windows() {
        for laps in SUPPORTED_FUEL_WINDOW_LAPS {
            let engines = EngineSettings {
                fuel_window_laps: laps,
                ..EngineSettings::default()
            };
            let config = engines.to_engine_config().expect("supported window");
            assert_eq!(config.fuel.window.lap_count(), Some(laps as usize));
        }
    }

    #[test]
    fn unsupported_fuel_windows_are_typed_errors() {
        for laps in [0, 4, 6, 8, 11, 50] {
            let mut settings = CoreSettings::default();
            settings.engines.fuel_window_laps = laps;
            assert_eq!(
                settings.validate(),
                Err(SettingsError::UnsupportedFuelWindow(laps))
            );
            assert_eq!(
                settings.engines.to_engine_config().err(),
                Some(SettingsError::UnsupportedFuelWindow(laps))
            );
        }
    }

    #[test]
    fn fuel_reserve_and_default_alerts_are_applied() {
        let engines = EngineSettings {
            default_alerts_enabled: false,
            fuel_window_laps: 3,
            fuel_reserve_laps: 2.5,
        };
        let config = engines.to_engine_config().expect("valid");
        assert!(!config.alert.include_default_rules);
        assert!((config.alert.fuel_reserve_laps - 2.5).abs() < f64::EPSILON);
        assert_eq!(config.fuel.reserve, ReserveMode::Laps(2.5));
        assert_eq!(config.fuel.window, FuelWindow::Laps3);

        for bad in [-0.1, 10.5, f64::NAN, f64::INFINITY] {
            let engines = EngineSettings {
                fuel_reserve_laps: bad,
                ..EngineSettings::default()
            };
            assert!(matches!(
                engines.validate(),
                Err(SettingsError::InvalidFuelReserve(_))
            ));
        }
    }

    #[test]
    fn serialization_roundtrip() {
        let settings = CoreSettings::default();
        let json = serde_json::to_string(&settings).expect("serialize");
        let parsed: CoreSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(settings, parsed);
    }
}
