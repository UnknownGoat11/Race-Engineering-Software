//! `AppCore`: the headless orchestration facade of the desktop app.
//!
//! Owns the Tokio `Runtime` (2 worker threads), the [`crate::clock::AppClock`], the driver and
//! engineer runs, the mode switch and the strict shutdown order (blueprint §1, §2, §3, §9, §10).
//!
//! Constructor:
//! `AppCore::new(settings: CoreSettings, sink: Arc<dyn UiSink>, credential_store: Arc<dyn CredentialStore>)`.
//!
//! Architectural invariants:
//! - Mode switch shuts down the other mode's clients before starting the new one (§10).
//! - Demo / synthetic pipeline is constructed with `recorder: None, net: None` so demo data is display-only
//!   and can NEVER be recorded or streamed (blueprint Decision 5, Owner Decision Q1).
//! - The live LMU driver pipeline gets a relay link ([`DriverLink`]) whenever a relay is configured. The
//!   link stays idle until the driver starts streaming (or `driver.auto_stream` is on and the driver is
//!   logged in, Q7). Recording always runs, independent of login and network (Q9).
//! - Driver commands are answered with typed errors (never ignored silently, DEFERRED 22).
//! - Strict shutdown order (§10): stop UI subscriptions & pacer -> stop demo -> stop reader thread (<= 1s)
//!   -> drain pipeline & finalize recording -> shutdown transport clients.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use parking_lot::{Mutex, RwLock};
use protocol::messages::{ClientRole, RecordingState};
use thiserror::Error;
use tracing::{info, warn};

use transport::auth::AuthManager;
use transport::credentials::CredentialStore;
use transport::driver::DriverClient;
use transport::endpoint::{Environment, ExtraCa, RelayEndpoint};
use transport::engineer::{EngineerClient, EngineerPhase, IdentityCard, JoinError};
use transport::queue::QueueError;
use transport::state::FailureReason;

use crate::bridge::hub::UiHub;
use crate::bridge::pacer::{PacerConfig, UiBridgePacer};
use crate::bridge::sink::UiSink;
use crate::bridge::status::{
    AlertDto, AlertLevel, AppMode, CoreStatus, DropsDto, EngineerDto, EngineerStatusDto, ErrorDto,
    LmuSourceStatus, NetworkStatusDto, Origin, RatesDto, RecordingStatusDto, SessionStatusDto,
    UiEvent,
};
use crate::clock::AppClock;
use crate::driver::link::{DriverLink, degraded_reason_name, network_status_dto};
use crate::driver::net::{LiveSink, NetPublisher};
use crate::driver::pipeline::{
    CommandRejected, DriverPipeline, PipelineCommand, PipelineError, PipelineSummary, StatusSlot,
};
use crate::driver::reader_thread::{
    ReaderThreadConfig, ReaderThreadHandle, SnapshotSource, spawn_reader_thread,
};
use crate::driver::recorder_sup::{RecorderSupervisor, RecorderSupervisorConfig};
use crate::driver::session::{DriverSessionSettings, LMU_STATE_UNKNOWN, lmu_source_state_name};
use crate::engineer::age::StreamHealth;
use crate::engineer::core::EngineerCore;
use crate::settings::{CoreSettings, SettingsError};

#[cfg(feature = "demo")]
use crate::driver::demo::DemoRunner;

/// Tracing component name for logging.
const COMPONENT: &str = "app_core::facade";

/// Capacity of a pipeline's bounded command queue.
const PIPELINE_COMMAND_CAPACITY: usize = 16;

/// How long a driver command waits for room in the pipeline command queue.
const COMMAND_SEND_TIMEOUT: Duration = Duration::from_secs(1);

/// How long a driver command waits for the pipeline's reply.
const COMMAND_REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Engineer status / alert evaluation interval (4 Hz).
const ENGINEER_TICK_INTERVAL: Duration = Duration::from_millis(250);

const NS_PER_S: u64 = 1_000_000_000;

