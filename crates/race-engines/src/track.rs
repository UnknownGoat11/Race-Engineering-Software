//! Track engine (spec §22–24, §146, §155–157): layout-unique track identity, track-map generation from
//! recorded laps, the versioned map file format with validation, projection of world positions onto the
//! circuit, sector location, the display transform and the directory-scoped map cache.
//!
//! # Track identity (spec §157)
//! The same track name can carry different layouts, so a [`TrackId`] combines the normalised LMU track name
//! (`mTrackName`) with the LMU track length rounded to the metre (`mLapDist` of the session):
//! `CIRCUIT_DE_SPA_FRANCORCHAMPS_7004`. Two layouts of one circuit differ in length; a layout change that
//! keeps the length to the metre is not distinguishable from LMU data (the REST layout name would be
//! needed, DEFERRED).
//!
//! # Map generation (spec §24, §155)
//! [`TrackMapBuilder`] bins the world position `(x, z)` of eligible laps by lap distance
//! ([`DEFAULT_BIN_M`] bins). Per bin it takes the median over laps and drops lap values further than
//! [`OUTLIER_DISTANCE_M`] from it (outliers), then removes isolated spikes along the line
//! ([`SPIKE_DISTANCE_M`]) and smooths with a circular moving average. Samples with a negative lap distance
//! (before the first line crossing, A-LMU-070) or beyond the lap length are ignored, as are pit-lane
//! samples (the pit entry and exit are recorded as markers). The map is generated once and persisted; it
//! is never rebuilt per sample.
//!
//! # Coordinates (A-LMU-025)
//! The map uses LMU world `x` and `z` (y is up) in metres. The display transform maps `screen_right = +x`
//! and `screen_up = +z`, with an optional rotation for orientation.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::CALCULATION_ENGINE_VERSION;
use crate::common::{Metric, finite, formula, lerp};

/// Tracing component name.
const COMPONENT: &str = "race_engines::track";

/// Current map file format version.
pub const TRACK_MAP_FORMAT_VERSION: u32 = 1;
/// Plausible track length range (m), as the registry's `track.length_m`.
pub const MIN_TRACK_LENGTH_M: f64 = 500.0;
/// Upper bound of a plausible track length (m).
pub const MAX_TRACK_LENGTH_M: f64 = 30_000.0;
/// Default distance bin (m) of a generated map.
pub const DEFAULT_BIN_M: f64 = 5.0;
/// Smallest allowed bin (m).
pub const MIN_BIN_M: f64 = 1.0;
/// Largest allowed bin (m).
pub const MAX_BIN_M: f64 = 50.0;
/// Laps kept per bin for the median (bounded memory).
pub const MAX_MAP_LAPS: usize = 5;
/// A lap value further than this (m) from the bin median is an outlier.
pub const OUTLIER_DISTANCE_M: f64 = 10.0;
/// A point further than this (m) from the midpoint of its neighbours is a spike.
pub const SPIKE_DISTANCE_M: f64 = 15.0;
/// Default smoothing half window (points) of the circular moving average.
pub const DEFAULT_SMOOTHING_HALF_WINDOW: usize = 2;
/// Largest allowed smoothing half window (points).
pub const MAX_SMOOTHING_HALF_WINDOW: usize = 20;
/// Share of distance bins that must have data to build a map.
pub const MIN_COVERAGE: f64 = 0.95;
/// Minimum number of points of a valid map.
pub const MIN_MAP_POINTS: usize = 20;
/// Maximum number of points of a valid map.
pub const MAX_MAP_POINTS: usize = 100_000;
/// Largest gap (m) between the last and the first point of a closed circuit.
pub const MAX_CLOSURE_GAP_M: f64 = 60.0;
/// Largest gap (m) between consecutive points.
pub const MAX_POINT_GAP_M: f64 = 250.0;
/// Coordinates beyond this magnitude (m) are rejected (registry `vehicle.pos_*` range).
pub const MAX_WORLD_COORD_M: f64 = 100_000.0;
/// Maximum number of sector marks.
pub const MAX_SECTOR_MARKS: usize = 64;
/// Segments searched on each side of the hint before a full projection search.
pub const PROJECTION_HINT_WINDOW: usize = 40;
/// A hinted projection closer than this (m) to the line is accepted without a full search.
pub const PROJECTION_HINT_ACCEPT_M: f64 = 30.0;
/// Largest map file accepted by the store (bytes).
pub const MAX_MAP_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Longest normalised track name in a [`TrackId`].
pub const MAX_TRACK_NAME_CHARS: usize = 64;

