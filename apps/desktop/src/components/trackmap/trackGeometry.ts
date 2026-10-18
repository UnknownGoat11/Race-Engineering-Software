/**
 * Geometric operations, path generation, projection, and online track geometry reconstruction.
 * Mirrors `crates/race-engines/src/track.rs`.
 */
import { classColorVar } from '../../lib/classes';
import { applyTransform } from './transform';
import type { MapPoint, MapTransform, TrackLengthSource, TrackMap, VehicleClass } from './types';

/**
 * Linearly interpolates value between (x0, y0) and (x1, y1) at x.
 */
function lerp(x0: number, y0: number, x1: number, y1: number, x: number): number {
  if (Math.abs(x1 - x0) < 1e-9) {
    return y0;
  }
  const t = (x - x0) / (x1 - x0);
  return y0 + t * (y1 - y0);
}

/**
 * Wraps distance into [0, lengthM).
 */
export function wrapDistance(distM: number, lengthM: number): number {
  if (lengthM <= 0 || !Number.isFinite(distM)) {
    return 0;
  }
  const rem = distM % lengthM;
  return rem < 0 ? rem + lengthM : rem;
}

/**
 * Computes world position (x, z) on the circuit line at lap distance `distM`.
 * Returns null if distance is not finite or map has fewer than 2 points.
 */
export function positionAt(map: TrackMap, distM: number): [number, number] | null {
  if (!Number.isFinite(distM) || map.points.length < 2) {
    return null;
  }
  const d = wrapDistance(distM, map.lengthM);
  const points = map.points;
  const n = points.length;
  const first = points[0];
  const last = points[n - 1];

  if (!first || !last) {
    return null;
  }

  // Binary search for first point with distM > d
  let low = 0;
  let high = n;
  while (low < high) {
    const mid = (low + high) >>> 1;
    const pt = points[mid];
    if (pt && pt.distM <= d) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }

  let a: MapPoint;
  let b: MapPoint;
  let aDist: number;
  let bDist: number;

  if (low === 0) {
    // Before first point: closing segment from previous lap
    a = last;
    b = first;
    aDist = last.distM - map.lengthM;
    bDist = first.distM;
  } else if (low === n) {
    // After last point: closing segment into next lap
    a = last;
    b = first;
    aDist = last.distM;
    bDist = first.distM + map.lengthM;
  } else {
    const prevPt = points[low - 1];
    const currPt = points[low];
    if (!prevPt || !currPt) {
      return null;
    }
    a = prevPt;
    b = currPt;
    aDist = a.distM;
    bDist = b.distM;
  }

  const px = lerp(aDist, a.x, bDist, b.x, d);
  const pz = lerp(aDist, a.z, bDist, b.z, d);
  return [px, pz];
}

/**
 * Generates an SVG path string from points transformed into viewport pixels.
 */
export function buildSvgPath(points: readonly MapPoint[], transform: MapTransform, closed = true): string {
  if (points.length < 2) {
    return '';
  }
  let path = '';
  for (const p of points) {
    const [u, v] = applyTransform(transform, p.x, p.z);
    if (path.length === 0) {
      path += `M ${u.toFixed(2)},${v.toFixed(2)}`;
    } else {
      path += ` L ${u.toFixed(2)},${v.toFixed(2)}`;
    }
  }
  if (closed && path.length > 0) {
    path += ' Z';
  }
  return path;
}

/**
 * Colour of a vehicle-class marker: the `--class-*` token from lib/classes (REDESIGN C1, owner approval
 * 2026-10-18). Returns a CSS `var(...)` usable as an SVG fill or a custom property; no hex lives here.
 */
export function classToColor(vehicleClass: VehicleClass): string {
  return classColorVar(vehicleClass);
}

export interface TrackGeometryBuilderOptions {
  /** Distance bin width in metres (default 25 m). */
  readonly binSizeM?: number;
  /** Minimum ratio of filled bins required before building/locking (default 0.9 = 90%). */
  readonly minBinsCoveredRatio?: number;
  /** Known or reported track length in metres. */
  readonly trackLengthM?: number | null;
  /** Circuit name or identifier. */
  readonly trackName?: string | null;
}

interface BinAccumulator {
  sumX: number;
  sumZ: number;
  count: number;
}

/**
 * Online Track Geometry Builder.
 * Reconstructs circuit geometry incrementally from player telemetry (POS_X/POS_Z + LAP_DISTANCE)
 * and competitor scoring frames (pos_x_m/pos_z_m + lap_dist_m).
 * Bins samples by lap distance, detects lap wrap-arounds, applies circular smoothing, and locks after
 * a complete valid lap.
 */
