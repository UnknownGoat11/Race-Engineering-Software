//! Staged connectivity test for the "Test Relay Connection" UI:
//! DNS → TCP → TLS → HELLO → AUTH → session service → RTT. Each stage reports pass / fail with a
//! human-readable detail or error and its duration; stages after a failure are skipped.
//!
//! * TLS validates the certificate exactly like a real connection (webpki roots, development-only
//!   extra CA) and requires ALPN `lmure/1`.
//! * HELLO checks the protocol version and the environment **before** AUTH, so a mismatching relay
//!   never sees a credential.
//! * AUTH uses the shared [`AuthManager`] (access token if valid, else the refresh token, whose
//!   rotation is persisted, even if the test is cancelled meanwhile), so running the test never
//!   invalidates the login.
//! * "Session service" is checked without side effects: the account was granted the requested role
//!   and the relay announces a usable session service (engineer slots and a driver grace period).
//!   No session is created or resumed by the test.
//! * RTT: [`RTT_PROBES`] PING/PONG exchanges, reporting min / avg / max and the clock offset.
//!
//! Note: run it while not streaming. Whether a relay replaces a live connection when the same
//! account authenticates again with the same role is relay policy (`DISCONNECT{Replaced}`).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use protocol::clock::ClockSync;
use protocol::messages::{ClientRole, DisconnectReason, Message, ServerHello};
use tokio::time::{Instant, timeout};

use crate::auth::AuthManager;
use crate::clock::LinkClock;
use crate::config::ClientConfig;
use crate::handshake::{self, Link};
use crate::session::{disconnect, error_chain};
use crate::transport::{FrameWriter, TcpTlsTelemetryTransport, framed_split};

/// PING/PONG exchanges of the RTT stage.
pub const RTT_PROBES: u64 = 5;
/// Pause between RTT probes.
pub const RTT_PROBE_INTERVAL: Duration = Duration::from_millis(200);

/// A stage of the test, in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Dns,
    Tcp,
    Tls,
    Hello,
    Auth,
    SessionService,
    Rtt,
}

impl Stage {
    /// Every stage, in order.
    pub const ALL: [Stage; 7] = [
        Stage::Dns,
        Stage::Tcp,
        Stage::Tls,
        Stage::Hello,
        Stage::Auth,
        Stage::SessionService,
        Stage::Rtt,
    ];

    /// Display label.
    pub const fn label(self) -> &'static str {
        match self {
            Stage::Dns => "DNS lookup",
            Stage::Tcp => "TCP connection",
            Stage::Tls => "TLS (certificate)",
            Stage::Hello => "Protocol handshake",
            Stage::Auth => "Login",
            Stage::SessionService => "Session service",
            Stage::Rtt => "Round-trip time",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Result of one stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    Passed {
        detail: String,
    },
    Failed {
        error: String,
    },
    /// Not run because an earlier stage failed.
    Skipped,
}

/// One stage with its outcome and duration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageResult {
    pub stage: Stage,
    pub outcome: StageOutcome,
    pub duration: Duration,
}

/// RTT statistics of the last stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RttSummary {
    pub samples: u64,
    pub min: Duration,
    pub avg: Duration,
    pub max: Duration,
    /// Clock offset to the relay (θ) and its uncertainty, if a sample was accepted.
    pub clock_offset_ns: Option<i64>,
    pub clock_uncertainty_ns: Option<u64>,
}

/// The whole test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectivityReport {
    /// The endpoint tested (`host:port`).
    pub endpoint: String,
    /// Every stage in order (skipped ones included).
    pub stages: Vec<StageResult>,
    pub rtt: Option<RttSummary>,
}

impl ConnectivityReport {
    /// Whether every stage passed.
    pub fn passed(&self) -> bool {
        self.stages
            .iter()
            .all(|s| matches!(s.outcome, StageOutcome::Passed { .. }))
    }

    /// The first failed stage.
    pub fn first_failure(&self) -> Option<&StageResult> {
        self.stages
            .iter()
            .find(|s| matches!(s.outcome, StageOutcome::Failed { .. }))
    }

    /// Outcome of one stage.
    pub fn stage(&self, stage: Stage) -> Option<&StageOutcome> {
        self.stages
            .iter()
            .find(|s| s.stage == stage)
            .map(|s| &s.outcome)
    }
}

struct Recorder {
    report: ConnectivityReport,
    started: Instant,
}

impl Recorder {
    fn begin(&mut self) {
        self.started = Instant::now();
    }

    fn pass(&mut self, stage: Stage, detail: String) {
        self.push(stage, StageOutcome::Passed { detail });
    }