/// Track engine errors.
#[derive(Debug, Error)]
pub enum TrackError {
    /// The track name is empty after normalisation.
    #[error("track name is empty")]
    EmptyName,
    /// The track length is missing, not finite or implausible.
    #[error("track length {0} m is not plausible")]
    ImplausibleLength(f64),
    /// A track id string is not of the form `NAME_LENGTH`.
    #[error("invalid track id {0:?}")]
    InvalidId(String),
    /// A builder option is out of range.
    #[error("invalid option: {0}")]
    InvalidOption(&'static str),
    /// Not enough clean data to build a map.
    #[error("not enough data: {laps} laps, {coverage_pct:.1} % of the lap covered")]
    InsufficientData {
        /// Laps collected.
        laps: usize,
        /// Covered share of the lap (%).
        coverage_pct: f64,
    },
    /// The map failed validation.
    #[error("invalid track map: {0}")]
    InvalidMap(String),
    /// The map file uses an unknown format version.
    #[error("unsupported track map version {0}")]
    UnsupportedVersion(u32),
    /// The map file is too large.
    #[error("track map file too large: {0} bytes")]
    FileTooLarge(u64),
    /// File system error.
    #[error("track map file {path}: {source}")]
    Io {
        /// File.
        path: PathBuf,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// JSON error.
    #[error("track map json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Layout-unique track id: normalised LMU track name plus the track length in metres.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TrackId(String);

impl TrackId {
    /// From the LMU track name and track length.
    pub fn from_lmu(track_name: &str, track_length_m: f64) -> Result<Self, TrackError> {
        if !(track_length_m.is_finite()
            && (MIN_TRACK_LENGTH_M..=MAX_TRACK_LENGTH_M).contains(&track_length_m))
        {
            return Err(TrackError::ImplausibleLength(track_length_m));
        }
        let mut name = String::with_capacity(track_name.len());
        for c in track_name.chars() {
            if c.is_ascii_alphanumeric() {
                name.push(c.to_ascii_uppercase());
            } else if !name.ends_with('_') && !name.is_empty() {
                name.push('_');
            }
        }
        let name: String = name
            .trim_end_matches('_')
            .chars()
            .take(MAX_TRACK_NAME_CHARS)
            .collect();
        let name = name.trim_end_matches('_');
        if name.is_empty() {
            return Err(TrackError::EmptyName);
        }
        // Rounded to the metre: the identity of the layout, not a measurement.
        let metres = track_length_m.round() as u32;
        Ok(Self(format!("{name}_{metres}")))
    }

    /// The id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_valid(s: &str) -> bool {
        let Some((name, len)) = s.rsplit_once('_') else {
            return false;
        };
        !name.is_empty()
            && name.len() <= MAX_TRACK_NAME_CHARS
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && !len.is_empty()
            && len.chars().all(|c| c.is_ascii_digit())
            && len.len() <= 6
    }
}

impl TryFrom<String> for TrackId {
    type Error = TrackError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        if Self::is_valid(&s) {
            Ok(Self(s))
        } else {
            Err(TrackError::InvalidId(s))
        }
    }
}

impl From<TrackId> for String {
    fn from(id: TrackId) -> Self {
        id.0
    }
}

impl std::fmt::Display for TrackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where map geometry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackMapSource {
    /// Generated from recorded laps.
    Generated,
    /// Checked by hand; preferred over generated geometry.
    Verified,
}

/// One point of the circuit line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapPoint {
    /// Lap distance (m).
    pub dist_m: f64,
    /// World x (m).
    pub x: f64,
    /// World z (m).
    pub z: f64,
}

/// A marked location on the circuit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapMarker {
    /// Lap distance (m).
    pub dist_m: f64,
    /// World x (m).
    pub x: f64,
    /// World z (m).
    pub z: f64,
}

/// The start of a sector.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectorMark {
    /// Zero-based index of the sector that starts here (sector 0 starts at the line).
    pub index: u32,
    /// Lap distance (m).
    pub dist_m: f64,
    /// World x (m).
    pub x: f64,
    /// World z (m).
    pub z: f64,
}

/// Axis-aligned bounds of the map (m).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapBounds {
    /// Smallest x.
    pub min_x: f64,
    /// Largest x.
    pub max_x: f64,
    /// Smallest z.
    pub min_z: f64,
    /// Largest z.
    pub max_z: f64,
}

