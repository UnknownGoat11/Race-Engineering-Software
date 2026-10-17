//! Pipeline command handling (DEFERRED 20 and 22): typed refusals instead of silent no-ops,
//! alert acknowledgement, relay session control from the session machine, streaming gating and
//! the network status block. Deterministic: mock live sink and mock relay control, real LMU
//! capture frames.

use std::sync::Arc;
use std::time::Instant;

use lmu_reader::{CaptureReplay, RawSnapshot};
use parking_lot::Mutex;
use protocol::frame::FramePriority;
use protocol::messages::{Message, SessionCreate};
use race_engines::alert::{AlertCategory, AlertClock, AlertConfig, AlertRule, Comparison};
use telemetry_model::events::Severity;
use transport::metrics::LinkMetrics;
use transport::queue::{LiveOutcome, QueueError};
use transport::state::{ConnectionState, ConnectionStatus};

use app_core::bridge::hub::UiHub;
use app_core::bridge::sink::{SinkClosed, SubscriptionId, UiSink};
use app_core::bridge::status::{Origin, UiEvent};
use app_core::clock::AppClock;
use app_core::driver::engines::EngineHubConfig;
use app_core::driver::net::{
    LinkSnapshot, LiveSink, NetPublisher, StreamControl, StreamControlError,
};
use app_core::driver::pipeline::{
    CommandRejected, DriverPipeline, PipelineCommand, PipelineError, PipelineInput,
    PipelineLmuInput,
};
use app_core::driver::session::DriverSessionSettings;

#[derive(Default)]
struct TestUiSink {
    events: Mutex<Vec<UiEvent>>,
}

impl UiSink for TestUiSink {
    fn send_batch(&self, _sub: SubscriptionId, _batch: Vec<u8>) -> Result<(), SinkClosed> {
        Ok(())
    }

    fn emit(&self, event: UiEvent) {
        self.events.lock().push(event);
    }
}

/// Live sink that records what the pipeline streams and reports a configurable link state.
struct MockSink {
    live: Mutex<Vec<Message>>,
    reliable: Mutex<Vec<Message>>,
    state: Mutex<ConnectionState>,
}

impl MockSink {
    fn new() -> Self {
        Self {
            live: Mutex::new(Vec::new()),
            reliable: Mutex::new(Vec::new()),
            state: Mutex::new(ConnectionState::Disconnected),
        }
    }
}

impl LiveSink for MockSink {
    fn send_live(
        &self,
        message: Message,
        _priority: FramePriority,
    ) -> Result<LiveOutcome, QueueError> {
        self.live.lock().push(message);
        Ok(LiveOutcome {
            replaced: false,
            superseded: 0,
        })
    }

    fn try_send_reliable(&self, message: Message) -> Result<(), QueueError> {
        self.reliable.lock().push(message);
        Ok(())
    }

    fn connection_status(&self) -> ConnectionStatus {
        ConnectionStatus {
            state: *self.state.lock(),
            ..ConnectionStatus::default()
        }
    }

    fn relay_time_of(&self, _at: Instant) -> Option<i64> {
        None
    }

