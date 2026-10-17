//! Tyre checks: the L/C/R → inner/outer side mapping (A-LMU-016) and the wear direction (A-LMU-040).
//!
//! **Side mapping evidence.** Under A-LMU-016 the car-frame left edge is the *outer* edge of the left
//! wheels and the *inner* edge of the right wheels. When a tyre is loaded on the outside of a corner its
//! contact patch moves towards its outer shoulder, so `outer − inner` should rise relative to straight
//! running. With a mirrored mapping that difference would fall on both axles. Tyre temperatures also depend
//! on camber, pressure and setup, so this is evidence, not proof: the check is INCONCLUSIVE unless all four
//! wheels agree clearly.

use telemetry_model::{ChannelId, ch};

use super::{
    CheckResult, EXACT_PASSTHROUGH_TOLERANCE, Outcome, PublishedVsRaw, TYRE_TEMP_LEFT,
    TYRE_TEMP_RIGHT, Verdict, WHEELS, describe_published, fmt_opt, gate_rows, pct,
};
use crate::analysis::Step;
use crate::stats::{Agreement, Mean};

/// Published inner / outer edge and inner − outer spread channels per wheel.
const TYRE_TEMP_INNER: [ChannelId; 4] = [
    ch::TYRE_TEMP_INNER_FL,
    ch::TYRE_TEMP_INNER_FR,
    ch::TYRE_TEMP_INNER_RL,
    ch::TYRE_TEMP_INNER_RR,
];
const TYRE_TEMP_OUTER: [ChannelId; 4] = [
    ch::TYRE_TEMP_OUTER_FL,
    ch::TYRE_TEMP_OUTER_FR,
    ch::TYRE_TEMP_OUTER_RL,
    ch::TYRE_TEMP_OUTER_RR,
];
const TYRE_TEMP_SPREAD: [ChannelId; 4] = [
    ch::TYRE_TEMP_SPREAD_FL,
    ch::TYRE_TEMP_SPREAD_FR,
    ch::TYRE_TEMP_SPREAD_RL,
    ch::TYRE_TEMP_SPREAD_RR,
];
/// Published inner / outer / spread must equal the A-LMU-016 mapping of the published L/R within this
/// (°C; both sides are stored as f32).
const EDGE_MAPPING_TOLERANCE_C: f64 = 0.01;

/// Tyre-edge samples need this speed (m/s).
const EDGE_MIN_SPEED_MPS: f64 = 20.0;
/// Straight running: |g_lat| below this (g).
const STRAIGHT_MAX_G_LAT: f64 = 0.2;
/// A corner starts above this |g_lat| (g) ...
const CORNER_ENTER_G_LAT: f64 = 1.0;
/// ... and lasts while |g_lat| stays above this (g; hysteresis).
const CORNER_STAY_G_LAT: f64 = 0.8;
/// Only samples at least this long into a sustained corner are used (s).
const CORNER_SETTLE_S: f64 = 1.0;
/// Loaded-corner samples needed per wheel.
const MIN_CORNER_SAMPLES: u64 = 300;
/// Straight samples needed per wheel.
const MIN_STRAIGHT_SAMPLES: u64 = 300;
/// `outer − inner` must move by more than this between straights and loaded corners (°C).
const EDGE_DELTA_MIN_C: f64 = 1.0;

/// Index of the outer edge (car-frame L = 0, R = 2) under A-LMU-016: left wheels' outer edge is the left
/// edge, right wheels' outer edge is the right edge.
fn outer_is_left(wheel: usize) -> bool {
    wheel.is_multiple_of(2)
}

/// `(loaded Δ, unloaded Δ)` of `outer − inner` vs straight running (°C).
type EdgeDeltas = (Option<f64>, Option<f64>);

#[derive(Debug, Clone, Copy, Default)]
struct EdgeAcc {
    straight: Mean,
    loaded: Mean,
    unloaded: Mean,
}

/// Tyre L/C/R → inner/outer mapping evidence from sustained cornering.
#[derive(Debug, Clone, Default)]
pub struct TyreEdgeCheck {
    wheels: [EdgeAcc; 4],
    corner: Option<(f64, f64)>,
    /// Published inner / outer / spread vs the A-LMU-016 mapping of the published car-frame L/R.
    published_edges: PublishedVsRaw,
}