/// Versioned track map (spec §156).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMap {
    /// Layout-unique track id.
    pub track_id: TrackId,
    /// File format version ([`TRACK_MAP_FORMAT_VERSION`]).
    pub version: u32,
    /// Generated or verified.
    pub source: TrackMapSource,
    /// Lap length (m).
    pub length_m: f64,
    /// Circuit line, ordered by lap distance.
    pub points: Vec<MapPoint>,
    /// Sector starts after the line, ordered by distance.
    pub sectors: Vec<SectorMark>,
    /// Start/finish line.
    pub start_finish: MapMarker,
    /// Pit-lane entry, if known.
    pub pit_entry: Option<MapMarker>,
    /// Pit-lane exit, if known.
    pub pit_exit: Option<MapMarker>,
    /// Laps the geometry was generated from (0 for a verified map).
    pub generated_from_laps: u32,
    /// [`CALCULATION_ENGINE_VERSION`] of the generator.
    pub engine_version: u32,
}

/// A world position projected onto the circuit line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Projection {
    /// Lap distance of the nearest point on the line (m).
    pub dist_m: f64,
    /// Lap progress 0..1.
    pub progress: f64,
    /// Signed distance from the line (m): positive to the left of the driving direction in the
    /// right-handed `(x, z)` top view.
    pub lateral_offset_m: f64,
    /// Nearest point on the line.
    pub x: f64,
    /// Nearest point on the line.
    pub z: f64,
    /// Segment index (hint for the next projection).
    pub segment: usize,
    /// Zero-based sector at the projected distance.
    pub sector: u32,
}

impl TrackMap {
    /// Validates the geometry (spec §156 "validate map files").
    pub fn validate(&self) -> Result<(), TrackError> {
        let bad = |m: String| Err(TrackError::InvalidMap(m));
        if self.version != TRACK_MAP_FORMAT_VERSION {
            return Err(TrackError::UnsupportedVersion(self.version));
        }
        if !(self.length_m.is_finite()
            && (MIN_TRACK_LENGTH_M..=MAX_TRACK_LENGTH_M).contains(&self.length_m))
        {
            return Err(TrackError::ImplausibleLength(self.length_m));
        }
        let n = self.points.len();
        if !(MIN_MAP_POINTS..=MAX_MAP_POINTS).contains(&n) {
            return bad(format!("{n} points"));
        }
        let coord_ok = |x: f64, z: f64| {
            x.is_finite()
                && z.is_finite()
                && x.abs() <= MAX_WORLD_COORD_M
                && z.abs() <= MAX_WORLD_COORD_M
        };
        let mut prev: Option<&MapPoint> = None;
        for (i, p) in self.points.iter().enumerate() {
            if !coord_ok(p.x, p.z) || !p.dist_m.is_finite() {
                return bad(format!("point {i} not finite or out of range"));
            }
            if p.dist_m < 0.0 || p.dist_m > self.length_m {
                return bad(format!("point {i} distance {} outside the lap", p.dist_m));
            }
            if let Some(q) = prev {
                if p.dist_m <= q.dist_m {
                    return bad(format!("point {i} distance not increasing"));
                }
                if planar(q.x, q.z, p.x, p.z) > MAX_POINT_GAP_M {
                    return bad(format!("gap before point {i}"));
                }
            }
            prev = Some(p);
        }
        if let (Some(first), Some(last)) = (self.points.first(), self.points.last())
            && planar(last.x, last.z, first.x, first.z) > MAX_CLOSURE_GAP_M
        {
            return bad("circuit is not closed".into());
        }
        if self.sectors.len() > MAX_SECTOR_MARKS {
            return bad(format!("{} sector marks", self.sectors.len()));
        }
        let mut last_dist = 0.0;
        for s in &self.sectors {
            if !coord_ok(s.x, s.z) || !(s.dist_m > last_dist && s.dist_m < self.length_m) {
                return bad(format!("sector mark {} out of order or range", s.index));
            }
            last_dist = s.dist_m;
        }
        for (name, m) in [
            ("start/finish", Some(&self.start_finish)),
            ("pit entry", self.pit_entry.as_ref()),
            ("pit exit", self.pit_exit.as_ref()),
        ] {
            if let Some(m) = m
                && (!coord_ok(m.x, m.z) || !m.dist_m.is_finite())
            {
                return bad(format!("{name} marker not finite"));
            }
        }
        Ok(())
    }

