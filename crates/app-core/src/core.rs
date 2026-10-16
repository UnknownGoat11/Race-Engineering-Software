//! `AppCore`: the facade the Tauri adapter talks to.
//!
//! Owns the tokio `Runtime` (2 workers), the [`crate::clock`] `AppClock`, the driver and engineer runs, the
//! mode switch and the shutdown order. Planned constructor: `AppCore::new(CoreSettings, Arc<dyn UiSink>,
//! Arc<dyn CredentialStore>)`.
//!
//! - Login and run setup go through transport's `AuthManager`, `DriverClient::spawn` and
//!   `EngineerClient::spawn`.
//! - A mode switch shuts down the other mode's clients before starting the new one.
//! - Shutdown order: stop UI subscriptions, stop the reader (join within 1 s), drain the pipeline,
//!   `RecordingSession::finish`, `DriverClient::shutdown` (keeps the resume token), engineer `shutdown`,
//!   flush logs.
//! - A pipeline run has an immutable origin (`Lmu` or `Synthetic`). A synthetic run is built without
//!   recorder and network sinks, so demo data can never be recorded or streamed (owner decision Q1).
//!
//! Stub: implemented in increment I-4.
