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
use app_core::driver::pipeline::CommandRejected;
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

    // 3. Send commands to demo pipeline: the demo is display-only, so recording commands and
    //    streaming are refused with typed errors (never silently ignored).
    core.driver_request_snapshot().expect("request snapshot");
    assert!(matches!(
        core.driver_add_marker("demo corner 1"),
        Err(CoreError::RecorderUnavailable)
    ));
    assert!(matches!(
        core.driver_set_recording(true),
        Err(CoreError::RecorderUnavailable)
    ));
    assert!(matches!(
        core.driver_start_streaming(),
        Err(CoreError::Rejected(CommandRejected::SyntheticNeverStreamed))
    ));
    assert!(core.driver_client().is_none());

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

    // Send driver commands: without a recorder or relay they are refused with typed errors.
    core.driver_request_snapshot().expect("request snapshot");
    assert!(matches!(
        core.driver_add_marker("test marker"),
        Err(CoreError::RecorderUnavailable)
    ));
    assert!(matches!(
        core.driver_set_recording(false),
        Err(CoreError::RecorderUnavailable)
    ));
    assert!(matches!(
        core.driver_start_streaming(),
        Err(CoreError::NetworkUnavailable)
    ));
    assert!(matches!(
        core.driver_stop_streaming(),
        Err(CoreError::NetworkUnavailable)
    ));
    assert!(matches!(
        core.driver_ack_alert(424_242),
        Err(CoreError::UnknownAlert(424_242))
    ));
    assert!(matches!(
        core.driver_request_connect_code(),
        Err(CoreError::NetworkUnavailable)
    ));
    // Ending the session works without a relay (finishes the recording, nothing to end remotely).
    core.driver_end_session().expect("end session");

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

