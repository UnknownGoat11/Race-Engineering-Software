//! `DriverLink`: the live LMU driver pipeline's connection to the relay (DEFERRED 20).
//!
//! Spawned by `AppCore` when a relay is configured, for the live LMU pipeline only (synthetic demo
//! data is never streamed, owner decision Q1). The transport [`DriverClient`] is created with the
//! app clock epoch, the shared [`AuthManager`] (backed by the credential store) and the endpoint of
//! the build environment, but it does NOT connect: streaming starts only through
//! [`StreamControl::start`], issued by the pipeline when the driver starts streaming or when
//! `driver.auto_stream` is on (owner decision Q7).
//!
//! Network failure never blocks acquisition or recording:
//! - the pipeline talks to the transport only through non-blocking calls (`send_live`,
//!   `try_send_reliable`) and a bounded control queue served by an async task here;
//! - transport events reach the pipeline through its bounded command queue with `try_send` (retried
//!   on an async timer, never blocking a thread).
//!
//! Engineer access (D-022): every engineer connection needs a fresh one-time code from the relay;
//! the driver rotates it with [`DriverClient::request_connect_code`] and removes engineers with
//! [`DriverClient::kick_engineer`]. Codes are never logged.

use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Sender, TrySendError};
use protocol::messages::{EngineerLink, SessionCreate};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use transport::auth::AuthManager;
use transport::driver::{DriverClient, DriverEvent, SessionEndOutcome, SnapshotTrigger};
use transport::endpoint::RelayEndpoint;
use transport::state::{DegradedReason, FailureReason};

use crate::bridge::status::{EngineerSlotDto, NetworkStatusDto};
use crate::clock::AppClock;
use crate::driver::net::{LinkSnapshot, LiveSink, StreamControl, StreamControlError};
use crate::driver::pipeline::PipelineCommand;

/// Tracing component name for logging.
const COMPONENT: &str = "app_core::driver::link";

/// Capacity of the bounded relay control queue (start/stop/end requests).
pub const CONTROL_QUEUE_CAPACITY: usize = 16;

/// Retry interval when the pipeline command queue is momentarily full.
const COMMAND_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Client identifier sent in HELLO by the driver link (never the account name).
const DRIVER_CLIENT_ID: &str = "app-core-driver";

/// `NetworkStatusDto::state` when no relay is configured or the pipeline can never stream.
pub const NETWORK_STATE_NOT_CONFIGURED: &str = "NOT_CONFIGURED";

/// `NetworkStatusDto::state` when the link cannot report its state.
pub const NETWORK_STATE_UNKNOWN: &str = "UNKNOWN";

/// Errors creating the driver link.
#[derive(Debug, Error)]
pub enum LinkError {
    /// The transport refused the client configuration.
    #[error("driver client configuration rejected: {0}")]
    Config(#[from] transport::ConfigError),
}

/// Requests served in order by the control task.
enum ControlRequest {
    Start(Box<SessionCreate>),
    Update(Box<SessionCreate>),
    Stop,
    End,
}

impl ControlRequest {
    const fn name(&self) -> &'static str {
        match self {
            ControlRequest::Start(_) => "start",
            ControlRequest::Update(_) => "update_session",
            ControlRequest::Stop => "stop",
            ControlRequest::End => "end_session",
        }
    }
}

/// [`StreamControl`] over a bounded queue: never blocks the caller.
#[derive(Debug, Clone)]
pub struct ChannelStreamControl {
    tx: mpsc::Sender<ControlRequest>,
}

impl std::fmt::Debug for ControlRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl ChannelStreamControl {
    fn push(&self, request: ControlRequest) -> Result<(), StreamControlError> {
        self.tx.try_send(request).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => StreamControlError::Busy,
            mpsc::error::TrySendError::Closed(_) => StreamControlError::Closed,
        })
    }
}

impl StreamControl for ChannelStreamControl {
    fn start(&self, session: SessionCreate) -> Result<(), StreamControlError> {
        self.push(ControlRequest::Start(Box::new(session)))
    }

    fn update_session(&self, session: SessionCreate) -> Result<(), StreamControlError> {
        self.push(ControlRequest::Update(Box::new(session)))
    }

    fn stop(&self) -> Result<(), StreamControlError> {
        self.push(ControlRequest::Stop)
    }

    fn end_session(&self) -> Result<(), StreamControlError> {
        self.push(ControlRequest::End)
    }
}

