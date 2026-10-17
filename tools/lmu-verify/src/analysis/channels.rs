//! Per-channel table: quality shares, distinct-value update rate, value distribution, rejections by reason,
//! values outside the registry range and "frozen while moving" detection, for every registry channel of the
//! published frames (registry order).
//!
//! Rates are measured on LMU's own clock (frame `source_et_s`): a value change counts once, and the time
//! base is the sum of ET steps between consecutive frames in which the channel was valid (steps longer than
//! [`MAX_STEP_S`] — pauses, gaps — are not counted).

use std::collections::BTreeMap;

use serde::Serialize;
use telemetry_model::channel::{ChannelDef, FrequencyClass, Interpolation};
use telemetry_model::validate::Rejection;
use telemetry_model::value::{Quality, Value};
use telemetry_model::{ch, registry};

use super::Step;
use crate::stats::{Summary, SummaryStats};

/// The car counts as moving above this speed (m/s).
pub const MOVING_SPEED_MPS: f64 = 5.0;
/// Longest ET step that counts towards a channel's valid time (s).
pub const MAX_STEP_S: f64 = 0.5;
/// A continuous physics channel is reported frozen when it never changed over at least this much moving
/// time (s).
pub const FROZEN_MIN_MOVING_S: f64 = 10.0;

/// Sample counts per quality.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct QualityCounts {
    /// `Valid`.
    pub valid: u64,
    /// `Invalid`.
    pub invalid: u64,
    /// `Unavailable`.
    pub unavailable: u64,
    /// `NotSupported`.
    pub not_supported: u64,
    /// `Stale`.
    pub stale: u64,
}

impl QualityCounts {
    fn add(&mut self, q: Quality) {
        match q {
            Quality::Valid => self.valid += 1,
            Quality::Invalid => self.invalid += 1,
            Quality::Unavailable => self.unavailable += 1,
            Quality::NotSupported => self.not_supported += 1,
            Quality::Stale => self.stale += 1,
        }
    }

    /// All samples.
    pub fn total(&self) -> u64 {
        self.valid + self.invalid + self.unavailable + self.not_supported + self.stale
    }
}

/// Percent of `part` in `total` (0 when empty).
pub fn percent(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * part as f64 / total as f64
    }
}

/// One row of the per-channel table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChannelRow {
    /// Channel id (`0x0001`).
    pub id: String,
    /// Canonical name.
    pub name: &'static str,
    /// Internal unit symbol.
    pub unit: &'static str,
    /// Source block / calculation.
    pub source: String,
    /// Measured / derived / estimated.
    pub provenance: String,
    /// Nominal update class.
    pub frequency: String,
    /// Raw LMU field or formula (registry `origin`).
    pub origin: &'static str,
    /// Samples per quality (one per published frame).
    pub counts: QualityCounts,
    /// `Valid` share (%).
    pub valid_pct: f64,
    /// `Invalid` share (%).
    pub invalid_pct: f64,
    /// `Unavailable` share (%).
    pub unavailable_pct: f64,
    /// `NotSupported` share (%).
    pub not_supported_pct: f64,
    /// `Stale` share (%).
    pub stale_pct: f64,
    /// Value changes between consecutive valid samples.
    pub changes: u64,
    /// LMU time covered by consecutive valid samples (s).
    pub valid_time_s: f64,
    /// Distinct-value update rate: `changes / valid_time_s` (Hz).
    pub change_rate_hz: Option<f64>,
    /// Distribution of the valid numeric values.
    pub stats: Option<SummaryStats>,
    /// Validation rejections by reason (`NON_FINITE`, `OUT_OF_RANGE`, `RATE_EXCEEDED`).
    pub rejections: BTreeMap<String, u64>,
    /// Registry plausible range.
    pub registry_range: Option<(f64, f64)>,
    /// Values rejected as outside the registry range.
    pub out_of_range: u64,
    /// Smallest and largest rejected out-of-range value.
    pub out_of_range_extremes: Option<(f64, f64)>,
    /// Valid samples while the car moved.
    pub moving_samples: u64,
    /// Longest run of one unchanged value while moving (s; continuous physics channels only).
    pub longest_constant_moving_s: Option<f64>,
    /// A continuous physics channel that never changed over at least [`FROZEN_MIN_MOVING_S`] of moving time.
    pub frozen_while_moving: bool,
}

#[derive(Debug, Clone, Default)]
struct ChannelAcc {
    counts: QualityCounts,
    summary: Summary,
    changes: u64,
    valid_time_s: f64,
    last: Option<Value>,
    last_et: Option<f64>,
    rejections: BTreeMap<&'static str, u64>,
    out_of_range: u64,
    oor_min: Option<f64>,
    oor_max: Option<f64>,
    moving_samples: u64,
    moving_changes: u64,
    moving_time_s: f64,
    run_start_et: Option<f64>,
    longest_run_s: Option<f64>,
}

/// Whether "constant while moving" is suspicious for a channel.
fn tracks_freezing(def: &ChannelDef) -> bool {
    def.frequency == FrequencyClass::Physics
        && def.interpolation == Interpolation::Continuous
        && def.is_float()
}

