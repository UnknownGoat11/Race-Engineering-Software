//! Relay metrics: lock-free counters, a fixed-bucket latency histogram and Prometheus text
//! exposition (format 0.0.4) for the admin `/metrics` endpoint.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

/// Upper bounds (µs) of the forward-latency histogram buckets (relay rx → socket hand-off).
pub const LATENCY_BUCKETS_US: [u64; 12] = [
    50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000,
];
/// Microseconds per second (Prometheus histograms are in seconds).
const MICROS_PER_SEC: f64 = 1_000_000.0;

/// A monotonically increasing counter.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    /// Adds one.
    pub fn inc(&self) {
        self.add(1);
    }

    /// Adds `n`.
    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }

    /// Current value.
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A gauge that goes up and down.
#[derive(Debug, Default)]
pub struct Gauge(AtomicU64);

impl Gauge {
    /// Adds one.
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Subtracts one (saturating at zero, so an unmatched `dec` never underflows).
    pub fn dec(&self) {
        // Explicit CAS loop (`fetch_update` is deprecated on newer toolchains).
        let mut current = self.0.load(Ordering::Relaxed);
        while current > 0 {
            match self.0.compare_exchange_weak(
                current,
                current - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }

    /// Current value.
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Fixed-bucket histogram of durations in microseconds.
#[derive(Debug, Default)]
pub struct LatencyHistogram {
    buckets: [AtomicU64; LATENCY_BUCKETS_US.len()],
    count: AtomicU64,
    sum_us: AtomicU64,
}

impl LatencyHistogram {
    /// Records one observation.
    pub fn observe_us(&self, us: u64) {
        if let Some(i) = LATENCY_BUCKETS_US.iter().position(|&le| us <= le) {
            self.buckets[i].fetch_add(1, Ordering::Relaxed);
        }
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(us, Ordering::Relaxed);
    }

    /// Number of observations.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Smallest bucket bound below which at least `q` (0..=1) of the observations lie, or `None`
    /// when empty or the quantile lies beyond the last bound.
    pub fn quantile_upper_bound_us(&self, q: f64) -> Option<u64> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        // Precision: observation counts stay far below 2^52.
        let target = (q.clamp(0.0, 1.0) * total as f64).ceil() as u64;
        let mut cumulative = 0u64;
        for (i, bucket) in self.buckets.iter().enumerate() {
            cumulative += bucket.load(Ordering::Relaxed);
            if cumulative >= target.max(1) {
                return Some(LATENCY_BUCKETS_US[i]);
            }
        }
        None
    }
}

/// Every relay metric.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Open client connections (after TCP accept).
    pub active_connections: Gauge,
    /// Telemetry-class frames received from drivers and engineers.
    pub telemetry_packets_received: Counter,
    /// Telemetry-class frames written to receivers (replays included).
    pub telemetry_packets_forwarded: Counter,
    /// Bytes read from client sockets.
    pub bytes_received: Counter,
    /// Bytes written to client sockets.
    pub bytes_forwarded: Counter,
    /// Failed AUTH / ACCOUNT_REGISTER attempts.
    pub auth_failures: Counter,
    /// Framing errors (fatal) and header-rule violations (skipped frames).
    pub packet_validation_failures: Counter,
    /// Accepted SESSION_RESUME (drivers and engineers).
    pub reconnects: Counter,
    /// Live frames replaced in a latest-wins slot before they were written.
    pub live_frames_dropped: Counter,
    /// Requests refused by a rate limit (auth, joins, frame rates, snapshot requests).
    pub rate_limited: Counter,
    /// SESSION_JOIN attempts refused (invalid, expired, full, closed, not authorized).
    pub join_failures: Counter,
    /// Connections refused at accept (connection limits) or before HELLO completed.
    pub connections_rejected: Counter,
    /// Receivers disconnected because they could not keep up with reliable frames.
    pub slow_consumer_disconnects: Counter,
    /// Connections closed because their login was revoked (or its account deleted / disabled).
    pub revoked_disconnects: Counter,
    /// Relay receive → socket hand-off latency of forwarded telemetry frames.
    pub relay_forward_latency: LatencyHistogram,
}

/// Point-in-time gauges computed by the session registry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RegistryGauges {
    pub active_sessions: u64,
    pub sessions_in_grace: u64,
    pub engineers_connected: u64,
}