    fn fail(mut self, stage: Stage, error: String) -> ConnectivityReport {
        tracing::info!(component = "transport.connectivity", %stage, error = %error, "stage failed");
        self.push(stage, StageOutcome::Failed { error });
        for later in Stage::ALL.iter().skip_while(|s| **s != stage).skip(1) {
            self.report.stages.push(StageResult {
                stage: *later,
                outcome: StageOutcome::Skipped,
                duration: Duration::ZERO,
            });
        }
        self.report
    }

    fn push(&mut self, stage: Stage, outcome: StageOutcome) {
        self.report.stages.push(StageResult {
            stage,
            outcome,
            duration: self.started.elapsed(),
        });
    }
}

/// Runs the staged test against `config.endpoint` as `role`. `auth` is the application's shared
/// [`AuthManager`] (the test authenticates silently, like a real connection).
pub async fn test_relay_connection(
    transport: &TcpTlsTelemetryTransport,
    config: &ClientConfig,
    auth: &Arc<AuthManager>,
    role: ClientRole,
) -> ConnectivityReport {
    let mut rec = Recorder {
        report: ConnectivityReport {
            endpoint: config.endpoint.to_string(),
            stages: Vec::with_capacity(Stage::ALL.len()),
            rtt: None,
        },
        started: Instant::now(),
    };
    let endpoint = &config.endpoint;

    rec.begin();
    let addrs = match transport.resolve(endpoint).await {
        Ok(addrs) => addrs,
        Err(e) => return rec.fail(Stage::Dns, error_chain(&e)),
    };
    let listed: Vec<String> = addrs.iter().map(|a| a.ip().to_string()).collect();
    rec.pass(
        Stage::Dns,
        format!("{} resolves to {}", endpoint.host(), listed.join(", ")),
    );

    rec.begin();
    let tcp = match transport.connect_tcp(&addrs).await {
        Ok(tcp) => tcp,
        Err(e) => return rec.fail(Stage::Tcp, error_chain(&e)),
    };
    let peer = tcp
        .peer_addr()
        .map_or_else(|_| endpoint.to_string(), |a| a.to_string());
    rec.pass(Stage::Tcp, format!("connected to {peer}"));

    rec.begin();
    let tls = match transport.handshake_tls(endpoint, tcp).await {
        Ok(tls) => tls,
        Err(e) => return rec.fail(Stage::Tls, error_chain(&e)),
    };
    let (version, suite) = TcpTlsTelemetryTransport::tls_details(&tls);
    rec.pass(
        Stage::Tls,
        format!(
            "{} {}, certificate trusted, ALPN lmure/1",
            version.unwrap_or_default(),
            suite.unwrap_or_default()
        ),
    );

    let (reader, writer, stats) = framed_split(tls);
    let clock = LinkClock::with_epoch(config.mono_epoch);
    let mut link = Link {
        reader,
        writer,
        stats,
        info: Default::default(),
        clock,
        reply_timeout: config.timing.handshake_timeout,
    };

    rec.begin();
    let server = match handshake::hello(&mut link, config, role).await {
        Ok(server) => server,
        Err(e) => return rec.fail(Stage::Hello, error_chain(&e)),
    };
    rec.pass(
        Stage::Hello,
        format!(
            "relay {} speaks protocol {}, environment {:?}",
            server.relay_version, server.protocol, server.env
        ),
    );

    rec.begin();
    // Detached like a real connection: cancelling the test during AUTH keeps the login intact.
    let auth_result =
        handshake::authenticate_detached(link, Arc::clone(auth), role, server.clone()).await;
    let (mut link, account) = match auth_result {
        Ok(authenticated) => authenticated,
        Err(e) => {
            let text = match e.disposition() {
                handshake::Disposition::Fail(reason) => reason.to_string(),
                handshake::Disposition::Retry => error_chain(&e),
            };
            return rec.fail(Stage::Auth, text);
        }
    };
    rec.pass(
        Stage::Auth,
        format!("logged in as {} ({:?})", account.username, account.role),
    );

    rec.begin();
    match session_service(&server, role) {
        Ok(detail) => rec.pass(Stage::SessionService, detail),
        Err(error) => return rec.fail(Stage::SessionService, error),
    }

    rec.begin();
    match rtt_probes(&mut link).await {
        Ok(summary) => {
            rec.report.rtt = Some(summary);
            rec.pass(
                Stage::Rtt,
                format!(
                    "{} samples: min {:.1} ms, avg {:.1} ms, max {:.1} ms",
                    summary.samples,
                    summary.min.as_secs_f64() * 1e3,
                    summary.avg.as_secs_f64() * 1e3,
                    summary.max.as_secs_f64() * 1e3
                ),
            );
        }
        Err(error) => return rec.fail(Stage::Rtt, error),
    }

    if link
        .send(&disconnect(DisconnectReason::ClientShutdown))
        .await
        .is_ok()
    {
        let _ = timeout(config.timing.close_timeout, link.writer.close()).await;
    }
    rec.report
}

