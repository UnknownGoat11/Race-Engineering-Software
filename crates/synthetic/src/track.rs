//! The closed synthetic circuit: straights and constant-radius corners, closed exactly, smoothed into
//! continuous curvature and sampled into ~1 m stations.
//!
//! Geometry conventions (LMU world, A-LMU-025): left-handed, y up; a top view shows +x to the right and
//! +z up. Heading is `atan2(forward.x, forward.z)` (as lmu-normalizer's `heading_rad`), so it grows when
//! the car turns right. Curvature is signed **+ = left turn**. Lap distance 0 is the start/finish line.

use std::f64::consts::{PI, TAU};

use telemetry_model::value::MAX_STR_VALUE_BYTES;

use crate::error::SyntheticError;
use crate::mode::synthetic_name;

/// Nominal distance between two stations; the exact spacing is `length / station count`.
pub const STATION_SPACING_M: f64 = 1.0;
/// Smallest corner radius accepted in a track specification.
const MIN_CORNER_RADIUS_M: f64 = 10.0;
/// Fewest elements of a plausible closed circuit.
const MIN_ELEMENTS: usize = 3;
/// Tolerance on the total turning angle of a closed circuit (must be ±360°).
const FULL_TURN_TOLERANCE_DEG: f64 = 1e-6;
/// One full turn.
const FULL_TURN_DEG: f64 = 360.0;
/// Arc subdivisions used by the analytic closure solve (exact for the arc chord sums used).
const CLOSURE_ARC_STEPS: usize = 256;
/// Smallest |sin| between the two closing straights (they must not be parallel).
const MIN_CLOSING_DETERMINANT: f64 = 0.05;
/// Curvature below which a station counts as straight (radius > 2 km).
const STRAIGHT_CURVATURE_PER_M: f64 = 5.0e-4;
/// World position of the start/finish line, so positions are not trivially zero (m).
const WORLD_OFFSET_X_M: f64 = 120.0;
const WORLD_OFFSET_Z_M: f64 = -340.0;
/// Phase of the second elevation harmonic (rad).
const ELEVATION_PHASE_RAD: f64 = 0.7;
/// Plausible circuit length range (m), inside the registry's track-length range.
const MIN_TRACK_LENGTH_M: f64 = 1_000.0;
const MAX_TRACK_LENGTH_M: f64 = 20_000.0;
/// Steepest accepted elevation gradient (dy/ds); the steepest real circuits stay below ~0.2.
const MAX_ELEVATION_SLOPE: f64 = 0.12;
/// Largest accepted vertical curvature d²y/ds² (1/m): ≈ 1.7 g of vertical acceleration at 92 m/s.
const MAX_VERTICAL_CURVATURE_PER_M: f64 = 0.002;

/// A straight followed by a constant-radius corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackElement {
    /// Straight before the corner (m).
    pub straight_m: f64,
    /// Corner angle in degrees, + = left.
    pub turn_deg: f64,
    /// Corner radius (m).
    pub radius_m: f64,
}

impl TrackElement {
    const fn new(straight_m: f64, turn_deg: f64, radius_m: f64) -> Self {
        Self {
            straight_m,
            turn_deg,
            radius_m,
        }
    }

    fn arc_m(&self) -> f64 {
        self.radius_m * self.turn_deg.to_radians().abs()
    }

    fn curvature(&self) -> f64 {
        self.turn_deg.signum() / self.radius_m
    }
}

/// Pit lane alongside the start/finish straight.
#[derive(Debug, Clone, PartialEq)]
pub struct PitLaneSpec {
    /// Pit entry (speed-limit line) distance before the start/finish line (m).
    pub entry_before_line_m: f64,
    /// Pit exit distance after the line (m).
    pub exit_after_line_m: f64,
    /// Pit box distance after the line (m).
    pub box_after_line_m: f64,
    /// Lateral offset of the pit lane from the racing line, + = right (m).
    pub lateral_offset_m: f64,
    /// Length over which the car moves between racing line and pit lane (m).
    pub taper_m: f64,
    /// Pit-lane speed limit (m/s).
    pub speed_limit_mps: f64,
}

