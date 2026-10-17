/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). SYNTHETIC track layout.
 *
 * A Sebring-like closed circuit (long back straight, hairpin, fast sweepers) built from hand-placed control
 * points through a closed Catmull-Rom spline, resampled every metre and scaled to `TRACK_LENGTH_M`. It is NOT
 * real circuit geometry: it only gives the synthetic car model plausible curvature (braking zones, corner
 * speeds) and the scoring feed plausible world coordinates.
 */

/** Displayed track name of the synthetic layout (every screen that shows it is labelled SYNTHETIC). */
export const TRACK_NAME = 'Sebring International Raceway';
/** Nominal lap length of the synthetic layout (m). */
export const TRACK_LENGTH_M = 6019;
/** Lap distances where sectors 2 and 3 start (m). */
export const SECTOR_STARTS_M: readonly [number, number] = [1985, 4130];

/** Control points (m) of the synthetic layout, clockwise, start/finish between the first two. */
const CONTROL_POINTS: readonly (readonly [number, number])[] = [
  [0, 0],
  [420, 0],
  [560, 60],
  [600, 180],
  [700, 260],
  [880, 250],
  [980, 330],
  [960, 470],
  [1080, 560],
  [1300, 560],
  [1420, 480],
  [1440, 340],
  [1380, 220],
  [1420, 100],
  [1560, 40],
  [1640, -80],
  [1600, -220],
  [1440, -300],
  [1100, -330],
  [700, -360],
  [300, -390],
  [60, -400],
  [-120, -350],
  [-180, -220],
  [-140, -90],
];

export interface TrackSample {
  readonly x: number;
  readonly z: number;
  /** Signed curvature (1/m), positive = left-hand corner. */
  readonly curvature: number;
}

function catmullRom(p0: number, p1: number, p2: number, p3: number, t: number): number {
  const t2 = t * t;
  const t3 = t2 * t;
  return 0.5 * (2 * p1 + (-p0 + p2) * t + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t2 + (-p0 + 3 * p1 - 3 * p2 + p3) * t3);
}

function point(i: number): readonly [number, number] {
  const n = CONTROL_POINTS.length;
  const p = CONTROL_POINTS[((i % n) + n) % n];
  if (p === undefined) throw new Error('control point index out of range');
  return p;
}

/** Dense spline polyline (unscaled). */
function densePolyline(stepsPerSegment: number): [number, number][] {
  const out: [number, number][] = [];
  for (let i = 0; i < CONTROL_POINTS.length; i++) {
    const p0 = point(i - 1);
    const p1 = point(i);
    const p2 = point(i + 1);
    const p3 = point(i + 2);
    for (let s = 0; s < stepsPerSegment; s++) {
      const t = s / stepsPerSegment;
      out.push([catmullRom(p0[0], p1[0], p2[0], p3[0], t), catmullRom(p0[1], p1[1], p2[1], p3[1], t)]);
    }
  }
  return out;
}

/** Track samples every metre of lap distance (index = metres from the start/finish line). */
function buildSamples(): TrackSample[] {
  const dense = densePolyline(200);
  // Cumulative length of the dense polyline.
  const cum: number[] = [0];
  for (let i = 1; i <= dense.length; i++) {
    const a = dense[i - 1];
    const b = dense[i % dense.length];
    if (a === undefined || b === undefined) throw new Error('polyline index out of range');
    cum.push((cum[i - 1] ?? 0) + Math.hypot(b[0] - a[0], b[1] - a[1]));
  }
  const rawLength = cum[cum.length - 1] ?? 1;
  const scale = TRACK_LENGTH_M / rawLength;

  const xs: number[] = [];
  const zs: number[] = [];
  let j = 0;
  for (let d = 0; d < TRACK_LENGTH_M; d++) {
    const target = d / scale;
    while (j < dense.length - 1 && (cum[j + 1] ?? Infinity) < target) j++;
    const a = dense[j];
    const b = dense[(j + 1) % dense.length];
    const c0 = cum[j] ?? 0;
    const c1 = cum[j + 1] ?? c0 + 1;
    if (a === undefined || b === undefined) throw new Error('polyline index out of range');
    const f = c1 > c0 ? (target - c0) / (c1 - c0) : 0;
    xs.push((a[0] + (b[0] - a[0]) * f) * scale);
    zs.push((a[1] + (b[1] - a[1]) * f) * scale);
  }

  // Curvature from the heading change over ±8 m, then smoothed over ±12 m.
  const n = xs.length;
  const at = (arr: readonly number[], i: number): number => arr[((i % n) + n) % n] ?? 0;
  const heading = (i: number): number => Math.atan2(at(zs, i + 1) - at(zs, i), at(xs, i + 1) - at(xs, i));
  const raw: number[] = [];
  const span = 8;
  for (let i = 0; i < n; i++) {
    let dh = heading(i + span) - heading(i - span);
    while (dh > Math.PI) dh -= 2 * Math.PI;
    while (dh < -Math.PI) dh += 2 * Math.PI;
    raw.push(dh / (2 * span));
  }
  const smooth = 12;
  const samples: TrackSample[] = [];
  for (let i = 0; i < n; i++) {
    let sum = 0;
    for (let k = -smooth; k <= smooth; k++) sum += at(raw, i + k);
    samples.push({ x: at(xs, i), z: at(zs, i), curvature: sum / (2 * smooth + 1) });
  }
  return samples;
}

/** One sample per metre of lap distance. */
export const TRACK_SAMPLES: readonly TrackSample[] = buildSamples();

/** World position (x, z) at lap distance `distM` (wrapped). */
export function trackPosition(distM: number): { x: number; z: number } {
  const n = TRACK_SAMPLES.length;
  const d = ((distM % n) + n) % n;
  const i = Math.floor(d);
  const f = d - i;
  const a = TRACK_SAMPLES[i];
  const b = TRACK_SAMPLES[(i + 1) % n];
  if (a === undefined || b === undefined) return { x: 0, z: 0 };
  return { x: a.x + (b.x - a.x) * f, z: a.z + (b.z - a.z) * f };
}

/** Sector index (0, 1, 2) at lap distance `distM`. */
export function sectorAt(distM: number): number {
  const d = ((distM % TRACK_LENGTH_M) + TRACK_LENGTH_M) % TRACK_LENGTH_M;
  if (d >= SECTOR_STARTS_M[1]) return 2;
  if (d >= SECTOR_STARTS_M[0]) return 1;
  return 0;
}