/// Side-effect-free session-service check.
fn session_service(server: &ServerHello, role: ClientRole) -> Result<String, String> {
    let limits = server.limits;
    if limits.max_engineers_per_session == 0 {
        return Err("the relay announces no engineer slots".to_owned());
    }
    match role {
        ClientRole::Driver => Ok(format!(
            "driver sessions available: up to {} engineers, resume within {} s",
            limits.max_engineers_per_session, limits.session_driver_grace_s
        )),
        ClientRole::Engineer => Ok(format!(
            "joining with a connection code is available (up to {} engineers per driver)",
            limits.max_engineers_per_session
        )),
    }
}

async fn rtt_probes<R, W>(link: &mut Link<R, W>) -> Result<RttSummary, String>
where
    R: crate::transport::FrameReader,
    W: FrameWriter,
{
    let mut sync = ClockSync::new();
    let mut rtts = Vec::new();
    'probes: for id in 1..=RTT_PROBES {
        if id > 1 {
            tokio::time::sleep(RTT_PROBE_INTERVAL).await;
        }
        let t0 = link.clock.now_ns();
        let sent = link
            .send(&Message::Ping(protocol::messages::Ping { id, t0 }))
            .await;
        if let Err(e) = sent {
            if rtts.is_empty() {
                return Err(error_chain(&e));
            }
            // Report the samples measured before the link failed.
            break 'probes;
        }
        // Wait for this PING's PONG (other frames are not expected here and are skipped).
        loop {
            match link.recv_message("PONG").await {
                Ok((_, Message::Pong(pong))) if pong.id == id => {
                    let t3 = link.clock.now_ns();
                    if let Err(e) = sync.on_pong(id, t0, &pong, t3) {
                        tracing::debug!(component = "transport.connectivity", error = %e, "clock sample rejected");
                    }
                    rtts.push(Duration::from_nanos(t3.saturating_sub(t0)));
                    break;
                }
                Ok(_) => continue,
                Err(e) if rtts.is_empty() => return Err(error_chain(&e)),
                Err(_) => break 'probes,
            }
        }
    }
    let (Some(min), Some(max)) = (rtts.iter().min().copied(), rtts.iter().max().copied()) else {
        return Err("no PONG received".to_owned());
    };
    let total: Duration = rtts.iter().sum();
    let samples = rtts.len() as u64;
    let estimate = sync.estimate();
    Ok(RttSummary {
        samples,
        min,
        avg: total / u32::try_from(samples).unwrap_or(1),
        max,
        clock_offset_ns: estimate.map(|e| e.offset_ns),
        clock_uncertainty_ns: estimate.map(|e| e.uncertainty_ns),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::messages::ServerLimits;
    use protocol::version::{Environment, ProtocolVersion};

    fn hello(max_engineers: u16) -> ServerHello {
        ServerHello {
            relay_version: "test".into(),
            protocol: ProtocolVersion::CURRENT,
            server_time_ns: 0,
            limits: ServerLimits {
                max_payload_bytes: 262_144,
                heartbeat_interval_ms: 1_000,
                heartbeat_timeout_ms: 5_000,
                max_engineers_per_session: max_engineers,
                session_driver_grace_s: 600,
            },
            env: Environment::Development,
        }
    }

    #[test]
    fn session_service_check() {
        assert!(session_service(&hello(2), ClientRole::Driver).is_ok());
        assert!(session_service(&hello(2), ClientRole::Engineer).is_ok());
        assert!(session_service(&hello(0), ClientRole::Engineer).is_err());
    }

    #[test]
    fn failure_skips_later_stages() {
        let rec = Recorder {
            report: ConnectivityReport {
                endpoint: "x:1".into(),
                stages: Vec::new(),
                rtt: None,
            },
            started: Instant::now(),
        };
        let report = rec.fail(Stage::Tcp, "refused".into());
        assert_eq!(
            report.stages.len(),
            Stage::ALL.len() - 1,
            "DNS was not recorded"
        );
        assert!(!report.passed());
        assert_eq!(report.first_failure().map(|s| s.stage), Some(Stage::Tcp));
        assert_eq!(report.stage(Stage::Rtt), Some(&StageOutcome::Skipped));
    }
}