    /// Bounds of the circuit line; `None` for an empty map.
    pub fn bounds(&self) -> Option<MapBounds> {
        let first = self.points.first()?;
        let mut b = MapBounds {
            min_x: first.x,
            max_x: first.x,
            min_z: first.z,
            max_z: first.z,
        };
        for p in &self.points {
            b.min_x = b.min_x.min(p.x);
            b.max_x = b.max_x.max(p.x);
            b.min_z = b.min_z.min(p.z);
            b.max_z = b.max_z.max(p.z);
        }
        Some(b)
    }

    /// Zero-based sector at lap distance `dist_m`.
    pub fn sector_at(&self, dist_m: f64) -> u32 {
        let d = self.wrap(dist_m);
        self.sectors
            .iter()
            .filter(|s| s.dist_m <= d)
            .map(|s| s.index)
            .next_back()
            .unwrap_or(0)
    }

    /// World position on the line at lap distance `dist_m` (wrapped into the lap), e.g. for another car
    /// known only by its scoring lap distance. `None` for a non-finite distance or an empty map.
    pub fn position_at(&self, dist_m: f64) -> Option<(f64, f64)> {
        if !dist_m.is_finite() {
            return None;
        }
        let d = self.wrap(dist_m);
        let n = self.points.len();
        let first = self.points.first()?;
        let last = self.points.last()?;
        let idx = self.points.partition_point(|p| p.dist_m <= d);
        let (a, b, a_dist, b_dist) = if idx == 0 {
            // Before the first point: on the closing segment, measured from the previous lap.
            (last, first, last.dist_m - self.length_m, first.dist_m)
        } else if idx == n {
            (last, first, last.dist_m, first.dist_m + self.length_m)
        } else {
            let a = &self.points[idx - 1];
            let b = &self.points[idx];
            (a, b, a.dist_m, b.dist_m)
        };
        Some((
            lerp(a_dist, a.x, b_dist, b.x, d),
            lerp(a_dist, a.z, b_dist, b.z, d),
        ))
    }

    /// Projects a world position onto the line. `hint` is the segment of the previous projection of the
    /// same car: the search starts around it and falls back to the whole line when the result is far away.
    pub fn project(&self, x: f64, z: f64, hint: Option<usize>) -> Option<Projection> {
        if !(x.is_finite() && z.is_finite()) || self.points.len() < 2 {
            return None;
        }
        let n = self.points.len();
        let mut best = None;
        if let Some(h) = hint.filter(|h| *h < n) {
            for k in 0..=2 * PROJECTION_HINT_WINDOW {
                let i = (h + n + k)
                    .checked_sub(PROJECTION_HINT_WINDOW)
                    .map(|v| v % n);
                if let Some(i) = i {
                    best = closer(best, self.project_on_segment(i, x, z));
                }
            }
            if best.is_some_and(|b: SegmentHit| b.distance_m <= PROJECTION_HINT_ACCEPT_M) {
                return best.map(|b| self.to_projection(b));
            }
        }
        for i in 0..n {
            best = closer(best, self.project_on_segment(i, x, z));
        }
        best.map(|b| self.to_projection(b))
    }

    /// The projected lap distance as a metric (`Derived`, [`formula::TRACK_PROJECTION`]).
    pub fn projected_distance(
        &self,
        x: Option<f64>,
        z: Option<f64>,
        hint: Option<usize>,
    ) -> Metric<f64> {
        let v = finite(x)
            .zip(finite(z))
            .and_then(|(x, z)| self.project(x, z, hint))
            .map(|p| p.dist_m);
        Metric::derived(formula::TRACK_PROJECTION, v)
    }

    fn wrap(&self, dist_m: f64) -> f64 {
        dist_m.rem_euclid(self.length_m)
    }