/// Full description of a circuit.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackSpec {
    /// Display name (prefixed with the synthetic label when built).
    pub name: String,
    /// Elements in driving order; element 0's straight contains the start/finish line.
    pub elements: Vec<TrackElement>,
    /// Indices of the two straights whose lengths are adjusted so the circuit closes exactly.
    pub closing_straights: (usize, usize),
    /// Start/finish line distance into element 0's (adjusted) straight (m).
    pub start_line_m: f64,
    /// Sector 2 and sector 3 start as fractions of the lap.
    pub sector_fractions: [f64; 2],
    /// Curvature smoothing window (emulates clothoid transitions) (m).
    pub smoothing_window_m: f64,
    /// Amplitudes of the two elevation harmonics (m).
    pub elevation_amplitudes_m: [f64; 2],
    /// Half the track width (m).
    pub half_width_m: f64,
    pub pit: PitLaneSpec,
}

impl TrackSpec {
    /// The default synthetic circuit: ~4.7 km, 10 corners (hairpins, a chicane, fast sweepers), a long
    /// start/finish straight with the pit lane and a ~1 km back straight.
    pub fn synthetic_circuit() -> Self {
        Self {
            name: "Synthetic Circuit".to_owned(),
            elements: vec![
                TrackElement::new(800.0, -90.0, 40.0),
                TrackElement::new(150.0, 45.0, 120.0),
                TrackElement::new(250.0, -40.0, 90.0),
                TrackElement::new(400.0, -100.0, 35.0),
                TrackElement::new(250.0, 40.0, 50.0),
                TrackElement::new(30.0, -40.0, 50.0),
                TrackElement::new(1_000.0, -90.0, 60.0),
                TrackElement::new(200.0, 60.0, 80.0),
                TrackElement::new(150.0, -60.0, 110.0),
                TrackElement::new(300.0, -85.0, 50.0),
            ],
            closing_straights: (0, 6),
            start_line_m: 650.0,
            sector_fractions: [0.32, 0.66],
            smoothing_window_m: 31.0,
            elevation_amplitudes_m: [6.0, 2.5],
            half_width_m: 7.0,
            pit: PitLaneSpec {
                entry_before_line_m: 300.0,
                exit_after_line_m: 320.0,
                box_after_line_m: 70.0,
                lateral_offset_m: 14.0,
                taper_m: 40.0,
                speed_limit_mps: 60.0 / 3.6,
            },
        }
    }
}

/// One sampled point of the centreline.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Station {
    x_m: f64,
    z_m: f64,
    /// Unwrapped heading at the station (rad).
    heading_rad: f64,
    /// Smoothed signed curvature, + = left (1/m).
    curvature_per_m: f64,
}

/// Interpolated track geometry at one lap distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPoint {
    /// World position (m), y = elevation.
    pub x_m: f64,
    pub y_m: f64,
    pub z_m: f64,
    /// Heading in -π..π (rad).
    pub heading_rad: f64,
    /// Signed curvature, + = left (1/m).
    pub curvature_per_m: f64,
    /// Elevation slope dy/ds.
    pub slope: f64,
    /// Elevation second derivative d²y/ds² (1/m).
    pub vertical_curvature_per_m: f64,
}

/// Pit lane geometry on a built track. Lap distances are measured along the racing line.
#[derive(Debug, Clone, PartialEq)]
pub struct PitLane {
    /// Lap distance of the pit entry line (m).
    pub entry_s_m: f64,
    /// Lap distance of the pit exit line (m).
    pub exit_s_m: f64,
    /// Lap distance of the pit box (m).
    pub box_s_m: f64,
    /// Lateral offset from the racing line, + = right (m).
    pub lateral_offset_m: f64,
    /// Transition length (m).
    pub taper_m: f64,
    /// Speed limit (m/s).
    pub speed_limit_mps: f64,
    track_length_m: f64,
}

impl PitLane {
    /// Length of the pit lane from entry to exit (m).
    pub fn lane_length_m(&self) -> f64 {
        (self.exit_s_m - self.entry_s_m).rem_euclid(self.track_length_m)
    }

    /// Distance from the entry line to `s`, if `s` lies inside the pit lane.
    pub fn progress_m(&self, s_m: f64) -> Option<f64> {
        let d = (s_m - self.entry_s_m).rem_euclid(self.track_length_m);
        (d <= self.lane_length_m()).then_some(d)
    }

    /// Distance from the entry line to the box (m).
    pub fn box_progress_m(&self) -> f64 {
        (self.box_s_m - self.entry_s_m).rem_euclid(self.track_length_m)
    }

