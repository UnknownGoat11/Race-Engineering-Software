/**
 * A graph data source over fixed arrays: reference overlays (e.g. a reference lap) and recordings already in
 * memory. Data is copied once into Float64Arrays at construction; `read` returns zero-copy subarray views of
 * the rows a range needs.
 */
import type { DataOrigin, GraphDataSource, GraphWindow, XRange } from './types';

/** Columns of a static source. `ts` defaults to `xs` (a time axis). */
export interface StaticSourceData {
  readonly xs: ArrayLike<number>;
  /** Row times in seconds for gap detection; defaults to `xs`. */
  readonly ts?: ArrayLike<number>;
  /** Values per channel id, one per row (NaN or null = no valid value). */
  readonly columns: ReadonlyMap<number, ArrayLike<number | null>>;
  /** Default 'recording'. */
  readonly origin?: DataOrigin;
  /** Generated test data (labelled "Synthetic Test Data", spec §108); default false. */
  readonly synthetic?: boolean;
}

function toFloat64(values: ArrayLike<number | null>, length: number): Float64Array {
  const out = new Float64Array(length);
  for (let i = 0; i < length; i++) {
    const v = values[i];
    out[i] = typeof v === 'number' && Number.isFinite(v) ? v : Number.NaN;
  }
  return out;
}

function isNonDecreasing(xs: Float64Array): boolean {
  for (let i = 1; i < xs.length; i++) {
    const a = xs[i - 1] ?? Number.NaN;
    const b = xs[i] ?? Number.NaN;
    if (!(b >= a)) return false;
  }
  return true;
}

/** First index with `xs[i] >= x` (or `> x` when `strict`), for a non-decreasing column. */
function bound(xs: Float64Array, x: number, strict: boolean): number {
  let lo = 0;
  let hi = xs.length;
  while (lo < hi) {
    const mid = lo + ((hi - lo) >> 1);
    const v = xs[mid] ?? Number.NaN;
    if (strict ? v <= x : v < x) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

class SliceWindow implements GraphWindow {
  constructor(
    readonly xs: Float64Array,
    readonly ts: Float64Array,
    readonly count: number,
    readonly monotonicX: boolean,
    private readonly columns: ReadonlyMap<number, Float64Array>,
  ) {}

  column(channelId: number): Float64Array | null {
    return this.columns.get(channelId) ?? null;
  }
}

/** Immutable rows from arrays (see the module comment). */
export class StaticSource implements GraphDataSource {
  readonly origin: DataOrigin;
  readonly synthetic: boolean;
  readonly #xs: Float64Array;
  readonly #ts: Float64Array;
  readonly #columns: ReadonlyMap<number, Float64Array>;
  readonly #monotonic: boolean;
  readonly #extent: XRange | null;

  constructor(data: StaticSourceData) {
    const length = data.xs.length;
    this.origin = data.origin ?? 'recording';
    this.synthetic = data.synthetic ?? false;
    this.#xs = toFloat64(data.xs, length);
    this.#ts = data.ts === undefined ? this.#xs : toFloat64(data.ts, length);
    this.#columns = new Map([...data.columns].map(([id, values]) => [id, toFloat64(values, length)] as const));
    this.#monotonic = isNonDecreasing(this.#xs);
    let min = Number.POSITIVE_INFINITY;
    let max = Number.NEGATIVE_INFINITY;
    for (const x of this.#xs) {
      if (!Number.isFinite(x)) continue;
      if (x < min) min = x;
      if (x > max) max = x;
    }
    this.#extent = min <= max ? { min, max } : null;
  }

  /** Static data never changes: listeners are never called. */
  subscribe(): () => void {
    return () => undefined;
  }

  extent(): XRange | null {
    return this.#extent;
  }

  read(range: XRange, channelIds: readonly number[]): GraphWindow {
    let start = 0;
    let end = this.#xs.length;
    if (this.#monotonic) {
      start = Math.max(0, bound(this.#xs, range.min, false) - 1);
      end = Math.min(this.#xs.length, bound(this.#xs, range.max, true) + 1);
    }
    const columns = new Map<number, Float64Array>();
    for (const id of channelIds) {
      const column = this.#columns.get(id);
      if (column !== undefined) columns.set(id, column.subarray(start, end));
    }
    const xs = this.#xs.subarray(start, end);
    const ts = this.#ts === this.#xs ? xs : this.#ts.subarray(start, end);
    return new SliceWindow(xs, ts, end - start, this.#monotonic, columns);
  }

  /** Recordings have no current car position. */
  current(): null {
    return null;
  }
}
