//! End-to-end integration tests for AppCore (Phase 2 Increment I-5).
//!
//! Blueprint coverage (§12 Increment I-5):
//! - Two in-process cores (Driver Core and Engineer Core) communicating over real wire protocol
//!   via real in-process `lmu-relay` test harness with synthetic `FaultProxy` in `ProxyMode::Opaque`.
//! - 5 mandatory test scenarios:
//!   1. New code per connection (one-time code redeemed, new code generated, old code rejected).
//!   2. Third engineer refused (max 2 engineers enforced by relay; session full).
//!   3. Kick (driver kicks engineer; connection closed, slot freed, new code issued).
//!   4. Blip reconnect with resume ticket (proxy drops connection; in-memory ticket reconnects without fresh code).
//!   5. Relay restart (proxy restart with downtime; automatic reconnect and session resume).
//! - DEFERRED 20/21: AppCore's own driver relay link (auto-stream off, start/stop/end streaming, fresh code
//!   per connection after a real disconnect, relay outage never stops recording, status per state).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lmu_reader::{
    LmuError, LockMode, RawSnapshot, ReaderDiagnostics, ReaderState, ReaderStats, SyncMode,
};
use lmu_relay::config::PasswordHashParams;
use lmu_relay::store::{AccountRole, MemoryStore, Store};
use lmu_relay::tls::{
    DEV_CA_FILE, DEV_CERT_FILE, DEV_KEY_FILE, ReloadingTls, write_dev_certificates,
};
use lmu_relay::{RelayBuilder, RelayConfig, RelayHandle};
use protocol::messages::{ClientRole, EngineerLink, Secret};
use synthetic::{FaultProxy, FaultProxyConfig, ProxyMode};
use transport::ConnectionState;
use transport::credentials::MemoryCredentialStore;

use app_core::bridge::sink::{SinkClosed, SubscriptionId, UiSink};
use app_core::bridge::status::{AppMode, UiEvent};
use app_core::core::AppCore;
use app_core::driver::reader_thread::SnapshotSource;
use app_core::settings::CoreSettings;

const PASSWORD: &str = "correct horse battery staple";
const DRIVER: &str = "dave";
const ENGINEER: &str = "erin";
const ENGINEER2: &str = "eddie";
const ENGINEER3: &str = "emma";

#[derive(Default)]
struct TestUiSink {
    batches: parking_lot::Mutex<Vec<(SubscriptionId, Vec<u8>)>>,
    events: parking_lot::Mutex<Vec<UiEvent>>,
}