impl TyreEdgeCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        if step.discontinuity {
            self.corner = None;
        }
        let (Some(frame), Some(t)) = (step.frame, step.raw) else {
            return;
        };
        let (Some(et), Some(v), Some(g)) = (
            frame.source_et_s,
            frame.f64(ch::SPEED),
            frame.f64(ch::G_LAT),
        ) else {
            self.corner = None;
            return;
        };
        if v < EDGE_MIN_SPEED_MPS {
            self.corner = None;
            return;
        }
        // Sustained-corner tracking: (start ET, direction sign).
        self.corner = match self.corner {
            Some((start, dir)) if g.abs() > CORNER_STAY_G_LAT && g.signum() == dir => {
                Some((start, dir))
            }
            _ if g.abs() > CORNER_ENTER_G_LAT => Some((et, g.signum())),
            _ => None,
        };
        let settled = self
            .corner
            .filter(|(start, _)| et - start >= CORNER_SETTLE_S)
            .map(|(_, dir)| dir);
        for (i, acc) in self.wheels.iter_mut().enumerate() {
            if t.wheels.get(i).is_none_or(|w| w.detached) {
                continue;
            }
            let (Some(l), Some(r)) = (
                TYRE_TEMP_LEFT.get(i).and_then(|&id| frame.f64(id)),
                TYRE_TEMP_RIGHT.get(i).and_then(|&id| frame.f64(id)),
            ) else {
                continue;
            };
            let (outer, inner) = if outer_is_left(i) { (l, r) } else { (r, l) };
            let published = |ids: &[ChannelId; 4]| ids.get(i).and_then(|&id| frame.f64(id));
            self.published_edges
                .push(published(&TYRE_TEMP_INNER), inner);
            self.published_edges
                .push(published(&TYRE_TEMP_OUTER), outer);
            self.published_edges
                .push(published(&TYRE_TEMP_SPREAD), inner - outer);
            let outer_minus_inner = outer - inner;
            if g.abs() < STRAIGHT_MAX_G_LAT {
                acc.straight.push(outer_minus_inner);
            }
            if let Some(dir) = settled {
                // g_lat < 0: accelerating to the right (right-hand corner) loads the left wheels.
                let loaded = if dir < 0.0 {
                    outer_is_left(i)
                } else {
                    !outer_is_left(i)
                };
                if loaded {
                    acc.loaded.push(outer_minus_inner);
                } else {
                    acc.unloaded.push(outer_minus_inner);
                }
            }
        }
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "tyre_side_mapping",
            "Tyre L/C/R → inner/outer mapping (loaded outside edge warms in long corners)",
            &["A-LMU-016"],
            &[
                "tyres.{w}.temp_surface_inner_c",
                "tyres.{w}.temp_surface_outer_c",
                "tyres.{w}.inner_outer_spread_c",
            ],
        );
        // Relative changes vs straight running: (loaded Δ, unloaded Δ) per wheel.
        let mut deltas: [EdgeDeltas; 4] = [(None, None); 4];
        let mut enough = true;
        for ((label, acc), d) in WHEELS.iter().zip(&self.wheels).zip(deltas.iter_mut()) {
            let straight = acc.straight.mean();
            let loaded = acc.loaded.mean().zip(straight).map(|(c, s)| c - s);
            let unloaded = acc.unloaded.mean().zip(straight).map(|(c, s)| c - s);
            *d = (loaded, unloaded);
            o.line(format!(
                "{label}: outer − inner (A-LMU-016 mapping): straight {} °C (n = {}); loaded (outside of a long corner) {} °C (n = {}, Δ {}); unloaded (inside) {} °C (n = {}, Δ {})",
                fmt_opt(straight, 2),
                acc.straight.count(),
                fmt_opt(acc.loaded.mean(), 2),
                acc.loaded.count(),
                fmt_opt(loaded, 2),
                fmt_opt(acc.unloaded.mean(), 2),
                acc.unloaded.count(),
                fmt_opt(unloaded, 2)
            ));
            enough &= acc.loaded.count() >= MIN_CORNER_SAMPLES
                && acc.unloaded.count() >= MIN_CORNER_SAMPLES
                && acc.straight.count() >= MIN_STRAIGHT_SAMPLES;
        }
        let count = |pick: fn(&EdgeDeltas) -> Option<f64>, positive: bool| {
            deltas
                .iter()
                .filter(|d| {
                    pick(d).is_some_and(|x| {
                        if positive {
                            x > EDGE_DELTA_MIN_C
                        } else {
                            x < -EDGE_DELTA_MIN_C
                        }
                    })
                })
                .count()
        };
        let loaded_up = count(|d| d.0, true);
        let loaded_down = count(|d| d.0, false);
        let unloaded_up = count(|d| d.1, true);
        let unloaded_down = count(|d| d.1, false);
        // Mirror symmetry: on each axle the left and right wheels must move the same way (a per-wheel
        // inner/centre/outer layout would mirror one side).
        let mirrored_axle = [(0usize, 1usize), (2, 3)].iter().any(|&(l, r)| {
            let opposite = |a: Option<f64>, b: Option<f64>| {
                a.zip(b).is_some_and(|(x, y)| {
                    x.abs() > EDGE_DELTA_MIN_C
                        && y.abs() > EDGE_DELTA_MIN_C
                        && x.signum() != y.signum()
                })
            };
            let (dl, dr) = (deltas.get(l).copied(), deltas.get(r).copied());
            let (Some(dl), Some(dr)) = (dl, dr) else {
                return false;
            };
            opposite(dl.0, dr.0) && opposite(dl.1, dr.1)
        });
        o.metric("loaded_outer_warms", loaded_up as f64)
            .metric("loaded_outer_cools", loaded_down as f64)
            .metric("unloaded_outer_warms", unloaded_up as f64)
            .metric("unloaded_outer_cools", unloaded_down as f64);
        o.line(format!(
            "expected under A-LMU-016 (body roll moves each contact patch towards the outside of the turn): the loaded tyre's outer edge warms (Δ > +{EDGE_DELTA_MIN_C} °C) and the unloaded tyre's inner edge warms (Δ < −{EDGE_DELTA_MIN_C} °C); a mirrored left or right side would flip one side's signs"
        ));
        // Camber signature: race setups run negative camber, so on straights the inner edge is hotter on
        // every wheel; a mirrored side would show its outer edge hotter. Supporting evidence only (setup-
        // dependent), it does not change the verdict.
        let straight_inner_hotter = self
            .wheels
            .iter()
            .filter(|acc| acc.straight.mean().is_some_and(|x| x < -EDGE_DELTA_MIN_C))
            .count();
        let straight_outer_hotter = self
            .wheels
            .iter()
            .filter(|acc| acc.straight.mean().is_some_and(|x| x > EDGE_DELTA_MIN_C))
            .count();
        o.metric("straight_inner_hotter", straight_inner_hotter as f64)
            .metric("straight_outer_hotter", straight_outer_hotter as f64);
        o.line(format!(
            "straight running (camber signature, supporting only): inner edge hotter by > {EDGE_DELTA_MIN_C} °C on {straight_inner_hotter}/4 wheels, outer edge hotter on {straight_outer_hotter}/4 (negative camber under A-LMU-016 ⇒ inner hotter on 4/4; a mirrored side would flip its wheels)"
        ));
        let summary = format!(
            "unloaded (inside) tyres: inner edge warms on {unloaded_down}/4 wheels; loaded (outside) tyres: outer edge warms on {loaded_up}/4, cools on {loaded_down}/4; straights: inner edge hotter on {straight_inner_hotter}/4 (camber){}",
            if mirrored_axle {
                "; left and right wheels move in opposite directions on an axle"
            } else {
                ""
            }
        );
        o.metric("published_edge_pairs", self.published_edges.pairs() as f64)
            .metric_opt(
                "published_edge_max_error_c",
                self.published_edges.max_error(),
            );
        o.line(format!(
            "published temp_surface_inner_c / temp_surface_outer_c / inner_outer_spread_c vs the A-LMU-016 mapping of the published car-frame L/R (inner − outer for the spread): {}",
            self.published_edges.describe()
        ));
        if self.published_edges.disagrees(EDGE_MAPPING_TOLERANCE_C) {
            return o.verdict(
                Verdict::Fail,
                format!(
                    "{summary}; the published inner/outer/spread channels do not follow the A-LMU-016 mapping ({})",
                    self.published_edges.describe()
                ),
            );
        }
        if !enough {
            return o.verdict(
                Verdict::Inconclusive,
                format!(
                    "{summary}; not enough sustained cornering in both directions (≥ {MIN_CORNER_SAMPLES} samples per wheel and side)"
                ),
            );
        }
        if mirrored_axle || (loaded_down == 4 && unloaded_up == 4) {
            o.verdict(Verdict::Fail, summary)
        } else if loaded_up == 4
            && unloaded_down == 4
            && !self.published_edges.agrees(EDGE_MAPPING_TOLERANCE_C)
        {
            o.verdict(
                Verdict::Inconclusive,
                format!("{summary}; the published inner/outer/spread channels were never published to compare"),
            )
        } else if loaded_up == 4 && unloaded_down == 4 {
            o.verdict(Verdict::Pass, summary)
        } else {
            o.verdict(Verdict::Inconclusive, summary)
        }
    }
}

