//! Integration tests for the `AppCore` orchestration facade (Phase 2 Increment I-4).
//!
//! Blueprint coverage (§1, §2, §3, §9, §10, §12, §14):
//! - AppCore construction with settings, sink, and credential store.
//! - Operating mode switching (Driver <-> Engineer) with clean teardown of prior services.
//! - Synthetic Demo lifecycle, display-only invariant, and command routing.
//! - Driver run lifecycle using mock snapshot sources.
//! - Strict ordered shutdown (§10) and idempotency.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use transport::credentials::MemoryCredentialStore;

use app_core::bridge::sink::{SinkClosed, SubscriptionId, UiSink};
use app_core::bridge::status::{AppMode, Origin, UiEvent};
use app_core::core::{AppCore, CoreError};
use app_core::driver::reader_thread::SnapshotSource;
use app_core::settings::CoreSettings;
use lmu_reader::{
    LmuError, LockMode, RawSnapshot, ReaderDiagnostics, ReaderState, ReaderStats, SyncMode,
};

#[derive(Default)]
struct TestUiSink {
    batches: Mutex<Vec<(SubscriptionId, Vec<u8>)>>,
    events: Mutex<Vec<UiEvent>>,
}

impl TestUiSink {
    fn new() -> Self {
        Self {
            batches: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        }
    }
}

impl UiSink for TestUiSink {
    fn send_batch(&self, sub: SubscriptionId, batch: Vec<u8>) -> Result<(), SinkClosed> {
        self.batches.lock().push((sub, batch));
        Ok(())
    }

    fn emit(&self, event: UiEvent) {
        self.events.lock().push(event);
    }
}

struct DummySnapshotSource;

impl SnapshotSource for DummySnapshotSource {
    fn next_snapshot(&mut self, _timeout: Duration) -> Result<Option<RawSnapshot>, LmuError> {
        Ok(None)
    }

    fn diagnostics(&self) -> ReaderDiagnostics {
        ReaderDiagnostics {
            state: ReaderState::WaitingForLmu,
            connection_id: 0,
            lmu_pid: None,
            mapping_size: None,
            sync_mode: Some(SyncMode::FrameGate),
            sync_reason: None,
            lock_mode: Some(LockMode::SdkLock),
            connected_for_ns: None,
            since_new_frame_ns: None,
            lock_busy_word: None,
            lock_waiters_word: None,
            lock_unavailable_ns: None,
            lock_stuck: false,
            lock_set_event_failures: 0,
            note: None,
            last_error: None,
        }
    }

    fn stats(&self) -> ReaderStats {
        ReaderStats::default()
    }

    fn close(&mut self) {}
}

fn create_test_core() -> (Arc<AppCore>, Arc<TestUiSink>) {
    let mut settings = CoreSettings::default();
    settings.driver.recording_enabled = false;
    let sink = Arc::new(TestUiSink::new());
    let creds = Arc::new(MemoryCredentialStore::new());

    let core = Arc::new(AppCore::new(settings, sink.clone(), creds).expect("create app core"));
    (core, sink)
}

#[test]
fn test_core_initialization_and_status() {
    let (core, _) = create_test_core();

    assert_eq!(core.mode(), AppMode::Driver);
    let status = core.status();
    assert_eq!(status.mode, AppMode::Driver);
    assert_eq!(status.origin, Origin::Lmu);
    assert!(!core.is_driver_running());
    assert!(!core.is_engineer_running());
    #[cfg(feature = "demo")]
    assert!(!core.is_demo_running());

    core.shutdown().expect("clean shutdown");
}

#[test]
fn test_core_mode_switching_lifecycle() {
    let (core, _) = create_test_core();

    // 1. Switch from Driver to Engineer mode
    core.set_mode(AppMode::Engineer)
        .expect("switch to engineer");
    assert_eq!(core.mode(), AppMode::Engineer);
    assert!(core.is_engineer_running());
    assert!(!core.is_driver_running());

    // 2. Switch back to Driver mode
    core.set_mode(AppMode::Driver).expect("switch to driver");
    assert_eq!(core.mode(), AppMode::Driver);
    assert!(!core.is_engineer_running());

    core.shutdown().expect("clean shutdown");
}

#[cfg(feature = "demo")]
#[test]
fn test_core_demo_mode_lifecycle_and_commands() {
    let (core, _) = create_test_core();

    // 1. Start demo in driver mode
    core.start_demo(synthetic::Scenario::normal_lap(1))
        .expect("start demo");
    assert!(core.is_demo_running());
    assert_eq!(core.status().origin, Origin::Synthetic);

    // 2. Starting demo again must fail
    let second_start = core.start_demo(synthetic::Scenario::normal_lap(1));
    assert!(second_start.is_err());

    // 3. Send commands to demo pipeline
    core.driver_request_snapshot().expect("request snapshot");
    core.driver_add_marker("demo corner 1").expect("add marker");

    // 4. Stop demo cleanly
    core.stop_demo().expect("stop demo");
    assert!(!core.is_demo_running());
    assert_eq!(core.status().origin, Origin::Lmu);

    core.shutdown().expect("clean shutdown");
}

#[cfg(feature = "demo")]
#[test]
fn test_core_demo_invalid_in_engineer_mode() {
    let (core, _) = create_test_core();

    // Switch to Engineer mode
    core.set_mode(AppMode::Engineer)
        .expect("switch to engineer");

    // Attempting to start demo while in Engineer mode must return InvalidMode error
    let res = core.start_demo(synthetic::Scenario::normal_lap(1));
    match res {
        Err(CoreError::InvalidMode(AppMode::Engineer)) => {}
        other => panic!("expected InvalidMode(Engineer), got {:?}", other),
    }

    core.shutdown().expect("clean shutdown");
}

#[test]
fn test_core_driver_with_source_and_commands() {
    let (core, _) = create_test_core();

    // Start driver with dummy snapshot source
    core.start_driver_with_source(DummySnapshotSource)
        .expect("start driver with source");
    assert!(core.is_driver_running());

    // Cannot start again while running
    assert!(core.start_driver_with_source(DummySnapshotSource).is_err());

    // Send driver commands
    core.driver_request_snapshot().expect("request snapshot");
    core.driver_add_marker("test marker").expect("add marker");

    // Stop driver cleanly
    core.stop_driver().expect("stop driver");
    assert!(!core.is_driver_running());

    core.shutdown().expect("clean shutdown");
}

#[test]
fn test_core_shutdown_strict_and_idempotent() {
    let (core, _) = create_test_core();

    core.start_driver_with_source(DummySnapshotSource)
        .expect("start driver");
    assert!(core.is_driver_running());

    // First shutdown stops all services and records state
    core.shutdown().expect("first shutdown");
    assert!(!core.is_driver_running());

    // Second shutdown is a no-op and succeeds idempotently
    core.shutdown().expect("second shutdown idempotent");

    // Operations after shutdown must return Err(ShutDown)
    assert!(matches!(
        core.set_mode(AppMode::Engineer),
        Err(CoreError::ShutDown)
    ));
    assert!(matches!(
        core.start_driver_with_source(DummySnapshotSource),
        Err(CoreError::ShutDown)
    ));
    #[cfg(feature = "demo")]
    assert!(matches!(
        core.start_demo(synthetic::Scenario::normal_lap(1)),
        Err(CoreError::ShutDown)
    ));
}