impl TestUiSink {
    fn new() -> Self {
        Self {
            batches: parking_lot::Mutex::new(Vec::new()),
            events: parking_lot::Mutex::new(Vec::new()),
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
    fn next_snapshot(&mut self, timeout: Duration) -> Result<Option<RawSnapshot>, LmuError> {
        std::thread::sleep(timeout.min(Duration::from_millis(50)));
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

struct TestRelay {
    handle: RelayHandle,
    ca_pem: Vec<u8>,
    addr: SocketAddr,
    dir: PathBuf,
}

impl TestRelay {
    async fn start() -> Self {
        let mut config = RelayConfig::development();
        config.auth.password_hash = PasswordHashParams::TEST_ONLY_FAST;
        config.timeouts.housekeeping_interval = Duration::from_millis(50);
        config.timeouts.heartbeat_degraded = Duration::from_secs(20);
        config.timeouts.heartbeat_timeout = Duration::from_secs(30);
        config.limits.code_redemptions_per_minute = 1_000;
        config.limits.code_redemptions_per_hour = 10_000;
        config.limits.auth_attempts_per_ip_per_minute = 1_000;

        let suffix = lmu_relay::secrets::random_u64().unwrap_or(42);
        let dir = std::env::temp_dir().join(format!("appcore-e2e-relay-{suffix:016x}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");

        write_dev_certificates(&dir, &["localhost".to_owned(), "127.0.0.1".to_owned()])
            .expect("write dev certificates");
        let tls = Arc::new(
            ReloadingTls::load(&dir.join(DEV_CERT_FILE), &dir.join(DEV_KEY_FILE))
                .expect("load tls"),
        );
        let store = Store::Memory(MemoryStore::new());
        let builder = RelayBuilder::new(config, store, tls);
        let handle = builder.start().await.expect("start relay");

        let relay = Self {
            addr: handle.local_addr(),
            ca_pem: std::fs::read(dir.join(DEV_CA_FILE)).expect("read ca pem"),
            handle,
            dir,
        };

        relay.create_accounts().await;
        relay
    }

    async fn create_accounts(&self) {
        for (name, display, role) in [
            (DRIVER, "Dave Driver", AccountRole::Driver),
            (ENGINEER, "Erin Engineer", AccountRole::Engineer),
            (ENGINEER2, "Eddie Engineer", AccountRole::Engineer),
            (ENGINEER3, "Emma Engineer", AccountRole::Engineer),
        ] {
            self.handle
                .auth()
                .create_account(name, display, role, &Secret::new(PASSWORD))
                .await
                .expect("create account");
        }
    }

    async fn shutdown(self) {
        self.handle.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn create_test_core(
    proxy_addr: SocketAddr,
    ca_pem: &[u8],
    mode: AppMode,
    username: &str,
    role: ClientRole,
) -> Arc<AppCore> {
    let settings = CoreSettings {
        mode,
        driver: app_core::settings::DriverSettings {
            recording_enabled: false,
            ..Default::default()
        },
        network: app_core::settings::NetworkSettings {
            relay_url: Some(format!("127.0.0.1:{}", proxy_addr.port())),
            extra_ca_pem: Some(ca_pem.to_vec()),
            ..Default::default()
        },
        ..Default::default()
    };

    let sink = Arc::new(TestUiSink::new());
    let creds = Arc::new(MemoryCredentialStore::new());
    let core = Arc::new(AppCore::new(settings, sink, creds).expect("create AppCore"));

    eprintln!("[core] Logging in as {username} ({role:?})...");
    let config = transport::ClientConfig::new(
        core.endpoint().clone(),
        env!("CARGO_PKG_VERSION"),
        format!("login-{username}"),
        core.clock().epoch(),
    );
    let transport = transport::TcpTlsTelemetryTransport::new();
    transport::login(
        &transport,
        &config,
        core.auth_manager(),
        role,
        username,
        Secret::new(PASSWORD),
    )
    .await
    .expect("login core");
    eprintln!("[core] Login complete for {username}!");

    core
}

async fn wait_for_connect_code(core: &AppCore) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(issued) = core.driver_connect_code().and_then(|cc| cc.code) {
            return issued.code.expose().to_string();
        }
        if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for connect code");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_new_code(core: &AppCore, old_code: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(issued) = core.driver_connect_code().and_then(|cc| cc.code) {
            let code_str = issued.code.expose().to_string();
            if code_str != old_code {
                return code_str;
            }
        }
        if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for new connect code");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_engineers_count(core: &AppCore, count: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if core.driver_engineers().len() == count {
            return;
        }
        if tokio::time::Instant::now() > deadline {
            let links: Vec<_> = core
                .driver_engineers()
                .iter()
                .map(|e| (e.connection_id, e.link))
                .collect();
            panic!(
                "timed out waiting for {} engineers, current count: {} (slots: {links:?})",
                count,
                links.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_engineer_connected(core: &AppCore) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if core
            .engineer_client()
            .is_some_and(|c| c.status().borrow().state == ConnectionState::Connected)
        {
            return;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for engineer client to be connected");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Scenario 1: New code per connection.
///
/// Driver starts and gets code A. Engineer 1 joins with code A. Relay issues new code B.
/// Old code A cannot be reused; Engineer 2 joining with code A fails.
/// Engineer 2 joins with code B successfully.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_new_code_per_connection() {
    eprintln!("[1] Starting relay...");
    let relay = TestRelay::start().await;
    eprintln!("[2] Starting proxy on {:?}", relay.addr);
    let mut proxy_cfg = FaultProxyConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)), relay.addr);
    proxy_cfg.mode = ProxyMode::Opaque;
    let proxy = FaultProxy::start(proxy_cfg).await.expect("start proxy");
    eprintln!("[2.1] Proxy listening on {:?}", proxy.local_addr());

    eprintln!("[3] Creating driver core...");
    let driver = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Driver,
        DRIVER,
        ClientRole::Driver,
    )
    .await;
    eprintln!("[4] Creating eng1 core...");
    let eng1 = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER,
        ClientRole::Engineer,
    )
    .await;
    eprintln!("[5] Creating eng2 core...");
    let eng2 = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER2,
        ClientRole::Engineer,
    )
    .await;

    eprintln!("[6] Starting driver pipeline...");
    driver
        .start_driver_with_source(DummySnapshotSource)
        .expect("start driver");
    eprintln!("[7] Spawning driver client...");
    driver.driver_start_streaming().expect("start streaming");

    eprintln!("[8] Waiting for connect code...");
    let code_a = wait_for_connect_code(&driver).await;
    eprintln!("[9] Got code A: {}", code_a);
    assert!(!code_a.is_empty());

    eprintln!("[10] Starting eng1...");
    eng1.start_engineer().expect("start eng1");
    eprintln!("[11] Joining eng1 with code A...");
    let card1 = eng1.engineer_join(&code_a).expect("eng1 join with code A");
    eprintln!("[12] Eng1 joined! Driver: {}", card1.driver_username);
    assert_eq!(card1.driver_username, DRIVER);
    wait_for_engineers_count(&driver, 1).await;

    // A fresh code B is issued after code A is redeemed
    let code_b = wait_for_new_code(&driver, &code_a).await;
    assert_ne!(code_a, code_b);

    // Old code A cannot be redeemed again
    eng2.start_engineer().expect("start eng2");
    let err = eng2.engineer_join(&code_a);
    assert!(err.is_err(), "old code A should be rejected");

    // New code B succeeds
    let card2 = eng2.engineer_join(&code_b).expect("eng2 join with code B");
    assert_eq!(card2.driver_username, DRIVER);
    wait_for_engineers_count(&driver, 2).await;

    eng1.shutdown().expect("eng1 shutdown");
    eng2.shutdown().expect("eng2 shutdown");
    driver.shutdown().expect("driver shutdown");
    proxy.shutdown().await;
    relay.shutdown().await;

    std::thread::spawn(move || {
        drop(eng1);
        drop(eng2);
        drop(driver);
    })
    .join()
    .expect("drop cores on dedicated thread");
}

/// Scenario 2: Third engineer refused.
///
/// Max 2 engineers allowed. Once 2 engineers join, session is full.
/// Attempt by a 3rd engineer to join is rejected with SessionFull.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_third_engineer_refused() {
    let relay = TestRelay::start().await;
    let mut proxy_cfg = FaultProxyConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)), relay.addr);
    proxy_cfg.mode = ProxyMode::Opaque;
    let proxy = FaultProxy::start(proxy_cfg).await.expect("start proxy");

    let driver = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Driver,
        DRIVER,
        ClientRole::Driver,
    )
    .await;
    let eng1 = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER,
        ClientRole::Engineer,
    )
    .await;
    let eng2 = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER2,
        ClientRole::Engineer,
    )
    .await;
    let eng3 = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER3,
        ClientRole::Engineer,
    )
    .await;

    driver
        .start_driver_with_source(DummySnapshotSource)
        .expect("start driver");
    driver.driver_start_streaming().expect("start streaming");

    let code_a = wait_for_connect_code(&driver).await;
    eng1.start_engineer().expect("start eng1");
    eng1.engineer_join(&code_a).expect("eng1 join");
    wait_for_engineers_count(&driver, 1).await;

    let code_b = wait_for_new_code(&driver, &code_a).await;
    eng2.start_engineer().expect("start eng2");
    eng2.engineer_join(&code_b).expect("eng2 join");
    wait_for_engineers_count(&driver, 2).await;

    // Both slots occupied. Driver connect code should be None (no slot available)
    tokio::time::sleep(Duration::from_millis(100)).await;
    let cc = driver.driver_connect_code();
    assert!(cc.as_ref().and_then(|c| c.code.as_ref()).is_none());

    // 3rd engineer attempting to join with any code fails with SessionFull
    eng3.start_engineer().expect("start eng3");
    let err = eng3.engineer_join(&code_b);
    assert!(err.is_err(), "third engineer must be refused");
    let err_str = format!("{err:?}");
    assert!(
        err_str.contains("SessionFull"),
        "error must indicate SessionFull, got: {err_str}"
    );

    eng1.shutdown().expect("eng1 shutdown");
    eng2.shutdown().expect("eng2 shutdown");
    eng3.shutdown().expect("eng3 shutdown");
    driver.shutdown().expect("driver shutdown");
    proxy.shutdown().await;
    relay.shutdown().await;

    std::thread::spawn(move || {
        drop(eng1);
        drop(eng2);
        drop(eng3);
        drop(driver);
    })
    .join()
    .expect("drop cores on dedicated thread");
}