/// Errors originating from the `AppCore` orchestration facade.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("settings error: {0}")]
    Settings(#[from] SettingsError),

    #[error("pipeline error: {0}")]
    Pipeline(#[from] PipelineError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("tokio runtime error: {0}")]
    Runtime(String),

    #[error("driver pipeline is already running")]
    DriverAlreadyRunning,

    #[error("driver pipeline is not running")]
    DriverNotRunning,

    #[error("engineer client is already running")]
    EngineerAlreadyRunning,

    #[error("engineer client is not running")]
    EngineerNotRunning,

    #[error("demo error: {0}")]
    Demo(String),

    #[error("operation not permitted in current mode: {0:?}")]
    InvalidMode(AppMode),

    #[error("core has already been shut down")]
    ShutDown,

    /// Redeeming a connection code failed (typed, so the UI can show the specific reason).
    #[error("join failed: {0}")]
    Join(#[from] JoinError),

    /// Local recording is not available: disabled in the settings, failed to open, or the
    /// active pipeline is the display-only demo.
    #[error("local recording is not available")]
    RecorderUnavailable,

    /// The live LMU driver pipeline is not connected to the relay (no relay configured, or the
    /// display-only demo is active).
    #[error("the driver is not connected to the relay")]
    NetworkUnavailable,

    /// Streaming needs a driver login (log in, then start streaming).
    #[error("log in as a driver first")]
    NotLoggedIn,

    /// The relay operation needs an active stream (start streaming first).
    #[error("start streaming first")]
    NotStreaming,

    /// No active alert has this id.
    #[error("no active alert with id {0}")]
    UnknownAlert(u64),

    /// The pipeline refused the command for another typed reason.
    #[error("command refused: {0}")]
    Rejected(CommandRejected),

    /// The relay link refused the request (queue full or closed).
    #[error("relay link error: {0}")]
    Network(String),
}

impl From<CommandRejected> for CoreError {
    fn from(rejected: CommandRejected) -> Self {
        match rejected {
            CommandRejected::RecorderUnavailable => CoreError::RecorderUnavailable,
            CommandRejected::NetworkUnavailable => CoreError::NetworkUnavailable,
            CommandRejected::NotLoggedIn => CoreError::NotLoggedIn,
            CommandRejected::UnknownAlert(id) => CoreError::UnknownAlert(id),
            other => CoreError::Rejected(other),
        }
    }
}

impl From<QueueError> for CoreError {
    fn from(error: QueueError) -> Self {
        match error {
            QueueError::NotStreaming => CoreError::NotStreaming,
            other => CoreError::Network(other.to_string()),
        }
    }
}

/// Active driver runtime components.
struct DriverRun {
    reader: Option<ReaderThreadHandle>,
    pipeline_thread: Option<JoinHandle<PipelineSummary>>,
    cmd_tx: Sender<PipelineCommand>,
    stop_flag: Arc<AtomicBool>,
    /// The relay link (live LMU pipeline with a configured relay only).
    link: Option<DriverLink>,
    /// Whether the pipeline owns a recorder (recording enabled and the store opened).
    recorder_attached: bool,
    /// Latest composite status published by the pipeline (4 Hz).
    status_slot: StatusSlot,
    /// Live mirror of the session machine's streaming state.
    streaming: Arc<AtomicBool>,
}

/// Active demo runtime components (display-only, feature `demo`).
#[cfg(feature = "demo")]
struct DemoRun {
    runner: DemoRunner,
    cmd_tx: Sender<PipelineCommand>,
    status_slot: StatusSlot,
}

/// Slot holding the engineer client while joining or joined.
type EngineerClientSlot = Arc<Mutex<Option<Arc<EngineerClient>>>>;

/// Active engineer runtime components.
struct EngineerRun {
    core: Arc<Mutex<EngineerCore>>,
    tick_abort: Option<tokio::task::AbortHandle>,
    /// The client is stored BEFORE a join is awaited, so `engineer_cancel` can interrupt it.
    client: EngineerClientSlot,
}

/// The central headless orchestration facade.
pub struct AppCore {
    runtime: tokio::runtime::Runtime,
    clock: AppClock,
    settings: RwLock<CoreSettings>,
    endpoint: RelayEndpoint,
    /// A relay is configured (the endpoint is fixed for the lifetime of the core).
    relay_configured: bool,
    sink: Arc<dyn UiSink>,
    credential_store: Arc<dyn CredentialStore>,
    auth_manager: Arc<AuthManager>,
    ui_hub: Arc<UiHub>,
    pacer: Mutex<Option<UiBridgePacer>>,

    active_mode: RwLock<AppMode>,
    driver_run: Mutex<Option<DriverRun>>,
    engineer_run: Mutex<Option<EngineerRun>>,
    #[cfg(feature = "demo")]
    demo_run: Mutex<Option<DemoRun>>,

    is_shutdown: AtomicBool,
}

impl AppCore {
    /// Constructs a new `AppCore` instance for a development relay.
    ///
    /// Equivalent to [`AppCore::new_with_environment`] with [`Environment::Development`]; the
    /// desktop app passes its build environment instead so the relay environment check matches.
    pub fn new(
        settings: CoreSettings,
        sink: Arc<dyn UiSink>,
        credential_store: Arc<dyn CredentialStore>,
    ) -> Result<Self, CoreError> {
        Self::new_with_environment(settings, Environment::Development, sink, credential_store)
    }

    /// Constructs a new `AppCore` whose relay endpoint expects the given deployment environment.
    ///
    /// The transport refuses a relay that announces a different environment, so a production
    /// build must pass [`Environment::Production`]. An extra trusted CA is only accepted in
    /// [`Environment::Development`] (D-016).
    pub fn new_with_environment(
        settings: CoreSettings,
        environment: Environment,
        sink: Arc<dyn UiSink>,
        credential_store: Arc<dyn CredentialStore>,
    ) -> Result<Self, CoreError> {
        settings.validate()?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("app-core-tokio")
            .enable_all()
            .build()
            .map_err(|e| CoreError::Runtime(e.to_string()))?;

        let relay_configured = settings.network.relay_url.is_some();
        let mut endpoint = match &settings.network.relay_url {
            Some(url) => RelayEndpoint::parse(url, "Configured", environment)
                .map_err(|e| CoreError::Settings(SettingsError::InvalidEndpoint(e.to_string())))?,
            None => RelayEndpoint::new("127.0.0.1", 443, "Local", environment)
                .map_err(|e| CoreError::Settings(SettingsError::InvalidEndpoint(e.to_string())))?,
        };
        if let Some(ref pem) = settings.network.extra_ca_pem {
            endpoint = endpoint
                .with_extra_ca(ExtraCa::Pem(pem.clone()))
                .map_err(|e| CoreError::Settings(SettingsError::InvalidEndpoint(e.to_string())))?;
        }

        let auth_manager = Arc::new(AuthManager::new(Arc::clone(&credential_store), &endpoint));
        let clock = AppClock::new();
        let ui_hub = Arc::new(UiHub::new());

        let pacer_config = PacerConfig {
            flush_hz: settings.ui.max_refresh_hz,
            status_hz: 4,
        };
        let pacer = UiBridgePacer::spawn(Arc::clone(&ui_hub), pacer_config, None::<fn()>);

        let initial_mode = settings.mode;

        let core = Self {
            runtime,
            clock,
            settings: RwLock::new(settings),
            endpoint,
            relay_configured,
            sink,
            credential_store,
            auth_manager,
            ui_hub,
            pacer: Mutex::new(Some(pacer)),
            active_mode: RwLock::new(initial_mode),
            driver_run: Mutex::new(None),
            engineer_run: Mutex::new(None),
            #[cfg(feature = "demo")]
            demo_run: Mutex::new(None),
            is_shutdown: AtomicBool::new(false),
        };

        info!(
            component = COMPONENT,
            initial_mode = ?initial_mode,
            relay_configured,
            "AppCore facade initialized"
        );

        Ok(core)
    }

    /// Executes an async operation on the core's Tokio runtime without blocking or nesting runtimes on the current thread.
    fn block_on_runtime<F, T>(&self, fut: F) -> Result<T, CoreError>
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.runtime.spawn(async move {
            let res = fut.await;
            let _ = tx.send(res);
        });
        rx.recv().map_err(|e| CoreError::Runtime(e.to_string()))
    }

    /// Returns the currently active operating mode.
    pub fn mode(&self) -> AppMode {
        *self.active_mode.read()
    }

    /// Returns the application monotonic clock.
    pub fn clock(&self) -> &AppClock {
        &self.clock
    }

    /// Returns a reference to the shared UI telemetry hub.
    pub fn ui_hub(&self) -> &Arc<UiHub> {
        &self.ui_hub
    }

    /// Returns a reference to the authentication manager.
    pub fn auth_manager(&self) -> &Arc<AuthManager> {
        &self.auth_manager
    }

    /// Returns a reference to the credential store.
    pub fn credential_store(&self) -> &Arc<dyn CredentialStore> {
        &self.credential_store
    }

    /// Returns a reference to the configured relay endpoint.
    pub fn endpoint(&self) -> &RelayEndpoint {
        &self.endpoint
    }

    /// Whether a relay is configured for this core.
    pub fn relay_configured(&self) -> bool {
        self.relay_configured
    }

    /// Returns a handle to the active engineer core, if running.
    pub fn engineer_core(&self) -> Option<Arc<Mutex<EngineerCore>>> {
        self.engineer_run
            .lock()
            .as_ref()
            .map(|r| Arc::clone(&r.core))
    }

    /// Returns a copy of the current configuration settings.
    pub fn settings(&self) -> CoreSettings {
        self.settings.read().clone()
    }

    /// Updates configuration settings at runtime after validation.
    pub fn update_settings(&self, patch: CoreSettings) -> Result<(), CoreError> {
        patch.validate()?;
        *self.settings.write() = patch;
        Ok(())
    }

    /// Validates and stores new settings, then pushes the live-changeable driver session
    /// settings (`auto_stream`) into the running LMU driver pipeline.
    ///
    /// The display-only demo pipeline is never touched (its origin stays synthetic and its
    /// auto-stream stays off). Settings that need a new pipeline or endpoint (recording
    /// directory, engine configuration, relay endpoint, UI refresh rate) take effect on the next
    /// driver start or app start respectively.
    pub fn apply_settings(&self, patch: CoreSettings) -> Result<(), CoreError> {
        if self.is_shutdown.load(Ordering::Relaxed) {
            return Err(CoreError::ShutDown);
        }
        patch.validate()?;
        let session_settings = DriverSessionSettings {
            origin: Origin::Lmu,
            auto_stream: patch.driver.auto_stream,
            ..DriverSessionSettings::default()
        };
        *self.settings.write() = patch;

        let driver_guard = self.driver_run.lock();
        if let Some(ref run) = *driver_guard {
            run.cmd_tx
                .send_timeout(
                    PipelineCommand::UpdateSettings(session_settings),
                    COMMAND_SEND_TIMEOUT,
                )
                .map_err(|e| CoreError::Pipeline(PipelineError::Channel(e.to_string())))?;
        }
        Ok(())
    }

    /// Makes `mode` the active mode and guarantees its services run.
    ///
    /// Unlike [`AppCore::set_mode`], this also starts the services when `mode` is already the
    /// active mode but nothing runs yet (first start, or after a failed start). Driver mode
    /// with a running demo counts as running (the demo is stopped explicitly by the user).
    pub fn activate_mode(&self, mode: AppMode) -> Result<(), CoreError> {
        self.set_mode(mode)?;
        match mode {
            AppMode::Driver => {
                #[cfg(feature = "demo")]
                let demo_running = self.is_demo_running();
                #[cfg(not(feature = "demo"))]
                let demo_running = false;
                if !demo_running && !self.is_driver_running() {
                    match self.start_driver() {
                        Ok(()) | Err(CoreError::DriverAlreadyRunning) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            AppMode::Engineer => {
                if !self.is_engineer_running() {
                    match self.start_engineer() {
                        Ok(()) | Err(CoreError::EngineerAlreadyRunning) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(())
    }

    /// Switches the operating mode between Driver and Engineer.
    ///
    /// Shuts down the previous mode's services before activating the new one (§10).
    pub fn set_mode(&self, new_mode: AppMode) -> Result<(), CoreError> {
        if self.is_shutdown.load(Ordering::Relaxed) {
            return Err(CoreError::ShutDown);
        }

        let current_mode = *self.active_mode.read();
        if current_mode == new_mode {
            return Ok(());
        }

        info!(
            component = COMPONENT,
            from = ?current_mode,
            to = ?new_mode,
            "switching application mode"
        );

        // 1. Shut down existing mode services
        match current_mode {
            AppMode::Driver => {
                #[cfg(feature = "demo")]
                let _ = self.stop_demo();
                let _ = self.stop_driver();
            }
            AppMode::Engineer => {
                let _ = self.stop_engineer();
            }
        }

        // 2. Set new mode
        *self.active_mode.write() = new_mode;

        // 3. Start new mode services
        match new_mode {
            AppMode::Driver => {
                self.start_driver()?;
            }
            AppMode::Engineer => {
                self.start_engineer()?;
            }
        }

        info!(component = COMPONENT, mode = ?new_mode, "mode switch completed");
        Ok(())
    }

    /// Starts driver mode using the standard live LMU reader.
    pub fn start_driver(&self) -> Result<(), CoreError> {
        let reader = lmu_reader::LmuReader::open(lmu_reader::ReaderConfig::default())
            .map_err(|e| CoreError::Pipeline(PipelineError::Channel(e.to_string())))?;
        self.start_driver_with_source(reader)
    }

    /// Starts driver mode using a user-supplied snapshot source (useful for testing and replay).
    ///
    /// When a relay is configured, the pipeline gets the relay link (idle until streaming).
    pub fn start_driver_with_source<S: SnapshotSource + Send + 'static>(
        &self,
        source: S,
    ) -> Result<(), CoreError> {
        self.start_driver_inner(source, None)
    }

    /// Starts driver mode using a user-supplied snapshot source and an explicit network publisher
    /// (tests with mock sinks). With `net: None` this is [`AppCore::start_driver_with_source`].
    pub fn start_driver_with_source_and_net<S: SnapshotSource + Send + 'static>(
        &self,
        source: S,
        net: Option<NetPublisher>,
    ) -> Result<(), CoreError> {
        self.start_driver_inner(source, net)
    }

    fn start_driver_inner<S: SnapshotSource + Send + 'static>(
        &self,
        source: S,
        net: Option<NetPublisher>,
    ) -> Result<(), CoreError> {
        if self.is_shutdown.load(Ordering::Relaxed) {
            return Err(CoreError::ShutDown);
        }
        let mut driver_lock = self.driver_run.lock();
        if driver_lock.is_some() {
            return Err(CoreError::DriverAlreadyRunning);
        }

        let settings = self.settings.read().clone();
        let engine_config = settings.engines.to_engine_config()?;
        let session_settings = DriverSessionSettings {
            origin: Origin::Lmu,
            auto_stream: settings.driver.auto_stream,
            ..DriverSessionSettings::default()
        };

        let recorder = if settings.driver.recording_enabled {
            let config = RecorderSupervisorConfig {
                recorder: recorder::RecorderConfig::new(&settings.driver.recording_dir),
                enabled: true,
            };
            match RecorderSupervisor::open(config, None, None) {
                Ok(rec) => Some(rec),
                Err(err) => {
                    // Never silent: live telemetry continues, but the UI is told that nothing
                    // is being recorded and why.
                    warn!(
                        component = COMPONENT,
                        error = %err,
                        "local recording could not be opened; driver runs without recording"
                    );
                    self.sink.emit(UiEvent::Error(ErrorDto {
                        category: "recording".to_string(),
                        message: format!("Local recording could not be started: {err}"),
                        fatal: false,
                    }));
                    None
                }
            }
        } else {
            None
        };
        let recorder_attached = recorder.is_some();

        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(PIPELINE_COMMAND_CAPACITY);
        let stop_flag = Arc::new(AtomicBool::new(false));

        // Relay link: live LMU data only, and only with a configured relay. A link failure never
        // stops acquisition or recording.
        let mut link = None;
        let net = match net {
            Some(net) => Some(net),
            None if self.relay_configured => {
                match DriverLink::spawn(
                    self.runtime.handle(),
                    self.endpoint.clone(),
                    &self.clock,
                    Arc::clone(&self.auth_manager),
                    cmd_tx.clone(),
                ) {
                    Ok(spawned) => {
                        let sink: Arc<dyn LiveSink> =
                            Arc::clone(spawned.client()) as Arc<dyn LiveSink>;
                        let publisher =
                            NetPublisher::new(sink, self.clock).with_control(spawned.control());
                        link = Some(spawned);
                        Some(publisher)
                    }
                    Err(err) => {
                        warn!(component = COMPONENT, error = %err, "relay link could not be created; driver runs without streaming");
                        self.sink.emit(UiEvent::Error(ErrorDto {
                            category: "network".to_string(),
                            message: format!("The relay connection could not be set up: {err}"),
                            fatal: false,
                        }));
                        None
                    }
                }
            }
            None => None,
        };

        let status_slot: StatusSlot = Arc::new(Mutex::new(None));
        let mut pipeline = DriverPipeline::new(
            self.clock,
            session_settings,
            engine_config,
            recorder,
            net,
            Arc::clone(&self.ui_hub),
            Some(Arc::clone(&self.sink)),
        )?;
        pipeline.set_status_slot(Arc::clone(&status_slot));
        let streaming = Arc::new(AtomicBool::new(false));
        pipeline.set_streaming_flag(Arc::clone(&streaming));

        let (snap_tx, snap_rx) = crossbeam_channel::bounded(256);
        let reader_handle =
            spawn_reader_thread(Box::new(source), ReaderThreadConfig::default(), snap_tx)
                .map_err(|e| CoreError::Io(std::io::Error::other(e.to_string())))?;
        pipeline.set_reader_diagnostics(reader_handle.diagnostics_source());

        let pipeline_thread = pipeline
            .spawn(snap_rx, cmd_rx, Arc::clone(&stop_flag))
            .map_err(CoreError::Io)?;

        // Login state gates manual and automatic streaming (auto-stream is off by default, Q7).
        let logged_in = self.driver_logged_in();
        if let Err(err) = cmd_tx.try_send(PipelineCommand::AuthStatus { logged_in }) {
            warn!(component = COMPONENT, error = %err, "login state not delivered to the pipeline");
        }

        let has_link = link.is_some();
        *driver_lock = Some(DriverRun {
            reader: Some(reader_handle),
            pipeline_thread: Some(pipeline_thread),
            cmd_tx,
            stop_flag,
            link,
            recorder_attached,
            status_slot,
            streaming,
        });

        info!(
            component = COMPONENT,
            recorder_attached,
            relay_link = has_link,
            "driver mode started"
        );
        Ok(())
    }

    /// Stops the running driver mode pipeline cleanly.
    pub fn stop_driver(&self) -> Result<(), CoreError> {
        let mut driver_lock = self.driver_run.lock();
        let Some(mut run) = driver_lock.take() else {
            return Ok(());
        };

        info!(component = COMPONENT, "stopping driver pipeline");

        // 1. Signal stop
        run.stop_flag.store(true, Ordering::Relaxed);

        // 2. Stop reader thread (join within 1 s per §10)
        if let Some(reader) = run.reader.take() {
            let _ = reader.join();
        }

        // 3. Send Shutdown command to pipeline to drain and finalize recording
        let _ = run
            .cmd_tx
            .send_timeout(PipelineCommand::Shutdown, COMMAND_SEND_TIMEOUT);

        // 4. Join pipeline thread
        if let Some(pipeline_thread) = run.pipeline_thread.take() {
            let _ = pipeline_thread.join();
        }

        // 5. Close the relay link (the relay session is kept for a resume)
        if let Some(link) = run.link.take() {
            let _ = self.block_on_runtime(link.shutdown());
        }

        info!(component = COMPONENT, "driver pipeline stopped");
        Ok(())
    }

    /// Whether driver mode pipeline is currently active.
    pub fn is_driver_running(&self) -> bool {
        self.driver_run.lock().is_some()
    }

    /// Starts a synthetic demo telemetry stream into the pipeline.
    ///
    /// Per blueprint Decision 5 and Owner Decision Q1:
    /// Demo pipeline is built with `recorder: None, net: None` so it can never be recorded or streamed.
    #[cfg(feature = "demo")]
    pub fn start_demo(&self, scenario: synthetic::Scenario) -> Result<(), CoreError> {
        if self.is_shutdown.load(Ordering::Relaxed) {
            return Err(CoreError::ShutDown);
        }
        if *self.active_mode.read() != AppMode::Driver {
            return Err(CoreError::InvalidMode(*self.active_mode.read()));
        }

        // Stop live LMU driver run if running
        let _ = self.stop_driver();

        let mut demo_lock = self.demo_run.lock();
        if demo_lock.is_some() {
            return Err(CoreError::Demo("demo session is already running".into()));
        }

        let session_settings = DriverSessionSettings {
            origin: Origin::Synthetic,
            auto_stream: false,
            ..DriverSessionSettings::default()
        };
        let engine_config = self.settings.read().engines.to_engine_config()?;

        // Guaranteed display-only: recorder: None, net: None
        let mut pipeline = DriverPipeline::new(
            self.clock,
            session_settings,
            engine_config,
            None,
            None,
            Arc::clone(&self.ui_hub),
            Some(Arc::clone(&self.sink)),
        )?;
        let status_slot: StatusSlot = Arc::new(Mutex::new(None));
        pipeline.set_status_slot(Arc::clone(&status_slot));

        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(PIPELINE_COMMAND_CAPACITY);
        let _guard = self.runtime.enter();
        let runner = DemoRunner::spawn(scenario, &self.clock, pipeline, cmd_tx.clone(), cmd_rx)
            .map_err(|e| CoreError::Demo(e.to_string()))?;

        *demo_lock = Some(DemoRun {
            runner,
            cmd_tx,
            status_slot,
        });
        info!(
            component = COMPONENT,
            "demo session started (Origin::Synthetic, display-only)"
        );
        Ok(())
    }

    /// Stops the active synthetic demo session.
    #[cfg(feature = "demo")]
    pub fn stop_demo(&self) -> Result<(), CoreError> {
        let mut demo_lock = self.demo_run.lock();
        if let Some(mut demo) = demo_lock.take() {
            info!(component = COMPONENT, "stopping demo session");
            let _ = demo.runner.stop();
        }
        Ok(())
    }

    /// Whether a demo session is currently active.
    #[cfg(feature = "demo")]
    pub fn is_demo_running(&self) -> bool {
        self.demo_run.lock().is_some()
    }

    #[cfg(not(feature = "demo"))]
    fn is_demo_running(&self) -> bool {
        false
    }

    /// Starts engineer mode services: the 4 Hz status / alert ticker. A driver is joined only
    /// through [`AppCore::engineer_join_unconfirmed`] with a user-typed code (never automatic).
    pub fn start_engineer(&self) -> Result<(), CoreError> {
        if self.is_shutdown.load(Ordering::Relaxed) {
            return Err(CoreError::ShutDown);
        }
        let mut engineer_lock = self.engineer_run.lock();
        if engineer_lock.is_some() {
            return Err(CoreError::EngineerAlreadyRunning);
        }

        let engineer_core = Arc::new(Mutex::new(EngineerCore::new(Arc::clone(&self.ui_hub))));
        let client_slot: EngineerClientSlot = Arc::new(Mutex::new(None));

        let core_ticker = Arc::clone(&engineer_core);
        let tick_slot = Arc::clone(&client_slot);
        let clock_tick = self.clock;
        let sink = Arc::clone(&self.sink);
        let hub = Arc::clone(&self.ui_hub);

        let tick_task = self.runtime.spawn(async move {
            let mut interval = tokio::time::interval(ENGINEER_TICK_INTERVAL);
            let mut last_phase: Option<String> = None;
            loop {
                interval.tick().await;
                let now_mono_ns = clock_tick.now_ns();
                let client = tick_slot.lock().clone();
                let (relay_ns, uncertainty_ns) = client.as_ref().map_or((None, None), |c| {
                    (
                        c.relay_time_of(Instant::now()),
                        c.clock().map(|e| e.uncertainty_ns),
                    )
                });
                let (output, status) = {
                    let mut core = core_ticker.lock();
                    let output = core.tick(now_mono_ns, relay_ns, uncertainty_ns);
                    let status =
                        engineer_core_status(&core, client.as_deref(), hub.total_ui_dropped());
                    (output, status)
                };

                for notice in output.alerts {
                    let (level, raised) = match notice.state {
                        race_engines::alert::AlertState::Raised => (AlertLevel::Warning, true),
                        race_engines::alert::AlertState::Cleared => (AlertLevel::Info, false),
                        _ => (AlertLevel::Info, true),
                    };
                    sink.emit(UiEvent::Alert(AlertDto {
                        id: notice.alert.id.to_string(),
                        name: notice.alert.rule_id.clone(),
                        level,
                        raised,
                        message: notice.alert.message.clone(),
                        timestamp_ns: (notice.alert.raised_s * 1e9) as u64,
                    }));
                }

                let phase = client
                    .as_ref()
                    .map(|c| c.phase().borrow().clone())
                    .unwrap_or_default();
                let phase_name = engineer_phase_name(&phase).to_owned();
                if last_phase.as_deref() != Some(phase_name.as_str()) {
                    sink.emit(UiEvent::Engineer(engineer_dto(&phase)));
                    last_phase = Some(phase_name);
                }
                sink.emit(UiEvent::Status(Box::new(status)));
            }
        });

        *engineer_lock = Some(EngineerRun {
            core: engineer_core,
            tick_abort: Some(tick_task.abort_handle()),
            client: client_slot,
        });

        info!(component = COMPONENT, "engineer mode started");
        Ok(())
    }

    /// Stops the running engineer mode services and disconnects from the driver.
    pub fn stop_engineer(&self) -> Result<(), CoreError> {
        let mut engineer_lock = self.engineer_run.lock();
        if let Some(mut run) = engineer_lock.take() {
            info!(component = COMPONENT, "stopping engineer services");
            if let Some(handle) = run.tick_abort.take() {
                handle.abort();
            }
            if let Some(client) = run.client.lock().take() {
                let _ = self.block_on_runtime(async move {
                    let _ = client.disconnect().await;
                });
            }
        }
        Ok(())
    }

    /// Whether engineer mode is currently active.
    pub fn is_engineer_running(&self) -> bool {
        self.engineer_run.lock().is_some()
    }

    /// Sends a discrete driver control command to the active driver pipeline (demo first)
    /// without waiting for its outcome. Prefer the typed `driver_*` methods.
    pub fn send_driver_command(&self, cmd: PipelineCommand) -> Result<(), CoreError> {
        let tx = self.active_driver_tx()?;
        tx.send_timeout(cmd, COMMAND_SEND_TIMEOUT)
            .map_err(|e| CoreError::Pipeline(PipelineError::Channel(e.to_string())))
    }

    /// The command queue of the pipeline receiving driver commands (the demo while it runs).
    fn active_driver_tx(&self) -> Result<Sender<PipelineCommand>, CoreError> {
        #[cfg(feature = "demo")]
        if let Some(ref demo) = *self.demo_run.lock() {
            return Ok(demo.cmd_tx.clone());
        }
        self.driver_run
            .lock()
            .as_ref()
            .map(|run| run.cmd_tx.clone())
            .ok_or(CoreError::DriverNotRunning)
    }

    /// Runs a command on the active driver pipeline and returns its typed outcome.
    fn request_driver(&self, cmd: PipelineCommand) -> Result<(), CoreError> {
        let tx = self.active_driver_tx()?;
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        tx.send_timeout(
            PipelineCommand::Request {
                command: Box::new(cmd),
                reply: reply_tx,
            },
            COMMAND_SEND_TIMEOUT,
        )
        .map_err(|e| CoreError::Pipeline(PipelineError::Channel(e.to_string())))?;
        match reply_rx.recv_timeout(COMMAND_REPLY_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(rejected)) => Err(rejected.into()),
            Err(e) => Err(CoreError::Pipeline(PipelineError::Channel(format!(
                "no reply from the driver pipeline: {e}"
            )))),
        }
    }

    /// Whether a driver refresh token exists. A credential store error counts as not logged in
    /// (logged, never treated as a login).
    fn driver_logged_in(&self) -> bool {
        match self.auth_manager.is_logged_in(ClientRole::Driver) {
            Ok(logged_in) => logged_in,
            Err(err) => {
                warn!(component = COMPONENT, error = %err, "credential store unreadable; treating the driver as logged out");
                false
            }
        }
    }

    /// Re-reads the driver login state and passes it to the live LMU pipeline (call after login,
    /// registration or logout). With `driver.auto_stream` on, a login can start streaming.
    pub fn refresh_login_state(&self) -> Result<bool, CoreError> {
        let logged_in = self.driver_logged_in();
        if !self.is_demo_running()
            && let Some(ref run) = *self.driver_run.lock()
        {
            run.cmd_tx
                .send_timeout(
                    PipelineCommand::AuthStatus { logged_in },
                    COMMAND_SEND_TIMEOUT,
                )
                .map_err(|e| CoreError::Pipeline(PipelineError::Channel(e.to_string())))?;
        }
        Ok(logged_in)
    }

    /// Starts live streaming to the relay: connects and binds (resumes or creates) the relay
    /// session. Requires the live LMU pipeline, a configured relay and a driver login. The demo
    /// is never streamed. Recording is unaffected.
    pub fn driver_start_streaming(&self) -> Result<(), CoreError> {
        self.refresh_login_state()?;
        self.request_driver(PipelineCommand::StartStreaming)
    }

    /// Stops live streaming (the relay session is kept for a resume; engineers stay attached).
    pub fn driver_stop_streaming(&self) -> Result<(), CoreError> {
        self.request_driver(PipelineCommand::StopStreaming)
    }

    /// Ends the current driver session: finishes the recording and ends the relay session
    /// (engineers see "driver ended the session").
    pub fn driver_end_session(&self) -> Result<(), CoreError> {
        self.request_driver(PipelineCommand::EndSession)
    }

    /// Toggles local recording on or off. Fails with [`CoreError::RecorderUnavailable`] when the
    /// active pipeline has no recorder.
    pub fn driver_set_recording(&self, enabled: bool) -> Result<(), CoreError> {
        self.request_driver(PipelineCommand::SetRecording(enabled))
    }

    /// Appends an annotation / bookmark marker to the active recording. Fails with
    /// [`CoreError::RecorderUnavailable`] without a recorder, or a typed refusal when no
    /// recording session is open.
    pub fn driver_add_marker(&self, text: &str) -> Result<(), CoreError> {
        self.request_driver(PipelineCommand::AddMarker(text.to_string()))
    }

    /// Acknowledges an active alert of the driver pipeline's alert engine.
    pub fn driver_ack_alert(&self, alert_id: u64) -> Result<(), CoreError> {
        self.request_driver(PipelineCommand::AckAlert(alert_id))
    }

    /// Requests an immediate full snapshot over UI and network streams.
    pub fn driver_request_snapshot(&self) -> Result<(), CoreError> {
        self.request_driver(PipelineCommand::SnapshotNeeded)
    }

    /// Whether driver commands currently reach a pipeline that records to disk.
    ///
    /// `false` while the display-only demo runs (it can never record), when no live driver
    /// pipeline runs, or when recording is disabled or its store failed to open.
    pub fn driver_recorder_attached(&self) -> bool {
        if self.is_demo_running() {
            return false;
        }
        self.driver_run
            .lock()
            .as_ref()
            .is_some_and(|r| r.recorder_attached)
    }

    /// Whether the live LMU driver pipeline has a relay link attached.
    ///
    /// `false` while the display-only demo runs (it is never streamed).
    pub fn driver_network_attached(&self) -> bool {
        if self.is_demo_running() {
            return false;
        }
        self.driver_client().is_some()
    }

    /// Returns the live driver pipeline's relay client, if a relay link exists.
    pub fn driver_client(&self) -> Option<Arc<DriverClient>> {
        self.driver_run
            .lock()
            .as_ref()
            .and_then(|r| r.link.as_ref())
            .map(|l| Arc::clone(l.client()))
    }

    /// Returns the current one-time connection code and slot usage, if the relay published one.
    /// The code is a secret: never log it.
    pub fn driver_connect_code(&self) -> Option<protocol::messages::ConnectCode> {
        self.driver_client()
            .and_then(|c| c.connect_code().borrow().clone())
    }

    /// Returns the engineers holding a slot in the driver's relay session.
    pub fn driver_engineers(&self) -> Vec<protocol::messages::EngineerEntry> {
        self.driver_client()
            .map(|c| c.engineers().borrow().clone())
            .unwrap_or_default()
    }

    /// Asks the relay for a new one-time connection code (the unused one is invalidated).
    /// Requires an active stream.
    pub fn driver_request_connect_code(&self) -> Result<(), CoreError> {
        let client = self.driver_client().ok_or(CoreError::NetworkUnavailable)?;
        client.request_connect_code().map_err(CoreError::from)
    }

    /// Removes an engineer from the session (its ticket dies; it needs a new code to return).
    pub fn driver_kick_engineer(&self, connection_id: u64) -> Result<(), CoreError> {
        let client = self.driver_client().ok_or(CoreError::NetworkUnavailable)?;
        client.kick_engineer(connection_id).map_err(CoreError::from)
    }

    /// Joins a driver session using a one-time connection code and confirms the identity card
    /// at once (headless use and tests). The desktop UI uses
    /// [`AppCore::engineer_join_unconfirmed`] followed by [`AppCore::engineer_confirm`] so the
    /// user sees the identity card before any telemetry is delivered (D-022).
    pub fn engineer_join(&self, code: &str) -> Result<IdentityCard, CoreError> {
        let card = self.engineer_join_unconfirmed(code)?;
        self.engineer_confirm()?;
        Ok(card)
    }

    /// Redeems a one-time connection code and returns the driver's identity card WITHOUT
    /// confirming it: the transport holds the stream until [`AppCore::engineer_confirm`];
    /// [`AppCore::engineer_cancel`] declines it (the code stays used).
    ///
    /// The client is stored before the join is awaited, so [`AppCore::engineer_cancel`] can
    /// interrupt a join in progress (the join then fails with [`JoinError::Cancelled`]).
    ///
    /// This is the only path that joins a driver: it requires a user-supplied code and is
    /// never called automatically (D-019/D-022). The code is neither logged nor persisted.
    pub fn engineer_join_unconfirmed(&self, code: &str) -> Result<IdentityCard, CoreError> {
        if *self.active_mode.read() != AppMode::Engineer {
            return Err(CoreError::InvalidMode(*self.active_mode.read()));
        }
        let (core_consumer, client_slot) = {
            let guard = self.engineer_run.lock();
            let run = guard.as_ref().ok_or(CoreError::EngineerNotRunning)?;
            (Arc::clone(&run.core), Arc::clone(&run.client))
        };

        let client = {
            let mut slot = client_slot.lock();
            if slot.is_some() {
                // One driver at a time: a second join must not orphan the current connection.
                return Err(CoreError::Join(JoinError::Busy));
            }
            let _guard = self.runtime.enter();
            let config = transport::ClientConfig::new(
                self.endpoint.clone(),
                env!("CARGO_PKG_VERSION"),
                "app-core-engineer",
                self.clock.epoch(),
            );
            let transport = transport::TcpTlsTelemetryTransport::new();
            let (client, mut events_rx) =
                EngineerClient::spawn(config, transport, Arc::clone(&self.auth_manager))
                    .map_err(|e| CoreError::Runtime(e.to_string()))?;
            let client = Arc::new(client);

            // Consumer piping events into EngineerCore (the transport delivers stream frames
            // only after confirmation). It ends when the client is dropped.
            let clock = self.clock;
            self.runtime.spawn(async move {
                while let Some(event) = events_rx.recv().await {
                    let now_mono_ns = clock.now_ns();
                    core_consumer.lock().handle_event(event, now_mono_ns);
                }
            });

            *slot = Some(Arc::clone(&client));
            client
        };

        let join_client = Arc::clone(&client);
        let code_str = code.to_string();
        let joined = self.block_on_runtime(async move { join_client.join(&code_str).await })?;

        let mut slot = client_slot.lock();
        let still_ours = slot.as_ref().is_some_and(|c| Arc::ptr_eq(c, &client));
        match joined {
            Ok(card) if still_ours => Ok(card),
            Ok(_) => {
                // Cancelled after the relay bound the join: release the slot on the relay.
                drop(slot);
                let _ = self.block_on_runtime(async move { client.cancel().await });
                Err(CoreError::Join(JoinError::Cancelled))
            }
            Err(err) => {
                if still_ours {
                    *slot = None;
                }
                Err(CoreError::Join(err))
            }
        }
    }

    /// Declines the identity card or interrupts a join in progress: the transport sends
    /// DISCONNECT and the code stays used, so reconnecting needs a new code from the driver
    /// (D-022).
    pub fn engineer_cancel(&self) -> Result<(), CoreError> {
        let client_opt = {
            let guard = self.engineer_run.lock();
            let run = guard.as_ref().ok_or(CoreError::EngineerNotRunning)?;
            run.client.lock().take()
        };
        let client = client_opt.ok_or(CoreError::EngineerNotRunning)?;
        self.block_on_runtime(async move { client.cancel().await })?
            .map_err(|e| CoreError::Runtime(e.to_string()))
    }

    /// Disconnects the active engineer client from the driver session.
    pub fn engineer_disconnect(&self) -> Result<(), CoreError> {
        let client_opt = {
            let guard = self.engineer_run.lock();
            guard.as_ref().and_then(|r| r.client.lock().take())
        };

        if let Some(client) = client_opt {
            let _ = self.block_on_runtime(async move {
                let _ = client.disconnect().await;
            });
        }
        Ok(())
    }

    /// Returns the active EngineerClient if joining or joined.
    pub fn engineer_client(&self) -> Option<Arc<EngineerClient>> {
        let guard = self.engineer_run.lock();
        guard.as_ref().and_then(|r| r.client.lock().clone())
    }

    /// Requests an immediate snapshot from the driver over the network.
    pub fn engineer_request_snapshot(&self) -> Result<(), CoreError> {
        let client = self
            .engineer_client()
            .ok_or(CoreError::EngineerNotRunning)?;
        self.block_on_runtime(async move {
            client
                .request_snapshot(protocol::messages::SnapshotReason::Manual)
                .await
        })?
        .map_err(|e| CoreError::Runtime(format!("{e:?}")))
    }

    /// Confirms stream reception after viewing driver identity card.
    pub fn engineer_confirm(&self) -> Result<(), CoreError> {
        let client = self
            .engineer_client()
            .ok_or(CoreError::EngineerNotRunning)?;
        self.block_on_runtime(async move { client.confirm().await })?
            .map_err(|e| CoreError::Runtime(format!("{e:?}")))
    }

    /// Composite status of the active mode. Every value is measured; what is not observed is
    /// reported as unknown (`UNKNOWN`, `None`), never as healthy.
    ///
    /// - Driver: the active pipeline's latest status (LMU source, session, recording, rates,
    ///   drops) with the relay link block refreshed now (state incl. DRIVER_PRESENCE / degraded
    ///   reason, connect code + expiry, engineers n/max, failure).
    /// - Engineer: phase, link state, stream age/health, driver away, and the driver's LMU and
    ///   recording state as last reported over the relay.
    pub fn status(&self) -> CoreStatus {
        match self.mode() {
            AppMode::Engineer => self.engineer_status(),
            AppMode::Driver => self.driver_status(),
        }
    }

    fn driver_status(&self) -> CoreStatus {
        #[cfg(feature = "demo")]
        if let Some(ref demo) = *self.demo_run.lock() {
            let mut status = demo
                .status_slot
                .lock()
                .clone()
                .unwrap_or_else(|| self.idle_driver_status(Origin::Synthetic));
            // The demo is never streamed: no relay applies to it.
            status.network = network_status_dto(None, false, false);
            return status;
        }

        let guard = self.driver_run.lock();
        let Some(run) = guard.as_ref() else {
            drop(guard);
            return self.idle_driver_status(Origin::Lmu);
        };
        let link = run.link.as_ref().and_then(DriverLink::snapshot);
        let streaming = run.streaming.load(Ordering::Relaxed);
        match run.status_slot.lock().clone() {
            Some(mut status) => {
                // Refresh the network block now (the pipeline's copy is up to 250 ms old).
                status.network =
                    network_status_dto(link.as_ref(), self.relay_configured, streaming);
                status.drops.ui_dropped = self.ui_hub.total_ui_dropped();
                status
            }
            None => {
                let mut status = self.idle_driver_status(Origin::Lmu);
                status.recording.state = if run.recorder_attached {
                    "unknown".to_owned()
                } else {
                    "disabled".to_owned()
                };
                status.network =
                    network_status_dto(link.as_ref(), self.relay_configured, streaming);
                status
            }
        }
    }

    /// Driver status while no pipeline reports (not started, or starting).
    fn idle_driver_status(&self, origin: Origin) -> CoreStatus {
        let mut network = network_status_dto(None, self.relay_configured, false);
        if self.relay_configured {
            // No relay client exists: certainly not connected.
            network.state = transport::ConnectionState::Disconnected.name().to_owned();
        }
        CoreStatus {
            mode: AppMode::Driver,
            origin,
            lmu: LmuSourceStatus {
                state: LMU_STATE_UNKNOWN.to_string(),
                detail: None,
                game_version: None,
            },
            session: empty_session(),
            recording: RecordingStatusDto {
                state: "off".to_string(),
                recording_id: None,
                bytes_written: 0,
                frames_written: 0,
                frames_lost: 0,
                disk_free_bytes: None,
                error: None,
            },
            network,
            rates: empty_rates(),
            drops: DropsDto {
                ui_dropped: self.ui_hub.total_ui_dropped(),
                ..DropsDto::default()
            },
            engineer: None,
        }
    }

    fn engineer_status(&self) -> CoreStatus {
        let ui_dropped = self.ui_hub.total_ui_dropped();
        let (core, client) = {
            let guard = self.engineer_run.lock();
            match guard.as_ref() {
                Some(run) => (Some(Arc::clone(&run.core)), run.client.lock().clone()),
                None => (None, None),
            }
        };
        match core {
            Some(core) => engineer_core_status(&core.lock(), client.as_deref(), ui_dropped),
            None => {
                let mut status = engineer_core_status_parts(None, None, ui_dropped);
                if let Some(ref mut engineer) = status.engineer {
                    engineer.phase = "not_started".to_owned();
                }
                status
            }
        }
    }

    /// Executes the strict ordered shutdown sequence (§10):
    /// 1. Stop UI subscriptions and pacer.
    /// 2. Stop demo if running.
    /// 3. Stop reader thread (join <= 1s).
    /// 4. Drain pipeline and finalize recording session.
    /// 5. Shutdown driver client (keeps resume token).
    /// 6. Shutdown engineer client.
    pub fn shutdown(&self) -> Result<(), CoreError> {
        if self.is_shutdown.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        info!(component = COMPONENT, "executing strict shutdown sequence");

        // 1. Stop UI subscriptions and pacer
        if let Some(mut pacer) = self.pacer.lock().take() {
            pacer.stop();
        }

        // 2. Stop demo session if running
        #[cfg(feature = "demo")]
        let _ = self.stop_demo();

        // 3 & 4 & 5. Stop reader, drain pipeline, finalize recording, close the relay link
        let _ = self.stop_driver();

        // 6. Shutdown engineer client and tasks
        let _ = self.stop_engineer();

        info!(component = COMPONENT, "AppCore shutdown complete");
        Ok(())
    }
}

impl Drop for AppCore {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn empty_session() -> SessionStatusDto {
    SessionStatusDto {
        session_id: None,
        session_type: None,
        track: None,
        car: None,
        vehicle_class: None,
        started_at_unix_s: None,
    }
}

fn empty_rates() -> RatesDto {
    RatesDto {
        telemetry_read_hz: None,
        scoring_read_hz: None,
        frames_sent_hz: None,
        bytes_sent_per_s: None,
        frames_dropped_total: 0,
    }
}

/// Stable snake_case name of an engineer phase.
fn engineer_phase_name(phase: &EngineerPhase) -> &'static str {
    match phase {
        EngineerPhase::Idle => "idle",
        EngineerPhase::Joining => "joining",
        EngineerPhase::AwaitingConfirmation(_) => "awaiting_confirmation",
        EngineerPhase::Monitoring(_) => "monitoring",
        EngineerPhase::Ended(_) => "ended",
    }
}

/// `core:engineer` payload for a phase.
fn engineer_dto(phase: &EngineerPhase) -> EngineerDto {
    let card = match phase {
        EngineerPhase::AwaitingConfirmation(card) | EngineerPhase::Monitoring(card) => Some(card),
        _ => None,
    };
    EngineerDto {
        phase: engineer_phase_name(phase).to_owned(),
        driver_name: card.map(|c| c.driver_display_name.clone()),
        car: card.and_then(|c| c.car.clone()),
        vehicle_class: card.and_then(|c| c.vehicle_class.clone()),
        track: card.and_then(|c| c.track.clone()),
        session_type: card.and_then(|c| c.session_type.map(|t| t.to_string())),
        ended_reason: match phase {
            EngineerPhase::Ended(reason) => Some(reason.message().to_owned()),
            _ => None,
        },
    }
}

const fn stream_health_name(health: StreamHealth) -> &'static str {
    match health {
        StreamHealth::NoData => "no_data",
        StreamHealth::Healthy => "healthy",
        StreamHealth::Stale => "stale",
    }
}

const fn recording_state_name(state: RecordingState) -> &'static str {
    match state {
        RecordingState::Off => "off",
        RecordingState::Active => "active",
        RecordingState::Failed => "failed",
    }
}

/// Engineer-mode composite status from the engineer core and client.
fn engineer_core_status(
    core: &EngineerCore,
    client: Option<&EngineerClient>,
    ui_dropped: u64,
) -> CoreStatus {
    engineer_core_status_parts(Some(core), client, ui_dropped)
}

fn engineer_core_status_parts(
    core: Option<&EngineerCore>,
    client: Option<&EngineerClient>,
    ui_dropped: u64,
) -> CoreStatus {
    let phase = client
        .map(|c| c.phase().borrow().clone())
        .unwrap_or_default();
    let card = match &phase {
        EngineerPhase::AwaitingConfirmation(card) | EngineerPhase::Monitoring(card) => {
            Some(card.clone())
        }
        _ => core.and_then(|c| c.active_identity().cloned()),
    };
    let link_status = client.map(|c| c.status().borrow().clone());
    let metrics = client.map(|c| c.metrics().borrow().clone());
    let last = core.and_then(EngineerCore::last_status);
    let driver_status = core.and_then(EngineerCore::latest_driver_status);
    let driver_recording = core.and_then(EngineerCore::latest_recording_status);
    let core_driver_away = core.is_some_and(EngineerCore::driver_away);

    let lmu = match driver_status {
        Some(ds) => LmuSourceStatus {
            state: lmu_source_state_name(ds.lmu_state).to_owned(),
            detail: ds.lmu_source.detail.clone(),
            game_version: None,
        },
        None => LmuSourceStatus {
            state: LMU_STATE_UNKNOWN.to_owned(),
            detail: None,
            game_version: None,
        },
    };

    let recording = match driver_recording {
        Some(rs) => RecordingStatusDto {
            state: recording_state_name(rs.state).to_owned(),
            recording_id: rs.recording_id.clone(),
            bytes_written: rs.bytes_written,
            frames_written: rs.frames_written,
            frames_lost: rs.frames_lost,
            disk_free_bytes: rs.disk_free_bytes,
            error: rs.error.clone(),
        },
        None => RecordingStatusDto {
            state: "unknown".to_owned(),
            recording_id: None,
            bytes_written: 0,
            frames_written: 0,
            frames_lost: 0,
            disk_free_bytes: None,
            error: None,
        },
    };

    let network = match link_status {
        Some(ref s) => NetworkStatusDto {
            state: s.state.name().to_owned(),
            streaming: false,
            rtt_ms: metrics
                .as_ref()
                .and_then(|m| m.rtt_smoothed.or(m.rtt))
                .map(|d| (d.as_secs_f64() * 1000.0) as f32),
            connect_code: None,
            code_expires_at_unix_s: None,
            engineers_connected: 0,
            engineers_max: protocol::limits::MAX_ENGINEERS_PER_SESSION,
            degraded_reason: s.degraded.map(|r| degraded_reason_name(r).to_owned()),
            driver_away: s.driver_away || core_driver_away,
            last_error: s.last_error.clone().or_else(|| s.persist_error.clone()),
            security_issue: s.security_issue,
            failure: s.failure.as_ref().map(ToString::to_string),
            relogin_required: matches!(s.failure, Some(FailureReason::ReloginRequired(_))),
            engineers: Vec::new(),
        },
        None => {
            let mut n = network_status_dto(None, true, false);
            n.state = transport::ConnectionState::Disconnected.name().to_owned();
            n.driver_away = core_driver_away;
            n
        }
    };

    let rates = match driver_status {
        Some(ds) => RatesDto {
            telemetry_read_hz: ds.rates.telemetry_read_hz,
            scoring_read_hz: ds.rates.scoring_read_hz,
            frames_sent_hz: ds.rates.frames_sent_hz,
            bytes_sent_per_s: ds.rates.bytes_sent_per_s,
            frames_dropped_total: ds.rates.frames_dropped_total,
        },
        None => empty_rates(),
    };

    let session = match card {
        Some(ref c) => SessionStatusDto {
            session_id: Some(c.session_id),
            session_type: c.session_type.map(|t| t.to_string()),
            track: c.track.clone(),
            car: c.car.clone(),
            vehicle_class: c.vehicle_class.clone(),
            started_at_unix_s: Some(c.started_at_ns / NS_PER_S),
        },
        None => empty_session(),
    };

    let engineer = EngineerStatusDto {
        phase: engineer_phase_name(&phase).to_owned(),
        driver_name: card.as_ref().map(|c| c.driver_display_name.clone()),
        ended_reason: match &phase {
            EngineerPhase::Ended(reason) => Some(reason.message().to_owned()),
            _ => None,
        },
        stream_health: last
            .map_or(stream_health_name(StreamHealth::NoData), |l| {
                stream_health_name(l.stream_health)
            })
            .to_owned(),
        stream_age_s: last.and_then(|l| l.stream_age_s),
        driver_away: core_driver_away || link_status.as_ref().is_some_and(|s| s.driver_away),
        incompatible_schema: last.is_some_and(|l| l.incompatible_schema),
    };

    CoreStatus {
        mode: AppMode::Engineer,
        origin: Origin::Lmu,
        lmu,
        session,
        recording,
        network,
        rates,
        drops: DropsDto {
            net_dropped_obsolete: metrics.as_ref().map_or(0, |m| m.relay_dropped),
            ui_dropped,
            ..DropsDto::default()
        },
        engineer: Some(engineer),
    }
}
