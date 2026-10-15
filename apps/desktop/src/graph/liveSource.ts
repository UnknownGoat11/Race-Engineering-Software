/**
 * Live data adapter: reads the rows a graph needs from the TelemetryStore's live ring (D-023 §3, spec §63, §123).
 *
 * Nothing is copied except the visible window: `read` copies the rows of the requested range (plus one row past
 * each edge) into reusable typed arrays, which only grow (by doubling) and are never reallocated per frame.
 *
 * X axis: on the time axis x is the ring timestamp (LMU session time, D-023 §5). On lap axes x is the value of a
 * caller-chosen x channel (lap distance, lap progress or lap time) in each row, and only the rows of the current
 * lap are returned: a lap starts where x drops by more than half of the largest |x| seen (see `advanceLapScan`).
 * The lap is found by an incremental scan that reads the ring in place (no copy) and visits each row once, except
 * the rows at the newest timestamp, which the store may still rewrite and are therefore rescanned. When the lap's
 * x never steps back (beyond jitter), the visible rows are found by binary search on x; otherwise (the car
 * reversed) the whole current lap within the ring is returned and the plot clips it to the range.
 */
import { FrequencyClass, Unit } from '../generated/channels';
import { channelDef } from '../telemetry/registry';
import type { ChannelSample, TelemetryStore } from '../telemetry/store';
import { checkDataKind } from './provenance';
import type { CurrentPosition, DataKind, GraphDataSource, GraphWindow, XAxisMode, XRange } from './types';

/** Options shared by every live source. */
interface LiveRingSourceCommon {
  /** Frequency class whose freshness marks the current-car marker stale / degraded (default Physics). */
  readonly freshnessClass?: FrequencyClass;
  /**
   * What the store holds: 'lmu' for the bridge's LMU telemetry, 'synthetic' for generated test data (labelled
   * "Synthetic Test Data", spec §108–§109). Required, no default: a forgotten flag is a type error, never a
   * synthetic feed labelled as LMU (silent-failure H2). The store itself does not carry the data origin yet.
   */
  readonly dataKind: DataKind;
}

/**
 * Options of a live source. A lap axis needs the channel that gives x (silent-failure M3: without one the graph
 * would show NO DATA forever while the feed is live), and that channel's unit must fit the mode.
 */
export type LiveRingSourceOptions =
  | (LiveRingSourceCommon & { readonly xMode: 'time'; readonly xChannel?: undefined })
  | (LiveRingSourceCommon & {
      readonly xMode: Exclude<XAxisMode, 'time'>;
      /** Channel giving x: `ch.LAP_DISTANCE` (lapDistance), `ch.LAP_PROGRESS` (lapFraction), `ch.LAP_TIME` (lapTime). */
      readonly xChannel: number;
    });

/** Unit an x channel must have on each lap axis. */
const LAP_AXIS_UNITS: Readonly<Record<Exclude<XAxisMode, 'time'>, Unit>> = {
  lapTime: Unit.Second,
  lapDistance: Unit.Metre,
  lapFraction: Unit.Fraction,
};

/** Throws a TypeError when a lap axis has no x channel, an unknown one, or one whose unit does not fit the mode. */
function checkXChannel(mode: XAxisMode, xChannel: unknown): number | null {
  if (mode === 'time') return null;
  if (typeof xChannel !== 'number' || !Number.isInteger(xChannel)) {
    throw new TypeError(`LiveRingSource on '${mode}' needs an xChannel (got ${String(xChannel)})`);
  }
  const def = channelDef(xChannel);
  if (def === undefined) throw new TypeError(`LiveRingSource xChannel ${String(xChannel)} is not a known channel`);
  const expected = LAP_AXIS_UNITS[mode];
  if (def.unit !== expected) {
    throw new TypeError(`LiveRingSource xChannel ${def.label} (${def.unit}) does not fit the '${mode}' axis (${expected})`);
  }
  return xChannel;
}