    /// Distance still to drive from `s` to the entry line (m), in 0..track length.
    pub fn distance_to_entry_m(&self, s_m: f64) -> f64 {
        (self.entry_s_m - s_m).rem_euclid(self.track_length_m)
    }

    /// Lateral offset of a car in the pit lane at `s` (with the entry/exit tapers).
    pub fn lateral_at(&self, s_m: f64) -> f64 {
        match self.progress_m(s_m) {
            None => 0.0,
            Some(d) => {
                let ramp = (d / self.taper_m)
                    .min((self.lane_length_m() - d) / self.taper_m)
                    .clamp(0.0, 1.0);
                ramp * self.lateral_offset_m
            }
        }
    }
}

/// A built circuit.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    name: String,
    length_m: f64,
    spacing_m: f64,
    stations: Vec<Station>,
    /// Heading change over one lap (±2π), for unwrapping across the line.
    lap_heading_change_rad: f64,
    sector_starts_m: [f64; 3],
    elevation_amplitudes_m: [f64; 2],
    half_width_m: f64,
    pit: PitLane,
}

impl Track {
    /// Builds and validates a circuit.
    pub fn build(spec: &TrackSpec) -> Result<Self, SyntheticError> {
        validate_spec(spec)?;
        let lengths = solve_closure(spec)?;
        let pieces = pieces(spec, &lengths);
        let length_m: f64 = pieces.iter().map(|p| p.length_m).sum();
        if !(MIN_TRACK_LENGTH_M..=MAX_TRACK_LENGTH_M).contains(&length_m) {
            return Err(SyntheticError::config(
                "track.elements",
                format!(
                    "circuit length {length_m:.0} m outside {MIN_TRACK_LENGTH_M}..{MAX_TRACK_LENGTH_M} m"
                ),
            ));
        }
        // Rounding to whole stations: the count is ≥ MIN_TRACK_LENGTH_M / spacing, far inside usize.
        let count = (length_m / STATION_SPACING_M).round().max(1.0) as usize;
        let spacing_m = length_m / count as f64;

        let raw = sample_curvature(&pieces, count, spacing_m);
        let window = smoothing_stations(spec.smoothing_window_m, spacing_m, count);
        let curvature = circular_moving_average(&raw, window);
        let mut stations = integrate(&curvature, spacing_m);

        // Rotate so that station 0 is the start/finish line on element 0's straight.
        let shift = ((spec.start_line_m / spacing_m).round() as usize) % count;
        stations.rotate_left(shift);
        let (x0, z0) = (stations[0].x_m, stations[0].z_m);
        let lap_heading_change_rad = -curvature.iter().sum::<f64>() * spacing_m;
        for (i, st) in stations.iter_mut().enumerate() {
            // Stations rotated from the start of the array have already completed the turn once.
            if i + shift >= count {
                st.heading_rad += lap_heading_change_rad;
            }
            st.x_m += WORLD_OFFSET_X_M - x0;
            st.z_m += WORLD_OFFSET_Z_M - z0;
        }

        // A lane reaching around the lap would wrap (rem_euclid) and leave the box outside it.
        let lane_span_m =
            spec.pit.entry_before_line_m + spec.pit.exit_after_line_m + 2.0 * spec.pit.taper_m;
        if lane_span_m >= length_m {
            return Err(SyntheticError::config(
                "track.pit",
                format!(
                    "pit lane with tapers ({lane_span_m:.0} m) must be shorter than the lap ({length_m:.0} m)"
                ),
            ));
        }
        validate_elevation(spec.elevation_amplitudes_m, length_m)?;
        let pit = PitLane {
            entry_s_m: length_m - spec.pit.entry_before_line_m,
            exit_s_m: spec.pit.exit_after_line_m,
            box_s_m: spec.pit.box_after_line_m,
            lateral_offset_m: spec.pit.lateral_offset_m,
            taper_m: spec.pit.taper_m,
            speed_limit_mps: spec.pit.speed_limit_mps,
            track_length_m: length_m,
        };
        let track = Self {
            name: synthetic_name(&spec.name),
            length_m,
            spacing_m,
            stations,
            lap_heading_change_rad,
            sector_starts_m: [
                0.0,
                spec.sector_fractions[0] * length_m,
                spec.sector_fractions[1] * length_m,
            ],
            elevation_amplitudes_m: spec.elevation_amplitudes_m,
            half_width_m: spec.half_width_m,
            pit,
        };
        track.validate_pit_straight()?;
        Ok(track)
    }