/// The running relay link of a live LMU driver pipeline.
pub struct DriverLink {
    client: Arc<DriverClient>,
    control: Arc<ChannelStreamControl>,
    control_task: JoinHandle<()>,
    events_task: JoinHandle<()>,
}

impl DriverLink {
    /// Spawns the transport client (idle, not connected) and its control and event tasks on
    /// `runtime`. `pipeline_tx` is the pipeline's bounded command queue.
    pub fn spawn(
        runtime: &tokio::runtime::Handle,
        endpoint: RelayEndpoint,
        clock: &AppClock,
        auth: Arc<AuthManager>,
        pipeline_tx: Sender<PipelineCommand>,
    ) -> Result<Self, LinkError> {
        let _guard = runtime.enter();
        let config = transport::ClientConfig::new(
            endpoint,
            env!("CARGO_PKG_VERSION"),
            DRIVER_CLIENT_ID,
            clock.epoch(),
        );
        let transport = transport::TcpTlsTelemetryTransport::new();
        let (client, events_rx) = DriverClient::spawn(config, transport, auth)?;
        let client = Arc::new(client);

        let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_CAPACITY);
        let control_task = runtime.spawn(run_control(Arc::clone(&client), control_rx));
        let events_task = runtime.spawn(forward_events(events_rx, pipeline_tx));

        info!(
            component = COMPONENT,
            "driver relay link ready (idle until streaming starts)"
        );
        Ok(Self {
            client,
            control: Arc::new(ChannelStreamControl { tx: control_tx }),
            control_task,
            events_task,
        })
    }

    /// The transport client (live sink for the `NetPublisher`, connect code and engineers).
    pub fn client(&self) -> &Arc<DriverClient> {
        &self.client
    }

    /// The relay session control handed to the pipeline.
    pub fn control(&self) -> Arc<dyn StreamControl> {
        Arc::clone(&self.control) as Arc<dyn StreamControl>
    }

    /// Current link state.
    pub fn snapshot(&self) -> Option<LinkSnapshot> {
        self.client.link_snapshot()
    }

    /// Stops the tasks and disconnects. The relay session is kept for a resume (the next start of
    /// this app run or the next app start resumes it): `shutdown` when this link is the client's
    /// last owner, else `stop`.
    pub async fn shutdown(self) {
        let DriverLink {
            client,
            control,
            control_task,
            events_task,
        } = self;
        drop(control);
        control_task.abort();
        events_task.abort();
        let _ = control_task.await;
        let _ = events_task.await;
        match Arc::try_unwrap(client) {
            Ok(client) => client.shutdown().await,
            Err(shared) => {
                warn!(
                    component = COMPONENT,
                    "driver client still shared at shutdown; stopping instead"
                );
                let _ = shared.stop().await;
            }
        }
        info!(component = COMPONENT, "driver relay link closed");
    }
}

/// Serves control requests in order (start/stop/end must not be reordered).
async fn run_control(client: Arc<DriverClient>, mut rx: mpsc::Receiver<ControlRequest>) {
    while let Some(request) = rx.recv().await {
        let name = request.name();
        let result = match request {
            ControlRequest::Start(session) => client.start(*session).await,
            ControlRequest::Update(session) => client.update_session(*session).await,
            ControlRequest::Stop => client.stop().await,
            ControlRequest::End => client.end_session().await,
        };
        match result {
            Ok(()) => debug!(
                component = COMPONENT,
                request = name,
                "relay control applied"
            ),
            Err(e) => {
                warn!(component = COMPONENT, request = name, error = %e, "relay control failed");
                break;
            }
        }
    }
}