const INITIAL_ROWS = 1024;
/** A lap reset is an x drop larger than this fraction of the largest |x| seen. */
const LAP_RESET_FRACTION = 0.5;
/**
 * A backwards x step larger than this fraction of the largest |x| seen means the lap's x is not monotonic (the
 * car reversed); smaller steps are jitter (e.g. lap distance while standing still) and keep binary search valid.
 */
const LAP_JITTER_FRACTION = 1e-3;

/**
 * Running summary of a lap scan over rows in time order. Plain data so the lap rule is testable without a ring;
 * `advanceLapScan` updates it in place.
 */
export interface LapScanState {
  /** Time of the first finite row of the current lap (NaN before any). */
  firstT: number;
  /** Last finite x (NaN before any). */
  lastX: number;
  /** Largest |x| seen (the scale of the reset and jitter thresholds). */
  maxAbs: number;
  /** Finite x extent of the current lap (`min > max` while it has none). */
  min: number;
  max: number;
  /** False once x stepped back by more than the jitter tolerance within the current lap. */
  monotonic: boolean;
}

/** An empty scan. */
export function newLapScan(): LapScanState {
  return {
    firstT: Number.NaN,
    lastX: Number.NaN,
    maxAbs: 0,
    min: Number.POSITIVE_INFINITY,
    max: Number.NEGATIVE_INFINITY,
    monotonic: true,
  };
}

/**
 * Adds the row `(t, x)` to `state`. Rows with a non-finite x or t are ignored (gaps do not end a lap). An x drop
 * larger than `LAP_RESET_FRACTION` × the largest |x| seen starts a new lap at this row.
 */
export function advanceLapScan(state: LapScanState, x: number, t: number): void {
  if (!Number.isFinite(x) || !Number.isFinite(t)) return;
  const abs = Math.abs(x);
  if (abs > state.maxAbs) state.maxAbs = abs;
  const prev = state.lastX;
  state.lastX = x;
  if (Number.isFinite(prev)) {
    const drop = prev - x;
    if (state.maxAbs > 0 && drop > state.maxAbs * LAP_RESET_FRACTION) {
      state.firstT = t;
      state.min = x;
      state.max = x;
      state.monotonic = true;
      return;
    }
    if (drop > state.maxAbs * LAP_JITTER_FRACTION) state.monotonic = false;
  } else if (!Number.isFinite(state.firstT)) {
    state.firstT = t;
  }
  if (x < state.min) state.min = x;
  if (x > state.max) state.max = x;
}

function copyLapScan(target: LapScanState, source: LapScanState): void {
  target.firstT = source.firstT;
  target.lastX = source.lastX;
  target.maxAbs = source.maxAbs;
  target.min = source.min;
  target.max = source.max;
  target.monotonic = source.monotonic;
}

/** Reusable columns that grow by doubling. */
class WindowBuffers implements GraphWindow {
  xs = new Float64Array(INITIAL_ROWS);
  ts = new Float64Array(INITIAL_ROWS);
  count = 0;
  monotonicX = true;
  readonly #columns = new Map<number, Float64Array>();
  readonly #requested = new Set<number>();