    /// Segment `i` runs from point `i` to point `i + 1`; the last one closes the circuit.
    fn project_on_segment(&self, i: usize, x: f64, z: f64) -> Option<SegmentHit> {
        let n = self.points.len();
        let a = self.points.get(i)?;
        let b = self.points.get((i + 1) % n)?;
        let b_dist = if i + 1 == n {
            b.dist_m + self.length_m
        } else {
            b.dist_m
        };
        let (dx, dz) = (b.x - a.x, b.z - a.z);
        let len2 = dx * dx + dz * dz;
        let t = if len2 > 0.0 {
            (((x - a.x) * dx + (z - a.z) * dz) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (px, pz) = (a.x + t * dx, a.z + t * dz);
        let cross = dx * (z - a.z) - dz * (x - a.x);
        let distance_m = planar(px, pz, x, z);
        Some(SegmentHit {
            segment: i,
            distance_m,
            lateral_m: if cross >= 0.0 {
                distance_m
            } else {
                -distance_m
            },
            dist_m: a.dist_m + t * (b_dist - a.dist_m),
            x: px,
            z: pz,
        })
    }

    fn to_projection(&self, h: SegmentHit) -> Projection {
        let dist_m = self.wrap(h.dist_m);
        Projection {
            dist_m,
            progress: (dist_m / self.length_m).clamp(0.0, 1.0 - f64::EPSILON),
            lateral_offset_m: h.lateral_m,
            x: h.x,
            z: h.z,
            segment: h.segment,
            sector: self.sector_at(dist_m),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SegmentHit {
    segment: usize,
    distance_m: f64,
    lateral_m: f64,
    dist_m: f64,
    x: f64,
    z: f64,
}

fn closer(a: Option<SegmentHit>, b: Option<SegmentHit>) -> Option<SegmentHit> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.distance_m < a.distance_m { b } else { a }),
        (a, b) => a.or(b),
    }
}

fn planar(x0: f64, z0: f64, x1: f64, z1: f64) -> f64 {
    ((x1 - x0).powi(2) + (z1 - z0).powi(2)).sqrt()
}

/// World `(x, z)` to screen `(u, v)` (pixels, v down): `screen_right = +x`, `screen_up = +z` (A-LMU-025),
/// rotated by `rotation_rad` (counter-clockwise on screen) and fitted into the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapTransform {
    /// Pixels per metre.
    pub scale: f64,
    /// Rotation (rad).
    pub rotation_rad: f64,
    /// World rotation centre x.
    pub centre_x: f64,
    /// World rotation centre z.
    pub centre_z: f64,
    /// Screen x of the centre.
    pub offset_u: f64,
    /// Screen y of the centre.
    pub offset_v: f64,
}

impl MapTransform {
    /// Fits `map` into a `width × height` viewport with `padding` pixels on every side. `None` when the map
    /// is empty, degenerate or the viewport has no room.
    pub fn fit(
        map: &TrackMap,
        width: f64,
        height: f64,
        padding: f64,
        rotation_rad: f64,
    ) -> Option<Self> {
        let b = map.bounds()?;
        let (w, h) = (width - 2.0 * padding, height - 2.0 * padding);
        if !(w > 0.0 && h > 0.0 && rotation_rad.is_finite()) {
            return None;
        }
        let (cx, cz) = (0.5 * (b.min_x + b.max_x), 0.5 * (b.min_z + b.max_z));
        let (sin, cos) = rotation_rad.sin_cos();
        let mut max_u: f64 = 0.0;
        let mut max_v: f64 = 0.0;
        for p in &map.points {
            let (dx, dz) = (p.x - cx, p.z - cz);
            max_u = max_u.max((dx * cos - dz * sin).abs());
            max_v = max_v.max((dx * sin + dz * cos).abs());
        }
        let scale = (0.5 * w / max_u).min(0.5 * h / max_v);
        (scale.is_finite() && scale > 0.0).then_some(Self {
            scale,
            rotation_rad,
            centre_x: cx,
            centre_z: cz,
            offset_u: 0.5 * width,
            offset_v: 0.5 * height,
        })
    }

    /// Screen position of a world position.
    pub fn apply(&self, x: f64, z: f64) -> (f64, f64) {
        let (sin, cos) = self.rotation_rad.sin_cos();
        let (dx, dz) = (x - self.centre_x, z - self.centre_z);
        let ru = dx * cos - dz * sin;
        let rv = dx * sin + dz * cos;
        (
            self.offset_u + self.scale * ru,
            self.offset_v - self.scale * rv,
        )
    }
}

/// Map generation options (spec §155: smoothing, scaling, sector and start/finish placement).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildOptions {
    /// Distance bin (m).
    pub bin_m: f64,
    /// Smoothing half window (points); 0 = none.
    pub smoothing_half_window: usize,
    /// Laps needed before a map is built.
    pub min_laps: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            bin_m: DEFAULT_BIN_M,
            smoothing_half_window: DEFAULT_SMOOTHING_HALF_WINDOW,
            min_laps: 1,
        }
    }
}

/// One sample for the map builder.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrackSample {
    /// Lap distance (m).
    pub lap_dist_m: Option<f64>,
    /// World x (m).
    pub x: Option<f64>,
    /// World z (m).
    pub z: Option<f64>,
    /// In the pit lane.
    pub in_pit_lane: Option<bool>,
    /// Zero-based sector index.
    pub sector_index: Option<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
struct BinAcc {
    sx: f64,
    sz: f64,
    sd: f64,
    n: u32,
}