export class TrackGeometryBuilder {
  readonly binSizeM: number;
  readonly minBinsCoveredRatio: number;
  private trackLengthM: number | null = null;
  private lengthSource: TrackLengthSource = 'observed';
  private trackName = 'Live Circuit';
  private readonly bins = new Map<number, BinAccumulator>();
  private maxObservedDistM = 0;
  private lastIngestedDistM: number | null = null;
  private hasLapWrapped = false;
  private lockedMap: TrackMap | null = null;

  constructor(options: TrackGeometryBuilderOptions = {}) {
    this.binSizeM = options.binSizeM && options.binSizeM > 0 ? options.binSizeM : 25;
    this.minBinsCoveredRatio = options.minBinsCoveredRatio && options.minBinsCoveredRatio > 0
      ? options.minBinsCoveredRatio
      : 0.9;
    if (options.trackLengthM && options.trackLengthM > 0) {
      this.trackLengthM = options.trackLengthM;
      this.lengthSource = 'reported';
    }
    if (options.trackName) {
      this.trackName = options.trackName;
    }
  }

  /**
   * Sets reported track length (e.g. from TRACK_LENGTH channel or props).
   */
  setTrackLength(lengthM: number | null | undefined, source: TrackLengthSource = 'reported'): void {
    if (lengthM !== null && lengthM !== undefined && Number.isFinite(lengthM) && lengthM > 0) {
      this.trackLengthM = lengthM;
      this.lengthSource = source;
    }
  }

  /**
   * Sets circuit name / id.
   */
  setTrackName(name: string | null | undefined): void {
    if (name && name.trim().length > 0) {
      this.trackName = name.trim();
    }
  }

  /**
   * Returns current source of track length ('reported' vs 'observed').
   */
  getTrackLengthSource(): TrackLengthSource {
    return this.lengthSource;
  }

  /**
   * Returns current known or observed track length in metres.
   */
  getTrackLength(): number | null {
    if (this.trackLengthM && this.trackLengthM > 0) {
      return this.trackLengthM;
    }
    if (this.maxObservedDistM > 0) {
      return this.maxObservedDistM + this.binSizeM;
    }
    return null;
  }

  /**
   * Ingests a single position sample at a known lap distance.
   * Rejects non-finite coordinates or negative distances.
   */
  ingest(x: number, z: number, distM: number): boolean {
    if (!Number.isFinite(x) || !Number.isFinite(z) || !Number.isFinite(distM) || distM < 0) {
      return false;
    }

    if (this.lockedMap) {
      return true;
    }

    // Detect lap distance wrap-around (e.g. from 5980 m to 20 m)
    if (this.lastIngestedDistM !== null && this.lastIngestedDistM > 1000 && distM < this.lastIngestedDistM - 800) {
      this.hasLapWrapped = true;
      if (this.trackLengthM === null || this.lengthSource === 'observed') {
        const inferred = Math.max(this.trackLengthM ?? 0, this.lastIngestedDistM + 10);
        this.trackLengthM = inferred;
        this.lengthSource = 'observed';
      }
    }
    this.lastIngestedDistM = distM;

    if (distM > this.maxObservedDistM) {
      this.maxObservedDistM = distM;
    }

    let bDist = distM;
    if (this.trackLengthM && this.trackLengthM > 0) {
      bDist = distM % this.trackLengthM;
    }

    const binIdx = Math.floor(bDist / this.binSizeM);
    const existing = this.bins.get(binIdx);
    if (!existing) {
      this.bins.set(binIdx, { sumX: x, sumZ: z, count: 1 });
    } else {
      existing.sumX += x;
      existing.sumZ += z;
      existing.count += 1;
    }

    return true;
  }

  /**
   * Ingests multiple points at once.
   */
  ingestPoints(points: readonly { x: number; z: number; distM: number }[]): void {
    for (const p of points) {
      this.ingest(p.x, p.z, p.distM);
    }
  }

  /**
   * Returns whether circuit geometry has been completed and locked.
   */
  isLocked(): boolean {
    return this.lockedMap !== null;
  }

  /**
   * Returns current lap coverage progress [0, 1].
   */
  progress(): number {
    if (this.lockedMap) {
      return 1;
    }
    const len = this.getTrackLength();
    if (!len || len <= 0) {
      return 0;
    }
    const totalBins = Math.max(1, Math.ceil(len / this.binSizeM));
    let covered = 0;
    for (let i = 0; i < totalBins; i++) {
      if (this.bins.has(i)) {
        covered++;
      }
    }
    return Math.min(1, covered / totalBins);
  }

