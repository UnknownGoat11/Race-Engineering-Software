//! Client configuration: the relay endpoint plus timing and queue parameters. Every default comes
//! from `protocol::timing` (PROTOCOL.md §7) or a named constant here.

use std::time::Duration;

use protocol::timing;
use thiserror::Error;

use crate::endpoint::RelayEndpoint;
use crate::liveness::LivenessConfig;
use crate::queue::DEFAULT_RELIABLE_CAPACITY;

/// Default timeout for each reply during the handshake (HELLO, AUTH, SESSION_*).
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Default period of the liveness evaluation (sub-second so state changes are timely).
pub const DEFAULT_LIVENESS_CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// Default period of the metrics publication.
pub const DEFAULT_METRICS_INTERVAL: Duration = Duration::from_secs(1);
/// A connection that stayed up this long resets the reconnect backoff.
pub const DEFAULT_BACKOFF_RESET_AFTER: Duration = Duration::from_secs(10);
/// Time allowed for a graceful close (DISCONNECT, TLS close_notify).
pub const DEFAULT_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
/// Default capacity of the application event channel.
pub const DEFAULT_EVENT_CAPACITY: usize = 1_024;
/// Default capacity of the engineer's buffer for reliable stream messages received before the
/// identity was confirmed.
pub const DEFAULT_PENDING_RELIABLE_CAPACITY: usize = 256;
/// Default capacity of the command channel of a client.
pub const DEFAULT_COMMAND_CAPACITY: usize = 64;

/// Timing parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimingConfig {
    /// PING period (`HEARTBEAT_INTERVAL_MS`).
    pub heartbeat_interval: Duration,
    /// DEGRADED / STALE / timeout thresholds.
    pub liveness: LivenessConfig,
    /// Period of the liveness evaluation.
    pub liveness_check_interval: Duration,
    /// Reconnect backoff bounds (`RECONNECT_MIN_MS` / `RECONNECT_MAX_MS`).
    pub reconnect_min: Duration,
    pub reconnect_max: Duration,
    /// A connection healthy for this long resets the backoff.
    pub backoff_reset_after: Duration,
    /// Timeout of each handshake reply.
    pub handshake_timeout: Duration,
    /// A single frame write taking longer than this means the link is dead.
    pub write_timeout: Duration,
    /// Graceful close budget.
    pub close_timeout: Duration,
    /// Engineer: how long after a drop the in-memory resume ticket is still tried
    /// (`ENGINEER_TICKET_TTL_S`).
    pub engineer_ticket_ttl: Duration,
    /// Metrics publication period.
    pub metrics_interval: Duration,
}

impl Default for TimingConfig {
    fn default() -> Self {
        let liveness = LivenessConfig::default();
        Self {
            heartbeat_interval: Duration::from_millis(timing::HEARTBEAT_INTERVAL_MS),
            liveness,
            liveness_check_interval: DEFAULT_LIVENESS_CHECK_INTERVAL,
            reconnect_min: Duration::from_millis(timing::RECONNECT_MIN_MS),
            reconnect_max: Duration::from_millis(timing::RECONNECT_MAX_MS),
            backoff_reset_after: DEFAULT_BACKOFF_RESET_AFTER,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            write_timeout: liveness.timeout_after,
            close_timeout: DEFAULT_CLOSE_TIMEOUT,
            engineer_ticket_ttl: Duration::from_secs(timing::ENGINEER_TICKET_TTL_S),
            metrics_interval: DEFAULT_METRICS_INTERVAL,
        }
    }
}

/// An unusable client configuration (refused by [`ClientConfig::validate`] and the clients'
/// `spawn`).
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ConfigError {
    #[error("timing parameter `{field}` must be greater than zero")]
    ZeroDuration { field: &'static str },
    #[error("reconnect_max ({max:?}) must not be below reconnect_min ({min:?})")]
    ReconnectBounds { min: Duration, max: Duration },
    #[error("loss_degraded_fraction must be a number in (0, 1], got {0}")]
    LossFraction(f64),
}

impl TimingConfig {
    /// Checks that every duration is non-zero (a zero period would panic or spin: e.g. a zero
    /// liveness check interval cannot drive a timer, zero reconnect delays would hammer the relay)
    /// and that the bounds are consistent.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let durations = [
            ("heartbeat_interval", self.heartbeat_interval),
            ("liveness.degraded_after", self.liveness.degraded_after),
            ("liveness.stale_after", self.liveness.stale_after),
            ("liveness.timeout_after", self.liveness.timeout_after),
            ("liveness.rtt_degraded", self.liveness.rtt_degraded),
            ("liveness_check_interval", self.liveness_check_interval),
            ("reconnect_min", self.reconnect_min),
            ("reconnect_max", self.reconnect_max),
            ("backoff_reset_after", self.backoff_reset_after),
            ("handshake_timeout", self.handshake_timeout),
            ("write_timeout", self.write_timeout),
            ("close_timeout", self.close_timeout),
            ("engineer_ticket_ttl", self.engineer_ticket_ttl),
            ("metrics_interval", self.metrics_interval),
        ];
        if let Some((field, _)) = durations.iter().find(|(_, d)| d.is_zero()) {
            return Err(ConfigError::ZeroDuration { field });
        }
        if self.reconnect_max < self.reconnect_min {
            return Err(ConfigError::ReconnectBounds {
                min: self.reconnect_min,
                max: self.reconnect_max,
            });
        }
        let loss = self.liveness.loss_degraded_fraction;
        if !(loss > 0.0 && loss <= 1.0) {
            return Err(ConfigError::LossFraction(loss));
        }
        Ok(())
    }
}