    fn link_snapshot(&self) -> Option<LinkSnapshot> {
        Some(LinkSnapshot {
            status: self.connection_status(),
            metrics: LinkMetrics::default(),
            connect_code: None,
            engineers: Vec::new(),
            clock: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ControlCall {
    Start,
    Update,
    Stop,
    End,
}

/// Relay control that records calls; can be made busy.
#[derive(Default)]
struct MockControl {
    calls: Mutex<Vec<ControlCall>>,
    busy: Mutex<bool>,
}

impl MockControl {
    fn push(&self, call: ControlCall) -> Result<(), StreamControlError> {
        if *self.busy.lock() {
            return Err(StreamControlError::Busy);
        }
        self.calls.lock().push(call);
        Ok(())
    }

    fn count(&self, call: &ControlCall) -> usize {
        self.calls.lock().iter().filter(|c| *c == call).count()
    }
}

impl StreamControl for MockControl {
    fn start(&self, _session: SessionCreate) -> Result<(), StreamControlError> {
        self.push(ControlCall::Start)
    }
    fn update_session(&self, _session: SessionCreate) -> Result<(), StreamControlError> {
        self.push(ControlCall::Update)
    }
    fn stop(&self) -> Result<(), StreamControlError> {
        self.push(ControlCall::Stop)
    }
    fn end_session(&self) -> Result<(), StreamControlError> {
        self.push(ControlCall::End)
    }
}

fn capture_snapshots(n: usize) -> Vec<RawSnapshot> {
    let path = std::path::PathBuf::from("D:/RaceEngineeringData/captures/pitstop1.lmuraw");
    let mut replay = CaptureReplay::open(&path).expect("open capture");
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        match replay.next_snapshot().expect("read capture") {
            Some(s) => out.push(s),
            None => break,
        }
    }
    out
}

struct Harness {
    pipeline: DriverPipeline,
    sink: Arc<MockSink>,
    control: Arc<MockControl>,
    ui: Arc<TestUiSink>,
}

fn harness(origin: Origin, engine_config: EngineHubConfig, with_net: bool) -> Harness {
    let clock = AppClock::new();
    let sink = Arc::new(MockSink::new());
    let control = Arc::new(MockControl::default());
    let ui = Arc::new(TestUiSink::default());
    let net = with_net.then(|| {
        NetPublisher::new(Arc::clone(&sink) as Arc<dyn LiveSink>, clock)
            .with_control(Arc::clone(&control) as Arc<dyn StreamControl>)
    });
    let pipeline = DriverPipeline::new(
        clock,
        DriverSessionSettings {
            origin,
            ..DriverSessionSettings::default()
        },
        engine_config,
        None,
        net,
        Arc::new(UiHub::new()),
        Some(Arc::clone(&ui) as Arc<dyn UiSink>),
    )
    .expect("pipeline");
    Harness {
        pipeline,
        sink,
        control,
        ui,
    }
}

fn feed(pipeline: &mut DriverPipeline, snapshots: &[RawSnapshot]) {
    for snap in snapshots {
        pipeline
            .step(PipelineInput::Lmu(Box::new(PipelineLmuInput {
                snapshot: Box::new(snap.clone()),
                overrun_before: false,
                stats: None,
            })))
            .expect("frame");
    }
}

fn command(pipeline: &mut DriverPipeline, cmd: PipelineCommand) -> Result<(), CommandRejected> {
    match pipeline.step(PipelineInput::Command(cmd)) {
        Ok(()) => Ok(()),
        Err(PipelineError::Rejected(r)) => Err(r),
        Err(other) => panic!("unexpected pipeline error: {other}"),
    }
}

/// Without a recorder or relay, recording and streaming commands are refused with typed errors.
#[test]
fn commands_without_recorder_or_net_are_typed_refusals() {
    let mut h = harness(Origin::Lmu, EngineHubConfig::default(), false);
    assert_eq!(
        command(&mut h.pipeline, PipelineCommand::SetRecording(true)),
        Err(CommandRejected::RecorderUnavailable)
    );
    assert_eq!(
        command(&mut h.pipeline, PipelineCommand::AddMarker("x".into())),
        Err(CommandRejected::RecorderUnavailable)
    );
    assert_eq!(
        command(&mut h.pipeline, PipelineCommand::StartStreaming),
        Err(CommandRejected::NetworkUnavailable)
    );
    assert_eq!(
        command(&mut h.pipeline, PipelineCommand::StopStreaming),
        Err(CommandRejected::NetworkUnavailable)
    );
    assert_eq!(
        command(&mut h.pipeline, PipelineCommand::AckAlert(9)),
        Err(CommandRejected::UnknownAlert(9))
    );
    // Network block: no relay -> NOT_CONFIGURED.
    assert_eq!(h.pipeline.build_status().network.state, "NOT_CONFIGURED");

    // The request/reply wrapper returns the refusal to the caller.
    let (tx, rx) = crossbeam_channel::bounded(1);
    command(
        &mut h.pipeline,
        PipelineCommand::Request {
            command: Box::new(PipelineCommand::SetRecording(false)),
            reply: tx,
        },
    )
    .expect("request itself is handled");
    assert_eq!(
        rx.try_recv().expect("reply"),
        Err(CommandRejected::RecorderUnavailable)
    );
}

/// Synthetic data is never streamed, even if a publisher were attached.
#[test]
fn synthetic_pipeline_refuses_streaming() {
    let mut h = harness(Origin::Synthetic, EngineHubConfig::default(), false);
    assert_eq!(
        command(&mut h.pipeline, PipelineCommand::StartStreaming),
        Err(CommandRejected::SyntheticNeverStreamed)
    );
}

/// Streaming: refused without login; manual start drives the relay control; telemetry is sent
/// only while streaming; SessionReady sends SCHEMA_NEGOTIATE; stop/end use the control.
#[test]
fn streaming_is_gated_and_drives_the_relay_control() {
    let snaps = capture_snapshots(300);
    let mut h = harness(Origin::Lmu, EngineHubConfig::default(), true);
    assert_eq!(h.pipeline.build_status().network.state, "DISCONNECTED");

    feed(&mut h.pipeline, &snaps[..100]);
    assert!(
        h.sink.live.lock().is_empty(),
        "nothing streams before start"
    );
    assert_eq!(h.control.count(&ControlCall::Start), 0, "auto-stream off");

    assert_eq!(
        command(&mut h.pipeline, PipelineCommand::StartStreaming),
        Err(CommandRejected::NotLoggedIn)
    );
    command(
        &mut h.pipeline,
        PipelineCommand::AuthStatus { logged_in: true },
    )
    .expect("auth");
    assert_eq!(
        h.control.count(&ControlCall::Start),
        0,
        "login alone never streams"
    );

    command(&mut h.pipeline, PipelineCommand::StartStreaming).expect("start");
    assert_eq!(h.control.count(&ControlCall::Start), 1);
    assert!(h.pipeline.build_status().network.streaming);
    // Idempotent: a second start does not restart the relay session.
    command(&mut h.pipeline, PipelineCommand::StartStreaming).expect("start again");
    assert_eq!(h.control.count(&ControlCall::Start), 1);

    command(&mut h.pipeline, PipelineCommand::NetSessionReady).expect("session ready");
    assert!(
        h.sink
            .reliable
            .lock()
            .iter()
            .any(|m| matches!(m, Message::SchemaNegotiate(_))),
        "schema negotiate after the session is bound"
    );
    *h.sink.state.lock() = ConnectionState::Connected;
    feed(&mut h.pipeline, &snaps[100..200]);
    {
        let live = h.sink.live.lock();
        assert!(
            live.iter()
                .any(|m| matches!(m, Message::TelemetrySnapshot(_))),
            "snapshot first after session ready"
        );
        assert!(
            live.iter().any(|m| matches!(m, Message::TelemetryDelta(_))),
            "live deltas while streaming"
        );
    }

    command(&mut h.pipeline, PipelineCommand::StopStreaming).expect("stop");
    assert_eq!(h.control.count(&ControlCall::Stop), 1);
    assert!(!h.pipeline.build_status().network.streaming);
    let sent = h.sink.live.lock().len();
    feed(&mut h.pipeline, &snaps[200..250]);
    assert_eq!(h.sink.live.lock().len(), sent, "nothing streams after stop");

    // End session while not streaming still ends the kept relay session.
    command(&mut h.pipeline, PipelineCommand::EndSession).expect("end");
    assert_eq!(h.control.count(&ControlCall::End), 1);
}

/// A relay control queue that is full reverts the start and reports a typed error.
#[test]
fn busy_relay_control_reverts_start() {
    let mut h = harness(Origin::Lmu, EngineHubConfig::default(), true);
    command(
        &mut h.pipeline,
        PipelineCommand::AuthStatus { logged_in: true },
    )
    .expect("auth");
    *h.control.busy.lock() = true;
    assert_eq!(
        command(&mut h.pipeline, PipelineCommand::StartStreaming),
        Err(CommandRejected::Control(StreamControlError::Busy))
    );
    assert!(!h.pipeline.build_status().network.streaming);
}

/// Auto-stream starts the relay session once logged in and LMU data is live.
#[test]
fn auto_stream_starts_once_logged_in() {
    let snaps = capture_snapshots(50);
    let clock = AppClock::new();
    let sink = Arc::new(MockSink::new());
    let control = Arc::new(MockControl::default());
    let net = NetPublisher::new(Arc::clone(&sink) as Arc<dyn LiveSink>, clock)
        .with_control(Arc::clone(&control) as Arc<dyn StreamControl>);
    let mut pipeline = DriverPipeline::new(
        clock,
        DriverSessionSettings {
            auto_stream: true,
            ..DriverSessionSettings::default()
        },
        EngineHubConfig::default(),
        None,
        Some(net),
        Arc::new(UiHub::new()),
        None,
    )
    .expect("pipeline");
    feed(&mut pipeline, &snaps[..20]);
    assert_eq!(control.count(&ControlCall::Start), 0, "not logged in yet");
    pipeline
        .step(PipelineInput::Command(PipelineCommand::AuthStatus {
            logged_in: true,
        }))
        .expect("auth");
    feed(&mut pipeline, &snaps[20..]);
    assert_eq!(control.count(&ControlCall::Start), 1);
}

/// Alert acknowledgement reaches the AlertEngine; unknown ids are refused.
#[test]
fn alert_acknowledgement_reaches_the_engine() {
    let config = EngineHubConfig {
        alert: AlertConfig {
            include_default_rules: false,
            rules: vec![AlertRule {
                id: "speed_test".into(),
                channel: "vehicle.speed_mps".into(),
                category: AlertCategory::DataQuality,
                severity: Severity::Warning,
                comparison: Comparison::Above,
                trigger: 5.0,
                clear: 4.0,
                min_duration_s: 0.0,
                cooldown_s: 0.0,
                message: "test".into(),
            }],
            ..AlertConfig::default()
        },
        ..EngineHubConfig::default()
    };
    let mut h = harness(Origin::Lmu, config, false);
    let mut frame = telemetry_model::frame::TelemetryFrame::new(1, 1_000_000_000, Some(1.0));
    let _ = frame.set_valid(
        telemetry_model::ch::SPEED,
        telemetry_model::value::Value::F32(10.0),
    );
    let notices = h.pipeline.engines_mut().alert.evaluate_frame(
        AlertClock::new(1.0, Some(1.0)),
        None,
        &frame,
    );
    let active: Vec<u64> = h
        .pipeline
        .engines_mut()
        .alert
        .active()
        .iter()
        .map(|a| a.id)
        .collect();
    if let Some(&id) = active.first() {
        command(&mut h.pipeline, PipelineCommand::AckAlert(id)).expect("ack");
        assert!(
            h.pipeline
                .engines_mut()
                .alert
                .active()
                .iter()
                .any(|a| a.id == id && a.acknowledged)
        );
    } else {
        panic!("test rule did not raise (notices: {notices:?})");
    }
    assert_eq!(
        command(&mut h.pipeline, PipelineCommand::AckAlert(u64::MAX)),
        Err(CommandRejected::UnknownAlert(u64::MAX))
    );
    let _ = &h.ui;
}