/// Mean position and lap distance of one lap's samples in one bin.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BinValue {
    x: f64,
    z: f64,
    d: f64,
}

/// Generates a [`TrackMap`] from eligible laps. Memory is bounded by the bin count times
/// [`MAX_MAP_LAPS`].
#[derive(Debug, Clone)]
pub struct TrackMapBuilder {
    track_id: TrackId,
    length_m: f64,
    options: BuildOptions,
    current: Vec<BinAcc>,
    laps: Vec<Vec<BinValue>>,
    lap_count: usize,
    current_sector_marks: Vec<(u32, f64)>,
    sector_marks: Vec<Vec<f64>>,
    prev_dist: Option<f64>,
    prev_sector: Option<u8>,
    prev_in_pit: Option<bool>,
    pit_entry: Option<MapMarker>,
    pit_exit: Option<MapMarker>,
}

impl TrackMapBuilder {
    /// A builder for `track_id` with lap length `length_m`.
    pub fn new(
        track_id: TrackId,
        length_m: f64,
        options: BuildOptions,
    ) -> Result<Self, TrackError> {
        if !(length_m.is_finite() && (MIN_TRACK_LENGTH_M..=MAX_TRACK_LENGTH_M).contains(&length_m))
        {
            return Err(TrackError::ImplausibleLength(length_m));
        }
        if !(options.bin_m.is_finite() && (MIN_BIN_M..=MAX_BIN_M).contains(&options.bin_m)) {
            return Err(TrackError::InvalidOption("bin_m"));
        }
        if options.smoothing_half_window > MAX_SMOOTHING_HALF_WINDOW {
            return Err(TrackError::InvalidOption("smoothing_half_window"));
        }
        if options.min_laps == 0 || options.min_laps > MAX_MAP_LAPS {
            return Err(TrackError::InvalidOption("min_laps"));
        }
        let bins = (length_m / options.bin_m).ceil() as usize;
        Ok(Self {
            track_id,
            length_m,
            options,
            current: vec![BinAcc::default(); bins],
            laps: vec![Vec::new(); bins],
            lap_count: 0,
            current_sector_marks: Vec::new(),
            sector_marks: Vec::new(),
            prev_dist: None,
            prev_sector: None,
            prev_in_pit: None,
            pit_entry: None,
            pit_exit: None,
        })
    }

    /// Eligible laps collected so far.
    pub fn laps_collected(&self) -> usize {
        self.lap_count
    }

    /// Feeds one sample of the lap in progress.
    pub fn add_sample(&mut self, s: &TrackSample) {
        let pos = finite(s.x)
            .zip(finite(s.z))
            .filter(|(x, z)| x.abs() <= MAX_WORLD_COORD_M && z.abs() <= MAX_WORLD_COORD_M);
        // Negative before the first crossing (A-LMU-070); beyond the length right at the line.
        let dist = finite(s.lap_dist_m).filter(|d| (0.0..self.length_m).contains(d));
        if let (Some(pit), Some(prev_pit), Some((x, z)), Some(d)) =
            (s.in_pit_lane, self.prev_in_pit, pos, dist)
        {
            let marker = MapMarker { dist_m: d, x, z };
            if pit && !prev_pit {
                self.pit_entry = Some(marker);
            } else if !pit && prev_pit {
                self.pit_exit = Some(marker);
            }
        }
        if s.in_pit_lane.is_some() {
            self.prev_in_pit = s.in_pit_lane;
        }
        if s.in_pit_lane == Some(true) {
            return;
        }
        let (Some((x, z)), Some(d)) = (pos, dist) else {
            return;
        };
        if let (Some(k), Some(pk), Some(pd)) = (s.sector_index, self.prev_sector, self.prev_dist)
            && k == pk.saturating_add(1)
            && k > 0
        {
            // Boundary between the two samples.
            self.current_sector_marks
                .push((u32::from(k), 0.5 * (pd + d)));
        }
        if s.sector_index.is_some() {
            self.prev_sector = s.sector_index;
        }
        self.prev_dist = Some(d);
        let idx = ((d / self.options.bin_m) as usize).min(self.current.len().saturating_sub(1));
        if let Some(b) = self.current.get_mut(idx) {
            b.sx += x;
            b.sz += z;
            b.sd += d;
            b.n = b.n.saturating_add(1);
        }
    }

