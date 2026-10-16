//! Driver-aid duty cycle (LMU_CHANNEL_MAP §18 `aids.tc_duty_frac` / `aids.abs_duty_frac`).
//!
//! LMU exposes traction control and ABS intervention only as booleans (`mTCActive`, `mABSActive`); there is
//! no intervention amount. The duty cycle stands in for it: the share of telemetry samples of a lap on which
//! the aid was intervening.
//!
//! # Rules
//! - A duty is published for the **last completed lap** only (event value), never for the lap in progress.
//! - The lap must have been observed from its start (the engine saw the previous lap boundary): the first lap
//!   after [`AidsEngine::new`] or after an elapsed-time regression is partial and has no duty.
//! - Every sample of the lap must carry the flag (a car without the aid publishes it `NotSupported`, a dropped
//!   frame `Unavailable`): one missing sample makes that aid's duty unknown (`None`), never a guessed 0.

use serde::Serialize;
use telemetry_model::{TelemetryFrame, ch};
use tracing::debug;

use crate::common::{Metric, finite, frame_bool, frame_f64};

/// Tracing component name.
const COMPONENT: &str = "race_engines::aids";

/// Formula ids of the aid outputs (equal to the registry origins).
pub mod formula {
    /// Share of a lap's telemetry samples with `mTCActive` set.
    pub const TC_DUTY: &str = "tc_active_duty_cycle";
    /// Share of a lap's telemetry samples with `mABSActive` set.
    pub const ABS_DUTY: &str = "abs_active_duty_cycle";
}

/// One telemetry sample of the aid flags.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AidsSample {
    /// LMU elapsed time (s).
    pub et_s: f64,
    /// Traction control intervening (`aids.tc_active`), when known.
    pub tc_active: Option<bool>,
    /// ABS intervening (`aids.abs_active`), when known.
    pub abs_active: Option<bool>,
}

impl AidsSample {
    /// From a canonical frame; `None` when the sample time is unknown.
    pub fn from_frame(frame: &TelemetryFrame) -> Option<Self> {
        let et_s = finite(frame.source_et_s).or_else(|| frame_f64(frame, ch::ELAPSED_TIME))?;
        Some(Self {
            et_s,
            tc_active: frame_bool(frame, ch::TC_ACTIVE),
            abs_active: frame_bool(frame, ch::ABS_ACTIVE),
        })
    }
}

/// Sample counts of one aid over the lap in progress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DutyCount {
    /// Samples with the flag set.
    active: u64,
    /// Samples with the flag known (set or clear).
    known: u64,
    /// Samples without the flag.
    missing: u64,
}

impl DutyCount {
    fn add(&mut self, flag: Option<bool>) {
        match flag {
            Some(on) => {
                self.known = self.known.saturating_add(1);
                if on {
                    self.active = self.active.saturating_add(1);
                }
            }
            None => self.missing = self.missing.saturating_add(1),
        }
    }

    /// Active share of the lap; `None` when any sample lacked the flag or no sample was seen.
    fn duty(&self) -> Option<f64> {
        if self.missing > 0 || self.known == 0 {
            return None;
        }
        finite(Some(self.active as f64 / self.known as f64))
    }
}

/// Aid outputs: the duty cycles of the last completed lap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AidsSnapshot {
    /// Traction-control duty of the last completed lap (0..1).
    pub tc_duty: Metric<f64>,
    /// ABS duty of the last completed lap (0..1).
    pub abs_duty: Metric<f64>,
}

impl Default for AidsSnapshot {
    fn default() -> Self {
        Self {
            tc_duty: Metric::derived(formula::TC_DUTY, None),
            abs_duty: Metric::derived(formula::ABS_DUTY, None),
        }
    }
}

