// Benchmark: TelemetryStore.applyBatch at the D-023 design load: 100 Hz telemetry over the whole registry
// (~500 channels), delivered in at most 60 batches per second (1-2 blocks per batch).
// Run: pnpm exec vitest bench --run src/telemetry/store.bench.ts
import { describe, test } from 'vitest';
import { CHANNELS, ValueType, ch, type ChannelDef } from '../generated/channels';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../test/tlvEncoder';
import { TelemetryStore } from './store';
import { UI_BATCH_FLAG_RESET } from './uiBatch';

const TELEMETRY_HZ = 100;
const BATCHES_PER_S = 60;
const SECONDS_OF_FIXTURES = 10;
const STRING_CHANNEL_PERIOD = 1_000;
const NS_PER_STEP = 10_000_000n;
const MS_PER_S = 1_000;
const US_PER_MS = 1_000;

/** A record for `def` whose value changes with `step` (a delta block re-sends every changed channel). */
function record(def: ChannelDef, step: number): FixtureRecord | null {
  const wave = Math.sin(step / 10 + def.id);
  switch (def.valueType) {
    case ValueType.F32:
      return rec.f32(def.id, Math.fround(wave * 100));
    case ValueType.F64:
      return rec.f64(def.id, wave * 1000 + step);
    case ValueType.I32:
      return rec.i32(def.id, step % 7);
    case ValueType.U32:
      return rec.u32(def.id, step);
    case ValueType.U8:
      return rec.u8(def.id, step % 256);
    case ValueType.I8:
      return rec.i8(def.id, (step % 200) - 100);
    case ValueType.Bool:
      return rec.bool(def.id, step % 2 === 0);
    case ValueType.U64:
      return rec.u64(def.id, BigInt(step));
    case ValueType.Str:
      // Text channels are per-session constants; they change rarely.
      return step % STRING_CHANNEL_PERIOD === 0 ? rec.str(def.id, `value ${step}`) : null;
    case ValueType.None:
      return null;
  }
}

/** Every registry channel changes in every block: the upper bound of a 100 Hz delta stream. */
function block(step: number): Uint8Array {
  const records = CHANNELS.map((def) => record(def, step)).filter((r): r is FixtureRecord => r !== null);
  return encodeTlvBlock({ sourceEtS: step / TELEMETRY_HZ, captureMonoNs: BigInt(step) * NS_PER_STEP }, records);
}

/**
 * SECONDS_OF_FIXTURES of telemetry cut into 60 batches per second (blocks distributed 2, 2, 1, …). The first
 * batch carries RESET, so cycling through the list restarts the session cleanly.
 */
function liveBatches(): Uint8Array[] {
  const batches: Uint8Array[] = [];
  let step = 0;
  for (let b = 0; b < SECONDS_OF_FIXTURES * BATCHES_PER_S; b++) {
    const end = Math.round(((b + 1) * TELEMETRY_HZ) / BATCHES_PER_S);
    const blocks: Uint8Array[] = [];
    while (step < end) blocks.push(block(step++));
    batches.push(encodeUiBatch(b === 0 ? UI_BATCH_FLAG_RESET : 0, b, blocks));
  }
  return batches;
}

const batches = liveBatches();
const recordsPerBlock = CHANNELS.filter((def) => record(def, 1) !== null).length;
const oneSecond = batches.slice(0, BATCHES_PER_S);

/** A store with a few subscribed channels; frames and timers never fire, so only applying data is measured. */
function freshStore(): TelemetryStore {
  const store = new TelemetryStore({ scheduleFrame: () => () => undefined, scheduleTimeout: () => () => undefined });
  for (const id of [ch.SPEED, ch.RPM, ch.GEAR, ch.THROTTLE, ch.BRAKE]) {
    store.subscribeChannel(id, () => undefined);
  }
  return store;
}

/** Applies `batch`, failing the benchmark if the store refuses it. */
function apply(store: TelemetryStore, batch: Uint8Array | undefined): void {
  if (batch === undefined) throw new Error('no batch');
  const result = store.applyBatch(batch);
  if (!result.ok) throw new Error(`batch refused: ${result.error.kind}`);
}

const ONE_BATCH = 'one batch (1-2 blocks)';
const ONE_SECOND = `one second of telemetry (${BATCHES_PER_S} batches, ${TELEMETRY_HZ} blocks)`;

describe(`TelemetryStore.applyBatch: ${recordsPerBlock} records per block, ${TELEMETRY_HZ} Hz, ${BATCHES_PER_S} batches/s`, () => {
  test('applyBatch at the design load', async ({ bench }) => {
    const single = freshStore();
    let next = 0;
    const perSecond = freshStore();
    const results = await bench.compare(
      bench(ONE_BATCH, () => {
        apply(single, batches[next % batches.length]);
        next += 1;
      }),
      bench(ONE_SECOND, () => {
        for (const batch of oneSecond) apply(perSecond, batch);
      }),
    );
    const batchMs = results.get(ONE_BATCH).latency.mean;
    const secondMs = results.get(ONE_SECOND).latency.mean;
    console.info(
      `applyBatch: ${(batchMs * US_PER_MS).toFixed(1)} µs per batch; one second of telemetry costs ` +
        `${secondMs.toFixed(2)} ms (${((secondMs / MS_PER_S) * 100).toFixed(2)} % of one core)`,
    );
  });
});