  /**
   * Attempts to build and lock the track map if sufficient lap coverage has been accumulated.
   * Returns null if coverage is insufficient.
   */
  build(): TrackMap | null {
    if (this.lockedMap) {
      return this.lockedMap;
    }

    const effectiveLength = this.getTrackLength();
    if (!effectiveLength || effectiveLength <= 0) {
      return null;
    }

    const totalBins = Math.ceil(effectiveLength / this.binSizeM);
    if (totalBins < 4) {
      return null;
    }

    let covered = 0;
    for (let i = 0; i < totalBins; i++) {
      if (this.bins.has(i)) {
        covered++;
      }
    }

    const ratio = covered / totalBins;
    const canLock = ratio >= this.minBinsCoveredRatio || (this.hasLapWrapped && ratio >= 0.85);
    if (!canLock) {
      return null;
    }

    // Collect all known bin averages
    const knownBins: { idx: number; x: number; z: number }[] = [];
    for (let i = 0; i < totalBins; i++) {
      const b = this.bins.get(i);
      if (b && b.count > 0) {
        knownBins.push({ idx: i, x: b.sumX / b.count, z: b.sumZ / b.count });
      }
    }

    if (knownBins.length < 3) {
      return null;
    }

    const lastKnown = knownBins[knownBins.length - 1];
    const firstKnown = knownBins[0];
    if (!lastKnown || !firstKnown) {
      return null;
    }

    // Interpolate points across all bins from 0 to totalBins - 1
    const rawPoints: MapPoint[] = [];
    for (let i = 0; i < totalBins; i++) {
      const b = this.bins.get(i);
      const distM = Math.min(effectiveLength - 1, i * this.binSizeM);
      if (b && b.count > 0) {
        rawPoints.push({ distM, x: b.sumX / b.count, z: b.sumZ / b.count });
      } else {
        // Find nearest preceding and succeeding known bins with circular wrap
        let prev = lastKnown;
        let next = firstKnown;
        for (const kb of knownBins) {
          if (kb.idx < i) {
            prev = kb;
          }
          if (kb.idx > i) {
            next = kb;
            break;
          }
        }
        let span = next.idx - prev.idx;
        let offset = i - prev.idx;
        if (span < 0) {
          span += totalBins;
          if (offset < 0) offset += totalBins;
        }
        const t = span > 0 ? offset / span : 0;
        const x = prev.x + t * (next.x - prev.x);
        const z = prev.z + t * (next.z - prev.z);
        rawPoints.push({ distM, x, z });
      }
    }

    // Circular smoothing: 2 passes of 3-point weighted moving average
    let smoothed: MapPoint[] = rawPoints.map((p) => ({ ...p }));
    const N = smoothed.length;
    for (let pass = 0; pass < 2; pass++) {
      const nextPass: MapPoint[] = [];
      for (let i = 0; i < N; i++) {
        const prev = smoothed[(i - 1 + N) % N];
        const curr = smoothed[i];
        const next = smoothed[(i + 1) % N];
        if (!prev || !curr || !next) continue;
        nextPass.push({
          distM: curr.distM,
          x: 0.25 * prev.x + 0.5 * curr.x + 0.25 * next.x,
          z: 0.25 * prev.z + 0.5 * curr.z + 0.25 * next.z,
        });
      }
      smoothed = nextPass;
    }

    const firstPoint = smoothed[0];
    if (!firstPoint) return null;

    const s1Dist = Math.round(effectiveLength / 3);
    const s2Dist = Math.round((2 * effectiveLength) / 3);
    const s1Idx = Math.min(N - 1, Math.floor(s1Dist / this.binSizeM));
    const s2Idx = Math.min(N - 1, Math.floor(s2Dist / this.binSizeM));
    const s1Pt = smoothed[s1Idx] ?? firstPoint;
    const s2Pt = smoothed[s2Idx] ?? firstPoint;

    const trackMap: TrackMap = {
      trackId: this.trackName,
      version: 1,
      source: 'generated',
      lengthM: Math.round(effectiveLength),
      points: smoothed,
      sectors: [
        { index: 1, distM: s1Dist, x: s1Pt.x, z: s1Pt.z },
        { index: 2, distM: s2Dist, x: s2Pt.x, z: s2Pt.z },
      ],
      startFinish: { distM: 0, x: firstPoint.x, z: firstPoint.z },
      pitEntry: null,
      pitExit: null,
      generatedFromLaps: 1,
    };

    this.lockedMap = trackMap;
    return trackMap;
  }

  /**
   * Resets all accumulated bins and unlocks the builder.
   */
  reset(): void {
    this.bins.clear();
    this.maxObservedDistM = 0;
    this.lastIngestedDistM = null;
    this.hasLapWrapped = false;
    this.lockedMap = null;
  }
}
