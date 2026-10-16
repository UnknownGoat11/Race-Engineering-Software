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
/// Windows application manifest, relative to the crate root.
const WINDOWS_MANIFEST: &str = "windows-app-manifest.xml";

/// Application commands exposed to the WebView. Keep in sync with `src/commands.rs` and
/// `capabilities/default.json`.
const COMMANDS: &[&str] = &[
    "get_app_info",
    "get_config",
    "set_mode",
    "update_settings",
    "get_core_status",
    "test_relay_connection",
    "log_ui_error",
    "login",
    "register_account",
    "logout",
    "get_login_state",
    "driver_start_streaming",
    "driver_stop_streaming",
    "driver_end_session",
    "driver_new_code",
    "driver_kick_engineer",
    "driver_set_recording",
    "driver_add_marker",
    "driver_ack_alert",
    "engineer_join",
    "engineer_confirm",
    "engineer_cancel",
    "engineer_disconnect",
    "engineer_request_snapshot",
    "demo_start",
    "demo_stop",
    "bridge_subscribe",
    "bridge_unsubscribe",
];

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

    // The Windows application manifest (Common Controls v6, required by the native confirm dialog) is embedded
    // by the linker for every linked target, so the unit-test executables get it as well; tauri-build would
    // only put it into the application binary's resources.
    let mut windows = tauri_build::WindowsAttributes::new();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let manifest = std::path::Path::new(&manifest_dir).join(WINDOWS_MANIFEST);
        println!("cargo:rerun-if-changed={WINDOWS_MANIFEST}");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }

    let attributes = tauri_build::Attributes::new()
        .windows_attributes(windows)
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build failed: {error:#}");
    }
}
