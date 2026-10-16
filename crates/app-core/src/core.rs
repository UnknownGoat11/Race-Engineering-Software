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
//! - Strict shutdown order (§10): stop UI subscriptions & pacer -> stop demo -> stop reader thread (<= 1s)
//!   -> drain pipeline & finalize recording -> shutdown transport clients.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::Sender;
use parking_lot::{Mutex, RwLock};
use thiserror::Error;
use tracing::{info, warn};

use transport::auth::AuthManager;
use transport::credentials::CredentialStore;
use transport::driver::DriverClient;
use transport::endpoint::{Environment, ExtraCa, RelayEndpoint};
use transport::engineer::{EngineerClient, IdentityCard, JoinError};

use crate::bridge::hub::UiHub;
use crate::bridge::pacer::{PacerConfig, UiBridgePacer};
use crate::bridge::sink::UiSink;
use crate::bridge::status::{
    AlertDto, AlertLevel, AppMode, CoreStatus, DropsDto, ErrorDto, LmuSourceStatus,
    NetworkStatusDto, Origin, RatesDto, RecordingStatusDto, SessionStatusDto, UiEvent,
};
use crate::clock::AppClock;
use crate::driver::engines::EngineHubConfig;
use crate::driver::pipeline::{DriverPipeline, PipelineCommand, PipelineError, PipelineSummary};
use crate::driver::reader_thread::{
    ReaderThreadConfig, ReaderThreadHandle, SnapshotSource, spawn_reader_thread,
};
use crate::driver::recorder_sup::{RecorderSupervisor, RecorderSupervisorConfig};
use crate::driver::session::DriverSessionSettings;
use crate::settings::{CoreSettings, SettingsError};

#[cfg(feature = "demo")]
use crate::driver::demo::DemoRunner;

/// Tracing component name for logging.
const COMPONENT: &str = "app_core::facade";

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

    /// The live LMU driver pipeline is not connected to the relay.
    #[error("the driver is not connected to the relay")]
    NetworkUnavailable,
}

/// Active driver runtime components.
struct DriverRun {
    reader: Option<ReaderThreadHandle>,
    pipeline_thread: Option<JoinHandle<PipelineSummary>>,
    cmd_tx: Sender<PipelineCommand>,
    stop_flag: Arc<AtomicBool>,
    client: Option<Arc<DriverClient>>,
    /// Whether the pipeline owns a recorder (recording enabled and the store opened).
    recorder_attached: bool,
}

/// Active demo runtime components (display-only, feature `demo`).
#[cfg(feature = "demo")]
struct DemoRun {
    runner: DemoRunner,
    cmd_tx: Sender<PipelineCommand>,
}

/// Active engineer runtime components.
struct EngineerRun {
    core: Arc<Mutex<crate::engineer::core::EngineerCore>>,
    consumer_abort: Option<tokio::task::AbortHandle>,
    tick_abort: Option<tokio::task::AbortHandle>,
    client: Arc<Mutex<Option<Arc<EngineerClient>>>>,
}

/// The central headless orchestration facade.
pub struct AppCore {
    runtime: tokio::runtime::Runtime,
    clock: AppClock,
    settings: RwLock<CoreSettings>,
    endpoint: RelayEndpoint,
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

    /// Returns a handle to the active engineer core, if running.
    pub fn engineer_core(&self) -> Option<Arc<Mutex<crate::engineer::core::EngineerCore>>> {
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
    /// directory, relay endpoint, UI refresh rate) take effect on the next driver start or
    /// app start respectively.
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
                .send(PipelineCommand::UpdateSettings(session_settings))
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
    pub fn start_driver_with_source<S: SnapshotSource + Send + 'static>(
        &self,
        source: S,
    ) -> Result<(), CoreError> {
        self.start_driver_with_source_and_net(source, None)
    }