fn rejection_label(r: &Rejection) -> &'static str {
    match r {
        Rejection::NonFinite => "NON_FINITE",
        Rejection::OutOfRange { .. } => "OUT_OF_RANGE",
        Rejection::RateExceeded { .. } => "RATE_EXCEEDED",
    }
}

/// Per-channel accumulator over the published frames.
#[derive(Debug, Clone)]
pub struct ChannelsAcc {
    rows: Vec<ChannelAcc>,
    frames: u64,
}

impl Default for ChannelsAcc {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelsAcc {
    /// One accumulator per registry channel.
    pub fn new() -> Self {
        Self {
            rows: vec![ChannelAcc::default(); registry::len()],
            frames: 0,
        }
    }

    /// Feeds one step (only steps with a new frame count).
    pub fn observe(&mut self, step: &Step<'_>) {
        let Some(frame) = step.frame else {
            return;
        };
        self.frames += 1;
        let et = frame.source_et_s;
        let moving = frame.f64(ch::SPEED).is_some_and(|v| v > MOVING_SPEED_MPS);
        for (index, rejection) in step
            .out
            .rejections
            .iter()
            .filter_map(|(id, r)| registry::index_of(*id).map(|i| (i, r)))
        {
            if let Some(acc) = self.rows.get_mut(index) {
                *acc.rejections
                    .entry(rejection_label(rejection))
                    .or_insert(0) += 1;
                if let Rejection::OutOfRange { value, .. } = rejection {
                    acc.out_of_range += 1;
                    acc.oor_min = Some(acc.oor_min.map_or(*value, |m| m.min(*value)));
                    acc.oor_max = Some(acc.oor_max.map_or(*value, |m| m.max(*value)));
                }
            }
        }
        for ((def, sample), acc) in frame.iter().zip(self.rows.iter_mut()) {
            acc.counts.add(sample.quality);
            let valid = sample.is_valid();
            if step.discontinuity || !valid {
                // History never spans a discontinuity or an invalid sample.
                acc.last_et = None;
                acc.run_start_et = None;
                if !valid {
                    acc.last = None;
                    continue;
                }
                if step.discontinuity {
                    acc.last = None;
                }
            }
            if let Some(x) = sample.value.as_f64() {
                acc.summary.push(x);
            }
            let changed = acc.last.as_ref().is_some_and(|prev| *prev != sample.value);
            let step_s = match (acc.last_et, et) {
                (Some(prev), Some(now)) if now > prev && now - prev <= MAX_STEP_S => {
                    Some(now - prev)
                }
                _ => None,
            };
            if let Some(dt) = step_s {
                acc.valid_time_s += dt;
                if changed {
                    acc.changes += 1;
                }
            }
            if moving {
                acc.moving_samples += 1;
                if let Some(dt) = step_s {
                    acc.moving_time_s += dt;
                    if changed {
                        acc.moving_changes += 1;
                    }
                }
            }
            if tracks_freezing(def) {
                match (moving, et) {
                    (true, Some(now)) => {
                        if changed || acc.run_start_et.is_none() || step_s.is_none() {
                            acc.run_start_et = Some(now);
                        }
                        if let Some(start) = acc.run_start_et {
                            let run = now - start;
                            acc.longest_run_s = Some(acc.longest_run_s.map_or(run, |m| m.max(run)));
                        }
                    }
                    _ => acc.run_start_et = None,
                }
            }
            if acc.last.as_ref() != Some(&sample.value) {
                acc.last = Some(sample.value.clone());
            }
            acc.last_et = et;
        }
    }

    /// Published frames seen.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Builds the table (registry order; channels never seen are omitted).
    pub fn finish(self) -> Vec<ChannelRow> {
        registry::CHANNELS
            .iter()
            .zip(self.rows)
            .filter(|(_, acc)| acc.counts.total() > 0)
            .map(|(def, acc)| {
                let total = acc.counts.total();
                let freezing = tracks_freezing(def);
                ChannelRow {
                    id: format!("{:#06x}", def.id.0),
                    name: def.name,
                    unit: def.unit.symbol(),
                    source: format!("{:?}", def.source),
                    provenance: format!("{:?}", def.provenance),
                    frequency: format!("{:?}", def.frequency),
                    origin: def.origin,
                    counts: acc.counts,
                    valid_pct: percent(acc.counts.valid, total),
                    invalid_pct: percent(acc.counts.invalid, total),
                    unavailable_pct: percent(acc.counts.unavailable, total),
                    not_supported_pct: percent(acc.counts.not_supported, total),
                    stale_pct: percent(acc.counts.stale, total),
                    changes: acc.changes,
                    valid_time_s: acc.valid_time_s,
                    change_rate_hz: (acc.valid_time_s > 0.0)
                        .then(|| acc.changes as f64 / acc.valid_time_s),
                    stats: acc.summary.stats(),
                    rejections: acc
                        .rejections
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v))
                        .collect(),
                    registry_range: def.valid_range,
                    out_of_range: acc.out_of_range,
                    out_of_range_extremes: acc.oor_min.zip(acc.oor_max),
                    moving_samples: acc.moving_samples,
                    longest_constant_moving_s: if freezing { acc.longest_run_s } else { None },
                    frozen_while_moving: freezing
                        && acc.moving_changes == 0
                        && acc.moving_time_s >= FROZEN_MIN_MOVING_S,
                }
            })
            .collect()
    }
}
