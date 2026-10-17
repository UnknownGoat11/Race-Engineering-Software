/**
 * Per-pixel min/max decimation of one series into polyline commands (spec §277), and gap detection (§219).
 *
 * Rows are walked once, in order. Consecutive rows that fall into the same device-pixel column are merged into
 * that column's minimum and maximum, and a column is drawn as a vertical stroke covering [min, max], so every
 * extreme of the raw data stays visible however many rows share a pixel. Line series emit at most 2 commands per
 * column: the pen enters the column at the extreme nearer to where it stands and leaves from the other one (the
 * order of rows inside one device pixel is not visible; the GPU stroke cost grows with the vertex count, which
 * the bench measured as the frame-rate limit). A column holding a single row is drawn through that row exactly,
 * so sparse (zoomed-in) data is traced vertex for vertex. Discrete channels keep their exit value per column
 * (needed to hold the step). X does not need to be monotonic (a column ends whenever the next row lands in
 * another column).
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
 * Traces `input` into `sink` and returns the number of commands emitted. A run of a single column holding one
 * value is emitted as a zero-length segment (drawn as a dot with round caps), so isolated samples remain visible.
 *
 * Performance: this is the per-row hot loop of every repaint (4 graphs × 6 series × 6 000 rows in the bench).
 * The loops keep all their state in plain locals and call only top-level helpers once per COLUMN; a closure
 * over that state makes V8 keep it in a heap context (2026-10-14 CPU profile of the bench, WebGL, 8 s:
 * `traceLines` self time 2.39 s with closures, 1.67 s without).
 */
export function tracePolyline(input: PolylineInput, sink: PathSink): number {
  return input.step ? traceSteps(input, sink) : traceLines(input, sink);
}

/**
 * Emits one closed line column [minY, maxY] at `colX`: entering at the extreme nearer `ref` (the pen, or the
 * column's first row when the pen is up) and leaving from the other. Returns the pen's new y.
 */
function lineColumn(sink: PathSink, penDown: boolean, ref: number, minY: number, maxY: number, colX: number): number {
  const nearMin = Math.abs(ref - minY) <= Math.abs(ref - maxY);
  const near = nearMin ? minY : maxY;
  const far = nearMin ? maxY : minY;
  if (penDown) sink.lineTo(colX, near);
  else sink.moveTo(colX, near);
  if (far !== near) {
    sink.lineTo(colX, far);
    return far;
  }
  return near;
}

/** Ends a line run: emits its open column, plus a zero-length segment for a lone value. Returns commands. */
function endLineRun(
  sink: PathSink,
  penDown: boolean,
  penY: number,
  firstY: number,
  minY: number,
  maxY: number,
  colX: number,
  runColumns: number,
): number {
  lineColumn(sink, penDown, penDown ? penY : firstY, minY, maxY, colX);
  if (minY !== maxY) return 2;
  if (runColumns === 1) {
    sink.lineTo(colX, minY);
    return 2;
  }
  return 1;
}

/** Line series: per column, enter at the extreme nearer the pen and leave from the other (≤ 2 commands). */
function traceLines(input: PolylineInput, sink: PathSink): number {
  const { xs, ts, ys, start, end, maxGapT } = input;
  const ax = input.x.scale;
  const bx = input.x.offset;
  const ay = input.y.scale;
  const by = input.y.offset;
  let commands = 0;

  let inRun = false; // a column is open
  let penDown = false; // the run has emitted its moveTo
  let penY = 0;
  let runColumns = 0;
  let col = 0;
  let colX = 0;
  let firstY = 0;
  let minY = 0;
  let maxY = 0;
  let prevT = 0;

  for (let i = start; i < end; i++) {
    const x = xs[i] ?? Number.NaN;
    const y = ys[i] ?? Number.NaN;
    const t = ts[i] ?? Number.NaN;
    if (!Number.isFinite(x) || !Number.isFinite(y) || !Number.isFinite(t)) {
      if (inRun) {
        commands += endLineRun(sink, penDown, penY, firstY, minY, maxY, colX, runColumns);
        inRun = false;
        penDown = false;
      }
      continue;
    }
    // A step back in time is a discontinuity too (rows out of order are never joined).
    const dt = t - prevT;
    if (inRun && !(dt >= 0 && dt <= maxGapT)) {
      commands += endLineRun(sink, penDown, penY, firstY, minY, maxY, colX, runColumns);
      inRun = false;
      penDown = false;
    }
    prevT = t;
    const py = clampPx(y * ay + by);
    const c = Math.floor(clampPx(x * ax + bx));
    if (inRun) {
      if (c === col) {
        if (py < minY) minY = py;
        if (py > maxY) maxY = py;
        continue;
      }
      penY = lineColumn(sink, penDown, penDown ? penY : firstY, minY, maxY, colX);
      commands += minY === maxY ? 1 : 2;
      penDown = true;
    } else {
      runColumns = 0;
    }
    inRun = true;
    runColumns += 1;
    col = c;
    colX = c + COLUMN_CENTRE;
    firstY = minY = maxY = py;
  }
  if (inRun) commands += endLineRun(sink, penDown, penY, firstY, minY, maxY, colX, runColumns);
  return commands;
}

/** Draws a closed step column's extent so that it ends on its exit value `lastY`. Returns commands. */
function stepColumn(sink: PathSink, colX: number, minY: number, maxY: number, lastY: number): number {
  // A single value in the column: the pen already stands on it (or this is a lone sample: a dot, see endStepRun).
  if (minY === maxY) return 0;
  if (lastY === maxY) {
    sink.lineTo(colX, minY);
    sink.lineTo(colX, maxY);
    return 2;
  }
  if (lastY === minY) {
    sink.lineTo(colX, maxY);
    sink.lineTo(colX, minY);
    return 2;
  }
  sink.lineTo(colX, minY);
  sink.lineTo(colX, maxY);
  sink.lineTo(colX, lastY);
  return 3;
}