/// Configuration of a driver or engineer client.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// The relay (from the user's settings; nothing is hard-coded).
    pub endpoint: RelayEndpoint,
    /// Application version (HELLO, SESSION_CREATE).
    pub app_version: String,
    /// Stable per-installation id (HELLO; diagnostics only).
    pub client_id: String,
    /// Telemetry schema versions this client produces / consumes.
    pub schema_versions: Vec<u16>,
    /// Epoch of the monotonic clock in frame headers and PINGs, and therefore of the clock offset θ
    /// the clients publish. It must be the epoch of the capture timestamps the offset is applied to
    /// (the LMU reader's `MonoClock::epoch()`); there is deliberately no default. Clients also offer
    /// `relay_time_of(Instant)`, which needs no shared epoch at all.
    pub mono_epoch: std::time::Instant,
    pub timing: TimingConfig,
    /// Capacity of the reliable outbound queue.
    pub reliable_queue_capacity: usize,
    /// Capacity of the application event channel.
    pub event_capacity: usize,
    /// Engineer: reliable stream messages buffered before the identity was confirmed.
    pub pending_reliable_capacity: usize,
}

impl ClientConfig {
    /// Configuration with defaults for everything but the endpoint, the identity and the monotonic
    /// epoch (see [`ClientConfig::mono_epoch`]: pass the LMU reader's epoch).
    pub fn new(
        endpoint: RelayEndpoint,
        app_version: impl Into<String>,
        client_id: impl Into<String>,
        mono_epoch: std::time::Instant,
    ) -> Self {
        Self {
            endpoint,
            app_version: app_version.into(),
            client_id: client_id.into(),
            schema_versions: vec![telemetry_model::TELEMETRY_SCHEMA_VERSION],
            mono_epoch,
            timing: TimingConfig::default(),
            reliable_queue_capacity: DEFAULT_RELIABLE_CAPACITY,
            event_capacity: DEFAULT_EVENT_CAPACITY,
            pending_reliable_capacity: DEFAULT_PENDING_RELIABLE_CAPACITY,
        }
    }

    /// Checks the configuration (currently the timing parameters, [`TimingConfig::validate`]).
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.timing.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_durations_and_inverted_bounds_are_refused() {
        assert_eq!(TimingConfig::default().validate(), Ok(()));
        let mut t = TimingConfig {
            liveness_check_interval: Duration::ZERO,
            ..TimingConfig::default()
        };
        assert_eq!(
            t.validate(),
            Err(ConfigError::ZeroDuration {
                field: "liveness_check_interval"
            })
        );
        t = TimingConfig {
            reconnect_min: Duration::from_secs(2),
            reconnect_max: Duration::from_secs(1),
            ..TimingConfig::default()
        };
        assert!(matches!(
            t.validate(),
            Err(ConfigError::ReconnectBounds { .. })
        ));
        t = TimingConfig::default();
        t.liveness.loss_degraded_fraction = f64::NAN;
        assert!(matches!(t.validate(), Err(ConfigError::LossFraction(_))));
        t = TimingConfig::default();
        t.liveness.timeout_after = Duration::ZERO;
        assert_eq!(
            t.validate(),
            Err(ConfigError::ZeroDuration {
                field: "liveness.timeout_after"
            })
        );
    }

    #[test]
    fn defaults_follow_protocol_timing() {
        let t = TimingConfig::default();
        assert_eq!(t.heartbeat_interval, Duration::from_millis(1_000));
        assert_eq!(t.liveness.degraded_after, Duration::from_millis(2_500));
        assert_eq!(t.liveness.stale_after, Duration::from_millis(3_000));
        assert_eq!(t.liveness.timeout_after, Duration::from_millis(5_000));
        assert_eq!(t.reconnect_min, Duration::from_millis(250));
        assert_eq!(t.reconnect_max, Duration::from_millis(5_000));
        assert_eq!(t.engineer_ticket_ttl, Duration::from_secs(600));
    }
}