/// Driving: above this speed (m/s).
const WEAR_DRIVING_SPEED_MPS: f64 = 10.0;
/// Wear changes below this are noise.
const WEAR_NOISE: f64 = 1e-7;
/// Total wear change needed for a verdict.
const MIN_WEAR_CHANGE: f64 = 1e-4;
/// Wear steps while driving (wheel × sample) needed for a verdict (≈ 5 s on four wheels).
const MIN_WEAR_STEPS: u64 = 2_000;
/// Share of the change in one direction that states the direction.
const WEAR_DIRECTION_SHARE: f64 = 0.95;
/// Below this share the direction is inconsistent.
const WEAR_INCONSISTENT_SHARE: f64 = 0.8;
/// A wear change across a stationary episode larger than this is a tyre change.
const WEAR_JUMP: f64 = 0.005;
/// Stationary below this speed (m/s).
const WEAR_STATIONARY_SPEED_MPS: f64 = 0.5;
/// A stationary episode ends above this speed (m/s; hysteresis).
const WEAR_STOP_END_SPEED_MPS: f64 = 2.0;
/// Tyre-change steps listed.
const MAX_JUMPS: usize = 12;

/// `mWear` direction while driving (A-LMU-040).
#[derive(Debug, Clone, Default)]
pub struct WearCheck {
    prev: [Option<f64>; 4],
    first: [Option<f64>; 4],
    last: [Option<f64>; 4],
    increase: f64,
    decrease: f64,
    steps: Agreement,
    jumps: Vec<String>,
    stop_start: Option<[Option<f64>; 4]>,
    tyre_changes_up: u64,
    /// Published `tyres.{w}.wear_frac` vs `mWear` (all four wheels, when published).
    published_wear: PublishedVsRaw,
}

