/**
 * Geometric operations, path generation, and projection along circuit geometry.
 * Mirrors `crates/race-engines/src/track.rs`.
 */
import { applyTransform } from './transform';
import type { MapPoint, MapTransform, TrackMap, VehicleClass } from './types';

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
 * Color mapping for vehicle class markers on track maps.
 */
export function classToColor(vehicleClass: VehicleClass): string {
  switch (vehicleClass) {
    case 'hypercar':
      return '#ef4444'; // Red
    case 'lmp2':
      return '#3b82f6'; // Blue
    case 'gt3':
      return '#f97316'; // Orange
    case 'gte':
      return '#10b981'; // Green
    case 'unknown':
    default:
      return '#9ca3af'; // Muted Gray
  }
}
