//! Demo mode (feature `demo`): runs the synthetic provider into the same pipeline.
//!
//! Built with `SyntheticTelemetryProvider::new(DemoMode::acknowledge(), scenario)` and fed through its
//! realtime stream. The run's origin is `Synthetic` and it has no recorder and no network sink, so demo data
//! is display-only on the local PC (owner decision Q1).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::Sender;
use synthetic::{DemoMode, Scenario, SyntheticError, SyntheticTelemetryProvider, SyntheticTick};
use thiserror::Error;
use tracing::{debug, info};

use crate::clock::AppClock;
use crate::driver::pipeline::{DriverPipeline, PipelineCommand, PipelineSummary};

/// Tracing component name.
const COMPONENT: &str = "app_core::driver::demo";

/// Error creating or running demo telemetry.
#[derive(Debug, Error)]
pub enum DemoError {
    #[error("synthetic provider error: {0}")]
    Synthetic(#[from] SyntheticError),

    #[error("failed to spawn pipeline thread: {0}")]
    Io(#[from] std::io::Error),

    #[error("runtime error: {0}")]
    Runtime(String),
}

/// Handle to a running demo telemetry session.
pub struct DemoRunner {
    stop_flag: Arc<AtomicBool>,
    feeder_handle: Option<tokio::task::JoinHandle<()>>,
    pipeline_thread: Option<JoinHandle<PipelineSummary>>,
    cmd_tx: Option<Sender<PipelineCommand>>,
}

impl DemoRunner {
    /// Spawns a new demo session using the specified scenario and feeds it into the pipeline thread.
    ///
    /// Requires an active Tokio runtime context to drive the realtime tick stream.
    pub fn spawn(
        scenario: Scenario,
        clock: &AppClock,
        pipeline: DriverPipeline,
        cmd_tx: Sender<PipelineCommand>,
        cmd_rx: crossbeam_channel::Receiver<PipelineCommand>,
    ) -> Result<Self, DemoError> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let (demo_tx, demo_rx) = crossbeam_channel::bounded::<Box<SyntheticTick>>(256);

        let pipeline_thread = pipeline
            .spawn_demo(demo_rx, cmd_rx, Arc::clone(&stop_flag))
            .map_err(DemoError::Io)?;

        let provider = SyntheticTelemetryProvider::new(DemoMode::acknowledge(), scenario)
            .map_err(DemoError::Synthetic)?;

        let mut stream = provider
            .into_realtime_with_epoch(256, clock.epoch())
            .map_err(DemoError::Synthetic)?;

        let feeder_stop = Arc::clone(&stop_flag);
        let feeder_handle = tokio::spawn(async move {
            info!(component = COMPONENT, "demo feeder task started");
            while !feeder_stop.load(Ordering::Relaxed) {
                match stream.recv().await {
                    Some(tick) => {
                        if demo_tx.send(Box::new(tick)).is_err() {
                            debug!(
                                component = COMPONENT,
                                "demo pipeline channel closed; ending feeder"
                            );
                            break;
                        }
                    }
                    None => {
                        debug!(
                            component = COMPONENT,
                            "demo stream completed; ending feeder"
                        );
                        break;
                    }
                }
            }
            stream.stop();
            info!(component = COMPONENT, "demo feeder task stopped");
        });

        Ok(Self {
            stop_flag,
            feeder_handle: Some(feeder_handle),
            pipeline_thread: Some(pipeline_thread),
            cmd_tx: Some(cmd_tx),
        })
    }

    /// Stops the demo feeder and pipeline thread gracefully.
    pub fn stop(&mut self) -> Option<PipelineSummary> {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(ref tx) = self.cmd_tx {
            let _ = tx.send(PipelineCommand::Shutdown);
        }
        if let Some(task) = self.feeder_handle.take() {
            task.abort();
        }
        self.pipeline_thread.take().and_then(|h| h.join().ok())
    }
}

impl Drop for DemoRunner {
    fn drop(&mut self) {
        self.stop();
    }
}