/// Forwards transport events to the pipeline without ever blocking a thread.
async fn forward_events(
    mut events: mpsc::Receiver<DriverEvent>,
    pipeline_tx: Sender<PipelineCommand>,
) {
    while let Some(event) = events.recv().await {
        let command = match event {
            DriverEvent::SessionReady { session_id, .. } => {
                info!(component = COMPONENT, session_id, "relay session bound");
                Some(PipelineCommand::NetSessionReady)
            }
            DriverEvent::SnapshotNeeded { trigger } => match trigger {
                // SessionReady already resets the stream (schema + snapshot).
                SnapshotTrigger::SessionReady(_) => None,
                SnapshotTrigger::SchemaDropped => Some(PipelineCommand::NetSessionReady),
                SnapshotTrigger::Requested(_) => Some(PipelineCommand::SnapshotNeeded),
            },
            DriverEvent::RelayError { code, message } => Some(PipelineCommand::NetNotice {
                category: "relay".to_owned(),
                message: format!("Relay reported {code:?}: {message}"),
            }),
            DriverEvent::SessionEnded {
                session_id,
                outcome,
            } => {
                info!(component = COMPONENT, session_id, outcome = ?outcome, "relay session ended");
                session_end_notice(outcome).map(|message| PipelineCommand::NetNotice {
                    category: "relay_session".to_owned(),
                    message,
                })
            }
        };
        let Some(mut command) = command else {
            continue;
        };
        loop {
            match pipeline_tx.try_send(command) {
                Ok(()) => break,
                Err(TrySendError::Full(back)) => {
                    command = back;
                    tokio::time::sleep(COMMAND_RETRY_INTERVAL).await;
                }
                Err(TrySendError::Disconnected(_)) => {
                    debug!(
                        component = COMPONENT,
                        "pipeline gone; stopping event forwarding"
                    );
                    return;
                }
            }
        }
    }
}

/// User-facing notice for a session end outcome that needs attention (`None` when delivered).
fn session_end_notice(outcome: SessionEndOutcome) -> Option<String> {
    match outcome {
        SessionEndOutcome::Delivered | SessionEndOutcome::AlreadyGone => None,
        SessionEndOutcome::Deferred => Some(
            "The relay could not be reached: the session end is sent when the connection returns."
                .to_owned(),
        ),
        SessionEndOutcome::Refused { code } => Some(format!(
            "The relay refused to end the session ({code:?}); it expires after its grace period."
        )),
        SessionEndOutcome::EndedByRelay => Some("The relay ended the session.".to_owned()),
        SessionEndOutcome::NotForgotten => {
            Some("The ended session could not be removed from the credential store.".to_owned())
        }
    }
}

/// Stable snake_case name of a degraded reason.
pub const fn degraded_reason_name(reason: DegradedReason) -> &'static str {
    match reason {
        DegradedReason::MissedHeartbeat => "missed_heartbeat",
        DegradedReason::HighRtt => "high_rtt",
        DegradedReason::PacketLoss => "packet_loss",
        DegradedReason::DriverAway => "driver_away",
        DegradedReason::NoStreamData => "no_stream_data",
        DegradedReason::AwaitingSchema => "awaiting_schema",
        DegradedReason::SequenceDesync => "sequence_desync",
    }
}

const fn engineer_link_name(link: EngineerLink) -> &'static str {
    match link {
        EngineerLink::Healthy => "healthy",
        EngineerLink::Degraded => "degraded",
        EngineerLink::Reconnecting => "reconnecting",
    }
}

const NS_PER_S: u64 = 1_000_000_000;