/// Scenario 3: Kick.
///
/// Driver kicks engineer 2. Engineer 2's connection terminates and slot is freed.
/// Relay issues a new code for the freed slot.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_driver_kick_engineer() {
    let relay = TestRelay::start().await;
    let mut proxy_cfg = FaultProxyConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)), relay.addr);
    proxy_cfg.mode = ProxyMode::Opaque;
    let proxy = FaultProxy::start(proxy_cfg).await.expect("start proxy");

    let driver = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Driver,
        DRIVER,
        ClientRole::Driver,
    )
    .await;
    let eng1 = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER,
        ClientRole::Engineer,
    )
    .await;
    let eng2 = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER2,
        ClientRole::Engineer,
    )
    .await;

    driver
        .start_driver_with_source(DummySnapshotSource)
        .expect("start driver");
    driver.driver_start_streaming().expect("start streaming");

    let code_a = wait_for_connect_code(&driver).await;
    eng1.start_engineer().expect("start eng1");
    eng1.engineer_join(&code_a).expect("eng1 join");
    wait_for_engineers_count(&driver, 1).await;

    let code_b = wait_for_new_code(&driver, &code_a).await;
    eng2.start_engineer().expect("start eng2");
    eng2.engineer_join(&code_b).expect("eng2 join");
    wait_for_engineers_count(&driver, 2).await;

    // Kick engineer 2
    let engs = driver.driver_engineers();
    assert_eq!(engs.len(), 2);
    let eng2_conn_id = engs
        .iter()
        .find(|e| e.username == ENGINEER2)
        .expect("find eng2")
        .connection_id;

    driver
        .driver_kick_engineer(eng2_conn_id)
        .expect("driver kick engineer");

    // Wait for count to drop to 1
    wait_for_engineers_count(&driver, 1).await;

    // A new code is issued because slot 2 was freed
    let new_code = wait_for_connect_code(&driver).await;
    assert!(!new_code.is_empty());
    assert_ne!(new_code, code_a);
    assert_ne!(new_code, code_b);

    eng1.shutdown().expect("eng1 shutdown");
    eng2.shutdown().expect("eng2 shutdown");
    driver.shutdown().expect("driver shutdown");
    proxy.shutdown().await;
    relay.shutdown().await;

    std::thread::spawn(move || {
        drop(eng1);
        drop(eng2);
        drop(driver);
    })
    .join()
    .expect("drop cores on dedicated thread");
}