    /// Display name (carries the synthetic prefix).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Lap length (m).
    pub fn length_m(&self) -> f64 {
        self.length_m
    }

    /// Half the track width (m).
    pub fn half_width_m(&self) -> f64 {
        self.half_width_m
    }

    /// Pit lane geometry.
    pub fn pit(&self) -> &PitLane {
        &self.pit
    }

    /// Start of each sector (lap distance, m); sector 1 starts at 0.
    pub fn sector_starts_m(&self) -> [f64; 3] {
        self.sector_starts_m
    }

    /// Zero-based sector index (0 = S1) of lap distance `s`.
    pub fn sector_index(&self, s_m: f64) -> u8 {
        let s = self.wrap(s_m);
        if s >= self.sector_starts_m[2] {
            2
        } else if s >= self.sector_starts_m[1] {
            1
        } else {
            0
        }
    }

    /// `s` folded into 0..length.
    pub fn wrap(&self, s_m: f64) -> f64 {
        s_m.rem_euclid(self.length_m)
    }

    /// Number of stations.
    pub fn station_count(&self) -> usize {
        self.stations.len()
    }

    /// Exact station spacing (m).
    pub fn spacing_m(&self) -> f64 {
        self.spacing_m
    }

    /// Smoothed curvature at station `index` (+ = left, 1/m).
    pub fn station_curvature(&self, index: usize) -> f64 {
        self.stations
            .get(index % self.stations.len().max(1))
            .map_or(0.0, |s| s.curvature_per_m)
    }

    /// Interpolated geometry of the racing line at lap distance `s`.
    pub fn point(&self, s_m: f64) -> TrackPoint {
        let s = self.wrap(s_m);
        let n = self.stations.len();
        let pos = s / self.spacing_m;
        // `pos` is in 0..n by construction of `wrap`; the modulo guards the s == length edge.
        let i = (pos.floor() as usize) % n;
        let f = pos - pos.floor();
        let a = self.stations[i];
        let b = self.stations[(i + 1) % n];
        let heading = a.heading_rad - a.curvature_per_m * f * self.spacing_m;
        TrackPoint {
            x_m: a.x_m + (b.x_m - a.x_m) * f,
            y_m: self.elevation(s),
            z_m: a.z_m + (b.z_m - a.z_m) * f,
            heading_rad: wrap_pi(heading),
            curvature_per_m: a.curvature_per_m + (b.curvature_per_m - a.curvature_per_m) * f,
            slope: self.elevation_slope(s),
            vertical_curvature_per_m: self.elevation_curvature(s),
        }
    }

    /// World position of a car at lap distance `s`, `lateral_m` to the right of the racing line.
    pub fn world_position(&self, s_m: f64, lateral_m: f64) -> (f64, f64, f64) {
        let p = self.point(s_m);
        // Right of the forward vector (sin h, cos h) in the x/z top view is (cos h, −sin h).
        (
            p.x_m + lateral_m * p.heading_rad.cos(),
            p.y_m,
            p.z_m - lateral_m * p.heading_rad.sin(),
        )
    }

    /// Total heading change over one lap (±2π; + = net right turns).
    pub fn lap_heading_change_rad(&self) -> f64 {
        self.lap_heading_change_rad
    }

    fn elevation(&self, s: f64) -> f64 {
        let w = TAU / self.length_m;
        let [a1, a2] = self.elevation_amplitudes_m;
        a1 * (w * s).sin() + a2 * (2.0 * w * s + ELEVATION_PHASE_RAD).sin()
    }

    fn elevation_slope(&self, s: f64) -> f64 {
        let w = TAU / self.length_m;
        let [a1, a2] = self.elevation_amplitudes_m;
        a1 * w * (w * s).cos() + 2.0 * a2 * w * (2.0 * w * s + ELEVATION_PHASE_RAD).cos()
    }

    fn elevation_curvature(&self, s: f64) -> f64 {
        let w = TAU / self.length_m;
        let [a1, a2] = self.elevation_amplitudes_m;
        -a1 * w * w * (w * s).sin() - 4.0 * a2 * w * w * (2.0 * w * s + ELEVATION_PHASE_RAD).sin()
    }