/// Builds the network block of `core:status` for the driver.
///
/// - `relay_configured == false` (or no link at all for a pipeline that can never stream):
///   `NOT_CONFIGURED`.
/// - A link that cannot report its state: `UNKNOWN` (never shown as healthy).
/// - Otherwise the transport state with RTT, connect code (display form) and expiry, engineers
///   n/max, degraded reason (protocol 1.1 DRIVER_PRESENCE included via `driver_away`), failure.
pub fn network_status_dto(
    link: Option<&LinkSnapshot>,
    relay_configured: bool,
    streaming: bool,
) -> NetworkStatusDto {
    let max_default = protocol::limits::MAX_ENGINEERS_PER_SESSION;
    let Some(link) = link else {
        return NetworkStatusDto {
            state: if relay_configured {
                NETWORK_STATE_UNKNOWN
            } else {
                NETWORK_STATE_NOT_CONFIGURED
            }
            .to_owned(),
            streaming,
            rtt_ms: None,
            connect_code: None,
            code_expires_at_unix_s: None,
            engineers_connected: 0,
            engineers_max: max_default,
            degraded_reason: None,
            driver_away: false,
            last_error: None,
            security_issue: false,
            failure: None,
            relogin_required: false,
            engineers: Vec::new(),
        };
    };

    let status = &link.status;
    let rtt_ms = link
        .metrics
        .rtt_smoothed
        .or(link.metrics.rtt)
        .map(|d| (d.as_secs_f64() * 1000.0) as f32);
    let (connect_code, code_expires_at_unix_s) = link
        .connect_code
        .as_ref()
        .and_then(|cc| cc.code.as_ref())
        .map_or((None, None), |issued| {
            (
                Some(transport::driver::display_connect_code(&issued.code)),
                Some(issued.expires_at_ns / NS_PER_S),
            )
        });
    let (engineers_connected, engineers_max) = link.connect_code.as_ref().map_or_else(
        || {
            (
                u16::try_from(link.engineers.len()).unwrap_or(u16::MAX),
                max_default,
            )
        },
        |cc| (cc.engineers_connected, cc.engineers_max),
    );
    let engineers = link
        .engineers
        .iter()
        .map(|e| EngineerSlotDto {
            connection_id: e.connection_id,
            username: e.username.clone(),
            display_name: e.display_name.clone(),
            link: engineer_link_name(e.link).to_owned(),
            connected_since_unix_s: e.connected_since_ns / NS_PER_S,
        })
        .collect();

    NetworkStatusDto {
        state: status.state.name().to_owned(),
        streaming,
        rtt_ms,
        connect_code,
        code_expires_at_unix_s,
        engineers_connected,
        engineers_max,
        degraded_reason: status.degraded.map(|r| degraded_reason_name(r).to_owned()),
        driver_away: status.driver_away,
        last_error: status
            .last_error
            .clone()
            .or_else(|| status.persist_error.clone()),
        security_issue: status.security_issue,
        failure: status.failure.as_ref().map(ToString::to_string),
        relogin_required: matches!(status.failure, Some(FailureReason::ReloginRequired(_))),
        engineers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::messages::{ConnectCode, EngineerEntry, IssuedCode, Secret};
    use transport::ConnectionState;
    use transport::metrics::LinkMetrics;
    use transport::state::{ConnectionStatus, ReloginCause};

    fn snapshot(status: ConnectionStatus) -> LinkSnapshot {
        LinkSnapshot {
            status,
            metrics: LinkMetrics::default(),
            connect_code: None,
            engineers: Vec::new(),
            clock: None,
        }
    }

    #[test]
    fn no_relay_is_not_configured_and_no_link_is_unknown() {
        let s = network_status_dto(None, false, false);
        assert_eq!(s.state, NETWORK_STATE_NOT_CONFIGURED);
        let s = network_status_dto(None, true, false);
        assert_eq!(s.state, NETWORK_STATE_UNKNOWN);
        assert!(s.connect_code.is_none());
    }

    #[test]
    fn link_state_code_engineers_and_failure_are_reported() {
        let mut status = ConnectionStatus {
            state: ConnectionState::Degraded,
            degraded: Some(DegradedReason::HighRtt),
            ..ConnectionStatus::default()
        };
        let mut snap = snapshot(status.clone());
        snap.metrics.rtt_smoothed = Some(Duration::from_millis(42));
        snap.connect_code = Some(ConnectCode {
            code: Some(IssuedCode {
                code: Secret::new("7KQ4MZ9T"),
                expires_at_ns: 1_700_000_000 * NS_PER_S,
            }),
            engineers_connected: 1,
            engineers_max: 2,
        });
        snap.engineers = vec![EngineerEntry {
            connection_id: 7,
            username: "erin".to_owned(),
            display_name: "Erin".to_owned(),
            connected_since_ns: 5 * NS_PER_S,
            link: EngineerLink::Healthy,
        }];
        let dto = network_status_dto(Some(&snap), true, true);
        assert_eq!(dto.state, "DEGRADED");
        assert!(dto.streaming);
        assert_eq!(dto.degraded_reason.as_deref(), Some("high_rtt"));
        assert_eq!(dto.connect_code.as_deref(), Some("7KQ4-MZ9T"));
        assert_eq!(dto.code_expires_at_unix_s, Some(1_700_000_000));
        assert_eq!((dto.engineers_connected, dto.engineers_max), (1, 2));
        assert_eq!(dto.engineers[0].connection_id, 7);
        assert_eq!(dto.engineers[0].link, "healthy");
        assert!(dto.rtt_ms.is_some_and(|r| (r - 42.0).abs() < 0.01));

        status.state = ConnectionState::Failed;
        status.degraded = None;
        status.failure = Some(FailureReason::ReloginRequired(ReloginCause::NotLoggedIn));
        let dto = network_status_dto(Some(&snapshot(status)), true, false);
        assert_eq!(dto.state, "FAILED");
        assert!(dto.relogin_required);
        assert!(dto.failure.is_some());
    }
}