    /// Ends the lap in progress; its samples are kept only when `eligible` (a clean, complete lap).
    pub fn lap_completed(&mut self, eligible: bool) {
        if eligible {
            for (acc, lap_values) in self.current.iter().zip(self.laps.iter_mut()) {
                if acc.n > 0 {
                    let n = f64::from(acc.n);
                    if lap_values.len() == MAX_MAP_LAPS {
                        lap_values.remove(0);
                    }
                    lap_values.push(BinValue {
                        x: acc.sx / n,
                        z: acc.sz / n,
                        d: acc.sd / n,
                    });
                }
            }
            let mut marks: Vec<f64> = Vec::new();
            for (k, d) in self.current_sector_marks.drain(..) {
                let i = usize::try_from(k).unwrap_or(usize::MAX);
                if i < MAX_SECTOR_MARKS {
                    if marks.len() < i {
                        marks.resize(i, f64::NAN);
                    }
                    if marks.len() == i {
                        marks.push(d);
                    }
                }
            }
            if self.sector_marks.len() == MAX_MAP_LAPS {
                self.sector_marks.remove(0);
            }
            self.sector_marks.push(marks);
            self.lap_count = self.lap_count.saturating_add(1);
        }
        self.current.iter_mut().for_each(|b| *b = BinAcc::default());
        self.current_sector_marks.clear();
        self.prev_dist = None;
        self.prev_sector = None;
    }

    /// Builds and validates the map.
    pub fn build(&self) -> Result<TrackMap, TrackError> {
        let bins = self.laps.len();
        let covered = self.laps.iter().filter(|v| !v.is_empty()).count();
        let coverage = if bins > 0 {
            covered as f64 / bins as f64
        } else {
            0.0
        };
        if self.lap_count < self.options.min_laps || coverage < MIN_COVERAGE {
            return Err(TrackError::InsufficientData {
                laps: self.lap_count,
                coverage_pct: 100.0 * coverage,
            });
        }
        // 1. Per bin: median over laps, outliers dropped, mean of the rest.
        // The point's distance is the mean distance of its samples, so position and distance agree.
        let mut raw: Vec<MapPoint> = Vec::with_capacity(covered);
        for values in &self.laps {
            let Some((mx, mz)) = median_xz(values) else {
                continue;
            };
            let kept: Vec<&BinValue> = values
                .iter()
                .filter(|v| planar(v.x, v.z, mx, mz) <= OUTLIER_DISTANCE_M)
                .collect();
            if kept.is_empty() {
                continue;
            }
            let k = kept.len() as f64;
            raw.push(MapPoint {
                dist_m: kept.iter().map(|v| v.d).sum::<f64>() / k,
                x: kept.iter().map(|v| v.x).sum::<f64>() / k,
                z: kept.iter().map(|v| v.z).sum::<f64>() / k,
            });
        }
        // 2. Spikes: a point far from the midpoint of its neighbours.
        let n = raw.len();
        let mut points: Vec<MapPoint> = Vec::with_capacity(n);
        for i in 0..n {
            let prev = &raw[(i + n - 1) % n];
            let next = &raw[(i + 1) % n];
            let p = &raw[i];
            let mid = (0.5 * (prev.x + next.x), 0.5 * (prev.z + next.z));
            if planar(p.x, p.z, mid.0, mid.1) <= SPIKE_DISTANCE_M {
                points.push(*p);
            } else {
                debug!(
                    component = COMPONENT,
                    dist_m = p.dist_m,
                    "map spike removed"
                );
            }
        }
        // 3. Smoothing: circular moving average of the positions.
        let points = smooth(&points, self.options.smoothing_half_window);
        let mut map = TrackMap {
            track_id: self.track_id.clone(),
            version: TRACK_MAP_FORMAT_VERSION,
            source: TrackMapSource::Generated,
            length_m: self.length_m,
            start_finish: MapMarker {
                dist_m: 0.0,
                x: 0.0,
                z: 0.0,
            },
            points,
            sectors: Vec::new(),
            pit_entry: self.pit_entry,
            pit_exit: self.pit_exit,
            generated_from_laps: u32::try_from(self.lap_count).unwrap_or(u32::MAX),
            engine_version: CALCULATION_ENGINE_VERSION,
        };
        if let Some((x, z)) = map.position_at(0.0) {
            map.start_finish = MapMarker { dist_m: 0.0, x, z };
        }
        // 4. Sector starts: median over laps.
        let max_marks = self.sector_marks.iter().map(Vec::len).max().unwrap_or(0);
        for i in 1..max_marks {
            let mut ds: Vec<f64> = self
                .sector_marks
                .iter()
                .filter_map(|m| m.get(i).copied())
                .filter(|d| d.is_finite())
                .collect();
            let Some(d) = median(&mut ds) else { continue };
            if let Some((x, z)) = map.position_at(d) {
                map.sectors.push(SectorMark {
                    index: u32::try_from(i).unwrap_or(u32::MAX),
                    dist_m: d,
                    x,
                    z,
                });
            }
        }
        map.validate()?;
        info!(
            component = COMPONENT,
            track = %map.track_id,
            points = map.points.len(),
            laps = self.lap_count,
            "track map generated"
        );
        Ok(map)
    }
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        0.5 * (v[m - 1] + v[m])
    } else {
        v[m]
    })
}