/// Scenario 4: Blip reconnect with resume ticket.
///
/// Proxy drops connections. Engineer 1 and Driver automatically reconnect using
/// in-memory tickets without needing a fresh connect code.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_blip_reconnect_with_resume_ticket() {
    let relay = TestRelay::start().await;
    let mut proxy_cfg = FaultProxyConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)), relay.addr);
    proxy_cfg.mode = ProxyMode::Opaque;
    let proxy = FaultProxy::start(proxy_cfg).await.expect("start proxy");

    let driver = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Driver,
        DRIVER,
        ClientRole::Driver,
    )
    .await;
    let eng1 = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER,
        ClientRole::Engineer,
    )
    .await;

    driver
        .start_driver_with_source(DummySnapshotSource)
        .expect("start driver");
    driver.driver_start_streaming().expect("start streaming");

    let code = wait_for_connect_code(&driver).await;
    eng1.start_engineer().expect("start eng1");
    eng1.engineer_join(&code).expect("eng1 join");
    wait_for_engineers_count(&driver, 1).await;
    wait_for_engineer_connected(&eng1).await;

    // Blip: drop all connections via proxy
    proxy.drop_connections();

    // Automatic reconnect: in-memory ticket resumes the session seamlessly
    wait_for_engineer_connected(&eng1).await;
    wait_for_engineers_count(&driver, 1).await;

    let engs = driver.driver_engineers();
    assert_eq!(engs.len(), 1);
    assert_eq!(engs[0].username, ENGINEER);
    assert_eq!(engs[0].link, EngineerLink::Healthy);

    eng1.shutdown().expect("eng1 shutdown");
    driver.shutdown().expect("driver shutdown");
    proxy.shutdown().await;
    relay.shutdown().await;

    std::thread::spawn(move || {
        drop(eng1);
        drop(driver);
    })
    .join()
    .expect("drop cores on dedicated thread");
}