/// Published `tyres.{w}.wear_frac` channel ids in LMU wheel order.
const TYRE_WEAR: [ChannelId; 4] = [
    ch::TYRE_WEAR_FL,
    ch::TYRE_WEAR_FR,
    ch::TYRE_WEAR_RL,
    ch::TYRE_WEAR_RR,
];

impl WearCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        if step.discontinuity {
            self.prev = [None; 4];
        }
        let (Some(frame), Some(t)) = (step.frame, step.raw) else {
            return;
        };
        let Some(v) = frame.f64(ch::SPEED) else {
            return;
        };
        for (i, w) in t.wheels.iter().enumerate() {
            if let Some(&id) = TYRE_WEAR.get(i) {
                self.published_wear.push(frame.f64(id), w.wear);
            }
            let (Some(prev), Some(first), Some(last)) = (
                self.prev.get_mut(i),
                self.first.get_mut(i),
                self.last.get_mut(i),
            ) else {
                continue;
            };
            if w.detached || !w.wear.is_finite() {
                *prev = None;
                continue;
            }
            first.get_or_insert(w.wear);
            *last = Some(w.wear);
            if let Some(p) = *prev {
                let d = w.wear - p;
                if v > WEAR_DRIVING_SPEED_MPS && d.abs() > WEAR_NOISE {
                    if d < 0.0 {
                        self.decrease -= d;
                    } else {
                        self.increase += d;
                    }
                    self.steps.push(d < 0.0);
                }
            }
            *prev = Some(w.wear);
        }
        // Tyre changes: compare the wear before and after each stationary episode.
        let now: [Option<f64>; 4] = std::array::from_fn(|i| {
            t.wheels
                .get(i)
                .filter(|w| !w.detached && w.wear.is_finite())
                .map(|w| w.wear)
        });
        if v < WEAR_STATIONARY_SPEED_MPS {
            self.stop_start.get_or_insert(now);
        } else if v > WEAR_STOP_END_SPEED_MPS
            && let Some(start) = self.stop_start.take()
        {
            let changed: Vec<String> = WHEELS
                .iter()
                .zip(start.iter().zip(&now))
                .filter_map(|(label, (a, b))| {
                    let (a, b) = ((*a)?, (*b)?);
                    ((b - a).abs() > WEAR_JUMP).then(|| format!("{label} {a:.4} → {b:.4}"))
                })
                .collect();
            if !changed.is_empty() {
                if changed.len() == WHEELS.len() {
                    self.tyre_changes_up += u64::from(
                        start
                            .iter()
                            .zip(&now)
                            .all(|(a, b)| a.zip(*b).is_some_and(|(a, b)| b > a)),
                    );
                }
                if self.jumps.len() < MAX_JUMPS {
                    self.jumps.push(format!(
                        "stationary until capture {:.1} s: {}",
                        step.capture_s,
                        changed.join(", ")
                    ));
                }
            }
        }
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "tyre_wear_direction",
            "Tyre wear direction (mWear) while driving",
            &["A-LMU-040"],
            &["tyres.{w}.wear_frac"],
        );
        let total = self.increase + self.decrease;
        let down_share = (total > 0.0).then(|| self.decrease / total);
        o.metric("decrease", self.decrease)
            .metric("increase", self.increase)
            .metric_opt("decrease_share", down_share);
        for (i, label) in WHEELS.iter().enumerate() {
            o.line(format!(
                "{label}: first {} → last {}",
                fmt_opt(self.first.get(i).copied().flatten(), 4),
                fmt_opt(self.last.get(i).copied().flatten(), 4)
            ));
        }
        o.line(format!(
            "while driving: total decrease {:.5}, increase {:.5} ({} of the change downwards over {} steps)",
            self.decrease,
            self.increase,
            pct(down_share),
            self.steps.total
        ));
        for j in &self.jumps {
            o.line(format!("tyre change: {j}"));
        }
        o.metric("tyre_changes_all_wheels_up", self.tyre_changes_up as f64);
        if total < MIN_WEAR_CHANGE || self.steps.total < MIN_WEAR_STEPS {
            return o.verdict(
                Verdict::Inconclusive,
                format!(
                    "no measurable wear while driving (change {total:.6} over {} steps)",
                    self.steps.total
                ),
            );
        }
        let share = down_share.unwrap_or(0.5);
        let dominant = share.max(1.0 - share);
        // A-LMU-040 is VERIFIED as 1.0 = new and falling: only that direction passes, a consistent rise is a
        // contradiction (the app's tyre-life displays would be silently inverted).
        let published = [(
            "tyres.{w}.wear_frac",
            self.published_wear,
            EXACT_PASSTHROUGH_TOLERANCE,
        )];
        describe_published(&mut o, &published);
        if share >= WEAR_DIRECTION_SHARE {
            gate_rows(
                o,
                &published,
                format!(
                    "decreases while driving ⇒ 1.0 = new tyre, falls with wear ({} of the change)",
                    pct(Some(share))
                ),
            )
        } else if 1.0 - share >= WEAR_DIRECTION_SHARE {
            o.verdict(
                Verdict::Fail,
                format!(
                    "contradicts A-LMU-040 (rises while driving: {} of the change upwards; documented 1.0 = new, falling)",
                    pct(Some(1.0 - share))
                ),
            )
        } else if dominant < WEAR_INCONSISTENT_SHARE {
            o.verdict(
                Verdict::Fail,
                format!(
                    "no consistent direction: {} of the change downwards",
                    pct(down_share)
                ),
            )
        } else {
            o.verdict(
                Verdict::Inconclusive,
                format!("mostly one direction ({} downwards)", pct(down_share)),
            )
        }
    }
}