impl Metrics {
    /// Prometheus text exposition of every metric.
    pub fn render(&self, gauges: RegistryGauges) -> String {
        let mut out = String::with_capacity(4096);
        gauge(
            &mut out,
            "lmu_relay_active_sessions",
            "Sessions held by the relay (driver attached or in grace).",
            gauges.active_sessions,
        );
        gauge(
            &mut out,
            "lmu_relay_sessions_in_grace",
            "Sessions whose driver is disconnected and inside the grace period.",
            gauges.sessions_in_grace,
        );
        gauge(
            &mut out,
            "lmu_relay_engineers_connected",
            "Engineer connections attached to a session.",
            gauges.engineers_connected,
        );
        gauge(
            &mut out,
            "lmu_relay_active_connections",
            "Open client connections.",
            self.active_connections.get(),
        );
        let counters: [(&str, &str, &Counter); 13] = [
            (
                "lmu_relay_telemetry_packets_received_total",
                "Telemetry-class frames received.",
                &self.telemetry_packets_received,
            ),
            (
                "lmu_relay_telemetry_packets_forwarded_total",
                "Telemetry-class frames written to receivers.",
                &self.telemetry_packets_forwarded,
            ),
            (
                "lmu_relay_bytes_received_total",
                "Bytes read from client sockets.",
                &self.bytes_received,
            ),
            (
                "lmu_relay_bytes_forwarded_total",
                "Bytes written to client sockets.",
                &self.bytes_forwarded,
            ),
            (
                "lmu_relay_auth_failures_total",
                "Failed authentication or registration attempts.",
                &self.auth_failures,
            ),
            (
                "lmu_relay_packet_validation_failures_total",
                "Framing errors and header-rule violations.",
                &self.packet_validation_failures,
            ),
            (
                "lmu_relay_reconnects_total",
                "Accepted session resumes (drivers and engineers).",
                &self.reconnects,
            ),
            (
                "lmu_relay_live_frames_dropped_total",
                "Live frames superseded in a latest-wins slot before being written.",
                &self.live_frames_dropped,
            ),
            (
                "lmu_relay_rate_limited_total",
                "Requests refused by a rate limit.",
                &self.rate_limited,
            ),
            (
                "lmu_relay_join_failures_total",
                "Refused SESSION_JOIN attempts.",
                &self.join_failures,
            ),
            (
                "lmu_relay_connections_rejected_total",
                "Connections refused by connection limits or failed handshakes.",
                &self.connections_rejected,
            ),
            (
                "lmu_relay_slow_consumer_disconnects_total",
                "Receivers disconnected for not keeping up with reliable frames.",
                &self.slow_consumer_disconnects,
            ),
            (
                "lmu_relay_revoked_disconnects_total",
                "Connections closed because their login was revoked or its account disabled.",
                &self.revoked_disconnects,
            ),
        ];
        for (name, help, counter) in counters {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} counter");
            let _ = writeln!(out, "{name} {}", counter.get());
        }
        self.render_latency(&mut out);
        out
    }

    fn render_latency(&self, out: &mut String) {
        let name = "lmu_relay_relay_forward_latency_seconds";
        let h = &self.relay_forward_latency;
        let _ = writeln!(
            out,
            "# HELP {name} Relay receive to socket hand-off latency of forwarded telemetry frames."
        );
        let _ = writeln!(out, "# TYPE {name} histogram");
        let mut cumulative = 0u64;
        for (i, le) in LATENCY_BUCKETS_US.iter().enumerate() {
            cumulative += h.buckets[i].load(Ordering::Relaxed);
            // Precision: bucket bounds are small integers, exact in f64.
            let le_s = *le as f64 / MICROS_PER_SEC;
            let _ = writeln!(out, "{name}_bucket{{le=\"{le_s}\"}} {cumulative}");
        }
        let count = h.count();
        let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {count}");
        let sum_s = h.sum_us.load(Ordering::Relaxed) as f64 / MICROS_PER_SEC;
        let _ = writeln!(out, "{name}_sum {sum_s}");
        let _ = writeln!(out, "{name}_count {count}");
    }
}

fn gauge(out: &mut String, name: &str, help: &str, value: u64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} gauge");
    let _ = writeln!(out, "{name} {value}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposition_contains_required_metrics() {
        let m = Metrics::default();
        m.telemetry_packets_received.add(3);
        m.active_connections.inc();
        m.active_connections.inc();
        m.active_connections.dec();
        m.relay_forward_latency.observe_us(80);
        m.relay_forward_latency.observe_us(400_000);
        let text = m.render(RegistryGauges {
            active_sessions: 2,
            ..RegistryGauges::default()
        });
        for needle in [
            "lmu_relay_active_sessions 2",
            "lmu_relay_active_connections 1",
            "lmu_relay_telemetry_packets_received_total 3",
            "lmu_relay_telemetry_packets_forwarded_total 0",
            "lmu_relay_bytes_received_total 0",
            "lmu_relay_bytes_forwarded_total 0",
            "lmu_relay_auth_failures_total 0",
            "lmu_relay_packet_validation_failures_total 0",
            "lmu_relay_reconnects_total 0",
            "lmu_relay_relay_forward_latency_seconds_bucket{le=\"0.0001\"} 1",
            "lmu_relay_relay_forward_latency_seconds_bucket{le=\"+Inf\"} 2",
            "lmu_relay_relay_forward_latency_seconds_count 2",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in\n{text}");
        }
    }

    #[test]
    fn gauge_never_underflows() {
        let g = Gauge::default();
        g.dec();
        assert_eq!(g.get(), 0);
    }

    #[test]
    fn quantiles_use_bucket_bounds() {
        let h = LatencyHistogram::default();
        assert_eq!(h.quantile_upper_bound_us(0.5), None);
        for _ in 0..9 {
            h.observe_us(40);
        }
        h.observe_us(2_000);
        assert_eq!(h.quantile_upper_bound_us(0.5), Some(50));
        assert_eq!(h.quantile_upper_bound_us(0.99), Some(2_500));
    }
}