    /// The pit lane (with its tapers) must lie on a straight.
    fn validate_pit_straight(&self) -> Result<(), SyntheticError> {
        let start = self.pit.entry_s_m - self.pit.taper_m;
        let span = self.pit.lane_length_m() + 2.0 * self.pit.taper_m;
        let steps = (span / self.spacing_m).ceil() as usize;
        for k in 0..=steps {
            let s = start + k as f64 * self.spacing_m;
            if self.point(s).curvature_per_m.abs() > STRAIGHT_CURVATURE_PER_M {
                return Err(SyntheticError::config(
                    "track.pit",
                    format!("pit lane is not on a straight (curved at lap distance {s:.0} m)"),
                ));
            }
        }
        Ok(())
    }
}

/// `angle` folded into -π..π.
pub fn wrap_pi(angle: f64) -> f64 {
    let a = (angle + PI).rem_euclid(TAU) - PI;
    if a < -PI { a + TAU } else { a }
}

/// Bounds the elevation harmonics' steepest gradient and vertical curvature on a lap of `length_m`.
fn validate_elevation(amplitudes_m: [f64; 2], length_m: f64) -> Result<(), SyntheticError> {
    let w = TAU / length_m;
    let [a1, a2] = amplitudes_m.map(f64::abs);
    let max_slope = w * (a1 + 2.0 * a2);
    let max_curvature = w * w * (a1 + 4.0 * a2);
    if max_slope > MAX_ELEVATION_SLOPE || max_curvature > MAX_VERTICAL_CURVATURE_PER_M {
        return Err(SyntheticError::config(
            "track.elevation_amplitudes_m",
            format!(
                "gradient up to {max_slope:.3} / vertical curvature up to {max_curvature:.4} 1/m exceeds \
                 {MAX_ELEVATION_SLOPE} / {MAX_VERTICAL_CURVATURE_PER_M} 1/m"
            ),
        ));
    }
    Ok(())
}

fn validate_spec(spec: &TrackSpec) -> Result<(), SyntheticError> {
    let name_len = synthetic_name(&spec.name).len();
    if name_len > MAX_STR_VALUE_BYTES {
        return Err(SyntheticError::config(
            "track.name",
            format!(
                "prefixed name is {name_len} bytes, the TRACK_NAME limit is {MAX_STR_VALUE_BYTES}"
            ),
        ));
    }
    if spec.elements.len() < MIN_ELEMENTS {
        return Err(SyntheticError::config(
            "track.elements",
            format!("need at least {MIN_ELEMENTS} elements"),
        ));
    }
    for e in &spec.elements {
        let ok = e.straight_m.is_finite()
            && e.straight_m >= 0.0
            && e.turn_deg.is_finite()
            && e.turn_deg != 0.0
            && e.radius_m.is_finite()
            && e.radius_m >= MIN_CORNER_RADIUS_M;
        if !ok {
            return Err(SyntheticError::config(
                "track.elements",
                format!("implausible element {e:?}"),
            ));
        }
    }
    let total: f64 = spec.elements.iter().map(|e| e.turn_deg).sum();
    if (total.abs() - FULL_TURN_DEG).abs() > FULL_TURN_TOLERANCE_DEG {
        return Err(SyntheticError::config(
            "track.elements",
            format!("corners must add up to ±360°, got {total}°"),
        ));
    }
    let (a, b) = spec.closing_straights;
    if a == b || a >= spec.elements.len() || b >= spec.elements.len() {
        return Err(SyntheticError::config(
            "track.closing_straights",
            "need two distinct element indices",
        ));
    }
    let [s2, s3] = spec.sector_fractions;
    if !(0.0 < s2 && s2 < s3 && s3 < 1.0) {
        return Err(SyntheticError::config(
            "track.sector_fractions",
            "need 0 < S2 < S3 < 1",
        ));
    }
    let p = &spec.pit;
    let pit_ok = [
        p.entry_before_line_m,
        p.exit_after_line_m,
        p.box_after_line_m,
        p.taper_m,
        p.speed_limit_mps,
    ]
    .iter()
    .all(|v| v.is_finite() && *v > 0.0)
        && p.lateral_offset_m.is_finite()
        && p.box_after_line_m + p.taper_m < p.exit_after_line_m;
    if !pit_ok {
        return Err(SyntheticError::config(
            "track.pit",
            "pit entry/exit/box distances must be positive with the box before the exit",
        ));
    }
    let numbers_ok = spec.start_line_m.is_finite()
        && spec.start_line_m >= 0.0
        && spec.smoothing_window_m.is_finite()
        && spec.smoothing_window_m >= 0.0
        && spec.half_width_m.is_finite()
        && spec.half_width_m > 0.0
        && spec.elevation_amplitudes_m.iter().all(|a| a.is_finite());
    if !numbers_ok {
        return Err(SyntheticError::config(
            "track",
            "start line, smoothing, width and elevation must be finite and non-negative",
        ));
    }
    Ok(())
}

