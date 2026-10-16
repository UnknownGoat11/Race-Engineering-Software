//! Headless orchestration core of the LMU Race Engineering desktop app (app-core blueprint).
//!
//! Everything the app does between LMU's shared memory, the relay and the WebView lives here, without a
//! `tauri` dependency, so the whole core is testable headlessly. `apps/desktop/src-tauri` is a thin adapter:
//! commands, a [`bridge::sink`] implementation over a Tauri IPC channel, and JSON event emission.
//!
//! Module tree (blueprint §3):
//! - [`core`]: the `AppCore` facade (tokio runtime, clock, driver and engineer runs, mode switch, shutdown
//!   order);
//! - [`clock`]: the `AppClock` shared by the reader, the transport and the recorder;
//! - [`hires`]: the high-resolution waitable timer behind the 60 Hz UI flush (the only unsafe code here);
//! - [`settings`]: plain `CoreSettings`;
//! - [`driver`]: acquisition, processing, recording and streaming on the driver PC;
//! - [`bridge`]: the UI data bridge (history, subscriptions, `UiBatch` pacing, JSON status DTOs);
//! - [`engineer`]: the engineer-side client (inbound stream, coalescing, ages, alerts).
//!
//! Owner rules enforced here: LMU is read-only (no LMU file or process access beyond `lmu-reader`), no
//! listeners, never auto-join an engineer session, demo data is never recorded or streamed (the
//! `demo` feature gates the synthetic provider), unavailable values are never published as zero, and no
//! `unwrap`/`expect` outside tests.

#![deny(clippy::unwrap_used)]
#![cfg_attr(not(test), deny(clippy::expect_used))]
#![deny(unsafe_code)]

pub mod bridge;
pub mod clock;
pub mod core;
pub mod driver;
pub mod engineer;
pub mod hires;
pub mod settings;

#[cfg(test)]
mod tests {
    /// Blueprint decision 1: app-core must stay testable headlessly, so it never depends on Tauri.
    #[test]
    fn manifest_has_no_tauri_dependency() {
        let manifest = include_str!("../Cargo.toml");
        let offending: Vec<&str> = manifest
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#'))
            .filter(|line| line.starts_with("tauri") || line.contains("\"tauri"))
            .collect();
        assert!(
            offending.is_empty(),
            "app-core must not depend on tauri: {offending:?}"
        );
    }

    /// Owner rule Q1: the synthetic provider is only reachable through the `demo` feature.
    #[test]
    fn synthetic_is_optional_and_demo_gated() {
        let manifest = include_str!("../Cargo.toml");
        assert!(
            manifest
                .lines()
                .any(|l| l.trim() == "synthetic = { workspace = true, optional = true }"),
            "synthetic must be an optional dependency"
        );
        assert!(
            manifest
                .lines()
                .any(|l| l.trim() == "demo = [\"dep:synthetic\"]"),
            "the demo feature must be what enables synthetic"
        );
    }
}
