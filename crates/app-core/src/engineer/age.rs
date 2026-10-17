//! Stream age and `net.*` channels on the engineer side.
//!
//! Computes the stream age locally from `relay_time_of` (or local monotonic fallback)
//! and writes the `net.*` channels. Drives the engineer-side `AlertEngine` at 4 Hz even
//! when no frame arrives, so a lost driver shows STALE and then NO DATA instead of frozen values.

use race_engines::alert::{AlertClock, AlertEngine, AlertNotice};
use telemetry_model::ch;
use telemetry_model::codec::RELAY_NS_UNKNOWN;
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::value::Value;

/// Default stream age in seconds before a stream is considered stale (2.0s).
pub const DEFAULT_STALE_THRESHOLD_S: f64 = 2.0;

/// Default stream age in seconds before a stream is considered lost / no data (10.0s).
pub const DEFAULT_LOST_THRESHOLD_S: f64 = 10.0;

/// Quality / health classification of the inbound stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamHealth {
    #[default]
    NoData,
    Healthy,
    Stale,
}

/// Diagnostic report produced by an age evaluation tick.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamAgeReport {
    /// Calculated stream age in seconds (`None` if no frame received yet).
    pub stream_age_s: Option<f64>,
    /// Stream health classification.
    pub health: StreamHealth,
    /// Estimated clock uncertainty in seconds (`None` if clock sync not yet acquired).
    pub uncertainty_s: Option<f64>,
    /// Alert notices raised or cleared during this evaluation tick.
    pub alerts: Vec<AlertNotice>,
}

/// Tracks inbound frame timing and evaluates stream freshness.
#[derive(Debug)]
pub struct StreamAgeTracker {
    last_capture_mono_ns: Option<u64>,
    last_capture_relay_ns_est: Option<i64>,
    last_received_mono_ns: Option<u64>,
    driver_clock_offset_ns: Option<i64>,
    driver_clock_uncertainty_ns: Option<u64>,
    stale_threshold_s: f64,
    lost_threshold_s: f64,
}

impl Default for StreamAgeTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamAgeTracker {
    /// Creates a new tracker with default stale (2.0s) and lost (10.0s) thresholds.
    pub fn new() -> Self {
        Self {
            last_capture_mono_ns: None,
            last_capture_relay_ns_est: None,
            last_received_mono_ns: None,
            driver_clock_offset_ns: None,
            driver_clock_uncertainty_ns: None,
            stale_threshold_s: DEFAULT_STALE_THRESHOLD_S,
            lost_threshold_s: DEFAULT_LOST_THRESHOLD_S,
        }
    }

    /// Sets custom stale and lost thresholds in seconds.
    pub fn with_thresholds(mut self, stale_s: f64, lost_s: f64) -> Self {
        self.stale_threshold_s = stale_s;
        self.lost_threshold_s = lost_s;
        self
    }

    /// Records an inbound telemetry frame's capture and receipt timestamps.
    pub fn record_frame(
        &mut self,
        capture_mono_ns: u64,
        capture_relay_ns_est: i64,
        received_mono_ns: u64,
    ) {
        self.last_capture_mono_ns = Some(capture_mono_ns);
        self.last_capture_relay_ns_est = Some(capture_relay_ns_est);
        self.last_received_mono_ns = Some(received_mono_ns);
    }

    /// Records driver clock offset and uncertainty from a `DriverStatus` message.
    pub fn record_driver_clock(&mut self, offset_ns: i64, uncertainty_ns: u64) {
        self.driver_clock_offset_ns = Some(offset_ns);
        self.driver_clock_uncertainty_ns = Some(uncertainty_ns);
    }

    /// Resets stream state after a disconnect or sequence reset.
    pub fn reset_stream(&mut self) {
        self.last_capture_mono_ns = None;
        self.last_capture_relay_ns_est = None;
        self.last_received_mono_ns = None;
    }

    /// Evaluates stream age and checks against the alert engine if present.
    ///
    /// Called at 4 Hz.
    pub fn tick(
        &mut self,
        now_mono_ns: u64,
        engineer_relay_ns: Option<i64>,
        engineer_clock_uncertainty_ns: Option<u64>,
        alert_engine: Option<&mut AlertEngine>,
    ) -> StreamAgeReport {
        let stream_age_s = match (self.last_capture_relay_ns_est, engineer_relay_ns) {
            (Some(capture_relay), Some(engineer_relay)) if capture_relay != RELAY_NS_UNKNOWN => {
                let age_ns = (engineer_relay - capture_relay).max(0);
                Some(age_ns as f64 / 1_000_000_000.0)
            }
            _ => self
                .last_received_mono_ns
                .map(|recv| now_mono_ns.saturating_sub(recv) as f64 / 1_000_000_000.0),
        };

        let uncertainty_s = match (
            self.driver_clock_uncertainty_ns,
            engineer_clock_uncertainty_ns,
        ) {
            (Some(d_unc), Some(e_unc)) => {
                let total_ns = d_unc.saturating_add(e_unc);
                Some(total_ns as f64 / 1_000_000_000.0)
            }
            _ => None,
        };

        let health = match stream_age_s {
            None => StreamHealth::NoData,
            Some(age) if age >= self.lost_threshold_s => StreamHealth::NoData,
            Some(age) if age >= self.stale_threshold_s => StreamHealth::Stale,
            Some(_) => StreamHealth::Healthy,
        };

        let alerts = match (stream_age_s, alert_engine) {
            (Some(age), Some(engine)) => {
                let mut eval_frame = TelemetryFrame::new(1, now_mono_ns, None);
                let _ = eval_frame.set_valid(ch::NET_STREAM_AGE, Value::F32(age as f32));
                engine.evaluate_frame(
                    AlertClock::new(now_mono_ns as f64 / 1_000_000_000.0, None),
                    None,
                    &eval_frame,
                )
            }
            _ => Vec::new(),
        };

        StreamAgeReport {
            stream_age_s,
            health,
            uncertainty_s,
            alerts,
        }
    }
}