/// Scenario 5: Relay restart.
///
/// Proxy restarts with simulated downtime (300 ms).
/// Clients reconnect and resume their session once the proxy accepts connections again.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_relay_restart() {
    let relay = TestRelay::start().await;
    let mut proxy_cfg = FaultProxyConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)), relay.addr);
    proxy_cfg.mode = ProxyMode::Opaque;
    let proxy = FaultProxy::start(proxy_cfg).await.expect("start proxy");

    let driver = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Driver,
        DRIVER,
        ClientRole::Driver,
    )
    .await;
    let eng1 = create_test_core(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER,
        ClientRole::Engineer,
    )
    .await;

    driver
        .start_driver_with_source(DummySnapshotSource)
        .expect("start driver");
    driver.driver_start_streaming().expect("start streaming");

    let code = wait_for_connect_code(&driver).await;
    eng1.start_engineer().expect("start eng1");
    eng1.engineer_join(&code).expect("eng1 join");
    wait_for_engineers_count(&driver, 1).await;
    wait_for_engineer_connected(&eng1).await;

    // Proxy restart with 300ms downtime: drops connections, refuses new ones for 300ms, then resumes
    proxy.restart(Duration::from_millis(300));

    // Clients reconnect and resume session
    wait_for_engineer_connected(&eng1).await;
    wait_for_engineers_count(&driver, 1).await;

    let engs = driver.driver_engineers();
    assert_eq!(engs.len(), 1);
    assert_eq!(engs[0].username, ENGINEER);
    assert_eq!(engs[0].link, EngineerLink::Healthy);

    eng1.shutdown().expect("eng1 shutdown");
    driver.shutdown().expect("driver shutdown");
    proxy.shutdown().await;
    relay.shutdown().await;

    std::thread::spawn(move || {
        drop(eng1);
        drop(driver);
    })
    .join()
    .expect("drop cores on dedicated thread");
}

// ---------------------------------------------------------------------------------------------
// DEFERRED 20/21: the driver relay path through AppCore (link, streaming, codes, status)
// ---------------------------------------------------------------------------------------------

/// Real LMU capture used as a paced live source (private recording on D:, never committed).
fn capture_path() -> PathBuf {
    PathBuf::from("D:/RaceEngineeringData/captures/pitstop1.lmuraw")
}

/// Replays a real capture at ~100 Hz with fresh capture times (loops at the end of the file).
struct PacedReplaySource {
    replay: lmu_reader::CaptureReplay<std::io::BufReader<std::fs::File>>,
    clock: app_core::clock::AppClock,
    seq: u64,
}

impl PacedReplaySource {
    fn open(clock: app_core::clock::AppClock) -> Self {
        Self {
            replay: lmu_reader::CaptureReplay::open(&capture_path()).expect("open capture"),
            clock,
            seq: 0,
        }
    }
}

impl SnapshotSource for PacedReplaySource {
    fn next_snapshot(&mut self, _timeout: Duration) -> Result<Option<RawSnapshot>, LmuError> {
        std::thread::sleep(Duration::from_millis(10));
        let next = match self.replay.next_snapshot().expect("read capture") {
            Some(snapshot) => snapshot,
            None => {
                self.replay =
                    lmu_reader::CaptureReplay::open(&capture_path()).expect("reopen capture");
                self.replay
                    .next_snapshot()
                    .expect("read capture")
                    .expect("capture has frames")
            }
        };
        let mut snapshot = next;
        self.seq += 1;
        snapshot.seq = self.seq;
        snapshot.capture_mono_ns = self.clock.now_ns();
        Ok(Some(snapshot))
    }

    fn diagnostics(&self) -> ReaderDiagnostics {
        let mut diag = DummySnapshotSource.diagnostics();
        diag.state = ReaderState::Connected;
        diag.connection_id = 1;
        diag
    }

    fn stats(&self) -> ReaderStats {
        ReaderStats::default()
    }

    fn close(&mut self) {}
}

async fn create_core_with(
    addr: SocketAddr,
    ca_pem: &[u8],
    mode: AppMode,
    username: &str,
    role: ClientRole,
    configure: impl FnOnce(&mut CoreSettings),
) -> Arc<AppCore> {
    let mut settings = CoreSettings {
        mode,
        driver: app_core::settings::DriverSettings {
            recording_enabled: false,
            ..Default::default()
        },
        network: app_core::settings::NetworkSettings {
            relay_url: Some(format!("127.0.0.1:{}", addr.port())),
            extra_ca_pem: Some(ca_pem.to_vec()),
            ..Default::default()
        },
        ..Default::default()
    };
    configure(&mut settings);
    let sink = Arc::new(TestUiSink::new());
    let creds = Arc::new(MemoryCredentialStore::new());
    let core = Arc::new(AppCore::new(settings, sink, creds).expect("create AppCore"));
    let config = transport::ClientConfig::new(
        core.endpoint().clone(),
        env!("CARGO_PKG_VERSION"),
        format!("login-{username}"),
        core.clock().epoch(),
    );
    let transport = transport::TcpTlsTelemetryTransport::new();
    transport::login(
        &transport,
        &config,
        core.auth_manager(),
        role,
        username,
        Secret::new(PASSWORD),
    )
    .await
    .expect("login core");
    core
}

