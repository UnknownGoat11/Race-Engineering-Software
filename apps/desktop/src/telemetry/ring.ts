/**
 * Fixed-capacity, columnar time-series ring buffer for live graphs — a TypeScript mirror of
 * `telemetry-model::ring::TelemetryRingBuffer` (spec §123–125, D-023 §3).
 *
 * Storage: one Float64Array timestamp column plus one typed column per selected channel, all of the same
 * capacity, allocated once. Channels whose every valid value is exact in single precision (F32, U8, I8, BOOL)
 * use a Float32Array; the others (F64, I32, U32, U64) a Float64Array. Appending overwrites the oldest row in
 * O(1) (head index arithmetic, never element shifting). A missing or non-valid value is stored as NaN and is
 * never returned as a value, so graphs break the line there instead of drawing a fabricated zero. Rows exist
 * only where samples arrived, so time gaps are gaps here too: point queries and segments never bridge more
 * than a caller-given `maxGapS`. Timestamps are non-decreasing; an earlier timestamp is a `TimeRegression`
 * error and the caller clears the buffer (new session).
 *
 * Differences from Rust, all additive: `InvalidCapacity` for capacities that are not a non-negative safe
 * integer (Rust's `usize` cannot express them); `pushRowGather`, `pushGap` and `replaceNewestRowGather` are
 * allocation-free variants of `push_row` for the store; `copyRange` fills caller-owned arrays for renderers.
 */
import { Interpolation, Quality, ValueType, type ChannelDef } from '../generated/channels';
import { channelDef, isNumericChannel } from './registry';
import { err, ok, type Result } from './result';

/**
 * Largest storage one buffer may allocate, in bytes (1 GiB, as `ring::MAX_RING_BYTES`). Like Rust, the check
 * counts 8 bytes per value; single-precision columns actually use half of that.
 */
export const MAX_RING_BYTES = 1024 * 1024 * 1024;

const BUDGET_BYTES_PER_VALUE = 8;
const F32_BYTES = 4;
const F64_BYTES = 8;

/** Errors from construction, appends and resizing (`ring::RingError`, plus `InvalidCapacity`). */
export type RingError =
  | { readonly kind: 'InvalidCapacity'; readonly capacity: number }
  | { readonly kind: 'ZeroCapacity' }
  | { readonly kind: 'CapacityTooLarge'; readonly capacity: number; readonly columns: number; readonly maxBytes: number }
  | { readonly kind: 'AllocationFailed'; readonly bytes: number }
  | { readonly kind: 'UnknownChannel'; readonly channelId: number }
  | { readonly kind: 'DuplicateChannel'; readonly channelId: number }
  | { readonly kind: 'NonNumericChannel'; readonly channelId: number }
  | { readonly kind: 'NonFiniteTime'; readonly t: number }
  | { readonly kind: 'TimeRegression'; readonly t: number; readonly newest: number }
  | { readonly kind: 'RowWidth'; readonly expected: number; readonly actual: number };

/** Half-open range of logical rows (0 = oldest). */
export interface RowRange {
  readonly start: number;
  readonly end: number;
}

type ColumnArray = Float32Array | Float64Array;

interface Column {
  readonly id: number;
  readonly interpolation: Interpolation;
  readonly single: boolean;
  values: ColumnArray;
  qualities: Uint8Array;
}

/** True for value types whose every value is exactly representable as an f32. */
function storesAsSingle(def: ChannelDef): boolean {
  return (
    def.valueType === ValueType.F32 ||
    def.valueType === ValueType.U8 ||
    def.valueType === ValueType.I8 ||
    def.valueType === ValueType.Bool
  );
}

/** Rust's `check_capacity`: the byte budget counts the timestamp column plus every channel column. */
function checkCapacity(capacity: number, channels: number): RingError | null {
  if (!Number.isSafeInteger(capacity) || capacity < 0) return { kind: 'InvalidCapacity', capacity };
  if (capacity === 0) return { kind: 'ZeroCapacity' };
  const columns = channels + 1;
  if (capacity * columns * BUDGET_BYTES_PER_VALUE > MAX_RING_BYTES) {
    return { kind: 'CapacityTooLarge', capacity, columns, maxBytes: MAX_RING_BYTES };
  }
  return null;
}

/** Allocates a NaN-filled array; an allocation failure is an error, never an exception. */
function allocateNaN<T extends ColumnArray>(make: () => T, bytes: number): Result<T, RingError> {
  try {
    const column = make();
    column.fill(Number.NaN);
    return ok(column);
  } catch {
    return err({ kind: 'AllocationFailed', bytes });
  }
}