  /** Makes room for `rows` rows (growing by doubling); on a time axis `xs` aliases `ts`. */
  ensure(rows: number, timeAxis: boolean): void {
    if (rows <= this.ts.length) {
      if (timeAxis) this.xs = this.ts;
      else if (this.xs === this.ts) this.xs = new Float64Array(this.ts.length);
      return;
    }
    let size = this.ts.length;
    while (size < rows) size *= 2;
    this.ts = new Float64Array(size);
    this.xs = timeAxis ? this.ts : new Float64Array(size);
    for (const id of [...this.#columns.keys()]) this.#columns.set(id, new Float64Array(size));
  }

  /** The value column of `id`, allocated on first use. */
  columnFor(id: number): Float64Array {
    let c = this.#columns.get(id);
    if (c?.length !== this.ts.length) {
      c = new Float64Array(this.ts.length);
      this.#columns.set(id, c);
    }
    return c;
  }

  setRequested(ids: readonly number[]): void {
    this.#requested.clear();
    for (const id of ids) this.#requested.add(id);
  }

  markMissing(id: number): void {
    this.#requested.delete(id);
  }

  column(channelId: number): Float64Array | null {
    if (!this.#requested.has(channelId)) return null;
    return this.#columns.get(channelId) ?? null;
  }
}

/** A GraphDataSource over the TelemetryStore live ring (see the module comment). */
export class LiveRingSource implements GraphDataSource {
  readonly origin = 'live' as const;
  readonly dataKind: DataKind;
  readonly #store: TelemetryStore;
  readonly #timeAxis: boolean;
  readonly #xChannel: number | null;
  readonly #freshnessClass: FrequencyClass;
  readonly #window = new WindowBuffers();
  /** Ring state the lap scan last saw: [length, oldest, newest, newest row's x]; unchanged → nothing to do. */
  #lapKey: readonly [number, number | null, number | null, number | null] | null = null;
  /** Scan of every row with time < `#lapBoundT` (final rows; the store only rewrites the newest row). */
  readonly #lapCommitted = newLapScan();
  #lapBoundT = Number.NaN;
  /** The last committed finite row, to notice a ring that was cleared and rebuilt between scans. */
  #lapCheckT = Number.NaN;
  #lapCheckX = Number.NaN;
  /** Scan of every row: the committed scan plus the rows at the newest timestamp. */
  readonly #lap = newLapScan();
  #lapStart = 0;
  #lapExtent: XRange | null = null;

  constructor(store: TelemetryStore, options: LiveRingSourceOptions) {
    this.#store = store;
    this.#timeAxis = options.xMode === 'time';
    this.#xChannel = checkXChannel(options.xMode, options.xChannel);
    this.#freshnessClass = options.freshnessClass ?? FrequencyClass.Physics;
    this.dataKind = checkDataKind(options.dataKind);
  }

  subscribe(listener: () => void): () => void {
    const offRing = this.#store.subscribeRing(listener);
    const offStream = this.#store.subscribeStream(listener);
    return () => {
      offRing();
      offStream();
    };
  }

  extent(): XRange | null {
    const ring = this.#store.ring;
    if (this.#timeAxis) {
      const min = ring.oldestTime();
      const max = ring.newestTime();
      return min === null || max === null ? null : { min, max };
    }
    this.#scanLap();
    return this.#lapExtent;
  }

  read(range: XRange, channelIds: readonly number[]): GraphWindow {
    const w = this.#window;
    w.setRequested(channelIds);
    if (this.#timeAxis) this.#readTime(range, channelIds);
    else this.#readLap(range, channelIds);
    return w;
  }

  current(): CurrentPosition | null {
    const ring = this.#store.ring;
    let x: number | null = null;
    if (this.#timeAxis) {
      x = ring.newestTime();
    } else if (this.#xChannel !== null) {
      const newest = ring.newest(this.#xChannel);
      x = newest === null ? null : newest[1];
    }
    if (x === null) return null;
    const t = ring.newestTime();
    if (t === null) return null;
    const state = this.#store.getStreamState(this.#freshnessClass);
    return { x, t, stale: state === 'stale' || state === 'noData', degraded: state === 'degraded' };
  }

  /** The store's own snapshot of the channel (same state rules as `ValueReadout`). */
  latest(channelId: number): ChannelSample {
    return this.#store.getSample(channelId);
  }

  #readTime(range: XRange, channelIds: readonly number[]): void {
    const w = this.#window;
    const ring = this.#store.ring;
    const inside = ring.rangeIndices(range.min, range.max);
    // One row past each edge, so lines reach the plot edges.
    const t0 = ring.timeAt(Math.max(0, inside.start - 1)) ?? Number.NaN;
    const t1 = ring.timeAt(Math.min(ring.length, inside.end + 1) - 1) ?? Number.NaN;
    // copyRange selects rows by TIME; equal timestamps are allowed, so count rows with the same time bounds.
    const { start, end } = ring.rangeIndices(t0, t1);
    const rows = Math.max(0, end - start);
    w.ensure(rows, true);
    w.count = rows;
    w.monotonicX = true;
    if (rows === 0) return;
    let timesCopied = false;
    for (const id of channelIds) {
      const copied = ring.copyRange(id, t0, t1, w.ts, w.columnFor(id));
      if (copied === 0) w.markMissing(id);
      else timesCopied = true;
    }
    if (!timesCopied) {
      for (let k = 0; k < rows; k++) w.ts[k] = ring.timeAt(start + k) ?? Number.NaN;
    }
  }

  #readLap(range: XRange, channelIds: readonly number[]): void {
    const w = this.#window;
    const ring = this.#store.ring;
    const xChannel = this.#xChannel;
    this.#scanLap();
    let first = this.#lapStart;
    let last = ring.length; // exclusive
    if (xChannel !== null && this.#lap.monotonic && first < last) {
      // One row past each edge, as on the time axis.
      first = Math.max(first, this.#xBound(xChannel, range.min, first, last, false) - 1);
      last = Math.min(last, this.#xBound(xChannel, range.max, first, last, true) + 1);
    }
    const t0 = last > first ? (ring.timeAt(first) ?? Number.NaN) : Number.NaN;
    const t1 = last > first ? (ring.timeAt(last - 1) ?? Number.NaN) : Number.NaN;
    const { start, end } = ring.rangeIndices(t0, t1);
    const rows = xChannel === null ? 0 : Math.max(0, end - start);
    w.ensure(rows, false);
    w.count = rows;
    if (rows === 0 || xChannel === null) {
      w.monotonicX = true;
      return;
    }
    ring.copyRange(xChannel, t0, t1, w.ts, w.xs);
    let monotonic = true;
    for (let k = 1; k < rows; k++) {
      if (!((w.xs[k] ?? Number.NaN) >= (w.xs[k - 1] ?? Number.NaN))) {
        monotonic = false;
        break;
      }
    }
    w.monotonicX = monotonic;
    for (const id of channelIds) {
      if (ring.copyRange(id, t0, t1, w.ts, w.columnFor(id)) === 0) w.markMissing(id);
    }
  }

  /**
   * First row in `[lo, hi)` whose x is `>= x` (`> x` when `strict`), by binary search on a lap whose finite x
   * never steps back beyond jitter. A row without x takes the x of the next row that has one (+∞ at the end),
   * which keeps the predicate monotonic across gap rows.
   */
  #xBound(xChannel: number, x: number, lo: number, hi: number, strict: boolean): number {
    const ring = this.#store.ring;
    const xAt = (i: number): number => {
      for (let j = i; j < hi; j++) {
        const v = ring.valueAt(xChannel, j);
        if (v !== null) return v;
      }
      return Number.POSITIVE_INFINITY;
    };
    let a = lo;
    let b = hi;
    while (a < b) {
      const mid = a + ((b - a) >> 1);
      const v = xAt(mid);
      if (strict ? v <= x : v < x) a = mid + 1;
      else b = mid;
    }
    return a;
  }

  /**
   * Brings the lap scan up to date with the ring (see the module comment) and derives the current lap's first
   * row and x extent. Cheap when the ring did not change.
   */
  #scanLap(): void {
    const ring = this.#store.ring;
    const xChannel = this.#xChannel;
    const length = ring.length;
    const newestX = xChannel === null || length === 0 ? null : ring.valueAt(xChannel, length - 1);
    const key = [length, ring.oldestTime(), ring.newestTime(), newestX] as const;
    const cached = this.#lapKey;
    if (cached?.every((v, i) => v === key[i]) === true) return;
    this.#lapKey = key;
    const committed = this.#lapCommitted;
    if (xChannel === null || length === 0) {
      copyLapScan(committed, newLapScan());
      copyLapScan(this.#lap, committed);
      this.#lapBoundT = Number.NaN;
      this.#lapCheckT = Number.NaN;
      this.#lapStart = 0;
      this.#lapExtent = null;
      return;
    }
    const oldest = ring.oldestTime() ?? Number.NaN;
    const newest = ring.newestTime() ?? Number.NaN;
    let from = 0;
    if (this.#canResume(xChannel, oldest, newest)) {
      from = ring.rangeIndices(this.#lapBoundT, newest).start;
    } else {
      copyLapScan(committed, newLapScan());
      this.#lapCheckT = Number.NaN;
    }
    // Rows older than the newest timestamp are final; the rows at it may still be rewritten.
    const firstNewest = ring.rangeIndices(newest, newest).start;
    for (let i = from; i < firstNewest; i++) {
      const x = ring.valueAt(xChannel, i);
      const t = ring.timeAt(i);
      if (x === null || t === null) continue;
      advanceLapScan(committed, x, t);
      this.#lapCheckT = t;
      this.#lapCheckX = x;
    }
    this.#lapBoundT = newest;
    const lap = this.#lap;
    copyLapScan(lap, committed);
    for (let i = Math.max(from, firstNewest); i < length; i++) {
      const x = ring.valueAt(xChannel, i);
      const t = ring.timeAt(i);
      if (x !== null && t !== null) advanceLapScan(lap, x, t);
    }
    if (!Number.isFinite(lap.firstT) || lap.firstT < oldest) {
      // The lap began before the oldest row still in the ring (laps are often longer than the live window).
      this.#lapStart = 0;
      this.#lapExtent = this.#ringExtent(xChannel, lap);
    } else {
      this.#lapStart = ring.rangeIndices(lap.firstT, newest).start;
      this.#lapExtent = lap.min <= lap.max ? { min: lap.min, max: lap.max } : null;
    }
  }

  /**
   * True when the committed scan still describes the ring's older rows: its boundary is inside the ring and its
   * last committed row is either still there unchanged or was evicted (rows are only appended and evicted).
   * False after the store cleared the ring (a new session or a RESET) and refilled it.
   */
  #canResume(xChannel: number, oldest: number, newest: number): boolean {
    const bound = this.#lapBoundT;
    if (!(bound >= oldest && bound <= newest)) return false;
    const checkT = this.#lapCheckT;
    if (!Number.isFinite(checkT) || checkT < oldest) return true;
    const ring = this.#store.ring;
    const { start, end } = ring.rangeIndices(checkT, checkT);
    return end > start && ring.valueAt(xChannel, start) === this.#lapCheckX;
  }

  /**
   * X extent of the current lap's rows that are still in the ring, when its first rows were evicted. On a
   * monotonic lap the minimum is the oldest row's x and the maximum the scanned one; otherwise the ring is read.
   */
  #ringExtent(xChannel: number, lap: LapScanState): XRange | null {
    const ring = this.#store.ring;
    const length = ring.length;
    if (lap.monotonic) {
      for (let i = 0; i < length; i++) {
        const x = ring.valueAt(xChannel, i);
        // Jitter may leave the oldest x a hair above the scanned maximum.
        if (x !== null) return { min: Math.min(x, lap.max), max: Math.max(x, lap.max) };
      }
      return null;
    }
    let min = Number.POSITIVE_INFINITY;
    let max = Number.NEGATIVE_INFINITY;
    for (let i = 0; i < length; i++) {
      const x = ring.valueAt(xChannel, i);
      if (x === null) continue;
      if (x < min) min = x;
      if (x > max) max = x;
    }
    return min <= max ? { min, max } : null;
  }
}