async fn wait_status(
    core: &AppCore,
    what: &str,
    timeout: Duration,
    mut condition: impl FnMut(&app_core::bridge::status::CoreStatus) -> bool,
) -> app_core::bridge::status::CoreStatus {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let status = core.status();
        if condition(&status) {
            return status;
        }
        if tokio::time::Instant::now() > deadline {
            panic!(
                "timed out waiting for {what}: lmu={} network={} streaming={} recording={} engineer={:?}",
                status.lmu.state,
                status.network.state,
                status.network.streaming,
                status.recording.state,
                status
                    .engineer
                    .as_ref()
                    .map(|e| (&e.phase, &e.stream_health))
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_engineer_frames(core: &AppCore) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(engineer) = core.engineer_core() {
            let stats = engineer.lock().stats().clone();
            if stats.snapshots_received > 0 && stats.deltas_received > 0 {
                return;
            }
        }
        if tokio::time::Instant::now() > deadline {
            panic!("engineer received no live frames");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn drop_on_thread(cores: Vec<Arc<AppCore>>) {
    std::thread::spawn(move || drop(cores))
        .join()
        .expect("drop cores on dedicated thread");
}

/// Driver logs in and streams live LMU data through AppCore's own relay link (auto-stream off:
/// nothing connects until the driver starts streaming). An engineer joins with the driver's
/// one-time code and receives live frames. After a real disconnect the old codes are dead: only
/// a fresh code from `driver_new_code` reconnects. Status reflects every step.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_driver_streams_engineer_joins_and_needs_fresh_codes() {
    let relay = TestRelay::start().await;
    let driver = create_core_with(
        relay.addr,
        &relay.ca_pem,
        AppMode::Driver,
        DRIVER,
        ClientRole::Driver,
        |_| {},
    )
    .await;
    let eng = create_core_with(
        relay.addr,
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER,
        ClientRole::Engineer,
        |_| {},
    )
    .await;

    driver
        .start_driver_with_source(PacedReplaySource::open(*driver.clock()))
        .expect("start driver");

    // Auto-stream is off: LMU is live, the link exists but stays disconnected, no code.
    let status = wait_status(&driver, "LMU live", Duration::from_secs(10), |s| {
        s.lmu.state == "LIVE"
    })
    .await;
    assert_eq!(status.network.state, "DISCONNECTED");
    assert!(!status.network.streaming);
    assert!(status.network.connect_code.is_none());
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(driver.driver_connect_code().is_none(), "never auto-connect");

    driver.driver_start_streaming().expect("start streaming");
    let code_a = wait_for_connect_code(&driver).await;
    let status = wait_status(&driver, "streaming + code", Duration::from_secs(10), |s| {
        s.network.state == "CONNECTED" && s.network.streaming && s.network.connect_code.is_some()
    })
    .await;
    assert_eq!(status.network.engineers_connected, 0);
    assert_eq!(status.network.engineers_max, 2);
    assert!(status.network.code_expires_at_unix_s.is_some());
    let shown = status.network.connect_code.expect("code shown");
    assert_eq!(
        shown.replace('-', ""),
        code_a,
        "status shows the display form"
    );
    assert!(status.network.rtt_ms.is_some() || status.network.state == "CONNECTED");

    eng.start_engineer().expect("start engineer");
    let card = eng.engineer_join(&code_a).expect("join with code A");
    assert_eq!(card.driver_username, DRIVER);
    wait_for_engineers_count(&driver, 1).await;
    wait_engineer_frames(&eng).await;

    let eng_status = wait_status(&eng, "engineer monitoring", Duration::from_secs(10), |s| {
        s.engineer.as_ref().is_some_and(|e| {
            e.phase == "monitoring" && e.stream_health == "healthy" && e.stream_age_s.is_some()
        }) && s.network.state == "CONNECTED"
    })
    .await;
    assert_eq!(eng_status.mode, AppMode::Engineer);
    assert!(!eng_status.engineer.expect("engineer").driver_away);
    // The driver's LMU and recording state arrive over the relay (DRIVER_STATUS, 1 Hz).
    let eng_status = wait_status(&eng, "driver status relayed", Duration::from_secs(5), |s| {
        s.lmu.state == "LIVE"
    })
    .await;
    assert_eq!(eng_status.recording.state, "off");

    let status = wait_status(&driver, "engineer listed", Duration::from_secs(5), |s| {
        s.network.engineers_connected == 1 && s.network.engineers.len() == 1
    })
    .await;
    assert_eq!(status.network.engineers[0].username, ENGINEER);
    assert_eq!(status.network.engineers[0].link, "healthy");

    // A fresh code B replaced A once A was redeemed.
    let code_b = wait_for_new_code(&driver, &code_a).await;

    // Real disconnect: the engineer leaves; its slot frees.
    eng.engineer_disconnect().expect("disconnect");
    wait_for_engineers_count(&driver, 0).await;

    // Reconnecting with the redeemed code A fails.
    let err = eng.engineer_join(&code_a);
    assert!(
        matches!(
            err,
            Err(app_core::core::CoreError::Join(
                transport::engineer::JoinError::CodeInvalid
            ))
        ),
        "redeemed code must be refused, got {err:?}"
    );

    // The driver issues a new code: B is invalidated, C works.
    driver.driver_request_connect_code().expect("new code");
    let code_c = wait_for_new_code(&driver, &code_b).await;
    assert_ne!(code_c, code_a);
    let err = eng.engineer_join(&code_b);
    assert!(err.is_err(), "replaced code B must be refused");
    eng.engineer_join(&code_c)
        .expect("rejoin with fresh code C");
    wait_for_engineers_count(&driver, 1).await;

    // Stop streaming keeps the relay session (engineers stay attached), status follows.
    driver.driver_stop_streaming().expect("stop streaming");
    wait_status(&driver, "stopped", Duration::from_secs(5), |s| {
        !s.network.streaming && s.network.state == "DISCONNECTED"
    })
    .await;
    assert!(matches!(
        driver.driver_request_connect_code(),
        Err(app_core::core::CoreError::NotStreaming)
    ));

    // Streaming again resumes the same relay session: the engineer is still attached.
    driver.driver_start_streaming().expect("restart streaming");
    wait_status(&driver, "streaming again", Duration::from_secs(10), |s| {
        s.network.state == "CONNECTED" && s.network.streaming
    })
    .await;
    wait_for_engineers_count(&driver, 1).await;

    // End session: works for real (the engineer sees the driver ended it).
    driver.driver_end_session().expect("end session");
    wait_status(&eng, "engineer ended", Duration::from_secs(10), |s| {
        s.engineer.as_ref().is_some_and(|e| e.phase == "ended")
    })
    .await;

    eng.shutdown().expect("eng shutdown");
    driver.shutdown().expect("driver shutdown");
    relay.shutdown().await;
    drop_on_thread(vec![eng, driver]);
}

/// A relay outage never stops acquisition or recording; the network status shows the outage,
/// the engineer (connected directly to the relay) sees the driver away (DRIVER_PRESENCE), and
/// the driver resumes the same session afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_relay_outage_does_not_stop_recording() {
    let relay = TestRelay::start().await;
    let mut proxy_cfg = FaultProxyConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)), relay.addr);
    proxy_cfg.mode = ProxyMode::Opaque;
    let proxy = FaultProxy::start(proxy_cfg).await.expect("start proxy");

    let suffix = lmu_relay::secrets::random_u64().unwrap_or(7);
    let rec_dir = std::env::temp_dir().join(format!("appcore-e2e-rec-{suffix:016x}"));
    let rec_dir_cfg = rec_dir.clone();
    let driver = create_core_with(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Driver,
        DRIVER,
        ClientRole::Driver,
        move |s| {
            s.driver.recording_enabled = true;
            s.driver.recording_dir = rec_dir_cfg;
        },
    )
    .await;
    // The engineer bypasses the proxy: only the driver loses the relay.
    let eng = create_core_with(
        relay.addr,
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER,
        ClientRole::Engineer,
        |_| {},
    )
    .await;

    driver
        .start_driver_with_source(PacedReplaySource::open(*driver.clock()))
        .expect("start driver");
    driver.driver_start_streaming().expect("start streaming");
    let code = wait_for_connect_code(&driver).await;
    eng.start_engineer().expect("start engineer");
    eng.engineer_join(&code).expect("join");
    wait_for_engineers_count(&driver, 1).await;

    wait_status(&driver, "recording active", Duration::from_secs(15), |s| {
        s.recording.state == "active" && s.recording.frames_written > 0
    })
    .await;

    // Outage: the relay is unreachable for the driver.
    proxy.restart(Duration::from_secs(6));
    let during = wait_status(&driver, "outage visible", Duration::from_secs(5), |s| {
        s.network.state != "CONNECTED"
    })
    .await;
    assert!(
        during.network.streaming,
        "streaming intent survives an outage"
    );
    let before = during.recording.frames_written;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let later = driver.status();
    assert_ne!(later.network.state, "CONNECTED");
    assert_eq!(later.lmu.state, "LIVE", "acquisition continues");
    assert_eq!(later.recording.state, "active", "recording continues");
    assert!(
        later.recording.frames_written > before,
        "recording keeps writing during the outage ({} -> {})",
        before,
        later.recording.frames_written
    );

    // The engineer is told the driver is away (protocol 1.1 DRIVER_PRESENCE).
    wait_status(&eng, "driver away", Duration::from_secs(5), |s| {
        s.network.driver_away || s.engineer.as_ref().is_some_and(|e| e.driver_away)
    })
    .await;

    // After the outage the driver resumes the same session; the engineer stays attached.
    wait_status(&driver, "reconnected", Duration::from_secs(20), |s| {
        s.network.state == "CONNECTED" && s.network.streaming
    })
    .await;
    wait_for_engineers_count(&driver, 1).await;
    wait_status(&eng, "driver back", Duration::from_secs(10), |s| {
        !s.network.driver_away && s.engineer.as_ref().is_some_and(|e| !e.driver_away)
    })
    .await;

    eng.shutdown().expect("eng shutdown");
    driver.shutdown().expect("driver shutdown");
    proxy.shutdown().await;
    relay.shutdown().await;
    drop_on_thread(vec![eng, driver]);
    let _ = std::fs::remove_dir_all(&rec_dir);
}