/** A NaN-filled value column of `capacity` rows. */
function nanColumn(capacity: number, single: boolean): Result<ColumnArray, RingError> {
  return single
    ? allocateNaN(() => new Float32Array(capacity), capacity * F32_BYTES)
    : allocateNaN(() => new Float64Array(capacity), capacity * F64_BYTES);
}

/** A NaN-filled timestamp column of `capacity` rows. */
function nanTimes(capacity: number): Result<Float64Array, RingError> {
  return allocateNaN(() => new Float64Array(capacity), capacity * F64_BYTES);
}

/** Fixed-capacity columnar ring buffer of timestamped samples. */
export class TelemetryRingBuffer {
  #capacity: number;
  /** Physical slot of the oldest row. */
  #head = 0;
  #length = 0;
  #times: Float64Array;
  readonly #columns: Column[];
  readonly #columnById: ReadonlyMap<number, number>;

  private constructor(capacity: number, times: Float64Array, columns: Column[]) {
    this.#capacity = capacity;
    this.#times = times;
    this.#columns = columns;
    this.#columnById = new Map(columns.map((c, i) => [c.id, i] as const));
  }

  /**
   * A buffer holding `capacity` rows of the given numeric channels (`TelemetryRingBuffer::new`). Errors, in
   * Rust's order: capacity, then each channel (unknown, non-numeric, duplicate), then allocation.
   */
  static create(capacity: number, channels: readonly number[]): Result<TelemetryRingBuffer, RingError> {
    const capacityError = checkCapacity(capacity, channels.length);
    if (capacityError !== null) return err(capacityError);
    const defs: ChannelDef[] = [];
    for (const id of channels) {
      const def = channelDef(id);
      if (def === undefined) return err({ kind: 'UnknownChannel', channelId: id });
      if (!isNumericChannel(def)) return err({ kind: 'NonNumericChannel', channelId: id });
      if (defs.some((d) => d.id === id)) return err({ kind: 'DuplicateChannel', channelId: id });
      defs.push(def);
    }
    const columns: Column[] = [];
    for (const def of defs) {
      const single = storesAsSingle(def);
      const values = nanColumn(capacity, single);
      if (!values.ok) return values;
      const qualities = new Uint8Array(capacity);
      qualities.fill(Quality.Unavailable);
      columns.push({ id: def.id, interpolation: def.interpolation, single, values: values.value, qualities });
    }
    const times = nanTimes(capacity);
    if (!times.ok) return times;
    return ok(new TelemetryRingBuffer(capacity, times.value, columns));
  }

  /** Maximum number of rows. */
  get capacity(): number {
    return this.#capacity;
  }

  /** Stored rows. */
  get length(): number {
    return this.#length;
  }

  /** True when no row is stored. */
  get isEmpty(): boolean {
    return this.#length === 0;
  }

  /** Selected channel ids, in column order. */
  channels(): number[] {
    return this.#columns.map((c) => c.id);
  }

