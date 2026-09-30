//! Build script.
//!
//! 1. Selects the deployment environment at build time from `LMURE_ENV`
//!    (`development` | `staging` | `production`). Default: `development` for debug builds, `production` for
//!    release builds. The value is baked into the binary as `LMURE_BUILD_ENV` and cannot be changed by
//!    editing the config file (security-relevant: e.g. the extra CA file is honoured only in development).
//! 2. Runs `tauri-build` with an explicit app manifest, so each application command gets its own
//!    permission and must be granted explicitly in `capabilities/*.json`.

// Cargo build-script directives are written to stdout by design.
#![allow(clippy::print_stdout)]

const ENV_VAR: &str = "LMURE_ENV";
const VALID_ENVIRONMENTS: [&str; 3] = ["development", "staging", "production"];

/// Application commands exposed to the WebView. Keep in sync with `src/commands.rs` and
/// `capabilities/default.json`.
const COMMANDS: &[&str] = &["get_app_info", "get_config", "set_mode"];

fn main() {
    println!("cargo:rerun-if-env-changed={ENV_VAR}");

    let profile = std::env::var("PROFILE").unwrap_or_default();
    let environment = match std::env::var(ENV_VAR) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_ascii_lowercase(),
        _ if profile == "release" => "production".to_owned(),
        _ => "development".to_owned(),
    };
    if !VALID_ENVIRONMENTS.contains(&environment.as_str()) {
        panic!("{ENV_VAR}={environment:?} is invalid; expected one of {VALID_ENVIRONMENTS:?}");
    }
    println!("cargo:rustc-env=LMURE_BUILD_ENV={environment}");

    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build failed: {error:#}");
    }
}