/// Heading (atan2(dx, dz) convention) at the start of element `index`'s straight.
fn heading_before(elements: &[TrackElement], index: usize) -> f64 {
    // A left turn (+) decreases the heading.
    -elements[..index]
        .iter()
        .map(|e| e.turn_deg.to_radians())
        .sum::<f64>()
}

/// End point of the unsmoothed circuit started at the origin with heading 0.
fn endpoint(elements: &[TrackElement], straights: &[f64]) -> (f64, f64) {
    let (mut x, mut z, mut h) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (e, &straight) in elements.iter().zip(straights) {
        x += straight * h.sin();
        z += straight * h.cos();
        let turn = e.turn_deg.to_radians();
        let step = e.arc_m() / CLOSURE_ARC_STEPS as f64;
        for k in 0..CLOSURE_ARC_STEPS {
            let hm = h - turn * (k as f64 + 0.5) / CLOSURE_ARC_STEPS as f64;
            x += step * hm.sin();
            z += step * hm.cos();
        }
        h -= turn;
    }
    (x, z)
}

/// Adjusts the two closing straights so the unsmoothed circuit ends where it started.
fn solve_closure(spec: &TrackSpec) -> Result<Vec<f64>, SyntheticError> {
    let mut lengths: Vec<f64> = spec.elements.iter().map(|e| e.straight_m).collect();
    let (ex, ez) = endpoint(&spec.elements, &lengths);
    let (a, b) = spec.closing_straights;
    let ha = heading_before(&spec.elements, a);
    let hb = heading_before(&spec.elements, b);
    let (ax, az) = (ha.sin(), ha.cos());
    let (bx, bz) = (hb.sin(), hb.cos());
    let det = ax * bz - az * bx;
    if det.abs() < MIN_CLOSING_DETERMINANT {
        return Err(SyntheticError::config(
            "track.closing_straights",
            "the two closing straights are (nearly) parallel",
        ));
    }
    // Solve da·(ax, az) + db·(bx, bz) = −(ex, ez) (Cramer's rule).
    let da = (-ex * bz + ez * bx) / det;
    let db = (-ez * ax + ex * az) / det;
    lengths[a] += da;
    lengths[b] += db;
    if lengths[a] < 0.0 || lengths[b] < 0.0 {
        return Err(SyntheticError::config(
            "track.closing_straights",
            format!(
                "closing the circuit needs negative straights ({:.0} m, {:.0} m)",
                lengths[a], lengths[b]
            ),
        ));
    }
    if spec.start_line_m > lengths[0] {
        return Err(SyntheticError::config(
            "track.start_line_m",
            format!(
                "start line {:.0} m is beyond the first straight ({:.0} m)",
                spec.start_line_m, lengths[0]
            ),
        ));
    }
    Ok(lengths)
}

/// A constant-curvature piece of the unsmoothed centreline.
#[derive(Debug, Clone, Copy)]
struct Piece {
    length_m: f64,
    curvature_per_m: f64,
}

fn pieces(spec: &TrackSpec, straights: &[f64]) -> Vec<Piece> {
    let mut out = Vec::with_capacity(spec.elements.len() * 2);
    for (e, &straight) in spec.elements.iter().zip(straights) {
        out.push(Piece {
            length_m: straight,
            curvature_per_m: 0.0,
        });
        out.push(Piece {
            length_m: e.arc_m(),
            curvature_per_m: e.curvature(),
        });
    }
    out
}

/// Mean curvature over each station interval (exact overlap integral with the pieces), so the summed
/// turning angle of the stations equals the specified corners exactly.
fn sample_curvature(pieces: &[Piece], count: usize, spacing_m: f64) -> Vec<f64> {
    let mut out = vec![0.0; count];
    let mut piece_start = 0.0;
    for p in pieces {
        let piece_end = piece_start + p.length_m;
        if p.curvature_per_m != 0.0 && p.length_m > 0.0 {
            // Station intervals overlapping [piece_start, piece_end).
            let first = (piece_start / spacing_m).floor() as usize;
            let last = ((piece_end / spacing_m).ceil() as usize).min(count);
            for (i, slot) in out.iter_mut().enumerate().take(last).skip(first) {
                let a = (i as f64 * spacing_m).max(piece_start);
                let b = ((i + 1) as f64 * spacing_m).min(piece_end);
                if b > a {
                    *slot += p.curvature_per_m * (b - a) / spacing_m;
                }
            }
        }
        piece_start = piece_end;
    }
    out
}

