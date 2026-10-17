//! Integration tests for Demo mode and synthetic telemetry isolation (Phase 2 Increment I-4).
//!
//! Enforces:
//! - Blueprint Decision 5 and Owner Decision Q1: Demo / synthetic data is NEVER recorded or streamed.
//! - Type-level and runtime guarantees: synthetic pipeline runs with `recorder: None, net: None`.
//! - Live feeds into `UiHub` with `Origin::Synthetic`.
//! - Realtime provider execution and graceful cancellation.

#![cfg(feature = "demo")]

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use synthetic::{DemoMode, Scenario, SyntheticTelemetryProvider, SyntheticTick};

use app_core::bridge::hub::UiHub;
use app_core::bridge::sink::{SinkClosed, SubscriptionId, UiSink};
use app_core::bridge::status::{Origin, UiEvent};
use app_core::clock::AppClock;
use app_core::driver::demo::DemoRunner;
use app_core::driver::engines::EngineHubConfig;
use app_core::driver::pipeline::{DriverPipeline, PipelineInput};
use app_core::driver::recorder_sup::{RecorderSupervisor, RecorderSupervisorConfig};
use app_core::driver::session::DriverSessionSettings;

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

#[test]
fn test_demo_cannot_reach_recorder_or_net_invariant() {
    let clock = AppClock::new();
    let session_settings = DriverSessionSettings {
        origin: Origin::Synthetic,
        ..Default::default()
    };

    let store_dir = std::env::temp_dir().join(format!(
        "test-demo-rec-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&store_dir).expect("temp dir");

    let recorder_config = RecorderSupervisorConfig {
        recorder: recorder::RecorderConfig::new(&store_dir),
        enabled: true,
    };
    let recorder = RecorderSupervisor::open(recorder_config, None, None).expect("open recorder");
    let ui_hub = Arc::new(UiHub::new());

    // 1. Attempting to construct a synthetic pipeline with recorder MUST fail
    let res_recorder = DriverPipeline::new(
        clock,
        session_settings.clone(),
        EngineHubConfig::default(),
        Some(recorder),
        None,
        ui_hub.clone(),
        None,
    );
    assert!(
        res_recorder.is_err(),
        "Synthetic pipeline MUST reject non-None recorder sink (Decision 5 / Q1)"
    );

    // 2. Constructing with recorder: None and net: None MUST succeed
    let res_valid = DriverPipeline::new(
        clock,
        session_settings,
        EngineHubConfig::default(),
        None,
        None,
        ui_hub,
        None,
    );
    assert!(res_valid.is_ok());
    let pipeline = res_valid.expect("pipeline");
    assert_eq!(pipeline.origin(), Origin::Synthetic);
    assert!(pipeline.recorder().is_none());

    let _ = std::fs::remove_dir_all(&store_dir);
}

#[test]
fn test_demo_pipeline_feeds_ui_hub_with_origin_synthetic() {
    let clock = AppClock::new();
    let session_settings = DriverSessionSettings {
        origin: Origin::Synthetic,
        ..Default::default()
    };

    let ui_hub = Arc::new(UiHub::new());
    let sink = Arc::new(TestUiSink::new());

    let mut pipeline = DriverPipeline::new(
        clock,
        session_settings,
        EngineHubConfig::default(),
        None,
        None,
        ui_hub.clone(),
        Some(sink.clone()),
    )
    .expect("create pipeline");

    let mut provider =
        SyntheticTelemetryProvider::new(DemoMode::acknowledge(), Scenario::normal_lap(1))
            .expect("create provider");

    // Feed 30 synthetic ticks through the pipeline
    for _ in 0..30 {
        let tick = provider.next_tick().expect("next tick");
        pipeline
            .step(PipelineInput::Demo(Box::new(tick)))
            .expect("step demo");
    }

    assert_eq!(pipeline.frames_processed(), 30);
    assert_eq!(pipeline.origin(), Origin::Synthetic);

    // Verify subscribe works cleanly and backfills historical blocks
    let sub_id = ui_hub.subscribe(sink.clone());
    ui_hub.flush_all();

    let batches = sink.batches.lock();
    assert!(
        !batches.is_empty(),
        "UiSink should have received batched data"
    );
    assert_eq!(batches[0].0, sub_id);
}

#[tokio::test]
async fn test_demo_runner_realtime_stream_and_stop() {
    let clock = AppClock::new();
    let session_settings = DriverSessionSettings {
        origin: Origin::Synthetic,
        ..Default::default()
    };

    let ui_hub = Arc::new(UiHub::new());
    let sink = Arc::new(TestUiSink::new());

    let pipeline = DriverPipeline::new(
        clock,
        session_settings,
        EngineHubConfig::default(),
        None,
        None,
        ui_hub,
        Some(sink),
    )
    .expect("create pipeline");

    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(16);

    let mut runner = DemoRunner::spawn(Scenario::normal_lap(1), &clock, pipeline, cmd_tx, cmd_rx)
        .expect("spawn demo runner");

    // Let realtime generator run for 100 ms
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Gracefully stop the runner
    let summary = runner.stop();
    assert!(summary.is_some(), "runner stop must return summary");
    let summary = summary.expect("summary");
    assert!(
        summary.frames_processed > 0,
        "runner must have processed frames: {}",
        summary.frames_processed
    );
}

#[test]
fn test_demo_stalled_mode_emits_stale_block() {
    let clock = AppClock::new();
    let session_settings = DriverSessionSettings {
        origin: Origin::Synthetic,
        ..Default::default()
    };

    let ui_hub = Arc::new(UiHub::new());
    let sink = Arc::new(TestUiSink::new());

    let mut pipeline = DriverPipeline::new(
        clock,
        session_settings,
        EngineHubConfig::default(),
        None,
        None,
        ui_hub.clone(),
        Some(sink.clone()),
    )
    .expect("create pipeline");

    // Feed a tick with frame: None (stalled source)
    let stalled_tick = SyntheticTick {
        origin: synthetic::DataOrigin::Synthetic(synthetic::SyntheticPurpose::Demo),
        tick: 1,
        capture_mono_ns: 1_000_000_000,
        session_et_s: 0.0,
        frame: None,
        scoring: None,
        injection: None,
        stale: Some(synthetic::StaleMode::NoFrames),
        events: Vec::new(),
    };

    pipeline
        .step(PipelineInput::Demo(Box::new(stalled_tick)))
        .expect("step stalled demo");

    // Subscribe and flush
    let _sub_id = ui_hub.subscribe(sink.clone());
    ui_hub.flush_all();

    let batches = sink.batches.lock();
    assert!(!batches.is_empty(), "stale block must be fanned out to UI");
}

#[test]
fn test_finding_1_demo_app_channels_valid_f32_types() {
    let clock = AppClock::new();
    let session_settings = DriverSessionSettings {
        origin: Origin::Synthetic,
        ..Default::default()
    };

    let ui_hub = Arc::new(UiHub::new());
    let sink = Arc::new(TestUiSink::new());

    let mut pipeline = DriverPipeline::new(
        clock,
        session_settings,
        EngineHubConfig::default(),
        None,
        None,
        ui_hub.clone(),
        Some(sink.clone()),
    )
    .expect("create pipeline");

    let mut provider =
        SyntheticTelemetryProvider::new(DemoMode::acknowledge(), Scenario::normal_lap(1))
            .expect("create provider");

    // Process a synthetic tick with telemetry and scoring
    let tick = provider.next_tick().expect("next tick");
    pipeline
        .step(PipelineInput::Demo(Box::new(tick)))
        .expect("step demo tick");

    // Subscribe and flush
    let _sub_id = ui_hub.subscribe(sink.clone());
    ui_hub.flush_all();

    let batches = sink.batches.lock();
    assert!(!batches.is_empty(), "UiSink should receive batch");

    let batch = &batches[0].1;
    let parsed = telemetry_model::uibatch::parse(batch).expect("parse batch");
    assert!(!parsed.blocks.is_empty(), "Parsed batch must have blocks");

    let mut found_pipeline_latency = false;
    let mut found_telemetry_age = false;
    let mut found_scoring_age = false;

    for block_slice in parsed.block_slices(batch) {
        let (_header, records) =
            telemetry_model::codec::decode(block_slice).expect("decode TLV block");
        for record in records {
            if record.channel == telemetry_model::ch::PIPELINE_LATENCY {
                found_pipeline_latency = true;
                assert_eq!(
                    record.sample.quality,
                    telemetry_model::value::Quality::Valid,
                    "PIPELINE_LATENCY must be valid"
                );
                assert!(
                    matches!(record.sample.value, telemetry_model::value::Value::F32(_)),
                    "PIPELINE_LATENCY must be F32 value, got {:?}",
                    record.sample.value
                );
            } else if record.channel == telemetry_model::ch::SRC_TELEMETRY_AGE {
                found_telemetry_age = true;
                assert_eq!(
                    record.sample.quality,
                    telemetry_model::value::Quality::Valid,
                    "SRC_TELEMETRY_AGE must be valid"
                );
                assert_eq!(
                    record.sample.value,
                    telemetry_model::value::Value::F32(0.0),
                    "SRC_TELEMETRY_AGE must be F32(0.0)"
                );
            } else if record.channel == telemetry_model::ch::SRC_SCORING_AGE {
                found_scoring_age = true;
                assert_eq!(
                    record.sample.quality,
                    telemetry_model::value::Quality::Valid,
                    "SRC_SCORING_AGE must be valid"
                );
                assert!(
                    matches!(record.sample.value, telemetry_model::value::Value::F32(_)),
                    "SRC_SCORING_AGE must be F32 value, got {:?}",
                    record.sample.value
                );
            }
        }
    }

    assert!(
        found_pipeline_latency,
        "PIPELINE_LATENCY must be published in demo frame (Finding 1)"
    );
    assert!(
        found_telemetry_age,
        "SRC_TELEMETRY_AGE must be published in demo frame (Finding 1)"
    );
    assert!(
        found_scoring_age,
        "SRC_SCORING_AGE must be published in demo frame (Finding 1)"
    );
}
