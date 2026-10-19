//! Application state shared by the Tauri commands.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;

use app_core::bridge::sink::UiSink;
use app_core::core::{AppCore, CoreError};
use app_core::settings::CoreSettings;
use transport::credentials::CredentialStore;

use crate::app_info::{AppInfo, BuildProfile, PRODUCT_NAME};
use crate::config::{
    AppConfig, ConfigError, ConfigIssue, ConfigStore, CoreSettingsError, Environment, FieldError,
    LoadOutcome, Mode, WindowConfig,
};
use crate::core_bridge::TauriUiSink;
use crate::logging::component;

/// Payload of `get_config` / `set_mode` / `update_settings`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigSnapshot {
    pub config: AppConfig,
    /// Absolute path of the config file.
    pub path: String,
    /// Present when the stored file could not be used at start-up (safe defaults are active).
    pub issue: Option<ConfigIssue>,
    /// The saved relay settings (host, port, extra CA) differ from the ones the running core uses; they take
    /// effect after an app restart.
    pub restart_required: bool,
    /// Present when the core could not start with the saved relay settings and runs without a relay.
    pub core_issue: Option<String>,
}

/// Static facts about this installation, resolved once at start-up.
#[derive(Debug, Clone)]
pub struct Identity {
    pub version: String,
    pub identifier: String,
    pub log_dir: Option<PathBuf>,
}

/// Whether the relay can be used by one-shot commands (login, register, connectivity test).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayReadiness {
    Ready,
    /// No relay host configured (or the core runs without one, see `core_issue`).
    NotConfigured,
    /// The saved relay differs from the one the core was started with.
    RestartRequired,
}

/// The relay-related settings a core instance was built with.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CoreNetwork {
    relay_address: Option<String>,
    extra_ca_file: Option<PathBuf>,
}

impl CoreNetwork {
    fn of(config: &AppConfig) -> Self {
        Self {
            relay_address: config.network.relay_address(),
            extra_ca_file: config
                .network
                .trusted_extra_ca_file(config.application.environment)
                .map(Path::to_path_buf),
        }
    }
}

pub struct AppState {
    store: ConfigStore,
    config: Mutex<AppConfig>,
    issue: Option<ConfigIssue>,
    environment: Environment,
    identity: Identity,
    core: Arc<AppCore>,
    sink: Arc<TauriUiSink>,
    /// Recording directory used when `telemetry.recording_dir` is unset.
    default_recording_dir: PathBuf,
    /// Relay settings of the running core (`None` address: no relay).
    core_network: CoreNetwork,
    core_issue: Option<String>,
}

