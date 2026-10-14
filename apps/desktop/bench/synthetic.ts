/**
 * SYNTHETIC BENCHMARK DATA (bench harness only; never imported by the app). Deterministic generated waveforms for
 * the graph renderer benchmark: every value is a pure function of (channel slot, row), so every run and every
 * renderer sees exactly the same data. Encoded with the test TLV/UiBatch encoder and fed through the real
 * TelemetryStore, so the benchmark exercises decode → ring → live source → engine → renderer end to end.
 */
import { ValueType, ch } from '../src/generated/channels';
import { channelDef } from '../src/telemetry/registry';
import { UI_BATCH_FLAG_BACKFILL, UI_BATCH_FLAG_RESET } from '../src/telemetry/uiBatch';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../src/test/tlvEncoder';

/** Row rate of the synthetic feed (LMU's nominal rate, A-LMU-001). */
export const BENCH_RATE_HZ = 100;
/** Seconds of history backfilled before measuring: the whole live window (spec §63). */
export const BENCH_WINDOW_S = 60;
/** Session time of row 0. */
const T0_S = 1_000;
/** Seconds per gear in the gear staircase. */
const GEAR_PERIOD_S = 2.5;
const GEARS = 6;
/** Waveform amplitude as a fraction of the channel's plausible span. */
const AMPLITUDE = 0.4;
/** Span used for channels without a finite plausible range. */
const FALLBACK_HALF_SPAN = 100;
const NOISE_SHARE = 0.08;

/** The 4 graphs × 6 channels the benchmark plots (one Discrete channel to exercise step drawing). */
export const BENCH_GRAPHS: readonly (readonly number[])[] = [
  [ch.SPEED, ch.THROTTLE, ch.BRAKE, ch.STEERING, ch.GEAR, ch.RPM],
  [ch.TYRE_PRESSURE_FL, ch.TYRE_PRESSURE_FR, ch.TYRE_PRESSURE_RL, ch.TYRE_PRESSURE_RR, ch.WATER_TEMP, ch.OIL_TEMP],
  [ch.TYRE_TEMP_MIDDLE_FL, ch.TYRE_TEMP_MIDDLE_FR, ch.TYRE_TEMP_MIDDLE_RL, ch.TYRE_TEMP_MIDDLE_RR, ch.ENGINE_TORQUE, ch.TURBO_BOOST],
  [ch.CLUTCH, ch.SPEED_FORWARD, ch.STEERING_TORQUE, ch.FUEL, ch.ENGINE_POWER, ch.CLUTCH_RPM],
];

/** Every benchmark channel, in graph order. */
export const BENCH_CHANNELS: readonly number[] = BENCH_GRAPHS.flat();

/** Deterministic pseudo-random value in [-0.5, 0.5) for (slot, row) (a classic shader hash). */
function hashNoise(slot: number, row: number): number {
  const s = Math.sin(row * 12.9898 + slot * 78.233) * 43758.5453;
  return s - Math.floor(s) - 0.5;
}

interface ChannelWave {
  readonly id: number;
  readonly discrete: boolean;
  readonly f32: boolean;
  readonly mid: number;
  readonly amp: number;
  readonly f1: number;
  readonly f2: number;
  readonly phase: number;
}

function waveFor(id: number, slot: number): ChannelWave {
  const def = channelDef(id);
  const range = def?.validRange ?? null;
  const finite = range !== null && Number.isFinite(range[0]) && Number.isFinite(range[1]);
  const lo = finite ? range[0] : -FALLBACK_HALF_SPAN;
  const hi = finite ? range[1] : FALLBACK_HALF_SPAN;
  return {
    id,
    discrete: def?.interpolation === 'Discrete',
    f32: def?.valueType === ValueType.F32,
    mid: (lo + hi) / 2,
    amp: ((hi - lo) / 2) * AMPLITUDE,
    // Slow and fast components differ per channel so lines do not overlap.
    f1: 0.05 + 0.013 * slot,
    f2: 0.7 + 0.11 * slot,
    phase: slot * 0.9,
  };
}

const WAVES: readonly ChannelWave[] = BENCH_CHANNELS.map(waveFor);

/** Session time (s) of `row`. */
export function rowTime(row: number): number {
  return T0_S + row / BENCH_RATE_HZ;
}

/** Value of the channel in benchmark slot `slot` at `row`. */
export function syntheticValue(slot: number, row: number): number {
  const w = WAVES[slot];
  if (w === undefined) return Number.NaN;
  const t = row / BENCH_RATE_HZ;
  if (w.discrete) return 1 + (Math.floor(t / GEAR_PERIOD_S + slot) % GEARS);
  const slow = Math.sin(2 * Math.PI * w.f1 * t + w.phase);
  const fast = Math.sin(2 * Math.PI * w.f2 * t);
  return w.mid + w.amp * ((1 - NOISE_SHARE) * (0.7 * slow + 0.3 * fast) + NOISE_SHARE * 2 * hashNoise(slot, row));
}

function recordsFor(row: number): FixtureRecord[] {
  return WAVES.map((w, slot) => {
    const v = syntheticValue(slot, row);
    if (w.discrete) return rec.i32(w.id, v);
    return w.f32 ? rec.f32(w.id, v) : rec.f64(w.id, v);
  });
}

/** Encodes rows `[from, to)` as TLV blocks, one per row (done before measuring, so encoding is not timed). */
export function syntheticBlocks(from: number, to: number): Uint8Array[] {
  const blocks: Uint8Array[] = [];
  for (let row = from; row < to; row++) blocks.push(encodeTlvBlock({ sourceEtS: rowTime(row) }, recordsFor(row)));
  return blocks;
}

/** Wraps `blocks` into one UiBatch with sequence number `seq`. The first batch (seq 0) resets and backfills. */
export function syntheticBatchOf(blocks: readonly Uint8Array[], seq: number): Uint8Array {
  const flags = seq === 0 ? UI_BATCH_FLAG_RESET | UI_BATCH_FLAG_BACKFILL : 0;
  return encodeUiBatch(flags, seq, blocks);
}

/** Encodes rows `[from, to)` as one UiBatch with sequence number `seq`. */
export function syntheticBatch(from: number, to: number, seq: number): Uint8Array {
  return syntheticBatchOf(syntheticBlocks(from, to), seq);
}
