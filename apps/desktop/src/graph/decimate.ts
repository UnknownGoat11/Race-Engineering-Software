/**
 * Per-pixel min/max decimation of one series into polyline commands (spec §277), and gap detection (§219).
 *
 * Rows are walked once, in order. Consecutive rows that fall into the same device-pixel column are merged into
 * that column's entry, minimum, maximum and exit values; a column is drawn as a vertical stroke covering
 * [min, max] that ends on the exit value, so every extreme of the raw data stays visible however many rows share
 * a pixel, and the output is at most ~4 commands per column. X does not need to be monotonic (a column ends
 * whenever the next row lands in another column).
 *
 * A line is never drawn across missing data: a row with a non-finite x, y or time ends the current run, and so
 * does a time step between consecutive rows that is larger than `maxGapT` or negative (times come from `ts`, not
 * from x, so lap distance axes break on time gaps too). Discrete channels are drawn as steps: the previous value is held
 * horizontally up to the next row, then the line jumps vertically (spec §125: never interpolated). No smoothing
 * is applied (spec §318).
 */
import type { Affine } from './scale';
import type { XRange } from './types';

/** Receives polyline commands (a CanvasRenderingContext2D or Path2D satisfies it). */
export interface PathSink {
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
}

/** One series to trace. Pixel maps are in device pixels. */
export interface PolylineInput {
  readonly xs: ArrayLike<number>;
  /** Row times in seconds, for the gap rule (may be `xs` on a time axis). */
  readonly ts: ArrayLike<number>;
  readonly ys: ArrayLike<number>;
  /** Rows `[start, end)` are traced. */
  readonly start: number;
  readonly end: number;
  readonly x: Affine;
  readonly y: Affine;
  /** Largest time step (s) a line may bridge; larger steps break it. */
  readonly maxGapT: number;
  /** Draw as steps (Discrete channels). */
  readonly step: boolean;
}

/**
 * Pixel coordinates are clamped to ±this so far off-plot points cannot overflow the rasteriser; the visible part
 * of a line leaving the plot is unchanged at this distance (it is clipped long before).
 */
export const PIXEL_CLAMP = 1e6;
/** Offset to a device-pixel column centre, so 1-px vertical strokes are crisp. */
const COLUMN_CENTRE = 0.5;

function clampPx(v: number): number {
  return v < -PIXEL_CLAMP ? -PIXEL_CLAMP : v > PIXEL_CLAMP ? PIXEL_CLAMP : v;
}

/**
 * Traces `input` into `sink` and returns the number of commands emitted. A run of a single column is emitted
 * as a zero-length segment (drawn as a dot with round caps), so isolated samples remain visible.
 */
export function tracePolyline(input: PolylineInput, sink: PathSink): number {
  const { xs, ts, ys, start, end, maxGapT, step } = input;
  const ax = input.x.scale;
  const bx = input.x.offset;
  const ay = input.y.scale;
  const by = input.y.offset;
  let commands = 0;

  let inRun = false;
  let col = 0; // current column index (floor of the x pixel)
  let colX = 0; // its centre
  let minY = 0;
  let maxY = 0;
  let lastY = 0;
  let prevT = 0;

  /** Draws the open column's extent so that it ends on its exit value. */
  const flush = (): void => {
    if (minY === maxY) {
      // A single value in the column: the pen already stands on it (or this is a lone sample: draw a dot).
      return;
    }
    if (lastY === maxY) {
      sink.lineTo(colX, minY);
      sink.lineTo(colX, maxY);
      commands += 2;
    } else if (lastY === minY) {
      sink.lineTo(colX, maxY);
      sink.lineTo(colX, minY);
      commands += 2;
    } else {
      sink.lineTo(colX, minY);
      sink.lineTo(colX, maxY);
      sink.lineTo(colX, lastY);
      commands += 3;
    }
  };

  let runColumns = 0;
  const endRun = (): void => {
    if (!inRun) return;
    flush();
    if (runColumns === 1 && minY === maxY) {
      sink.lineTo(colX, lastY);
      commands += 1;
    }
    inRun = false;
  };

  for (let i = start; i < end; i++) {
    const x = xs[i] ?? Number.NaN;
    const y = ys[i] ?? Number.NaN;
    const t = ts[i] ?? Number.NaN;
    if (!Number.isFinite(x) || !Number.isFinite(y) || !Number.isFinite(t)) {
      endRun();
      continue;
    }
    // A step back in time is a discontinuity too (rows out of order are never joined).
    const dt = t - prevT;
    if (inRun && !(dt >= 0 && dt <= maxGapT)) endRun();
    prevT = t;
    const px = clampPx(x * ax + bx);
    const py = clampPx(y * ay + by);
    const c = Math.floor(px);
    if (!inRun) {
      inRun = true;
      runColumns = 1;
      col = c;
      colX = c + COLUMN_CENTRE;
      minY = maxY = lastY = py;
      sink.moveTo(colX, py);
      commands += 1;
      continue;
    }
    if (c === col) {
      if (py < minY) minY = py;
      if (py > maxY) maxY = py;
      lastY = py;
      continue;
    }
    flush();
    const nextX = c + COLUMN_CENTRE;
    if (step) {
      // Hold the previous value up to the new row, then jump (Discrete: never interpolated).
      sink.lineTo(nextX, lastY);
      commands += 1;
      if (py !== lastY) {
        sink.lineTo(nextX, py);
        commands += 1;
      }
    } else {
      sink.lineTo(nextX, py);
      commands += 1;
    }
    col = c;
    colX = nextX;
    minY = maxY = lastY = py;
    runColumns += 1;
  }
  endRun();
  return commands;
}

/**
 * X intervals of significant gaps (spec §219) in rows `[start, end)`: wherever consecutive rows with finite
 * times are more than `maxGapT` seconds apart, the interval between their x values. Overlapping or touching
 * intervals are merged; the result is ordered by x for monotonic windows. Appends to `out` and returns it.
 */
export function findTimeGaps(
  xs: ArrayLike<number>,
  ts: ArrayLike<number>,
  start: number,
  end: number,
  maxGapT: number,
  out: XRange[] = [],
): XRange[] {
  let prevT = Number.NaN;
  let prevX = Number.NaN;
  for (let i = start; i < end; i++) {
    const t = ts[i] ?? Number.NaN;
    const x = xs[i] ?? Number.NaN;
    if (!Number.isFinite(t) || !Number.isFinite(x)) continue;
    if (Number.isFinite(prevT) && t - prevT > maxGapT && x > prevX) {
      const last = out[out.length - 1];
      if (last !== undefined && last.max >= prevX) {
        out[out.length - 1] = { min: last.min, max: Math.max(last.max, x) };
      } else {
        out.push({ min: prevX, max: x });
      }
    }
    prevT = t;
    prevX = x;
  }
  return out;
}