/// Deterministic duty-cycle engine. Feed every frame with [`AidsEngine::update`] and call
/// [`AidsEngine::lap_completed`] right after the sample on which the timing engine completed a lap.
#[derive(Debug, Clone, Default)]
pub struct AidsEngine {
    tc: DutyCount,
    abs: DutyCount,
    /// The lap in progress started at an observed boundary.
    lap_observed_from_start: bool,
    last_et: Option<f64>,
    snapshot: AidsSnapshot,
}

impl AidsEngine {
    /// A new engine (the first lap is partial).
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes one sample. A non-finite time is ignored; an elapsed-time regression drops the lap in
    /// progress (it becomes partial) and the last lap's duty (it belongs to the previous time base).
    pub fn update(&mut self, sample: &AidsSample) {
        let et = sample.et_s;
        if !et.is_finite() {
            return;
        }
        if self.last_et.is_some_and(|t| et < t) {
            debug!(
                component = COMPONENT,
                et, "elapsed time regression: lap in progress partial"
            );
            self.tc = DutyCount::default();
            self.abs = DutyCount::default();
            self.lap_observed_from_start = false;
            self.snapshot = AidsSnapshot::default();
        }
        self.last_et = Some(et);
        self.tc.add(sample.tc_active);
        self.abs.add(sample.abs_active);
    }

    /// Closes the lap in progress: its duty becomes the published value when it was observed from its
    /// start, and the next lap starts at this observed boundary.
    pub fn lap_completed(&mut self) {
        let observed = self.lap_observed_from_start;
        self.snapshot = AidsSnapshot {
            tc_duty: Metric::derived(formula::TC_DUTY, self.tc.duty().filter(|_| observed)),
            abs_duty: Metric::derived(formula::ABS_DUTY, self.abs.duty().filter(|_| observed)),
        };
        self.tc = DutyCount::default();
        self.abs = DutyCount::default();
        self.lap_observed_from_start = true;
    }

    /// Current outputs.
    pub fn snapshot(&self) -> AidsSnapshot {
        self.snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(et_s: f64, tc: Option<bool>, abs: Option<bool>) -> AidsSample {
        AidsSample {
            et_s,
            tc_active: tc,
            abs_active: abs,
        }
    }

    #[test]
    fn duty_is_the_active_share_of_a_fully_observed_lap() {
        let mut e = AidsEngine::new();
        e.update(&sample(0.0, Some(true), Some(false)));
        e.lap_completed();
        // The first lap was joined mid-way: no duty.
        assert_eq!(e.snapshot().tc_duty.value, None);
        for i in 0..10 {
            e.update(&sample(1.0 + f64::from(i), Some(i < 3), Some(i == 9)));
        }
        assert_eq!(e.snapshot().tc_duty.value, None, "lap in progress");
        e.lap_completed();
        let s = e.snapshot();
        assert_eq!(s.tc_duty.value, Some(0.3));
        assert_eq!(s.abs_duty.value, Some(0.1));
        assert_eq!(s.tc_duty.formula, formula::TC_DUTY);
    }

    #[test]
    fn a_missing_flag_makes_the_duty_unknown_never_zero() {
        let mut e = AidsEngine::new();
        e.lap_completed();
        e.update(&sample(1.0, Some(false), None));
        e.update(&sample(2.0, None, None));
        e.lap_completed();
        assert_eq!(e.snapshot().tc_duty.value, None);
        assert_eq!(e.snapshot().abs_duty.value, None, "aid not supported");
    }

    #[test]
    fn time_regression_makes_the_lap_partial() {
        let mut e = AidsEngine::new();
        e.lap_completed();
        e.update(&sample(10.0, Some(true), Some(true)));
        e.lap_completed();
        assert_eq!(e.snapshot().tc_duty.value, Some(1.0));
        e.update(&sample(5.0, Some(false), Some(false)));
        assert_eq!(e.snapshot().tc_duty.value, None, "previous time base");
        e.lap_completed();
        assert_eq!(e.snapshot().tc_duty.value, None, "partial lap");
    }
}
