/**
 * Scale computation: data extents, autoscale ranges, nice tick steps and value → pixel mappings. Pure functions;
 * ranges are computed in DISPLAY units of an axis (lib/units.ts `AxisDisplay.factor`) so ticks land on round
 * display values (e.g. 0, 50, 100 km/h).
 */
import type { XRange } from './types';

/** `px = value × scale + offset`. */
export interface Affine {
  readonly scale: number;
  readonly offset: number;
}

/** Ticks of an axis: values in display units, the step between them and the decimals their labels need. */
export interface Ticks {
  readonly values: readonly number[];
  readonly step: number;
  readonly decimals: number;
}

/** Mantissas of nice steps (1, 2, 5 × 10^k). */
const NICE_MANTISSAS: readonly number[] = [1, 2, 5, 10];
/** Clock-friendly steps in seconds from one second up (sub-second steps use 1-2-5). */
const CLOCK_STEPS_S: readonly number[] = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 14400, 21600, 43200, 86400];
/** Most decimals a tick label gets (steps below 1e-6 do not occur on telemetry axes). */
const MAX_TICK_DECIMALS = 6;
/** Fraction of |value| used as the half-span of a flat (constant) series' autoscale range. */
const FLAT_RANGE_FRACTION = 0.1;
/** Half-span of a flat autoscale range around zero, in display units. */
const FLAT_RANGE_AT_ZERO = 1;
/** Ticks are generated up to this many steps (guards against degenerate inputs). */
const MAX_TICKS = 1000;

/** Affine map sending `d0 → p0` and `d1 → p1` (`d0 !== d1`). */
export function linearAffine(d0: number, d1: number, p0: number, p1: number): Affine {
  const scale = (p1 - p0) / (d1 - d0);
  return { scale, offset: p0 - d0 * scale };
}

/** Inverse of an affine map (pixel → value). */
export function invertAffine(a: Affine): Affine {
  return { scale: 1 / a.scale, offset: -a.offset / a.scale };
}

/** Applies an affine map. */
export function applyAffine(a: Affine, v: number): number {
  return v * a.scale + a.offset;
}

/**
 * Min and max of the finite values in `values[start, end)` whose x (`xs[i]`) lies in `xRange` (when given);
 * null if there is none. NaN (gaps) are skipped, never treated as 0.
 */
export function dataExtent(
  values: ArrayLike<number>,
  start: number,
  end: number,
  xs?: ArrayLike<number>,
  xRange?: XRange,
): XRange | null {
  let min = Number.POSITIVE_INFINITY;
  let max = Number.NEGATIVE_INFINITY;
  for (let i = start; i < end; i++) {
    const v = values[i];
    if (v === undefined || !Number.isFinite(v)) continue;
    if (xs !== undefined && xRange !== undefined) {
      const x = xs[i];
      if (x === undefined || !(x >= xRange.min && x <= xRange.max)) continue;
    }
    if (v < min) min = v;
    if (v > max) max = v;
  }
  return min <= max ? { min, max } : null;
}

/** Union of two optional ranges. */
export function unionRange(a: XRange | null, b: XRange | null): XRange | null {
  if (a === null) return b;
  if (b === null) return a;
  return { min: Math.min(a.min, b.min), max: Math.max(a.max, b.max) };
}

/** Number of decimals that represents every multiple of `step` exactly enough for a label. */
export function decimalsForStep(step: number): number {
  if (!(step > 0) || !Number.isFinite(step)) return 0;
  // A step of 2.5e-k never occurs (mantissas are 1, 2, 5), so the exponent alone decides.
  const decimals = Math.ceil(-Math.log10(step) - 1e-9);
  return Math.min(MAX_TICK_DECIMALS, Math.max(0, decimals));
}

/** Smallest 1-2-5 × 10^k step that splits `span` into at most `targetCount` intervals. */
export function niceStep(span: number, targetCount: number): number {
  if (!(span > 0) || !Number.isFinite(span)) return 1;
  const raw = span / Math.max(1, targetCount);
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  for (const m of NICE_MANTISSAS) {
    if (m * magnitude >= raw * (1 - 1e-9)) return m * magnitude;
  }
  return 10 * magnitude;
}

/** Like `niceStep`, but steps of one second or more are clock-friendly (1, 2, 5, 10, 15, 30 s, 1, 2, 5 min, …). */
export function niceClockStep(span: number, targetCount: number): number {
  const step = niceStep(span, targetCount);
  if (step < 1) return step;
  const raw = span / Math.max(1, targetCount);
  for (const s of CLOCK_STEPS_S) {
    if (s >= raw * (1 - 1e-9)) return s;
  }
  return step;
}

/** Multiples of `step` within `[min, max]`, with the decimals their labels need. */
export function ticksForStep(min: number, max: number, step: number): Ticks {
  const values: number[] = [];
  if (!(step > 0) || !Number.isFinite(min) || !Number.isFinite(max) || max < min) {
    return { values, step, decimals: 0 };
  }
  const first = Math.ceil(min / step - 1e-9);
  const last = Math.floor(max / step + 1e-9);
  const decimals = decimalsForStep(step);
  for (let k = first; k <= last && values.length < MAX_TICKS; k++) {
    // Rounding to the label precision removes binary artefacts (0.30000000000000004).
    values.push(Number((k * step).toFixed(decimals)));
  }
  return { values, step, decimals };
}

/** Nice ticks for `[min, max]` (about `targetCount` intervals). */
export function niceTicks(min: number, max: number, targetCount: number): Ticks {
  return ticksForStep(min, max, niceStep(max - min, targetCount));
}

/**
 * Autoscale range for a data extent in display units: widened to whole multiples of a nice tick step so the
 * axis starts and ends on a labelled tick. A flat extent (constant value) gets a span around its value; no data
 * gives null (the axis then shows no scale rather than an invented one).
 */
export function autoRange(extent: XRange | null, targetTicks: number): XRange | null {
  if (extent === null || !Number.isFinite(extent.min) || !Number.isFinite(extent.max)) return null;
  let { min, max } = extent;
  if (min === max) {
    const half = min === 0 ? FLAT_RANGE_AT_ZERO : Math.abs(min) * FLAT_RANGE_FRACTION;
    min -= half;
    max += half;
  }
  const step = niceStep(max - min, targetTicks);
  const decimals = decimalsForStep(step);
  return {
    min: Number((Math.floor(min / step + 1e-9) * step).toFixed(decimals)),
    max: Number((Math.ceil(max / step - 1e-9) * step).toFixed(decimals)),
  };
}

/** Clamps `v` into `[lo, hi]`. */
export function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}
