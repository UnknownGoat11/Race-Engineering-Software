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
use tracing::info;

use transport::auth::AuthManager;
use transport::credentials::CredentialStore;
use transport::endpoint::{Environment, RelayEndpoint};

use crate::bridge::hub::UiHub;
use crate::bridge::pacer::{PacerConfig, UiBridgePacer};
use crate::bridge::sink::UiSink;
use crate::bridge::status::{
    AlertDto, AlertLevel, AppMode, CoreStatus, DropsDto, LmuSourceStatus, NetworkStatusDto, Origin,
    RatesDto, RecordingStatusDto, SessionStatusDto, UiEvent,
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
}

/// Active driver runtime components.
struct DriverRun {
    reader: Option<ReaderThreadHandle>,
    pipeline_thread: Option<JoinHandle<PipelineSummary>>,
    cmd_tx: Sender<PipelineCommand>,
    stop_flag: Arc<AtomicBool>,
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
}

/// The central headless orchestration facade.
pub struct AppCore {
    runtime: tokio::runtime::Runtime,
    clock: AppClock,
    settings: RwLock<CoreSettings>,
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
    /// Constructs a new `AppCore` instance.
    pub fn new(
        settings: CoreSettings,
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

        let endpoint = match &settings.network.relay_url {
            Some(url) => RelayEndpoint::parse(url, "Configured", Environment::Development)
                .map_err(|e| CoreError::Settings(SettingsError::InvalidEndpoint(e.to_string())))?,
            None => RelayEndpoint::new("127.0.0.1", 443, "Local", Environment::Development)
                .map_err(|e| CoreError::Settings(SettingsError::InvalidEndpoint(e.to_string())))?,
        };

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
            RecorderSupervisor::open(config, None, None).ok()
        } else {
            None
        };

        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(16);
        let stop_flag = Arc::new(AtomicBool::new(false));

        let pipeline = DriverPipeline::new(
            self.clock,
            session_settings,
            EngineHubConfig::default(),
            recorder,
            None,
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
        });

        info!(component = COMPONENT, "driver mode started");
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