fn median_xz(values: &[BinValue]) -> Option<(f64, f64)> {
    let mut xs: Vec<f64> = values.iter().map(|p| p.x).collect();
    let mut zs: Vec<f64> = values.iter().map(|p| p.z).collect();
    Some((median(&mut xs)?, median(&mut zs)?))
}

fn smooth(points: &[MapPoint], half: usize) -> Vec<MapPoint> {
    let n = points.len();
    if half == 0 || n < 2 * half + 1 {
        return points.to_vec();
    }
    let w = (2 * half + 1) as f64;
    (0..n)
        .map(|i| {
            let (mut sx, mut sz) = (0.0, 0.0);
            for k in 0..=2 * half {
                let p = &points[(i + n + k - half) % n];
                sx += p.x;
                sz += p.z;
            }
            MapPoint {
                dist_m: points[i].dist_m,
                x: sx / w,
                z: sz / w,
            }
        })
        .collect()
}

/// Directory-scoped cache of track map files (spec §155: "store map files under `data/tracks/` or another
/// clearly documented location"; the caller chooses the directory). Files are `<TRACK_ID>.json` for
/// generated maps and `<TRACK_ID>.verified.json` for hand-checked maps, which win on load. Writes are
/// atomic (temporary file and rename).
#[derive(Debug, Clone)]
pub struct TrackMapStore {
    dir: PathBuf,
}

impl TrackMapStore {
    /// A store over `dir` (created on the first save).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// File of a generated (`verified == false`) or verified map.
    pub fn path_for(&self, id: &TrackId, verified: bool) -> PathBuf {
        let suffix = if verified { ".verified.json" } else { ".json" };
        self.dir.join(format!("{}{suffix}", id.as_str()))
    }

    /// Loads the map of `id`, preferring the verified file. `Ok(None)` when neither exists. A file that
    /// fails validation or belongs to another track is an error.
    pub fn load(&self, id: &TrackId) -> Result<Option<TrackMap>, TrackError> {
        for verified in [true, false] {
            let path = self.path_for(id, verified);
            let meta = match fs::metadata(&path) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(TrackError::Io { path, source }),
            };
            if meta.len() > MAX_MAP_FILE_BYTES {
                return Err(TrackError::FileTooLarge(meta.len()));
            }
            let bytes = fs::read(&path).map_err(|source| TrackError::Io {
                path: path.clone(),
                source,
            })?;
            let map: TrackMap = serde_json::from_slice(&bytes)?;
            map.validate()?;
            if &map.track_id != id {
                return Err(TrackError::InvalidMap(format!(
                    "{} holds track {}",
                    path.display(),
                    map.track_id
                )));
            }
            debug!(component = COMPONENT, path = %path.display(), "track map loaded");
            return Ok(Some(map));
        }
        Ok(None)
    }

    /// Validates and saves `map` (to the verified file when its source is `Verified`). Returns the path.
    pub fn save(&self, map: &TrackMap) -> Result<PathBuf, TrackError> {
        map.validate()?;
        let path = self.path_for(&map.track_id, map.source == TrackMapSource::Verified);
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |source| TrackError::Io { path, source }
        };
        fs::create_dir_all(&self.dir).map_err(io(&self.dir))?;
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(map)?;
        {
            let mut f = fs::File::create(&tmp).map_err(io(&tmp))?;
            f.write_all(&json).map_err(io(&tmp))?;
            f.sync_all().map_err(io(&tmp))?;
        }
        if let Err(source) = fs::rename(&tmp, &path) {
            if let Err(e) = fs::remove_file(&tmp) {
                warn!(component = COMPONENT, error = %e, "temporary map file not removed");
            }
            return Err(TrackError::Io { path, source });
        }
        info!(component = COMPONENT, path = %path.display(), "track map saved");
        Ok(path)
    }
}

#[cfg(test)]
mod tests;