/// `driver.auto_stream = true` streams by itself once logged in and LMU data is live.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_auto_stream_starts_when_enabled() {
    let relay = TestRelay::start().await;
    let driver = create_core_with(
        relay.addr,
        &relay.ca_pem,
        AppMode::Driver,
        DRIVER,
        ClientRole::Driver,
        |s| s.driver.auto_stream = true,
    )
    .await;
    driver
        .start_driver_with_source(PacedReplaySource::open(*driver.clock()))
        .expect("start driver");
    let code = wait_for_connect_code(&driver).await;
    assert!(!code.is_empty());
    let status = driver.status();
    assert!(status.network.streaming);

    driver.shutdown().expect("driver shutdown");
    relay.shutdown().await;
    drop_on_thread(vec![driver]);
}

/// An engineer that drops without DISCONNECT keeps its slot for the resume ticket (D-022 blip
/// reconnect, by design). The driver status shows that slot as `reconnecting`, never as a healthy
/// connected engineer, and `healthy` again once the engineer resumed with its ticket.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_dropped_engineer_shows_reconnecting_until_it_resumes() {
    let relay = TestRelay::start().await;
    let mut proxy_cfg = FaultProxyConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)), relay.addr);
    proxy_cfg.mode = ProxyMode::Opaque;
    let proxy = FaultProxy::start(proxy_cfg).await.expect("start proxy");

    // The driver connects directly; only the engineer goes through the proxy.
    let driver = create_core_with(
        relay.addr,
        &relay.ca_pem,
        AppMode::Driver,
        DRIVER,
        ClientRole::Driver,
        |_| {},
    )
    .await;
    let eng = create_core_with(
        proxy.local_addr(),
        &relay.ca_pem,
        AppMode::Engineer,
        ENGINEER,
        ClientRole::Engineer,
        |_| {},
    )
    .await;

    driver
        .start_driver_with_source(PacedReplaySource::open(*driver.clock()))
        .expect("start driver");
    driver.driver_start_streaming().expect("start streaming");
    let code = wait_for_connect_code(&driver).await;
    eng.start_engineer().expect("start engineer");
    eng.engineer_join(&code).expect("join");
    wait_status(&driver, "engineer healthy", Duration::from_secs(10), |s| {
        s.network.engineers.len() == 1 && s.network.engineers[0].link == "healthy"
    })
    .await;

    // Network drop for the engineer only (no DISCONNECT reaches the relay).
    proxy.restart(Duration::from_secs(5));
    let status = wait_status(&driver, "slot reconnecting", Duration::from_secs(10), |s| {
        s.network.engineers.len() == 1 && s.network.engineers[0].link == "reconnecting"
    })
    .await;
    assert_eq!(
        status.network.engineers_connected, 1,
        "the slot is held for the resume ticket"
    );

    // The engineer resumes with its in-memory ticket (no new code) and is healthy again.
    wait_status(
        &driver,
        "engineer healthy again",
        Duration::from_secs(20),
        |s| s.network.engineers.len() == 1 && s.network.engineers[0].link == "healthy",
    )
    .await;

    eng.shutdown().expect("eng shutdown");
    driver.shutdown().expect("driver shutdown");
    proxy.shutdown().await;
    relay.shutdown().await;
    drop_on_thread(vec![eng, driver]);
}
