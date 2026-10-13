/**
 * Frame-time statistics for graph rendering (spec §121 performance targets): a fixed-capacity recorder of
 * durations (no allocation per frame, no array shifting) and nearest-rank percentiles. Used by the engine's
 * diagnostics and by the benchmark harness.
 */

/** Summary of a set of durations in milliseconds. All fields are NaN for an empty set. */
export interface DurationSummary {
  readonly count: number;
  readonly mean: number;
  readonly p50: number;
  readonly p95: number;
  readonly p99: number;
  readonly max: number;
  /** Durations strictly above the long-frame threshold. */
  readonly long: number;
}

/** Nearest-rank percentile `p` (0–100) of ascending `sorted` values; NaN when empty. */
export function percentile(sorted: ArrayLike<number>, p: number): number {
  const n = sorted.length;
  if (n === 0) return Number.NaN;
  const rank = Math.min(n, Math.max(1, Math.ceil((p / 100) * n)));
  return sorted[rank - 1] ?? Number.NaN;
}

const P50 = 50;
const P95 = 95;
const P99 = 99;

/** Summarises `durations`, counting those above `longMs` as long frames. */
export function summarize(durations: ArrayLike<number>, longMs: number): DurationSummary {
  const sorted = Float64Array.from(durations).sort();
  const n = sorted.length;
  let sum = 0;
  let long = 0;
  for (const d of sorted) {
    sum += d;
    if (d > longMs) long += 1;
  }
  return {
    count: n,
    mean: n === 0 ? Number.NaN : sum / n,
    p50: percentile(sorted, P50),
    p95: percentile(sorted, P95),
    p99: percentile(sorted, P99),
    max: n === 0 ? Number.NaN : (sorted[n - 1] ?? Number.NaN),
    long,
  };
}

/** Keeps the newest `capacity` durations in a ring. */
export class DurationRecorder {
  readonly #values: Float64Array;
  #next = 0;
  #count = 0;

  constructor(capacity: number) {
    this.#values = new Float64Array(Math.max(1, Math.floor(capacity)));
  }

  /** Number of stored durations. */
  get count(): number {
    return this.#count;
  }

  /** Records one duration (ms); non-finite values are ignored. */
  record(ms: number): void {
    if (!Number.isFinite(ms)) return;
    this.#values[this.#next] = ms;
    this.#next = (this.#next + 1) % this.#values.length;
    if (this.#count < this.#values.length) this.#count += 1;
  }

  /** Stored durations, oldest first (a copy). */
  values(): Float64Array {
    const out = new Float64Array(this.#count);
    const capacity = this.#values.length;
    const first = (this.#next - this.#count + capacity) % capacity;
    for (let i = 0; i < this.#count; i++) out[i] = this.#values[(first + i) % capacity] ?? Number.NaN;
    return out;
  }

  /** Forgets every duration. */
  reset(): void {
    this.#next = 0;
    this.#count = 0;
  }
}