impl AppState {
    /// Build the state and its `AppCore`.
    ///
    /// The core is built from the loaded config. If the relay part cannot be used (unreadable extra CA file,
    /// endpoint rejected), the core starts without a relay instead of blocking start-up, and the reason is
    /// reported in [`ConfigSnapshot::core_issue`]. Any other core failure is returned.
    pub fn build(
        store: ConfigStore,
        loaded: LoadOutcome,
        environment: Environment,
        identity: Identity,
        default_recording_dir: PathBuf,
        sink: Arc<TauriUiSink>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, CoreError> {
        let config = &loaded.config;
        let ui_sink: Arc<dyn UiSink> = Arc::clone(&sink) as Arc<dyn UiSink>;
        let attempt = config
            .to_core_settings(&default_recording_dir)
            .map_err(|e| e.to_string())
            .and_then(|settings| {
                AppCore::new_with_environment(
                    settings,
                    environment.to_transport(),
                    Arc::clone(&ui_sink),
                    Arc::clone(&credentials),
                )
                .map_err(|e| e.to_string())
            });

        let (core, core_network, core_issue) = match attempt {
            Ok(core) => (core, CoreNetwork::of(config), None),
            Err(reason) => {
                tracing::error!(
                    component = component::APP,
                    error = %reason,
                    "core cannot use the saved relay settings; starting without a relay"
                );
                let mut fallback = config.clone();
                fallback.network.relay_host.clear();
                fallback.network.extra_ca_file = None;
                let settings = fallback
                    .to_core_settings(&default_recording_dir)
                    .map_err(|e| CoreError::Runtime(e.to_string()))?;
                let core = AppCore::new_with_environment(
                    settings,
                    environment.to_transport(),
                    ui_sink,
                    credentials,
                )?;
                let network = CoreNetwork {
                    relay_address: None,
                    extra_ca_file: None,
                };
                (core, network, Some(reason))
            }
        };

        Ok(Self {
            store,
            config: Mutex::new(loaded.config),
            issue: loaded.issue,
            environment,
            identity,
            core: Arc::new(core),
            sink,
            default_recording_dir,
            core_network,
            core_issue,
        })
    }

    pub fn core(&self) -> &Arc<AppCore> {
        &self.core
    }

    pub fn sink(&self) -> &Arc<TauriUiSink> {
        &self.sink
    }

    pub fn environment(&self) -> Environment {
        self.environment
    }

    fn current(&self) -> AppConfig {
        self.config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The saved operating mode (`None` until the user picked one).
    pub fn mode(&self) -> Option<Mode> {
        self.current().application.mode
    }

    fn snapshot_of(&self, config: AppConfig) -> ConfigSnapshot {
        let restart_required = CoreNetwork::of(&config) != self.core_network;
        ConfigSnapshot {
            config,
            path: self.store.path().display().to_string(),
            issue: self.issue.clone(),
            restart_required,
            core_issue: self.core_issue.clone(),
        }
    }

    pub fn app_info(&self) -> AppInfo {
        AppInfo {
            name: PRODUCT_NAME.to_owned(),
            version: self.identity.version.clone(),
            identifier: self.identity.identifier.clone(),
            build_profile: BuildProfile::current(),
            environment: self.environment,
            log_dir: self
                .identity
                .log_dir
                .as_ref()
                .map(|dir| dir.display().to_string()),
        }
    }

    pub fn snapshot(&self) -> ConfigSnapshot {
        self.snapshot_of(self.current())
    }

    /// Whether one-shot relay commands may run against the core's endpoint.
    pub fn relay_readiness(&self) -> RelayReadiness {
        if self.core_network.relay_address.is_none() {
            RelayReadiness::NotConfigured
        } else if CoreNetwork::of(&self.current()) != self.core_network {
            RelayReadiness::RestartRequired
        } else {
            RelayReadiness::Ready
        }
    }

    /// Persist a new operating mode. The in-memory config only changes if the file write succeeded. Starting the
    /// mode's services is a separate step ([`AppCore::activate_mode`]) so a core failure never loses the choice.
    pub fn set_mode(&self, mode: Mode) -> Result<ConfigSnapshot, ConfigError> {
        let mut guard = self.config.lock().unwrap_or_else(PoisonError::into_inner);
        let previous = guard.application.mode;
        let mut next = guard.clone();
        next.application.mode = Some(mode);
        next.validate().map_err(ConfigError::Invalid)?;
        self.store.save(&next)?;
        *guard = next.clone();
        drop(guard);
        tracing::info!(
            component = component::CONFIG,
            previous = previous.map(Mode::as_str),
            mode = mode.as_str(),
            "operating mode saved"
        );
        Ok(self.snapshot_of(next))
    }

    /// Validate and persist a full configuration from the UI and return it with the core settings to apply.
    ///
    /// * `application.mode` cannot change here (use `set_mode`, which also switches the core); a different value
    ///   is rejected.
    /// * `application.environment` is informational and always set to the build environment.
    /// * Everything the core needs is derived and checked before anything is written.
    pub fn update_settings(
        &self,
        mut next: AppConfig,
    ) -> Result<(ConfigSnapshot, CoreSettings), ConfigError> {
        let mut guard = self.config.lock().unwrap_or_else(PoisonError::into_inner);
        if next.application.mode != guard.application.mode {
            return Err(ConfigError::Invalid(vec![FieldError {
                field: "application.mode".to_owned(),
                message: "cannot be changed by update_settings; use set_mode".to_owned(),
            }]));
        }
        next.application.environment = self.environment;
        next.validate().map_err(ConfigError::Invalid)?;
        let core_settings =
            next.to_core_settings(&self.default_recording_dir)
                .map_err(|error| match error {
                    CoreSettingsError::ExtraCaUnreadable { .. } => {
                        ConfigError::Invalid(vec![FieldError {
                            field: "network.extra_ca_file".to_owned(),
                            message: error.to_string(),
                        }])
                    }
                })?;
        core_settings.validate().map_err(|error| {
            ConfigError::Invalid(vec![FieldError {
                field: "settings".to_owned(),
                message: error.to_string(),
            }])
        })?;
        self.store.save(&next)?;
        *guard = next.clone();
        drop(guard);
        tracing::info!(component = component::CONFIG, "settings saved");
        Ok((self.snapshot_of(next), core_settings))
    }

    /// Persist the main-window geometry (called when the window closes).
    pub fn save_window_geometry(&self, window: WindowConfig) -> Result<(), ConfigError> {
        let mut guard = self.config.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.window == window {
            return Ok(());
        }
        let mut next = guard.clone();
        next.window = window;
        next.validate().map_err(ConfigError::Invalid)?;
        self.store.save(&next)?;
        *guard = next;
        Ok(())
    }

    /// The saved window geometry.
    pub fn window_geometry(&self) -> WindowConfig {
        self.current().window
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::ConfigIssueKind;

    pub(crate) fn identity() -> Identity {
        Identity {
            version: "1.1.0".to_owned(),
            identifier: "com.lmurace.engineering".to_owned(),
            log_dir: None,
        }
    }

    pub(crate) fn test_state(
        store: ConfigStore,
        loaded: LoadOutcome,
        environment: Environment,
        identity: Identity,
    ) -> AppState {
        let sink = Arc::new(TauriUiSink::headless());
        let recordings = store.dir().join("recordings");
        let creds = Arc::new(transport::credentials::MemoryCredentialStore::new());
        AppState::build(
            store,
            loaded,
            environment,
            identity,
            recordings,
            sink,
            creds,
        )
        .expect("test state")
    }

    fn fresh_state(environment: Environment) -> (tempfile::TempDir, ConfigStore, AppState) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ConfigStore::new(dir.path());
        let state = test_state(
            store.clone(),
            store.load(environment),
            environment,
            identity(),
        );
        (dir, store, state)
    }

    #[test]
    fn set_mode_persists_and_survives_reload() {
        let (_dir, store, state) = fresh_state(Environment::Development);
        assert_eq!(state.snapshot().config.application.mode, None);

        let snapshot = state.set_mode(Mode::Engineer).expect("set mode");
        assert_eq!(snapshot.config.application.mode, Some(Mode::Engineer));
        assert_eq!(
            state.snapshot().config.application.mode,
            Some(Mode::Engineer)
        );

        let reloaded = store.load(Environment::Development);
        assert_eq!(reloaded.issue, None);
        assert_eq!(reloaded.config.application.mode, Some(Mode::Engineer));
    }

    #[test]
    fn failed_save_leaves_state_unchanged() {
        let dir = tempfile::tempdir().expect("temp dir");
        // A regular file where the config directory should be makes every save fail.
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").expect("write blocker");
        let store = ConfigStore::new(&blocker);
        let loaded = LoadOutcome {
            config: AppConfig::defaults_for(Environment::Development),
            issue: None,
        };
        let state = test_state(store, loaded, Environment::Development, identity());

        let error = state.set_mode(Mode::Driver).expect_err("save must fail");
        assert!(matches!(error, ConfigError::Io { .. }), "{error}");
        assert_eq!(state.snapshot().config.application.mode, None);
    }

    #[test]
    fn load_issue_is_exposed_in_snapshots() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("config.json"), "{").expect("write");
        let store = ConfigStore::new(dir.path());
        let state = test_state(
            store.clone(),
            store.load(Environment::Development),
            Environment::Development,
            identity(),
        );

        let issue = state.snapshot().issue.expect("issue");
        assert_eq!(issue.kind, ConfigIssueKind::Invalid);
        // Saving a new mode writes a fresh valid file; the preserved invalid copy stays untouched.
        state.set_mode(Mode::Driver).expect("set mode");
        assert_eq!(store.load(Environment::Development).issue, None);
        let preserved = issue.preserved_as.expect("preserved");
        assert_eq!(std::fs::read_to_string(preserved).expect("read"), "{");
    }