/// Odd window length in stations, at most the station count.
fn smoothing_stations(window_m: f64, spacing_m: f64, count: usize) -> usize {
    let w = (window_m / spacing_m).round() as usize;
    let odd = if w.is_multiple_of(2) { w + 1 } else { w };
    odd.min(if count.is_multiple_of(2) {
        count - 1
    } else {
        count
    })
    .max(1)
}

/// Circular moving average; it preserves the sum, so the total turn stays exactly ±2π.
fn circular_moving_average(values: &[f64], window: usize) -> Vec<f64> {
    let n = values.len();
    let half = window / 2;
    let mut out = Vec::with_capacity(n);
    let mut acc: f64 = (0..window).map(|k| values[(n + k - half) % n]).sum();
    for i in 0..n {
        out.push(acc / window as f64);
        acc += values[(i + half + 1) % n] - values[(n + i - half) % n];
    }
    out
}

/// Integrates heading and position from curvature (midpoint rule) and removes the small closure residual
/// left by smoothing by distributing it linearly along the lap.
fn integrate(curvature: &[f64], spacing_m: f64) -> Vec<Station> {
    let n = curvature.len();
    let mut stations = Vec::with_capacity(n);
    let (mut x, mut z, mut h) = (0.0_f64, 0.0_f64, 0.0_f64);
    for &k in curvature {
        stations.push(Station {
            x_m: x,
            z_m: z,
            heading_rad: h,
            curvature_per_m: k,
        });
        let mid = h - k * spacing_m / 2.0;
        x += spacing_m * mid.sin();
        z += spacing_m * mid.cos();
        h -= k * spacing_m;
    }
    for (i, st) in stations.iter_mut().enumerate() {
        let f = i as f64 / n as f64;
        st.x_m -= x * f;
        st.z_m -= z * f;
    }
    stations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track::build(&TrackSpec::synthetic_circuit()).expect("default track builds")
    }

    fn rejected_field(spec: &TrackSpec) -> &'static str {
        match Track::build(spec) {
            Err(SyntheticError::InvalidConfig { field, .. }) => field,
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    /// Regression (review pass 2, F4): a pit lane reaching around the lap wrapped and stranded the car.
    #[test]
    fn pit_lane_longer_than_the_lap_is_rejected() {
        let mut spec = TrackSpec::synthetic_circuit();
        spec.pit.entry_before_line_m = 2_214.0;
        spec.pit.exit_after_line_m = 2_600.0;
        assert_eq!(rejected_field(&spec), "track.pit");
    }

    /// Regression (review pass 2, F5): elevation amplitudes were only checked for being finite.
    #[test]
    fn steep_elevation_is_rejected() {
        let mut spec = TrackSpec::synthetic_circuit();
        spec.elevation_amplitudes_m = [2_000.0, 0.0];
        assert_eq!(rejected_field(&spec), "track.elevation_amplitudes_m");
        spec.elevation_amplitudes_m = [0.0, -150.0];
        assert_eq!(rejected_field(&spec), "track.elevation_amplitudes_m");
        spec.elevation_amplitudes_m = [40.0, 20.0];
        assert!(Track::build(&spec).is_ok());
    }

    /// Regression (review pass 2, F6): an over-long name built fine, then every tick failed.
    #[test]
    fn track_name_longer_than_the_string_limit_is_rejected() {
        let mut spec = TrackSpec::synthetic_circuit();
        spec.name = "x".repeat(MAX_STR_VALUE_BYTES);
        assert_eq!(rejected_field(&spec), "track.name");
        let prefix_len = synthetic_name("").len();
        spec.name = "x".repeat(MAX_STR_VALUE_BYTES - prefix_len);
        assert_eq!(synthetic_name(&spec.name).len(), MAX_STR_VALUE_BYTES);
        assert!(Track::build(&spec).is_ok());
    }

    #[test]
    fn default_circuit_is_closed_and_about_4_5_km() {
        let t = track();
        assert!(
            (4_300.0..=5_000.0).contains(&t.length_m()),
            "{}",
            t.length_m()
        );
        let a = t.point(0.0);
        let b = t.point(t.length_m() - 1e-9);
        assert!((a.x_m - b.x_m).hypot(a.z_m - b.z_m) < 0.5);
        assert!((t.lap_heading_change_rad().abs() - TAU).abs() < 1e-9);
        assert!(t.name().starts_with(crate::mode::SYNTHETIC_NAME_PREFIX));
        // Consecutive stations are one spacing apart (no jumps anywhere, including across the line).
        for i in 0..t.station_count() {
            let p = t.point(i as f64 * t.spacing_m());
            let q = t.point((i + 1) as f64 * t.spacing_m());
            let d = (p.x_m - q.x_m).hypot(p.z_m - q.z_m);
            assert!((d - t.spacing_m()).abs() < 0.01, "station {i}: {d}");
        }
    }

    #[test]
    fn heading_follows_positions() {
        let t = track();
        for s in (0..t.station_count()).step_by(97) {
            let s = s as f64 * t.spacing_m();
            let p = t.point(s);
            let q = t.point(s + 0.5);
            let chord = (q.x_m - p.x_m).atan2(q.z_m - p.z_m);
            assert!(wrap_pi(chord - p.heading_rad).abs() < 0.02, "at {s}");
        }
    }

    #[test]
    fn no_self_intersection_and_sane_corners() {
        let t = track();
        let pts: Vec<(f64, f64)> = (0..t.station_count())
            .step_by(10)
            .map(|i| {
                let p = t.point(i as f64 * t.spacing_m());
                (p.x_m, p.z_m)
            })
            .collect();
        let n = pts.len();
        let orient = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
            (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
        };
        for i in 0..n {
            for j in i + 2..n {
                if i == 0 && j == n - 1 {
                    continue;
                }
                let (p, q, r, s) = (pts[i], pts[(i + 1) % n], pts[j], pts[(j + 1) % n]);
                let crosses = orient(p, q, r) * orient(p, q, s) < 0.0
                    && orient(r, s, p) * orient(r, s, q) < 0.0;
                assert!(!crosses, "segments {i} and {j} cross");
            }
        }
        let max_k = (0..t.station_count())
            .map(|i| t.station_curvature(i).abs())
            .fold(0.0, f64::max);
        assert!(max_k < 1.0 / 30.0 && max_k > 1.0 / 60.0, "{max_k}");
    }

    #[test]
    fn pit_lane_and_sectors() {
        let t = track();
        let pit = t.pit();
        assert!(pit.progress_m(0.0).is_some());
        assert!(pit.progress_m(t.length_m() / 2.0).is_none());
        assert!((pit.lateral_at(0.0) - pit.lateral_offset_m).abs() < 1e-9);
        assert_eq!(pit.lateral_at(t.length_m() / 2.0), 0.0);
        assert!(pit.box_progress_m() < pit.lane_length_m());
        assert_eq!(t.sector_index(0.0), 0);
        assert_eq!(t.sector_index(t.length_m() * 0.5), 1);
        assert_eq!(t.sector_index(t.length_m() * 0.9), 2);
        let (x, _, z) = t.world_position(0.0, 10.0);
        let p = t.point(0.0);
        assert!(((x - p.x_m).hypot(z - p.z_m) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn invalid_specs_are_rejected() {
        let mut spec = TrackSpec::synthetic_circuit();
        spec.elements[0].turn_deg = -80.0;
        assert!(Track::build(&spec).is_err(), "turns no longer add to 360°");
        let mut spec = TrackSpec::synthetic_circuit();
        spec.elements[2].radius_m = 1.0;
        assert!(Track::build(&spec).is_err());
        let mut spec = TrackSpec::synthetic_circuit();
        spec.closing_straights = (3, 3);
        assert!(Track::build(&spec).is_err());
        let mut spec = TrackSpec::synthetic_circuit();
        spec.pit.entry_before_line_m = 900.0;
        assert!(
            Track::build(&spec).is_err(),
            "pit lane would reach a corner"
        );
    }

    #[test]
    fn wrap_pi_folds() {
        assert!((wrap_pi(3.0 * PI) - PI).abs() < 1e-12 || (wrap_pi(3.0 * PI) + PI).abs() < 1e-12);
        assert!((wrap_pi(0.5) - 0.5).abs() < 1e-12);
        assert!((wrap_pi(-0.5 - TAU) + 0.5).abs() < 1e-12);
    }
}
