/**
 * Cursor lookups on a graph window (spec §81 readouts, §125 interpolation policy).
 *
 * `sampleAt` generalises `TelemetryRingBuffer.sampleAtTime` (ring.ts, mirroring Rust `sample_at_time`) from time
 * to any x: it finds the rows bracketing x, estimates the time at x between them, and then applies the ring's
 * rules — Continuous channels are interpolated only between two valid rows at most `maxGapT` apart; Discrete
 * channels return the last row at or before x (never interpolated), only while that row is at most `maxGapT`
 * older; next to a gap, across a longer gap or outside the rows the result is null. On a time axis it gives
 * exactly `sampleAtTime`'s answers (tested). When x is crossed more than once (non-monotonic lap axes) the
 * newest crossing wins.
 */
import type { GraphWindow } from './types';

/** First row index with `xs[i] > x` in a non-decreasing column (binary search over `[0, count)`). */
function upperBound(xs: Float64Array, count: number, x: number): number {
  let lo = 0;
  let hi = count;
  while (lo < hi) {
    const mid = lo + ((hi - lo) >> 1);
    if ((xs[mid] ?? Number.NaN) <= x) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** First row index with `xs[i] >= x` in a non-decreasing column. */
function lowerBound(xs: Float64Array, count: number, x: number): number {
  let lo = 0;
  let hi = count;
  while (lo < hi) {
    const mid = lo + ((hi - lo) >> 1);
    if ((xs[mid] ?? Number.NaN) < x) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * Rows `[start, end)` needed to draw `range`: the rows inside it plus one row beyond each edge (so lines reach the
 * plot edges). Every row for a window whose x is not monotonic (the plot clips).
 */
export function visibleRows(window: GraphWindow, range: { readonly min: number; readonly max: number }): {
  readonly start: number;
  readonly end: number;
} {
  const { xs, count } = window;
  if (!window.monotonicX) return { start: 0, end: count };
  const start = Math.max(0, lowerBound(xs, count, range.min) - 1);
  const end = Math.min(count, upperBound(xs, count, range.max) + 1);
  return { start, end: Math.max(start, end) };
}

/**
 * Index `i` of the newest row pair with `xs[i] <= x <= xs[i + 1]`, `count - 1` when x equals the last row's x,
 * or -1 when x is outside every pair.
 */
function bracket(window: GraphWindow, x: number): number {
  const { xs, count } = window;
  if (count === 0 || !Number.isFinite(x)) return -1;
  if (window.monotonicX) {
    const after = upperBound(xs, count, x);
    // Last row at or before x.
    const i = after - 1;
    if (i < 0) return -1;
    if (i === count - 1) return xs[i] === x ? i : -1;
    return i;
  }
  for (let i = count - 1; i >= 0; i--) {
    const x0 = xs[i] ?? Number.NaN;
    if (i === count - 1) {
      if (x0 === x) return i;
      continue;
    }
    const x1 = xs[i + 1] ?? Number.NaN;
    if (x0 <= x && x <= x1) return i;
  }
  return -1;
}

/**
 * Value of `column` at `x` (see the module comment), or null. `discrete` selects the Discrete rule; `maxGapT` is
 * the largest time step (s) a value may be carried across.
 */
export function sampleAt(
  window: GraphWindow,
  column: Float64Array,
  x: number,
  discrete: boolean,
  maxGapT: number,
): number | null {
  if (Number.isNaN(maxGapT) || maxGapT < 0) return null;
  const i = bracket(window, x);
  if (i < 0) return null;
  const { xs, ts } = window;
  const x0 = xs[i] ?? Number.NaN;
  const v0 = column[i] ?? Number.NaN;
  const t0 = ts[i] ?? Number.NaN;
  if (!Number.isFinite(v0) || !Number.isFinite(t0)) return null;
  if (x0 === x) return v0;
  const x1 = xs[i + 1] ?? Number.NaN;
  const t1 = ts[i + 1] ?? Number.NaN;
  if (!Number.isFinite(x1) || !Number.isFinite(t1) || !(x1 > x0) || t1 < t0) return null;
  const fraction = (x - x0) / (x1 - x0);
  if (discrete) {
    // On a time axis x is the time itself (exact, as sampleAtTime); otherwise estimate it between the rows.
    const tAtX = xs === ts ? x : t0 + fraction * (t1 - t0);
    return tAtX - t0 <= maxGapT ? v0 : null;
  }
  const v1 = column[i + 1] ?? Number.NaN;
  if (!Number.isFinite(v1) || t1 - t0 > maxGapT) return null;
  return v0 + (v1 - v0) * fraction;
}

/**
 * Session time (s) at `x`: the time of the row at or before `x` (the same row `sampleAt` reads), or null when `x`
 * is outside the window or that row has no time. Used to tell a cursor on the newest data from one in history.
 */
export function rowTimeAt(window: GraphWindow, x: number): number | null {
  const i = bracket(window, x);
  if (i < 0) return null;
  const t = window.ts[i] ?? Number.NaN;
  return Number.isFinite(t) ? t : null;
}

/** X of the row nearest to `x` (the earlier row on ties; the newest occurrence on non-monotonic axes), or null. */
export function nearestRowX(window: GraphWindow, x: number): number | null {
  const { xs, count } = window;
  if (count === 0 || !Number.isFinite(x)) return null;
  if (window.monotonicX) {
    const after = upperBound(xs, count, x);
    // `after` is the first row > x; the row before it is <= x.
    let best: number | null = null;
    let bestD = Number.POSITIVE_INFINITY;
    for (const i of [after - 1, after]) {
      const xi = xs[i];
      if (i < 0 || i >= count || xi === undefined || !Number.isFinite(xi)) continue;
      const d = Math.abs(xi - x);
      if (d < bestD) {
        bestD = d;
        best = xi;
      }
    }
    return best;
  }
  let best: number | null = null;
  let bestD = Number.POSITIVE_INFINITY;
  for (let i = count - 1; i >= 0; i--) {
    const xi = xs[i] ?? Number.NaN;
    if (!Number.isFinite(xi)) continue;
    const d = Math.abs(xi - x);
    if (d < bestD) {
      bestD = d;
      best = xi;
    }
  }
  return best;
}