    /// Starts driver mode using a user-supplied snapshot source and optional network publisher.
    pub fn start_driver_with_source_and_net<S: SnapshotSource + Send + 'static>(
        &self,
        source: S,
        net: Option<crate::driver::net::NetPublisher>,
    ) -> Result<(), CoreError> {
        if self.is_shutdown.load(Ordering::Relaxed) {
            return Err(CoreError::ShutDown);
        }
        let mut driver_lock = self.driver_run.lock();
        if driver_lock.is_some() {
            return Err(CoreError::DriverAlreadyRunning);
        }

        let settings = self.settings.read().clone();
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

        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(16);
        let stop_flag = Arc::new(AtomicBool::new(false));

        let pipeline = DriverPipeline::new(
            self.clock,
            session_settings,
            EngineHubConfig::default(),
            recorder,
            net,
            Arc::clone(&self.ui_hub),
            Some(Arc::clone(&self.sink)),
        )?;

        let (snap_tx, snap_rx) = crossbeam_channel::bounded(256);
        let reader_handle =
            spawn_reader_thread(Box::new(source), ReaderThreadConfig::default(), snap_tx)
                .map_err(|e| CoreError::Io(std::io::Error::other(e.to_string())))?;

        let pipeline_thread = pipeline
            .spawn(snap_rx, cmd_rx, Arc::clone(&stop_flag))
            .map_err(CoreError::Io)?;

        *driver_lock = Some(DriverRun {
            reader: Some(reader_handle),
            pipeline_thread: Some(pipeline_thread),
            cmd_tx,
            stop_flag,
            client: None,
            recorder_attached,
        });

        info!(
            component = COMPONENT,
            recorder_attached, "driver mode started"
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
        let _ = run.cmd_tx.send(PipelineCommand::Shutdown);

        // 4. Join pipeline thread
        if let Some(pipeline_thread) = run.pipeline_thread.take() {
            let _ = pipeline_thread.join();
        }

        // 5. Stop driver client
        if let Some(client) = run.client.take() {
            let _ = self.block_on_runtime(async move {
                let _ = client.stop().await;
            });
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

        // Guaranteed display-only: recorder: None, net: None
        let pipeline = DriverPipeline::new(
            self.clock,
            session_settings,
            EngineHubConfig::default(),
            None,
            None,
            Arc::clone(&self.ui_hub),
            Some(Arc::clone(&self.sink)),
        )?;

        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(16);
        let _guard = self.runtime.enter();
        let runner = DemoRunner::spawn(scenario, &self.clock, pipeline, cmd_tx.clone(), cmd_rx)
            .map_err(|e| CoreError::Demo(e.to_string()))?;

        *demo_lock = Some(DemoRun { runner, cmd_tx });
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

    /// Starts engineer mode consumer and 4 Hz evaluation ticker.
    pub fn start_engineer(&self) -> Result<(), CoreError> {
        if self.is_shutdown.load(Ordering::Relaxed) {
            return Err(CoreError::ShutDown);
        }
        let mut engineer_lock = self.engineer_run.lock();
        if engineer_lock.is_some() {
            return Err(CoreError::EngineerAlreadyRunning);
        }

        let engineer_core = Arc::new(Mutex::new(crate::engineer::core::EngineerCore::new(
            Arc::clone(&self.ui_hub),
        )));

        let (event_tx, mut event_rx) =
            tokio::sync::mpsc::channel::<transport::engineer::EngineerEvent>(128);
        let core_consumer = Arc::clone(&engineer_core);
        let clock = self.clock;

        let consumer_task = self.runtime.spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let now_mono_ns = clock.now_ns();
                core_consumer.lock().handle_event(event, now_mono_ns);
            }
        });

        let core_ticker = Arc::clone(&engineer_core);
        let clock_tick = self.clock;
        let sink = Arc::clone(&self.sink);

        let tick_task = self.runtime.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            loop {
                interval.tick().await;
                let now_mono_ns = clock_tick.now_ns();
                let output = core_ticker.lock().tick(now_mono_ns, None, None);

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
            }
        });

        *engineer_lock = Some(EngineerRun {
            core: engineer_core,
            consumer_abort: Some(consumer_task.abort_handle()),
            tick_abort: Some(tick_task.abort_handle()),
            client: Arc::new(Mutex::new(None)),
        });

        // Drop sender unused in local stub until joined
        drop(event_tx);

        info!(component = COMPONENT, "engineer mode started");
        Ok(())
    }

    /// Stops the running engineer mode consumer.
    pub fn stop_engineer(&self) -> Result<(), CoreError> {
        let mut engineer_lock = self.engineer_run.lock();
        if let Some(mut run) = engineer_lock.take() {
            info!(component = COMPONENT, "stopping engineer services");
            if let Some(handle) = run.consumer_abort.take() {
                handle.abort();
            }
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

    /// Sends a discrete driver control command to the active driver pipeline.
    pub fn send_driver_command(&self, cmd: PipelineCommand) -> Result<(), CoreError> {
        #[cfg(feature = "demo")]
        if let Some(ref demo) = *self.demo_run.lock() {
            return demo
                .cmd_tx
                .send(cmd)
                .map_err(|e| CoreError::Pipeline(PipelineError::Channel(e.to_string())));
        }

        let driver_guard = self.driver_run.lock();
        let Some(ref run) = *driver_guard else {
            return Err(CoreError::DriverNotRunning);
        };

        run.cmd_tx
            .send(cmd)
            .map_err(|e| CoreError::Pipeline(PipelineError::Channel(e.to_string())))
    }

    /// Requests live streaming to begin on the driver side.
    pub fn driver_start_streaming(&self) -> Result<(), CoreError> {
        self.send_driver_command(PipelineCommand::StartStreaming)
    }

    /// Requests live streaming to stop on the driver side (keeps session for resume).
    pub fn driver_stop_streaming(&self) -> Result<(), CoreError> {
        self.send_driver_command(PipelineCommand::StopStreaming)
    }

    /// Ends the current driver session explicitly.
    pub fn driver_end_session(&self) -> Result<(), CoreError> {
        self.send_driver_command(PipelineCommand::EndSession)
    }

    /// Toggles local recording on or off.
    pub fn driver_set_recording(&self, enabled: bool) -> Result<(), CoreError> {
        self.send_driver_command(PipelineCommand::SetRecording(enabled))
    }

    /// Appends an annotation / bookmark marker to the active recording.
    pub fn driver_add_marker(&self, text: &str) -> Result<(), CoreError> {
        self.send_driver_command(PipelineCommand::AddMarker(text.to_string()))
    }

    /// Requests an immediate full snapshot over UI and network streams.
    pub fn driver_request_snapshot(&self) -> Result<(), CoreError> {
        self.send_driver_command(PipelineCommand::SnapshotNeeded)
    }

    /// Whether driver commands currently reach a pipeline that records to disk.
    ///
    /// `false` while the display-only demo runs (it can never record), when no live driver
    /// pipeline runs, or when recording is disabled or its store failed to open.
    pub fn driver_recorder_attached(&self) -> bool {
        #[cfg(feature = "demo")]
        if self.is_demo_running() {
            return false;
        }
        self.driver_run
            .lock()
            .as_ref()
            .is_some_and(|r| r.recorder_attached)
    }

    /// Whether the live LMU driver pipeline has a relay client attached.
    ///
    /// `false` while the display-only demo runs (it is never streamed).
    pub fn driver_network_attached(&self) -> bool {
        #[cfg(feature = "demo")]
        if self.is_demo_running() {
            return false;
        }
        self.driver_client().is_some()
    }

    /// Connects the driver transport client to the relay and binds the session.
    pub fn spawn_driver_client(&self) -> Result<Arc<DriverClient>, CoreError> {
        if *self.active_mode.read() != AppMode::Driver {
            return Err(CoreError::InvalidMode(*self.active_mode.read()));
        }
        if self.driver_run.lock().is_none() {
            return Err(CoreError::DriverNotRunning);
        }

        let _guard = self.runtime.enter();

        let config = transport::ClientConfig::new(
            self.endpoint.clone(),
            env!("CARGO_PKG_VERSION"),
            "app-core-driver",
            self.clock.epoch(),
        );
        let transport = transport::TcpTlsTelemetryTransport::new();
        let (client, mut events_rx) =
            DriverClient::spawn(config, transport, Arc::clone(&self.auth_manager))
                .map_err(|e| CoreError::Runtime(e.to_string()))?;
        let client = Arc::new(client);

        let session_create = protocol::messages::SessionCreate {
            identity: telemetry_model::scoring::SessionIdentity::default(),
            lmu_version: None,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
            capabilities: telemetry_model::scoring::VehicleCapabilities::default(),
        };

        let start_client = Arc::clone(&client);
        self.block_on_runtime(async move { start_client.start(session_create).await })?
            .map_err(|e| CoreError::Runtime(format!("{e:?}")))?;

        let mut driver_guard = self.driver_run.lock();
        if let Some(ref mut run) = *driver_guard {
            let cmd_tx = run.cmd_tx.clone();
            self.runtime.spawn(async move {
                while let Some(event) = events_rx.recv().await {
                    match event {
                        transport::driver::DriverEvent::SnapshotNeeded { trigger: _ } => {
                            let _ = cmd_tx.send(PipelineCommand::SnapshotNeeded);
                        }
                        transport::driver::DriverEvent::SessionReady { .. } => {
                            let _ = cmd_tx.send(PipelineCommand::SnapshotNeeded);
                        }
                        _ => {}
                    }
                }
            });
            run.client = Some(Arc::clone(&client));
        }

        Ok(client)
    }

    /// Returns the active DriverClient if spawned.
    pub fn driver_client(&self) -> Option<Arc<DriverClient>> {
        self.driver_run
            .lock()
            .as_ref()
            .and_then(|r| r.client.clone())
    }

    /// Returns the active engineer connection code, if published by the relay.
    pub fn driver_connect_code(&self) -> Option<protocol::messages::ConnectCode> {
        let guard = self.driver_run.lock();
        guard
            .as_ref()
            .and_then(|r| r.client.as_ref())
            .and_then(|c| c.connect_code().borrow().clone())
    }

    /// Returns the currently connected engineers.
    pub fn driver_engineers(&self) -> Vec<protocol::messages::EngineerEntry> {
        let guard = self.driver_run.lock();
        guard
            .as_ref()
            .and_then(|r| r.client.as_ref())
            .map(|c| c.engineers().borrow().clone())
            .unwrap_or_default()
    }

    /// Requests a new connection code from the relay.
    pub fn driver_request_connect_code(&self) -> Result<(), CoreError> {
        let client = self.driver_client().ok_or(CoreError::DriverNotRunning)?;
        client
            .request_connect_code()
            .map_err(|e| CoreError::Runtime(format!("{e:?}")))
    }

    /// Kicks an engineer from the active driver session.
    pub fn driver_kick_engineer(&self, connection_id: u64) -> Result<(), CoreError> {
        let client = self.driver_client().ok_or(CoreError::DriverNotRunning)?;
        client
            .kick_engineer(connection_id)
            .map_err(|e| CoreError::Runtime(format!("{e:?}")))
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
        if client_slot.lock().is_some() {
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

        // Attempt join:
        let join_client = Arc::clone(&client);
        let code_str = code.to_string();
        let card = self.block_on_runtime(async move { join_client.join(&code_str).await })??;

        // Spawn background consumer piping events into EngineerCore (the transport delivers
        // stream frames only after confirmation):
        let clock = self.clock;
        self.runtime.spawn(async move {
            while let Some(event) = events_rx.recv().await {
                let now_mono_ns = clock.now_ns();
                core_consumer.lock().handle_event(event, now_mono_ns);
            }
        });

        *client_slot.lock() = Some(client);
        Ok(card)
    }

    /// Declines the identity card (or aborts a pending join): the transport sends DISCONNECT and
    /// the code stays used, so reconnecting needs a new code from the driver (D-022).
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

    /// Returns the active EngineerClient if joined.
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

    /// Returns a composite snapshot of the core status.
    pub fn status(&self) -> CoreStatus {
        let mode = self.mode();
        let origin = {
            #[cfg(feature = "demo")]
            if self.is_demo_running() {
                Origin::Synthetic
            } else {
                Origin::Lmu
            }
            #[cfg(not(feature = "demo"))]
            Origin::Lmu
        };

        CoreStatus {
            mode,
            origin,
            lmu: LmuSourceStatus {
                state: "idle".to_string(),
                detail: None,
                game_version: None,
            },
            session: SessionStatusDto {
                session_id: None,
                session_type: None,
                track: None,
                car: None,
                vehicle_class: None,
                started_at_unix_s: None,
            },
            recording: RecordingStatusDto {
                state: "off".to_string(),
                recording_id: None,
                bytes_written: 0,
                frames_written: 0,
                frames_lost: 0,
                disk_free_bytes: None,
                error: None,
            },
            network: NetworkStatusDto {
                state: "disconnected".to_string(),
                rtt_ms: None,
                connect_code: None,
                code_expires_at_unix_s: None,
                engineers_connected: 0,
                engineers_max: 2,
                degraded_reason: None,
                driver_away: false,
                last_error: None,
                security_issue: false,
            },
            rates: RatesDto {
                telemetry_read_hz: None,
                scoring_read_hz: None,
                frames_sent_hz: None,
                bytes_sent_per_s: None,
                frames_dropped_total: 0,
            },
            drops: DropsDto {
                reader_overrun: 0,
                pipeline_overrun: 0,
                recorder_overrun: 0,
                net_dropped_obsolete: 0,
                ui_dropped: self.ui_hub.total_ui_dropped(),
            },
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

        // 3 & 4. Stop reader, drain pipeline, finalize recording
        let _ = self.stop_driver();

        // 5 & 6. Shutdown engineer client and tasks
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