    #[test]
    fn app_info_reports_environment_and_profile() {
        let (_dir, _store, state) = fresh_state(Environment::Staging);
        let info = state.app_info();
        assert_eq!(info.environment, Environment::Staging);
        assert_eq!(info.build_profile, BuildProfile::current());
        assert_eq!(info.name, "LMU Race Engineering");
        assert_eq!(info.version, "1.1.0");
    }

    #[test]
    fn update_settings_persists_new_sections_and_derives_core_settings() {
        let (dir, store, state) = fresh_state(Environment::Development);
        let mut next = state.snapshot().config;
        next.driver.auto_stream = true;
        next.engines.default_alerts_enabled = false;
        next.window.width = Some(1280);
        next.application.environment = Environment::Production; // informational: overridden

        let (snapshot, core) = state.update_settings(next).expect("update");
        assert!(snapshot.config.driver.auto_stream);
        assert_eq!(
            snapshot.config.application.environment,
            Environment::Development
        );
        assert!(core.driver.auto_stream);
        assert_eq!(
            core.driver.recording_dir,
            dir.path().join("recordings"),
            "same default recording dir as at start-up (no double join)"
        );

        let reloaded = store.load(Environment::Development);
        assert_eq!(reloaded.issue, None);
        assert_eq!(reloaded.config, snapshot.config);
    }