fn core_with_settings(settings: CoreSettings) -> (Arc<AppCore>, Arc<TestUiSink>) {
    let sink = Arc::new(TestUiSink::new());
    let creds = Arc::new(MemoryCredentialStore::new());
    let core = Arc::new(AppCore::new(settings, sink.clone(), creds).expect("create app core"));
    (core, sink)
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

/// DEFERRED 21: no hard-coded idle snapshot; what is not observed is unknown, never "ok".
#[test]
fn test_status_reports_unknown_and_real_states() {
    let (core, sink) = create_test_core();

    // Idle driver mode: LMU not observed, no relay configured.
    let idle = core.status();
    assert_eq!(idle.lmu.state, "UNKNOWN");
    assert_eq!(idle.network.state, "NOT_CONFIGURED");
    assert!(!idle.network.streaming);
    assert!(idle.network.connect_code.is_none());
    assert!(idle.engineer.is_none());

    // Running driver pipeline: the reader reports WaitingForLmu -> NOT_RUNNING (real state),
    // recording disabled in these settings.
    core.start_driver_with_source(DummySnapshotSource)
        .expect("start driver");
    assert!(wait_until(Duration::from_secs(3), || core
        .status()
        .lmu
        .state
        == "NOT_RUNNING"));
    let running = core.status();
    assert_eq!(running.recording.state, "disabled");
    assert_eq!(running.network.state, "NOT_CONFIGURED");
    assert!(
        sink.events
            .lock()
            .iter()
            .any(|e| matches!(e, UiEvent::Status(s) if s.lmu.state == "NOT_RUNNING")),
        "the pipeline emits the real status as core:status"
    );
    core.stop_driver().expect("stop driver");

    // Engineer mode: phase idle, link disconnected, driver's LMU/recording unknown.
    core.set_mode(AppMode::Engineer).expect("engineer mode");
    let eng = core.status();
    assert_eq!(eng.mode, AppMode::Engineer);
    let engineer = eng.engineer.expect("engineer block");
    assert_eq!(engineer.phase, "idle");
    assert_eq!(engineer.stream_health, "no_data");
    assert!(engineer.stream_age_s.is_none());
    assert_eq!(eng.network.state, "DISCONNECTED");
    assert_eq!(eng.lmu.state, "UNKNOWN");
    assert_eq!(eng.recording.state, "unknown");
    assert!(wait_until(Duration::from_secs(2), || {
        sink.events
            .lock()
            .iter()
            .any(|e| matches!(e, UiEvent::Status(s) if s.mode == AppMode::Engineer))
    }));
    assert!(
        sink.events
            .lock()
            .iter()
            .any(|e| matches!(e, UiEvent::Engineer(d) if d.phase == "idle"))
    );

    core.shutdown().expect("shutdown");
}

/// DEFERRED 20: with a relay configured the live pipeline gets an idle relay link; streaming
/// needs a driver login (auto-stream is off by default) and network commands need a stream.
#[test]
fn test_relay_link_is_idle_until_streaming_and_requires_login() {
    let mut settings = CoreSettings::default();
    settings.driver.recording_enabled = false;
    settings.network.relay_url = Some("127.0.0.1:9".to_owned());
    let (core, _) = core_with_settings(settings);
    assert!(core.relay_configured());

    core.start_driver_with_source(DummySnapshotSource)
        .expect("start driver");
    assert!(core.driver_network_attached());
    assert!(core.driver_client().is_some());
    std::thread::sleep(Duration::from_millis(400));
    let status = core.status();
    assert_eq!(status.network.state, "DISCONNECTED");
    assert!(!status.network.streaming);
    assert!(core.driver_connect_code().is_none());

    // Not logged in: typed refusal, the link stays idle.
    assert!(matches!(
        core.driver_start_streaming(),
        Err(CoreError::NotLoggedIn)
    ));
    // No stream: new code / kick are refused by the transport with a typed error.
    assert!(matches!(
        core.driver_request_connect_code(),
        Err(CoreError::NotStreaming)
    ));
    assert!(matches!(
        core.driver_kick_engineer(1),
        Err(CoreError::NotStreaming)
    ));
    assert_eq!(core.status().network.state, "DISCONNECTED");

    core.shutdown().expect("shutdown");
}

/// DEFERRED 23: unsupported fuel windows are typed configuration errors.
#[test]
fn test_engine_settings_are_validated() {
    let mut settings = CoreSettings::default();
    settings.engines.fuel_window_laps = 4;
    let sink = Arc::new(TestUiSink::new());
    let creds = Arc::new(MemoryCredentialStore::new());
    match AppCore::new(settings, sink, creds) {
        Err(CoreError::Settings(app_core::settings::SettingsError::UnsupportedFuelWindow(4))) => {}
        Err(other) => panic!("expected UnsupportedFuelWindow(4), got {other:?}"),
        Ok(_) => panic!("a 4-lap fuel window must be rejected"),
    }

    let (core, _) = create_test_core();
    let mut bad = core.settings();
    bad.engines.fuel_window_laps = 7;
    assert!(matches!(
        core.apply_settings(bad),
        Err(CoreError::Settings(
            app_core::settings::SettingsError::UnsupportedFuelWindow(7)
        ))
    ));
    let mut good = core.settings();
    good.engines.fuel_window_laps = 10;
    good.engines.default_alerts_enabled = false;
    core.apply_settings(good)
        .expect("10-lap window is supported");
    core.start_driver_with_source(DummySnapshotSource)
        .expect("driver starts with the applied engine settings");
    core.shutdown().expect("shutdown");
}

/// DEFERRED 25: `engineer_cancel` interrupts a join that is still connecting.
#[test]
fn test_engineer_cancel_interrupts_a_join_in_progress() {
    // A "relay" that accepts TCP but never answers: the join hangs in the TLS handshake.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blackhole");
    let port = listener.local_addr().expect("addr").port();
    let held = Arc::new(Mutex::new(Vec::new()));
    let held_accept = Arc::clone(&held);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            held_accept.lock().push(stream);
        }
    });

    let mut settings = CoreSettings {
        mode: AppMode::Engineer,
        ..CoreSettings::default()
    };
    settings.network.relay_url = Some(format!("127.0.0.1:{port}"));
    let (core, _) = core_with_settings(settings);
    core.start_engineer().expect("engineer services");

    let joiner = Arc::clone(&core);
    let started = std::time::Instant::now();
    let join = std::thread::spawn(move || joiner.engineer_join_unconfirmed("7KQ4-MZ9T"));

    assert!(
        wait_until(Duration::from_secs(5), || core.engineer_client().is_some()),
        "the client is stored before the join is awaited"
    );
    std::thread::sleep(Duration::from_millis(300));
    // A second join while one is in progress is refused, not orphaning the first.
    assert!(matches!(
        core.engineer_join_unconfirmed("7KQ4-MZ9T"),
        Err(CoreError::Join(transport::engineer::JoinError::Busy))
    ));

    core.engineer_cancel().expect("cancel during join");
    let result = join.join().expect("join thread");
    assert!(
        matches!(
            result,
            Err(CoreError::Join(transport::engineer::JoinError::Cancelled))
        ),
        "join must end as cancelled, got {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancel must interrupt the join promptly"
    );
    assert!(core.engineer_client().is_none());
    assert_eq!(core.status().engineer.expect("engineer").phase, "idle");

    core.shutdown().expect("shutdown");
    drop(held);
}