/** Ends a step run: its open column, plus a zero-length segment for a lone value. Returns commands. */
function endStepRun(sink: PathSink, colX: number, minY: number, maxY: number, lastY: number, runColumns: number): number {
  const commands = stepColumn(sink, colX, minY, maxY, lastY);
  if (runColumns === 1 && minY === maxY) {
    sink.lineTo(colX, lastY);
    return commands + 1;
  }
  return commands;
}

/** Discrete series: columns end on their exit value so the step can hold it. */
function traceSteps(input: PolylineInput, sink: PathSink): number {
  const { xs, ts, ys, start, end, maxGapT } = input;
  const ax = input.x.scale;
  const bx = input.x.offset;
  const ay = input.y.scale;
  const by = input.y.offset;
  let commands = 0;

  let inRun = false;
  let runColumns = 0;
  let col = 0; // current column index (floor of the x pixel)
  let colX = 0; // its centre
  let minY = 0;
  let maxY = 0;
  let lastY = 0;
  let prevT = 0;

  for (let i = start; i < end; i++) {
    const x = xs[i] ?? Number.NaN;
    const y = ys[i] ?? Number.NaN;
    const t = ts[i] ?? Number.NaN;
    if (!Number.isFinite(x) || !Number.isFinite(y) || !Number.isFinite(t)) {
      if (inRun) {
        commands += endStepRun(sink, colX, minY, maxY, lastY, runColumns);
        inRun = false;
      }
      continue;
    }
    // A step back in time is a discontinuity too (rows out of order are never joined).
    const dt = t - prevT;
    if (inRun && !(dt >= 0 && dt <= maxGapT)) {
      commands += endStepRun(sink, colX, minY, maxY, lastY, runColumns);
      inRun = false;
    }
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
    commands += stepColumn(sink, colX, minY, maxY, lastY);
    const nextX = c + COLUMN_CENTRE;
    // Hold the previous value up to the new row, then jump (Discrete: never interpolated).
    sink.lineTo(nextX, lastY);
    commands += 1;
    if (py !== lastY) {
      sink.lineTo(nextX, py);
      commands += 1;
    }
    col = c;
    colX = nextX;
    minY = maxY = lastY = py;
    runColumns += 1;
  }
  if (inRun) commands += endStepRun(sink, colX, minY, maxY, lastY, runColumns);
  return commands;
}

/**
 * X intervals of significant gaps (spec §219) in rows `[start, end)`: wherever consecutive rows with finite
 * times are more than `maxGapT` seconds apart, the interval between their x values (in either x direction, so a
 * lap axis that steps back still shows it). Rows for which `isBreakRow` is true carry no data (e.g. the all-NaN row
 * the store inserts in the middle of a dropped stretch) and are skipped, so the gap is measured between the data
 * rows around them: a drop shorter than 2 × `maxGapT` is no longer split into two harmless-looking halves
 * (silent-failure M2). Overlapping or touching intervals are merged; the result is ordered by x for monotonic
 * windows. Appends to `out` and returns it.
 */
export function findTimeGaps(
  xs: ArrayLike<number>,
  ts: ArrayLike<number>,
  start: number,
  end: number,
  maxGapT: number,
  out: XRange[] = [],
  isBreakRow?: (row: number) => boolean,
): XRange[] {
  let prevT = Number.NaN;
  let prevX = Number.NaN;
  for (let i = start; i < end; i++) {
    const t = ts[i] ?? Number.NaN;
    const x = xs[i] ?? Number.NaN;
    if (!Number.isFinite(t) || !Number.isFinite(x)) continue;
    if (isBreakRow?.(i) === true) continue;
    if (Number.isFinite(prevT) && t - prevT > maxGapT && x !== prevX) {
      const lo = Math.min(prevX, x);
      const hi = Math.max(prevX, x);
      const last = out[out.length - 1];
      if (last !== undefined && last.max >= lo && last.min <= hi) {
        out[out.length - 1] = { min: Math.min(last.min, lo), max: Math.max(last.max, hi) };
      } else {
        out.push({ min: lo, max: hi });
      }
    }
    prevT = t;
    prevX = x;
  }
  return out;
}

/**
 * X intervals where one series has no value between two rows that have one (an invalid / unavailable run or a
 * short dropout on a regular row clock), in rows `[start, end)`: from the x of the last valid row before the run
 * to the x of the first valid row after it. Leading and trailing runs (the channel has not started or stopped)
 * are not reported. Such a break may be narrower than one pixel column, so the overlay marks it with a minimum
 * width (silent-failure M2). Appends to `out` and returns it.
 */
export function findValueBreaks(
  xs: ArrayLike<number>,
  ys: ArrayLike<number>,
  start: number,
  end: number,
  out: XRange[] = [],
): XRange[] {
  let lastX = Number.NaN;
  let inRun = false;
  for (let i = start; i < end; i++) {
    const x = xs[i] ?? Number.NaN;
    if (!Number.isFinite(x)) continue;
    const y = ys[i] ?? Number.NaN;
    if (!Number.isFinite(y)) {
      if (Number.isFinite(lastX)) inRun = true;
      continue;
    }
    if (inRun) {
      out.push({ min: Math.min(lastX, x), max: Math.max(lastX, x) });
      inRun = false;
    }
    lastX = x;
  }
  return out;
}
