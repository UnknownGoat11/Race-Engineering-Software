//! Build-time application facts: version, build profile and deployment environment.

use serde::Serialize;

use crate::config::Environment;

/// Environment baked in by `build.rs` from `LMURE_ENV` (validated there).
const BUILD_ENVIRONMENT: &str = env!("LMURE_BUILD_ENV");

/// Human-readable product name (matches `productName` in `tauri.conf.json`).
pub const PRODUCT_NAME: &str = "Race Engineering Tool";

/// The deployment environment this binary was built for.
///
/// This is authoritative: it cannot be overridden by the config file or at runtime. If the baked value were
/// ever unparseable (it is validated by `build.rs`), the most restrictive environment is assumed.
pub fn build_environment() -> Environment {
    Environment::parse(BUILD_ENVIRONMENT).unwrap_or(Environment::Production)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildProfile {
    Debug,
    Release,
}

impl BuildProfile {
    pub const fn current() -> Self {
        if cfg!(debug_assertions) {
            BuildProfile::Debug
        } else {
            BuildProfile::Release
        }
    }
}

/// Payload of the `get_app_info` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub identifier: String,
    pub build_profile: BuildProfile,
    pub environment: Environment,
    /// Directory of the JSON log files, if logging to file could be initialised.
    pub log_dir: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_environment_matches_profile_default_or_override() {
        let env = build_environment();
        match std::option_env!("LMURE_ENV") {
            // Explicit override at build time.
            Some(value) if !value.trim().is_empty() => {
                assert_eq!(Some(env), Environment::parse(value.trim()));
            }
            // Tests are built with the debug profile unless --release is used.
            _ if cfg!(debug_assertions) => assert_eq!(env, Environment::Development),
            _ => assert_eq!(env, Environment::Production),
        }
    }

    #[test]
    fn build_profile_serializes_lowercase() {
        let json = serde_json::to_string(&BuildProfile::Release).expect("serialize");
        assert_eq!(json, "\"release\"");
    }
}