    #[test]
    fn update_settings_rejects_mode_changes_and_invalid_values_without_writing() {
        let (_dir, store, state) = fresh_state(Environment::Development);
        let before = state.snapshot().config;

        let mut mode_change = before.clone();
        mode_change.application.mode = Some(Mode::Engineer);
        let error = state.update_settings(mode_change).expect_err("mode change");
        assert!(matches!(&error, ConfigError::Invalid(e) if e[0].field == "application.mode"));

        let mut invalid = before.clone();
        invalid.ui.max_refresh_hz = 120;
        let error = state.update_settings(invalid).expect_err("invalid");
        assert!(matches!(&error, ConfigError::Invalid(e) if e[0].field == "ui.max_refresh_hz"));

        let mut missing_ca = before.clone();
        missing_ca.network.extra_ca_file = Some(store.dir().join("absent.pem"));
        let error = state.update_settings(missing_ca).expect_err("missing CA");
        assert!(matches!(&error, ConfigError::Invalid(e) if e[0].field == "network.extra_ca_file"));

        assert_eq!(state.snapshot().config, before);
        assert!(!store.path().exists(), "nothing written");
    }

    #[test]
    fn relay_change_requires_restart_and_unset_relay_is_not_configured() {
        let (_dir, _store, state) = fresh_state(Environment::Development);
        assert_eq!(state.relay_readiness(), RelayReadiness::NotConfigured);
        assert!(!state.snapshot().restart_required);

        let mut next = state.snapshot().config;
        next.network.relay_host = "relay.example.com".to_owned();
        let (snapshot, _) = state.update_settings(next).expect("update");
        assert!(snapshot.restart_required);
        // The core still has no relay: it is not configured until restart.
        assert_eq!(state.relay_readiness(), RelayReadiness::NotConfigured);
    }

    #[test]
    fn relay_settings_of_the_running_core_are_ready_and_changes_need_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ConfigStore::new(dir.path());
        let mut config = AppConfig::defaults_for(Environment::Development);
        config.network.relay_host = "relay.example.com".to_owned();
        store.save(&config).expect("save");
        let state = test_state(
            store.clone(),
            store.load(Environment::Development),
            Environment::Development,
            identity(),
        );
        assert_eq!(state.relay_readiness(), RelayReadiness::Ready);
        assert_eq!(state.core().endpoint().host(), "relay.example.com");

        let mut next = state.snapshot().config;
        next.network.relay_port = 8443;
        state.update_settings(next).expect("update");
        assert_eq!(state.relay_readiness(), RelayReadiness::RestartRequired);
    }

    #[test]
    fn unusable_relay_settings_start_the_core_without_relay_and_report_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ConfigStore::new(dir.path());
        let mut config = AppConfig::defaults_for(Environment::Development);
        config.network.relay_host = "relay.example.com".to_owned();
        config.network.extra_ca_file = Some(dir.path().join("missing.pem"));
        let loaded = LoadOutcome {
            config,
            issue: None,
        };
        let state = test_state(store, loaded, Environment::Development, identity());
        let snapshot = state.snapshot();
        assert!(snapshot.core_issue.is_some_and(|m| m.contains("extra CA")));
        assert!(snapshot.restart_required);
        assert_eq!(state.relay_readiness(), RelayReadiness::NotConfigured);
    }

    #[test]
    fn window_geometry_roundtrips() {
        let (_dir, store, state) = fresh_state(Environment::Development);
        let geometry = WindowConfig {
            x: Some(10),
            y: Some(20),
            width: Some(1400),
            height: Some(900),
            maximized: false,
        };
        state.save_window_geometry(geometry.clone()).expect("save");
        assert_eq!(state.window_geometry(), geometry);
        assert_eq!(store.load(Environment::Development).config.window, geometry);

        let invalid = WindowConfig {
            width: Some(10),
            ..geometry.clone()
        };
        assert!(state.save_window_geometry(invalid).is_err());
        assert_eq!(state.window_geometry(), geometry);
    }
}
