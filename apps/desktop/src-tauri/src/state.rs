//! Application state shared by the Tauri commands.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::Serialize;

use crate::app_info::{AppInfo, BuildProfile, PRODUCT_NAME};
use crate::config::{
    AppConfig, ConfigError, ConfigIssue, ConfigStore, Environment, LoadOutcome, Mode,
};
use crate::logging::component;

/// Payload of `get_config` / `set_mode`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigSnapshot {
    pub config: AppConfig,
    /// Absolute path of the config file.
    pub path: String,
    /// Present when the stored file could not be used at start-up (safe defaults are active).
    pub issue: Option<ConfigIssue>,
}

/// Static facts about this installation, resolved once at start-up.
#[derive(Debug, Clone)]
pub struct Identity {
    pub version: String,
    pub identifier: String,
    pub log_dir: Option<PathBuf>,
}

pub struct AppState {
    store: ConfigStore,
    config: Mutex<AppConfig>,
    issue: Option<ConfigIssue>,
    environment: Environment,
    identity: Identity,
}

impl AppState {
    pub fn new(
        store: ConfigStore,
        loaded: LoadOutcome,
        environment: Environment,
        identity: Identity,
    ) -> Self {
        Self {
            store,
            config: Mutex::new(loaded.config),
            issue: loaded.issue,
            environment,
            identity,
        }
    }

    fn current(&self) -> AppConfig {
        self.config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn snapshot_of(&self, config: AppConfig) -> ConfigSnapshot {
        ConfigSnapshot {
            config,
            path: self.store.path().display().to_string(),
            issue: self.issue.clone(),
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

    /// Persist a new operating mode. The in-memory config only changes if the file write succeeded.
    pub fn set_mode(&self, mode: Mode) -> Result<ConfigSnapshot, ConfigError> {
        let mut guard = self.config.lock().unwrap_or_else(PoisonError::into_inner);
        let previous = guard.application.mode;
        let mut next = guard.clone();
        next.application.mode = Some(mode);
        next.application.environment = self.environment;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigIssueKind;

    fn identity() -> Identity {
        Identity {
            version: "0.1.0".to_owned(),
            identifier: "com.lmurace.engineering".to_owned(),
            log_dir: None,
        }
    }

    #[test]
    fn set_mode_persists_and_survives_reload() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ConfigStore::new(dir.path());
        let state = AppState::new(
            store.clone(),
            store.load(Environment::Development),
            Environment::Development,
            identity(),
        );
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
        let state = AppState::new(store, loaded, Environment::Development, identity());

        let error = state.set_mode(Mode::Driver).expect_err("save must fail");
        assert!(matches!(error, ConfigError::Io { .. }), "{error}");
        assert_eq!(state.snapshot().config.application.mode, None);
    }

    #[test]
    fn load_issue_is_exposed_in_snapshots() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("config.json"), "{").expect("write");
        let store = ConfigStore::new(dir.path());
        let state = AppState::new(
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
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ConfigStore::new(dir.path());
        let state = AppState::new(
            store.clone(),
            store.load(Environment::Staging),
            Environment::Staging,
            identity(),
        );
        let info = state.app_info();
        assert_eq!(info.environment, Environment::Staging);
        assert_eq!(info.build_profile, BuildProfile::current());
        assert_eq!(info.name, "LMU Race Engineering");
        assert_eq!(info.version, "0.1.0");
    }
}