  /** Physical slot of logical row `i` (0 = oldest); valid for `0 <= i < capacity`. */
  #slot(i: number): number {
    const s = this.#head + i;
    return s >= this.#capacity ? s - this.#capacity : s;
  }

  #column(id: number): Column | undefined {
    const index = this.#columnById.get(id);
    return index === undefined ? undefined : this.#columns[index];
  }

  #checkTime(t: number): RingError | null {
    if (!Number.isFinite(t)) return { kind: 'NonFiniteTime', t };
    const newest = this.newestTime();
    if (newest !== null && t < newest) return { kind: 'TimeRegression', t, newest };
    return null;
  }

  /** Slot for a new row, evicting the oldest row when full. */
  #claimSlot(): number {
    if (this.#length < this.#capacity) {
      const slot = this.#slot(this.#length);
      this.#length += 1;
      return slot;
    }
    const slot = this.#head;
    this.#head = this.#head + 1 === this.#capacity ? 0 : this.#head + 1;
    return slot;
  }

  /** Appends a row of values in column order (`push_row`); null, NaN and ±Infinity are stored as gaps. */
  pushRow(t: number, values: ArrayLike<number | null>, qualities?: ArrayLike<Quality | null>): RingError | null {
    if (values.length !== this.#columns.length) {
      return { kind: 'RowWidth', expected: this.#columns.length, actual: values.length };
    }
    const timeError = this.#checkTime(t);
    if (timeError !== null) return timeError;
    const slot = this.#claimSlot();
    this.#times[slot] = t;
    this.#columns.forEach((column, i) => {
      const v = values[i];
      const isFinite = typeof v === 'number' && Number.isFinite(v);
      column.values[slot] = isFinite ? v : Number.NaN;
      const q = qualities?.[i];
      if (typeof q === 'number') {
        column.qualities[slot] = q;
      } else {
        column.qualities[slot] = isFinite ? Quality.Valid : Quality.Unavailable;
      }
    });
    return null;
  }

  /**
   * Appends a row whose value for column `c` is `source[sourceIndex[c]]` (allocation-free `push_row`; the
   * store passes its dense current-value table). Non-finite values are stored as gaps.
   */
  pushRowGather(
    t: number,
    source: Float64Array,
    sourceIndex: Int32Array,
    sourceQuality?: ArrayLike<number>,
  ): RingError | null {
    if (sourceIndex.length !== this.#columns.length) {
      return { kind: 'RowWidth', expected: this.#columns.length, actual: sourceIndex.length };
    }
    const timeError = this.#checkTime(t);
    if (timeError !== null) return timeError;
    this.#writeGather(this.#claimSlot(), t, source, sourceIndex, sourceQuality);
    return null;
  }

  /**
   * Overwrites the newest row with `source[sourceIndex[c]]`, keeping its timestamp. Returns false (and
   * changes nothing) when the buffer is empty or the width does not match.
   */
  replaceNewestRowGather(
    source: Float64Array,
    sourceIndex: Int32Array,
    sourceQuality?: ArrayLike<number>,
  ): boolean {
    if (this.#length === 0 || sourceIndex.length !== this.#columns.length) return false;
    const slot = this.#slot(this.#length - 1);
    this.#writeGather(slot, this.#times[slot] ?? Number.NaN, source, sourceIndex, sourceQuality);
    return true;
  }

  #writeGather(
    slot: number,
    t: number,
    source: Float64Array,
    sourceIndex: Int32Array,
    sourceQuality?: ArrayLike<number>,
  ): void {
    this.#times[slot] = t;
    const columns = this.#columns;
    for (let c = 0; c < columns.length; c++) {
      const idx = sourceIndex[c] ?? -1;
      const v = source[idx] ?? Number.NaN;
      const column = columns[c];
      if (column !== undefined) {
        const isFinite = Number.isFinite(v);
        column.values[slot] = isFinite ? v : Number.NaN;
        if (sourceQuality !== undefined && idx >= 0) {
          const q = sourceQuality[idx];
          column.qualities[slot] = typeof q === 'number' ? q : (isFinite ? Quality.Valid : Quality.Unavailable);
        } else {
          column.qualities[slot] = isFinite ? Quality.Valid : Quality.Unavailable;
        }
      }
    }
  }

  /** Appends a row in which every channel is a gap (a marker for dropped data, spec §219). */
  pushGap(t: number): RingError | null {
    const timeError = this.#checkTime(t);
    if (timeError !== null) return timeError;
    const slot = this.#claimSlot();
    this.#times[slot] = t;
    for (const column of this.#columns) {
      column.values[slot] = Number.NaN;
      column.qualities[slot] = Quality.Valid;
    }
    return null;
  }

  /** Drops every row (capacity and channels are kept). */
  clear(): void {
    this.#head = 0;
    this.#length = 0;
  }

  /**
   * Changes the capacity, keeping the newest rows that fit. On error (invalid, zero, over the byte budget,
   * allocation failure) the buffer is left unchanged.
   */
  resize(capacity: number): RingError | null {
    const capacityError = checkCapacity(capacity, this.#columns.length);
    if (capacityError !== null) return capacityError;
    const keep = Math.min(this.#length, capacity);
    const skip = this.#length - keep;
    const times = nanTimes(capacity);
    if (!times.ok) return times.error;
    const columns: ColumnArray[] = [];
    const qualities: Uint8Array[] = [];
    for (const column of this.#columns) {
      const values = nanColumn(capacity, column.single);
      if (!values.ok) return values.error;
      columns.push(values.value);
      const qs = new Uint8Array(capacity);
      qs.fill(Quality.Unavailable);
      qualities.push(qs);
    }
    // Copy only after every allocation succeeded.
    for (let dst = 0; dst < keep; dst++) {
      const src = this.#slot(skip + dst);
      times.value[dst] = this.#times[src] ?? Number.NaN;
      this.#columns.forEach((column, c) => {
        const target = columns[c];
        if (target !== undefined) target[dst] = column.values[src] ?? Number.NaN;
        const targetQ = qualities[c];
        if (targetQ !== undefined) targetQ[dst] = column.qualities[src] ?? Quality.Unavailable;
      });
    }
    this.#columns.forEach((column, c) => {
      const target = columns[c];
      if (target !== undefined) column.values = target;
      const targetQ = qualities[c];
      if (targetQ !== undefined) column.qualities = targetQ;
    });
    this.#times = times.value;
    this.#capacity = capacity;
    this.#head = 0;
    this.#length = keep;
    return null;
  }

  /** Timestamp of logical row `i` (0 = oldest), or null outside the stored rows. */
  timeAt(i: number): number | null {
    if (!Number.isInteger(i) || i < 0 || i >= this.#length) return null;
    return this.#times[this.#slot(i)] ?? null;
  }

  /** Value of `id` at logical row `i`; null for gaps, unselected channels and rows outside the buffer. */
  valueAt(id: number, i: number): number | null {
    if (!Number.isInteger(i) || i < 0 || i >= this.#length) return null;
    const column = this.#column(id);
    if (column === undefined) return null;
    const v = column.values[this.#slot(i)];
    return v !== undefined && Number.isFinite(v) ? v : null;
  }

  /** Quality of `id` at logical row `i`; null for unselected channels and rows outside the buffer. */
  qualityAt(id: number, i: number): Quality | null {
    if (!Number.isInteger(i) || i < 0 || i >= this.#length) return null;
    const column = this.#column(id);
    if (column === undefined) return null;
    const q = column.qualities[this.#slot(i)];
    return q !== undefined ? (q as Quality) : null;
  }

  /** Oldest stored timestamp. */
  oldestTime(): number | null {
    return this.timeAt(0);
  }

  /** Newest stored timestamp. */
  newestTime(): number | null {
    return this.#length === 0 ? null : this.timeAt(this.#length - 1);
  }

  /** Newest valid `[time, value]` of `id` (skips trailing gaps). */
  newest(id: number): readonly [number, number] | null {
    for (let i = this.#length - 1; i >= 0; i--) {
      const v = this.valueAt(id, i);
      const t = this.timeAt(i);
      if (v !== null && t !== null) return [t, v];
    }
    return null;
  }

  /** Oldest valid `[time, value]` of `id`. */
  oldest(id: number): readonly [number, number] | null {
    for (let i = 0; i < this.#length; i++) {
      const v = this.valueAt(id, i);
      const t = this.timeAt(i);
      if (v !== null && t !== null) return [t, v];
    }
    return null;
  }

  /** Every stored timestamp, oldest first (a copy; renderers use these to detect time gaps). */
  times(): Float64Array {
    const out = new Float64Array(this.#length);
    for (let i = 0; i < this.#length; i++) {
      out[i] = this.#times[this.#slot(i)] ?? Number.NaN;
    }
    return out;
  }

  #timeOf(i: number): number {
    return this.#times[this.#slot(i)] ?? Number.NaN;
  }

  /** First logical row with time >= t (binary search; timestamps are non-decreasing). */
  #lowerBound(t: number): number {
    let lo = 0;
    let hi = this.#length;
    while (lo < hi) {
      const mid = lo + ((hi - lo) >> 1);
      if (this.#timeOf(mid) < t) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  /** First logical row with time > t. */
  #upperBound(t: number): number {
    let lo = 0;
    let hi = this.#length;
    while (lo < hi) {
      const mid = lo + ((hi - lo) >> 1);
      if (this.#timeOf(mid) <= t) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  /** Logical rows with `t0 <= time <= t1` (empty for NaN bounds or `t1 < t0`). */
  rangeIndices(t0: number, t1: number): RowRange {
    if (Number.isNaN(t0) || Number.isNaN(t1) || t1 < t0) return { start: 0, end: 0 };
    return { start: this.#lowerBound(t0), end: this.#upperBound(t1) };
  }

  /** `[time, value]` rows of `id` with `t0 <= time <= t1`; gaps have a null value. Empty for unselected ids. */
  range(id: number, t0: number, t1: number): (readonly [number, number | null])[] {
    if (this.#column(id) === undefined) return [];
    const { start, end } = this.rangeIndices(t0, t1);
    const out: (readonly [number, number | null])[] = [];
    for (let i = start; i < end; i++) {
      out.push([this.#timeOf(i), this.valueAt(id, i)]);
    }
    return out;
  }

  /**
   * Copies the rows of `id` with `t0 <= time <= t1` into caller-owned arrays (gaps stay NaN) and returns the
   * number of rows written, at most the shorter array's length. Allocation-free counterpart of `range`.
   */
  copyRange(
    id: number,
    t0: number,
    t1: number,
    outTimes: Float64Array,
    outValues: Float64Array,
    outQualities?: Uint8Array,
  ): number {
    const column = this.#column(id);
    if (column === undefined) return 0;
    const { start, end } = this.rangeIndices(t0, t1);
    const count = Math.min(
      end - start,
      outTimes.length,
      outValues.length,
      outQualities !== undefined ? outQualities.length : Number.POSITIVE_INFINITY,
    );
    for (let k = 0; k < count; k++) {
      const slot = this.#slot(start + k);
      outTimes[k] = this.#times[slot] ?? Number.NaN;
      outValues[k] = column.values[slot] ?? Number.NaN;
      if (outQualities !== undefined) {
        outQualities[k] = column.qualities[slot] ?? Quality.Unavailable;
      }
    }
    return count;
  }

  /** Logical row whose timestamp is nearest to `t` (earlier row on ties); null when empty or `t` is NaN. */
  nearestIndex(t: number): number | null {
    if (this.#length === 0 || Number.isNaN(t)) return null;
    const after = this.#lowerBound(t);
    if (after === 0) return 0;
    if (after === this.#length) return this.#length - 1;
    const before = after - 1;
    const dBefore = t - this.#timeOf(before);
    const dAfter = this.#timeOf(after) - t;
    return dAfter < dBefore ? after : before;
  }

  /** `[time, value]` of the row nearest to `t` (value null if that row is a gap); null for unselected ids. */
  nearest(id: number, t: number): readonly [number, number | null] | null {
    if (this.#column(id) === undefined) return null;
    const i = this.nearestIndex(t);
    if (i === null) return null;
    return [this.#timeOf(i), this.valueAt(id, i)];
  }

  /**
   * Value of `id` at time `t`, never fabricated across a time gap (`sample_at_time`). Continuous channels
   * are linearly interpolated between the bracketing rows, only if both are valid and at most `maxGapS`
   * apart; discrete channels return the last row at or before `t` (never interpolated), only if that row is
   * at most `maxGapS` older than `t`. Outside the stored span, next to a missing value, across a longer gap,
   * or when `maxGapS` is NaN or negative, the result is null.
   */
  sampleAtTime(id: number, t: number, maxGapS: number): number | null {
    const column = this.#column(id);
    if (column === undefined) return null;
    if (this.#length === 0 || !Number.isFinite(t) || Number.isNaN(maxGapS) || maxGapS < 0) return null;
    const newest = this.newestTime();
    const oldest = this.oldestTime();
    if (newest === null || oldest === null || t < oldest || t > newest) return null;
    // Last row at or before t (exists because t >= oldest).
    const prev = this.#upperBound(t) - 1;
    if (prev < 0) return null;
    const t0 = this.#timeOf(prev);
    const v0 = this.valueAt(id, prev);
    if (v0 === null) return null;
    if (t0 === t) return v0;
    if (column.interpolation === Interpolation.Discrete) return t - t0 <= maxGapS ? v0 : null;
    // `next` is the first row after t, so t0 < t < t1 and the division is by a positive span.
    const next = prev + 1;
    const t1 = this.timeAt(next);
    const v1 = this.valueAt(id, next);
    if (t1 === null || v1 === null) return null;
    if (t1 - t0 > maxGapS) return null;
    return v0 + ((v1 - v0) * (t - t0)) / (t1 - t0);
  }

  /**
   * Contiguous runs of valid values of `id` within `[t0, t1]`, split at gaps (missing values) and wherever
   * consecutive timestamps are more than `maxGapS` apart. Renderers draw one polyline per run (spec §219).
   */
  segments(id: number, t0: number, t1: number, maxGapS: number): RowRange[] {
    const out: RowRange[] = [];
    if (this.#column(id) === undefined) return out;
    let current: { start: number; end: number } | null = null;
    const { start, end } = this.rangeIndices(t0, t1);
    for (let i = start; i < end; i++) {
      const valid = this.valueAt(id, i) !== null;
      const contiguous = current !== null && this.#timeOf(i) - this.#timeOf(current.end - 1) <= maxGapS;
      if (valid && contiguous && current !== null) {
        current.end = i + 1;
      } else if (valid) {
        if (current !== null) out.push(current);
        current = { start: i, end: i + 1 };
      } else if (current !== null) {
        out.push(current);
        current = null;
      }
    }
    if (current !== null) out.push(current);
    return out;
  }
}
