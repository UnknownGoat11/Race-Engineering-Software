import { describe, expect, it } from 'vitest';
import { CHANNELS, FrequencyClass, Quality, ValueType, ch } from '../generated/channels';
import { FakeClock, FakeFrames } from '../test/fakeScheduler';
import {
  f32FromBits,
  f64FromBits,
  findVector,
  loadGoldenFile,
  loadManifest,
  syntheticBatch,
  type GoldenValue,
} from '../test/golden';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../test/tlvEncoder';
import {
  ENVIRONMENT_DEGRADED_AFTER_MS,
  SPEC_DEGRADED_AFTER_MS,
  SPEC_STALE_AFTER_MS,
  STATIC_STALE_AFTER_MS,
} from './freshness';
import { TelemetryStore, type TelemetryStoreOptions } from './store';
import { UI_BATCH_FLAG_BACKFILL, UI_BATCH_FLAG_GAP_BEFORE, UI_BATCH_FLAG_RESET } from './uiBatch';

const manifest = loadManifest();

interface Harness {
  readonly store: TelemetryStore;
  readonly clock: FakeClock;
  readonly frames: FakeFrames;
  /** Applies a batch of blocks, each `[et, records]`; throws if the store refuses it. */
  readonly send: (flags: number, blocks: readonly (readonly [number, readonly FixtureRecord[]])[]) => void;
}

function harness(options: Partial<TelemetryStoreOptions> = {}): Harness {
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({
    now: clock.now,
    scheduleFrame: frames.schedule,
    scheduleTimeout: clock.scheduleTimeout,
    ...options,
  });
  let seq = 0;
  const send: Harness['send'] = (flags, blocks) => {
    seq = (flags & UI_BATCH_FLAG_RESET) !== 0 ? 0 : seq + 1;
    const encoded = blocks.map(([sourceEtS, records]) => encodeTlvBlock({ sourceEtS }, records));
    const result = store.applyBatch(encodeUiBatch(flags, seq, encoded));
    if (!result.ok) throw new Error(`batch refused: ${JSON.stringify(result.error)}`);
  };
  return { store, clock, frames, send };
}

/** The value a golden sample describes, as the store exposes it (U64 as bigint, BOOL as boolean). */
function goldenStoreValue(v: GoldenValue): unknown {
  switch (v.type) {
    case 'None':
      return null;
    case 'F32':
      return f32FromBits(v.bits);
    case 'F64':
      return f64FromBits(v.bits);
    case 'U64':
      return BigInt(v.value);
    case 'Str':
    case 'Bool':
    case 'I32':
    case 'U32':
    case 'U8':
    case 'I8':
      return v.value;
  }
}

describe('applying golden batches mirrors TelemetryFrame::apply_tlv', () => {
  const vectors = manifest.uiBatch.flatMap((v) => ('ok' in v.expect ? [[v.name, v.file, v.expect.ok] as const] : []));

  it.each(vectors)('%s', (_name, file, expected) => {
    const { store } = harness();
    const result = store.applyBatch(loadGoldenFile(file));
    const apply = expected.apply;
    if ('rejected' in apply) {
      const { blockIndex, reason } = apply.rejected;
      expect(result.ok).toBe(false);
      if (result.ok) return;
      if (reason.kind === 'SchemaMismatch') {
        expect(result.error).toEqual({
          kind: 'schemaMismatch',
          blockIndex,
          found: reason.found,
          expected: reason.expected,
        });
      } else {
        expect(result.error).toEqual({ kind: 'block', blockIndex, error: reason.error });
      }
      // Never partially applied: block 0 was valid, yet nothing of it is visible.
      expect(CHANNELS.every((def) => store.getSample(def.id).state === 'noData')).toBe(true);
      expect(store.ring.length).toBe(0);
      expect(store.getStatus()).toMatchObject({ state: 'rejected', batchesRejected: 1, batchesApplied: 0 });
      return;
    }
    if (!result.ok) throw new Error(`refused: ${JSON.stringify(result.error)}`);
    const frame = apply.accepted;
    expect({
      applied: result.value.applied,
      unknownSkipped: result.value.unknownSkipped,
      invalidated: result.value.invalidated,
    }).toEqual({ applied: frame.applied, unknownSkipped: frame.unknownSkipped, invalidated: frame.invalidated });
    if (expected.backfill) {
      // Backfilled samples are not confirmed live until a live batch arrives.
      store.applyBatch(encodeUiBatch(0, (expected.batchSeq + 1) >>> 0, []));
    }
    for (const want of frame.samples) {
      const sample = store.getSample(want.channel);
      expect(sample.quality, `quality of 0x${want.channel.toString(16)}`).toBe(want.quality);
      const wantValue = goldenStoreValue(want.value);
      expect(Object.is(sample.value, wantValue), `value of 0x${want.channel.toString(16)}`).toBe(true);
    }
  });

  it.each(manifest.uiBatchSequences.map((s) => [s.name, s] as const))('sequence %s', (_name, sequence) => {
    const { store } = harness();
    const gaps = sequence.files.map((file) => {
      const result = store.applyBatch(loadGoldenFile(file));
      if (!result.ok) throw new Error(`refused ${file}`);
      return result.value.gapBefore;
    });
    expect(gaps).toEqual(sequence.gapBefore);
  });

  it('refuses structurally invalid golden batches without touching state', () => {
    const { store, send } = harness();
    send(0, [[1, [rec.f32(ch.SPEED, 10)]]]);
    const invalid = [
      ...manifest.uiBatch.map((v) => [v.name, loadGoldenFile(v.file), v.expect] as const),
      ...manifest.uiBatchSynthetic.map((v) => [v.name, syntheticBatch(v), v.expect] as const),
    ].filter(([, , expected]) => 'error' in expected);
    expect(invalid.length).toBeGreaterThan(0);
    for (const [name, bytes, expected] of invalid) {
      const result = store.applyBatch(bytes);
      expect(result.ok, name).toBe(false);
      if (!result.ok && 'error' in expected) {
        expect(result.error).toEqual({ kind: 'envelope', error: expected.error });
      }
    }
    expect(store.getSample(ch.SPEED).value).toBe(10);
    expect(store.ring.length).toBe(1);
    expect(store.getStatus().state).toBe('rejected');
  });
});

describe('batch semantics (D-023)', () => {
  it('a refused batch leaves values and ring untouched and surfaces the error state', () => {
    const { store, send } = harness();
    send(0, [[1, [rec.f32(ch.SPEED, 10), rec.i32(ch.GEAR, 2)]]]);
    const rejected = store.applyBatch(loadGoldenFile(findVector(manifest.uiBatch, 'rejected_schema_mismatch').file));
    expect(rejected).toEqual({
      ok: false,
      error: { kind: 'schemaMismatch', blockIndex: 1, found: 2, expected: 1 },
    });
    expect(store.getSample(ch.SPEED).value).toBe(10);
    expect(store.getSample(ch.GEAR).value).toBe(2);
    expect([...store.ring.times()]).toEqual([1]);
    const status = store.getStatus();
    expect(status.state).toBe('rejected');
    expect(status.lastRejection).toEqual(rejected.ok ? null : rejected.error);
    send(0, [[2, [rec.f32(ch.SPEED, 11)]]]);
    expect(store.getStatus().state).toBe('receiving');
  });

  it('RESET discards every cached value and the ring before applying', () => {
    const { store, send } = harness();
    send(0, [
      [1, [rec.f32(ch.SPEED, 10), rec.i32(ch.GEAR, 2)]],
      [1.01, [rec.f32(ch.SPEED, 11)]],
    ]);
    send(UI_BATCH_FLAG_RESET, [[5, [rec.f32(ch.SPEED, 20)]]]);
    expect(store.getSample(ch.GEAR).state).toBe('noData');
    expect(store.getSample(ch.GEAR).quality).toBeNull();
    expect(store.getSample(ch.SPEED).value).toBe(20);
    expect([...store.ring.times()]).toEqual([5]);
    expect(store.getStatus().resets).toBe(1);
  });

  it('BACKFILL prefills the ring; values count as confirmed only after a live batch', () => {
    const { store, send } = harness();
    send(UI_BATCH_FLAG_RESET | UI_BATCH_FLAG_BACKFILL, [
      [10, [rec.f32(ch.SPEED, 1), rec.str(ch.VEHICLE_NAME, 'Oreca 07')]],
      [10.01, [rec.f32(ch.SPEED, 2)]],
    ]);
    send(UI_BATCH_FLAG_BACKFILL, [[10.02, [rec.f32(ch.SPEED, 3)]]]);
    expect([...store.ring.times()]).toEqual([10, 10.01, 10.02]);
    expect(store.ring.valueAt(ch.SPEED, 0)).toBe(1);
    // Not confirmed by a live batch: the physics value is STALE and shows no value, and so is a static
    // per-session value (the link has not shown it to be current yet).
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'stale', value: null, quality: Quality.Valid });
    expect(store.getSample(ch.VEHICLE_NAME)).toMatchObject({ state: 'stale', value: null });
    expect(store.getStreamState(FrequencyClass.Physics)).toBe('stale');
    send(0, []);
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'live', value: 3 });
    expect(store.getSample(ch.VEHICLE_NAME)).toMatchObject({ state: 'live', value: 'Oreca 07' });
    expect(store.getStreamState(FrequencyClass.Physics)).toBe('live');
  });

  it('GAP_BEFORE inserts a gap row so graphs break and point queries never bridge the drop', () => {
    const { store, send } = harness();
    send(0, [
      [100, [rec.f32(ch.SPEED, 80)]],
      [100.01, [rec.f32(ch.SPEED, 81)]],
    ]);
    send(UI_BATCH_FLAG_GAP_BEFORE, [[104.01, [rec.f32(ch.SPEED, 30)]]]);
    expect([...store.ring.times()]).toEqual([100, 100.01, 102.01, 104.01]);
    expect(store.ring.segments(ch.SPEED, 0, 200, 10)).toEqual([
      { start: 0, end: 2 },
      { start: 3, end: 4 },
    ]);
    expect(store.ring.sampleAtTime(ch.SPEED, 103, 10)).toBeNull();
    expect(store.getStatus().gaps).toBe(1);
    // A value sent after the gap is current; the gap itself asks for a resync (see the next test).
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'live', value: 30 });
    expect(store.getStatus().resyncNeeded).toBe(true);
  });

  it('a lost delta leaves every cached value unconfirmed (STALE, NaN in the ring) until re-sent or RESET (review F2)', () => {
    const { store, frames } = harness();
    const apply = (flags: number, seq: number, et: number, records: readonly FixtureRecord[]) => {
      const result = store.applyBatch(encodeUiBatch(flags, seq, [encodeTlvBlock({ sourceEtS: et }, records)]));
      if (!result.ok) throw new Error(`batch refused: ${JSON.stringify(result.error)}`);
      return result.value;
    };
    expect(apply(UI_BATCH_FLAG_RESET, 0, 1, [rec.i32(ch.GEAR, 3), rec.f32(ch.SPEED, 50)])).toMatchObject({
      gapBefore: false,
      resyncNeeded: false,
    });
    let gearCalls = 0;
    store.subscribeChannel(ch.GEAR, () => {
      gearCalls += 1;
    });
    // Batch 1 (GEAR = 4) is lost; batch 2 carries only SPEED.
    expect(apply(0, 2, 1.02, [rec.f32(ch.SPEED, 51)])).toMatchObject({ gapBefore: true, resyncNeeded: true });
    frames.runFrame();
    expect(gearCalls).toBe(1);
    expect(store.getSample(ch.GEAR)).toMatchObject({ state: 'stale', value: null, quality: Quality.Valid });
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'live', value: 51 });
    expect(store.getStatus().resyncNeeded).toBe(true);
    // Graphs break at the gap and never carry the possibly outdated gear forward.
    expect(store.ring.range(ch.GEAR, 0, 2)).toEqual([
      [1, 3],
      [1.01, null],
      [1.02, null],
    ]);
    expect(store.ring.range(ch.SPEED, 0, 2)).toEqual([
      [1, 50],
      [1.01, null],
      [1.02, 51],
    ]);
    // Batches keep flowing: the gear stays unconfirmed however long that lasts.
    for (let seq = 3; seq < 60; seq++) apply(0, seq, 1 + seq / 100, [rec.f32(ch.SPEED, seq)]);
    expect(store.getSample(ch.GEAR).state).toBe('stale');
    // Re-sent (even unchanged), it is confirmed again, but other values may still be missing.
    expect(apply(0, 60, 1.6, [rec.i32(ch.GEAR, 3)])).toMatchObject({ gapBefore: false, resyncNeeded: true });
    expect(store.getSample(ch.GEAR)).toMatchObject({ state: 'live', value: 3 });
    expect(store.ring.valueAt(ch.GEAR, store.ring.length - 1)).toBe(3);
    // A new subscription (RESET, even one flagged GAP_BEFORE) rebuilds the whole state.
    expect(apply(UI_BATCH_FLAG_RESET | UI_BATCH_FLAG_GAP_BEFORE, 0, 2, [rec.i32(ch.GEAR, 4)])).toMatchObject({
      resyncNeeded: false,
    });
    expect(store.getSample(ch.GEAR)).toMatchObject({ state: 'live', value: 4 });
    expect(store.getStatus().resyncNeeded).toBe(false);
  });

  it('treats a batch_seq jump as GAP_BEFORE and wrap-around as contiguous', () => {
    const { store } = harness();
    const block = (et: number, v: number) => encodeTlvBlock({ sourceEtS: et }, [rec.f32(ch.SPEED, v)]);
    expect(store.applyBatch(encodeUiBatch(0, 0xffffffff, [block(1, 1)])).ok).toBe(true);
    const wrapped = store.applyBatch(encodeUiBatch(0, 0, [block(2, 2)]));
    expect(wrapped.ok && wrapped.value.gapBefore).toBe(false);
    const jumped = store.applyBatch(encodeUiBatch(0, 5, [block(3, 3)]));
    expect(jumped.ok && jumped.value.gapBefore).toBe(true);
    expect(jumped.ok && jumped.value.resyncNeeded).toBe(true);
    expect(store.getStatus()).toMatchObject({ seqJumps: 1, gaps: 1, lastBatchSeq: 5, resyncNeeded: true });
    expect([...store.ring.times()]).toEqual([1, 2, 2.5, 3]);
  });

  it('clears the ring when session time goes backwards (new session)', () => {
    const { store, send } = harness();
    send(0, [
      [500, [rec.f32(ch.SPEED, 50)]],
      [500.01, [rec.f32(ch.SPEED, 51)]],
    ]);
    send(0, [[3, [rec.f32(ch.SPEED, 0.5)]]]);
    expect([...store.ring.times()]).toEqual([3]);
    expect(store.getStatus().ringClears).toBe(1);
  });

  it('keeps one ring row per session time (a repeated ET replaces the newest row)', () => {
    const { store, send } = harness();
    send(0, [
      [7, [rec.f32(ch.SPEED, 1)]],
      [7, [rec.f32(ch.SPEED, 2)]],
    ]);
    expect([...store.ring.times()]).toEqual([7]);
    expect(store.ring.valueAt(ch.SPEED, 0)).toBe(2);
  });

  it('blocks without a finite ET update values but never enter the ring', () => {
    const { store, send } = harness();
    send(0, [[Number.NaN, [rec.f32(ch.SPEED, 52)]]]);
    expect(store.getSample(ch.SPEED)).toMatchObject({ value: 52, sourceEtS: null });
    expect(store.ring.length).toBe(0);
  });

  it('stores every ring row with the current value of every ring channel (deltas are not gaps)', () => {
    const { store, send } = harness();
    send(0, [
      [1, [rec.f32(ch.SPEED, 10), rec.f32(ch.RPM, 5000)]],
      [1.01, [rec.f32(ch.SPEED, 11)]],
      [1.02, [rec.quality(ch.RPM, Quality.Unavailable)]],
    ]);
    expect(store.ring.range(ch.RPM, 0, 2)).toEqual([
      [1, 5000],
      [1.01, 5000],
      [1.02, null],
    ]);
  });

  it('keeps per-sample timestamps from the TLV header and the local receive time', () => {
    const { store, clock } = harness();
    clock.advance(1234);
    const block = encodeTlvBlock(
      { sourceEtS: 42.5, captureMonoNs: 99n, captureRelayNsEst: 1_759_000_000_000_000_000n },
      [rec.f32(ch.SPEED, 1)],
    );
    store.applyBatch(encodeUiBatch(0, 1, [block]));
    expect(store.getSample(ch.SPEED)).toMatchObject({
      receivedAtMs: 1234,
      captureMonoNs: 99n,
      captureRelayNsEst: 1_759_000_000_000_000_000n,
      sourceEtS: 42.5,
    });
    // The relay-clock sentinel reads as unknown.
    store.applyBatch(encodeUiBatch(0, 2, [encodeTlvBlock({ sourceEtS: 43 }, [rec.f32(ch.SPEED, 2)])]));
    expect(store.getSample(ch.SPEED).captureRelayNsEst).toBeNull();
  });

  it('keeps U64 values exact and exposes text and boolean values unchanged', () => {
    const { store, send } = harness();
    send(0, [
      [
        1,
        [rec.u64(ch.REC_BYTES, 2n ** 53n + 1n), rec.str(ch.VEHICLE_NAME, 'Porsche 963'), rec.bool(ch.TC_ACTIVE, true)],
      ],
    ]);
    expect(store.getSample(ch.REC_BYTES).value).toBe(2n ** 53n + 1n);
    expect(store.getSample(ch.VEHICLE_NAME).value).toBe('Porsche 963');
    expect(store.getSample(ch.TC_ACTIVE).value).toBe(true);
  });

  it('marks records that contradict the registry INVALID and never shows their value', () => {
    const { store, send } = harness();
    send(0, [
      [
        1,
        [
          rec.f64(ch.SPEED, 50), // SPEED is F32
          rec.f32(ch.RPM, Number.POSITIVE_INFINITY),
          { channel: ch.GEAR, quality: Quality.Invalid, value: { type: ValueType.I32, value: 4 } },
        ],
      ],
    ]);
    for (const id of [ch.SPEED, ch.RPM, ch.GEAR]) {
      expect(store.getSample(id)).toMatchObject({ state: 'invalid', quality: Quality.Invalid, value: null });
    }
    expect(store.getStatus().invalidated).toBe(2);
  });

  it('reads ids unknown to this build as NOT SUPPORTED and skips their records', () => {
    const { store, send } = harness();
    send(0, [[1, [rec.f32(0x0eff, 1)]]]);
    const sample = store.getSample(0x0eff);
    expect(sample).toMatchObject({ def: null, state: 'notSupported', quality: Quality.NotSupported, value: null });
    expect(store.getSample(0x0eff)).toBe(sample);
    expect(store.getStatus().unknownSkipped).toBe(1);
  });

  it('shows NO DATA, never zero, for a channel that has received nothing', () => {
    const { store } = harness();
    const sample = store.getSample(ch.FUEL);
    expect(sample).toMatchObject({ state: 'noData', quality: null, value: null, receivedAtMs: null });
    expect(store.getStreamState(FrequencyClass.Physics)).toBe('noData');
  });
});

describe('notifications: narrow, coalesced to one per animation frame', () => {
  it('notifies a channel subscriber once per frame however many batches arrived', () => {
    const { store, frames, send } = harness();
    let calls = 0;
    store.subscribeChannel(ch.SPEED, () => {
      calls += 1;
    });
    for (let i = 0; i < 5; i++) send(0, [[i, [rec.f32(ch.SPEED, i)]]]);
    expect(calls).toBe(0);
    expect(frames.pending).toBe(1);
    frames.runFrame();
    expect(calls).toBe(1);
    frames.runFrame();
    expect(calls).toBe(1);
  });

  it('does not notify subscribers of channels that did not change', () => {
    const { store, frames, send } = harness();
    send(0, [[1, [rec.f32(ch.SPEED, 1), rec.f32(ch.RPM, 1)]]]);
    frames.runFrame();
    let speedCalls = 0;
    store.subscribeChannel(ch.SPEED, () => {
      speedCalls += 1;
    });
    const before = store.getSample(ch.SPEED);
    send(0, [[1.01, [rec.f32(ch.RPM, 2), rec.f32(ch.SPEED, 1)]]]); // SPEED re-sent unchanged
    frames.runFrame();
    expect(speedCalls).toBe(0);
    expect(store.getSample(ch.SPEED)).toBe(before);
  });

  it('calls a channel-set subscriber once per frame even when several of its channels changed', () => {
    const { store, frames, send } = harness();
    let calls = 0;
    store.subscribeChannels([ch.SPEED, ch.RPM, ch.GEAR], () => {
      calls += 1;
    });
    send(0, [[1, [rec.f32(ch.SPEED, 1), rec.f32(ch.RPM, 2), rec.i32(ch.GEAR, 3)]]]);
    frames.runFrame();
    expect(calls).toBe(1);
  });

  it('keeps snapshot identity stable until the channel changes', () => {
    const { store, send } = harness();
    send(0, [[1, [rec.f32(ch.SPEED, 1)]]]);
    const a = store.getSample(ch.SPEED);
    expect(store.getSample(ch.SPEED)).toBe(a);
    send(0, [[1.01, [rec.f32(ch.SPEED, 2)]]]);
    const b = store.getSample(ch.SPEED);
    expect(b).not.toBe(a);
    expect(b.value).toBe(2);
    const selection = store.selectChannels([ch.SPEED, ch.RPM]);
    const first = selection.getSnapshot();
    expect(selection.getSnapshot()).toBe(first);
    send(0, [[1.02, [rec.f32(ch.RPM, 9)]]]);
    const second = selection.getSnapshot();
    expect(second).not.toBe(first);
    expect(second[0]).toBe(b);
  });

  it('stops notifying after unsubscribe and after dispose', () => {
    const { store, frames, send } = harness();
    let calls = 0;
    const unsubscribe = store.subscribeChannel(ch.SPEED, () => {
      calls += 1;
    });
    unsubscribe();
    send(0, [[1, [rec.f32(ch.SPEED, 1)]]]);
    frames.runFrame();
    expect(calls).toBe(0);
    store.subscribeStatus(() => {
      calls += 1;
    });
    send(0, [[2, [rec.f32(ch.SPEED, 2)]]]);
    store.dispose();
    frames.runFrame();
    expect(calls).toBe(0);
  });

  it('runs every listener even if one throws, then reports the failure', () => {
    const { store, frames, send } = harness();
    let reached = false;
    store.subscribeChannel(ch.SPEED, () => {
      throw new Error('listener failed');
    });
    store.subscribeChannel(ch.SPEED, () => {
      reached = true;
    });
    send(0, [[1, [rec.f32(ch.SPEED, 1)]]]);
    expect(() => {
      frames.runFrame();
    }).toThrow('listener failed');
    expect(reached).toBe(true);
  });

  it('notifies only what changed in each frame, also after a listener threw (reused flush set, review F8)', () => {
    const { store, frames, send } = harness();
    const calls = { speed: 0, rpm: 0, thrower: 0 };
    store.subscribeChannel(ch.SPEED, () => {
      calls.speed += 1;
    });
    store.subscribeChannel(ch.RPM, () => {
      calls.rpm += 1;
    });
    store.subscribeChannel(ch.GEAR, () => {
      calls.thrower += 1;
      throw new Error('listener failed');
    });
    send(0, [[1, [rec.f32(ch.SPEED, 1), rec.i32(ch.GEAR, 2)]]]);
    expect(() => {
      frames.runFrame();
    }).toThrow('listener failed');
    send(0, [[1.01, [rec.f32(ch.RPM, 2)]]]);
    frames.runFrame();
    send(0, [[1.02, [rec.f32(ch.RPM, 3)]]]);
    frames.runFrame();
    expect(calls).toEqual({ speed: 1, rpm: 2, thrower: 1 });
  });

  it('notifies ring subscribers when rows are appended', () => {
    const { store, frames, send } = harness();
    let calls = 0;
    store.subscribeRing(() => {
      calls += 1;
    });
    send(0, [
      [1, [rec.f32(ch.SPEED, 1)]],
      [2, [rec.f32(ch.SPEED, 2)]],
    ]);
    frames.runFrame();
    expect(calls).toBe(1);
  });
});

describe('staleness (spec §45, §126) against an injected clock', () => {
  it('moves a physics value LIVE → DEGRADED → STALE without new data, and back on the next batch', () => {
    const { store, clock, frames, send } = harness();
    const states: string[] = [];
    store.subscribeChannel(ch.SPEED, () => {
      states.push(store.getSample(ch.SPEED).state);
    });
    send(0, [[1, [rec.f32(ch.SPEED, 70)]]]);
    frames.runFrame();
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'live', value: 70 });

    clock.advance(SPEC_DEGRADED_AFTER_MS - 1);
    expect(store.getSample(ch.SPEED).state).toBe('live');
    clock.advance(1);
    frames.runFrame();
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'degraded', value: 70 });

    clock.advance(SPEC_STALE_AFTER_MS - SPEC_DEGRADED_AFTER_MS);
    expect(store.getSample(ch.SPEED).state).toBe('degraded'); // exactly 1 s: not yet stale
    clock.advance(1);
    frames.runFrame();
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'stale', value: null });
    expect(store.getStreamState(FrequencyClass.Physics)).toBe('stale');
    expect(store.liveBatchAgeMs()).toBe(SPEC_STALE_AFTER_MS + 1);

    send(0, [[2, []]]); // an empty live batch confirms every channel (deltas of complete frames)
    frames.runFrame();
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'live', value: 70 });
    expect(states).toEqual(['live', 'degraded', 'stale', 'live']);
  });

  it('uses per-class thresholds: environment data tolerates longer stalls', () => {
    const { store, clock, send } = harness();
    send(0, [[1, [rec.f32(ch.SPEED, 1), rec.f32(ch.AIR_TEMP, 21)]]]);
    clock.advance(1_500);
    expect(store.getSample(ch.SPEED).state).toBe('stale');
    expect(store.getSample(ch.AIR_TEMP).state).toBe('live');
    clock.advance(1_000);
    expect(store.getSample(ch.AIR_TEMP).state).toBe('degraded');
    clock.advance(10_000);
    expect(store.getSample(ch.AIR_TEMP).state).toBe('stale');
    expect(clock.pendingTimers).toBe(0); // nothing left to transition
  });

  it('static data is never DEGRADED but goes STALE when the link is silent for over 1 s (review F4)', () => {
    const { store, clock, send } = harness();
    send(0, [[1, [rec.str(ch.VEHICLE_NAME, 'Porsche 963')]]]);
    clock.advance(STATIC_STALE_AFTER_MS);
    expect(store.getSample(ch.VEHICLE_NAME)).toMatchObject({ state: 'live', value: 'Porsche 963' });
    clock.advance(1);
    expect(store.getSample(ch.VEHICLE_NAME)).toMatchObject({ state: 'stale', value: null });
    expect(store.getStreamState(FrequencyClass.Static)).toBe('stale');
    clock.advance(10 * 60_000); // a 10-minute outage: still STALE, never shown as current
    expect(store.getSample(ch.VEHICLE_NAME).state).toBe('stale');
    send(0, [[2, []]]);
    expect(store.getSample(ch.VEHICLE_NAME)).toMatchObject({ state: 'live', value: 'Porsche 963' });
    // A static threshold may go from LIVE straight to STALE (degradedAfterMs = Infinity is valid).
    expect(
      () =>
        new TelemetryStore({
          freshnessThresholds: { Static: { degradedAfterMs: Number.POSITIVE_INFINITY, staleAfterMs: 5 } },
        }),
    ).not.toThrow();
  });

  it('re-arms the freshness timer earlier when a batch revives a class after a stall (review F1)', () => {
    const { store, clock, send } = harness();
    send(0, [[1, [rec.f32(ch.SPEED, 50), rec.f32(ch.AIR_TEMP, 21)]]]);
    // Stall: physics goes STALE, environment DEGRADED; only environment's STALE timer is left pending.
    clock.advance(2_500);
    expect(store.getSample(ch.SPEED).state).toBe('stale');
    expect(store.getSample(ch.AIR_TEMP).state).toBe('degraded');
    send(0, [[2, [rec.f32(ch.SPEED, 60)]]]);
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'live', value: 60 });
    // Stall again: the transitions happen on time, not when the old timer fires.
    clock.advance(SPEC_DEGRADED_AFTER_MS - 1);
    expect(store.getSample(ch.SPEED).state).toBe('live');
    clock.advance(1);
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'degraded', value: 60 });
    clock.advance(SPEC_STALE_AFTER_MS - SPEC_DEGRADED_AFTER_MS);
    expect(store.getSample(ch.SPEED).state).toBe('degraded');
    clock.advance(1);
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'stale', value: null });
    // Environment was revived by the same batch: DEGRADED again exactly 2 s after it.
    clock.advance(ENVIRONMENT_DEGRADED_AFTER_MS - SPEC_STALE_AFTER_MS - 2);
    expect(store.getSample(ch.AIR_TEMP).state).toBe('live');
    clock.advance(1);
    expect(store.getSample(ch.AIR_TEMP).state).toBe('degraded');
  });

  it('keeps at most one freshness timer pending while batches stream', () => {
    const { store, clock, send } = harness();
    for (let i = 0; i < 120; i++) {
      send(0, [[i / 60, [rec.f32(ch.SPEED, i)]]]);
      clock.advance(16);
      expect(clock.pendingTimers).toBeLessThanOrEqual(1);
    }
    expect(store.getSample(ch.SPEED).state).toBe('live');
  });

  it('adds the reported LMU source age to the class age, and says when only the link is measured (review F3)', () => {
    const { store, clock, frames, send } = harness();
    send(0, [[1, [rec.f32(ch.SPEED, 50), rec.f64(ch.BEST_LAP_TIME, 100.5)]]]);
    // No source age reported: freshness covers the link only, and says so.
    expect(store.getStreamBasis(FrequencyClass.Physics)).toBe('linkOnly');
    expect(store.getStreamBasis(FrequencyClass.Scoring)).toBe('linkOnly');
    expect(store.getStreamBasis(FrequencyClass.Environment)).toBe('linkOnly');
    expect(store.getStreamBasis(FrequencyClass.Diagnostic)).toBe('endToEnd');
    expect(store.getStreamBasis(FrequencyClass.Static)).toBe('endToEnd');
    let streamCalls = 0;
    store.subscribeStream(() => {
      streamCalls += 1;
    });

    // The core reports LMU telemetry 0.5 s old: a brand-new batch is already DEGRADED.
    send(0, [[1.01, [rec.f32(ch.SRC_TELEMETRY_AGE, 0.5), rec.f32(ch.SRC_SCORING_AGE, 0)]]]);
    frames.runFrame();
    expect(streamCalls).toBe(1);
    expect(store.getStreamBasis(FrequencyClass.Physics)).toBe('endToEnd');
    expect(store.getStreamState(FrequencyClass.Physics)).toBe('degraded');
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'degraded', value: 50 });
    expect(store.streamAgeMs(FrequencyClass.Physics)).toBe(500);
    expect(store.getSample(ch.BEST_LAP_TIME)).toMatchObject({ state: 'live', value: 100.5 });
    clock.advance(SPEC_STALE_AFTER_MS - 500);
    expect(store.getSample(ch.SPEED).state).toBe('degraded');
    clock.advance(1);
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'stale', value: null });

    // LMU's scoring freezes while the core keeps publishing physics: scoring goes STALE on its own.
    send(0, [[1.02, [rec.f32(ch.SRC_TELEMETRY_AGE, 0), rec.f32(ch.SRC_SCORING_AGE, 1.5), rec.f32(ch.SPEED, 51)]]]);
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'live', value: 51 });
    expect(store.getStreamState(FrequencyClass.Scoring)).toBe('stale');
    expect(store.getSample(ch.BEST_LAP_TIME)).toMatchObject({ state: 'stale', value: null });
    expect(store.streamAgeMs(FrequencyClass.Scoring)).toBe(1_500);

    // The source age becomes unavailable: back to link-only freshness.
    send(0, [[1.03, [rec.quality(ch.SRC_SCORING_AGE, Quality.Unavailable)]]]);
    expect(store.getStreamBasis(FrequencyClass.Scoring)).toBe('linkOnly');
    expect(store.getSample(ch.BEST_LAP_TIME)).toMatchObject({ state: 'live', value: 100.5 });
    expect(store.streamAgeMs(FrequencyClass.Scoring)).toBe(0);
  });

  it('accepts configured thresholds and rejects invalid ones', () => {
    const { store, clock, send } = harness({
      freshnessThresholds: { Physics: { degradedAfterMs: 50, staleAfterMs: 100 } },
    });
    send(0, [[1, [rec.f32(ch.SPEED, 1)]]]);
    clock.advance(60);
    expect(store.getSample(ch.SPEED).state).toBe('degraded');
    expect(
      () => new TelemetryStore({ freshnessThresholds: { Physics: { degradedAfterMs: 2, staleAfterMs: 1 } } }),
    ).toThrow(RangeError);
  });

  it('an in-band STALE quality and an UNAVAILABLE sample show no value', () => {
    const { store, send } = harness();
    send(0, [[1, [rec.quality(ch.SPEED, Quality.Stale), rec.quality(ch.RPM, Quality.Unavailable)]]]);
    expect(store.getSample(ch.SPEED)).toMatchObject({ state: 'stale', value: null, quality: Quality.Stale });
    expect(store.getSample(ch.RPM)).toMatchObject({ state: 'unavailable', value: null });
  });

  it('notifies stream subscribers on class transitions only', () => {
    const { store, clock, frames, send } = harness();
    let calls = 0;
    store.subscribeStream(() => {
      calls += 1;
    });
    send(0, [[1, [rec.f32(ch.SPEED, 1)]]]);
    frames.runFrame();
    expect(calls).toBe(1); // Physics: noData → live
    send(0, [[1.01, [rec.f32(ch.SPEED, 2)]]]);
    frames.runFrame();
    expect(calls).toBe(1);
    clock.advance(SPEC_DEGRADED_AFTER_MS);
    frames.runFrame();
    expect(calls).toBe(2);
  });
});

describe('live window and ring sizing (spec §63, A-LMU-001)', () => {
  it('validates the configured window', () => {
    expect(() => new TelemetryStore({ liveWindowS: 29 })).toThrow(RangeError);
    expect(() => new TelemetryStore({ liveWindowS: 121 })).toThrow(RangeError);
    const { store } = harness({ liveWindowS: 30 });
    expect(() => {
      store.setLiveWindowS(Number.NaN);
    }).toThrow(RangeError);
    store.setLiveWindowS(120);
    expect(store.liveWindowS).toBe(120);
  });

  it('grows the ring when the measured row rate exceeds the nominal rate, keeping the whole window', () => {
    const { store } = harness({ liveWindowS: 30, ringChannels: [ch.SPEED] });
    const initialCapacity = store.ring.capacity;
    const rateHz = 250;
    let seq = 0;
    for (let i = 0; i <= 31 * rateHz; i++) {
      const block = encodeTlvBlock({ sourceEtS: i / rateHz }, [rec.f32(ch.SPEED, i % 100)]);
      store.applyBatch(encodeUiBatch(0, seq++, [block]));
    }
    expect(store.ring.capacity).toBeGreaterThan(initialCapacity);
    const newest = store.ring.newestTime() ?? 0;
    const oldest = store.ring.oldestTime() ?? 0;
    expect(newest - oldest).toBeGreaterThanOrEqual(30);
  });

  it('keeps the newest rows when the window shrinks', () => {
    const { store, send } = harness({ liveWindowS: 60, ringChannels: [ch.SPEED] });
    send(0, [
      [1, [rec.f32(ch.SPEED, 1)]],
      [2, [rec.f32(ch.SPEED, 2)]],
    ]);
    store.setLiveWindowS(30);
    expect([...store.ring.times()]).toEqual([1, 2]);
  });

  it('defaults to every provided, non-static numeric channel', () => {
    const { store } = harness();
    const ids = new Set(store.ring.channels());
    expect(ids.has(ch.SPEED)).toBe(true);
    expect(ids.has(ch.VEHICLE_NAME)).toBe(false); // text
    expect(ids.has(ch.STEERING_RANGE_PHYSICAL)).toBe(false); // static
  });

  it('reset() discards values and ring like a RESET batch', () => {
    const { store, send } = harness();
    send(0, [[1, [rec.f32(ch.SPEED, 1)]]]);
    store.reset();
    expect(store.getSample(ch.SPEED).state).toBe('noData');
    expect(store.ring.length).toBe(0);
  });
});

describe('frontend store items (DEFERRED 16, 17, 18)', () => {
  it('DEFERRED 16: stale channels are written as NaN rows in the ring while STALE label stays', () => {
    const { store, send } = harness({ ringChannels: [ch.SPEED, ch.BEST_LAP_TIME] });
    // First batch: Physics (SPEED) and Scoring (BEST_LAP_TIME)
    send(0, [[1, [rec.f32(ch.SPEED, 100), rec.f64(ch.BEST_LAP_TIME, 92.5)]]]);
    expect(store.ring.valueAt(ch.SPEED, 0)).toBe(100);
    expect(store.ring.valueAt(ch.BEST_LAP_TIME, 0)).toBe(92.5);

    // Scoring stalls while Physics continues: simulate scoring age passing stale threshold (e.g. 5 s)
    // Send physics frame with scoring age = 6 s
    send(0, [
      [
        1.01,
        [
          rec.f32(ch.SPEED, 101),
          rec.f32(ch.SRC_SCORING_AGE, 6.0), // 6 seconds old -> Scoring goes STALE
        ],
      ],
    ]);

    expect(store.getStreamState(FrequencyClass.Scoring)).toBe('stale');
    expect(store.getSample(ch.BEST_LAP_TIME).state).toBe('stale');

    // In the ring: row 1 has SPEED 101, but BEST_LAP_TIME is NaN (line breaks) and quality is Quality.Stale!
    expect(store.ring.valueAt(ch.SPEED, 1)).toBe(101);
    expect(store.ring.valueAt(ch.BEST_LAP_TIME, 1)).toBeNull(); // NaN stored, valueAt returns null
    expect(store.ring.qualityAt(ch.BEST_LAP_TIME, 1)).toBe(Quality.Stale);

    // Next physics frame: scoring still stale, still NaN in the ring
    send(0, [[1.02, [rec.f32(ch.SPEED, 102)]]]);
    expect(store.ring.valueAt(ch.SPEED, 2)).toBe(102);
    expect(store.ring.valueAt(ch.BEST_LAP_TIME, 2)).toBeNull();
    expect(store.ring.qualityAt(ch.BEST_LAP_TIME, 2)).toBe(Quality.Stale);

    // Scoring recovers: fresh scoring update arrives
    send(0, [
      [
        1.03,
        [
          rec.f32(ch.SPEED, 103),
          rec.f64(ch.BEST_LAP_TIME, 93.0),
          rec.f32(ch.SRC_SCORING_AGE, 0.05), // fresh -> Scoring recovers
        ],
      ],
    ]);

    expect(store.getStreamState(FrequencyClass.Scoring)).toBe('live');
    expect(store.getSample(ch.BEST_LAP_TIME).state).toBe('live');
    expect(store.ring.valueAt(ch.SPEED, 3)).toBe(103);
    expect(store.ring.valueAt(ch.BEST_LAP_TIME, 3)).toBe(93.0);
    expect(store.ring.qualityAt(ch.BEST_LAP_TIME, 3)).toBe(Quality.Valid);
  });

  it('DEFERRED 17: ring buffer stores per-row quality for invalid and unavailable samples', () => {
    const { store, send } = harness({ ringChannels: [ch.SPEED, ch.GEAR] });
    // Row 0: SPEED valid, GEAR invalid
    send(0, [
      [
        10,
        [
          rec.f32(ch.SPEED, 150),
          rec.quality(ch.GEAR, Quality.Invalid),
        ],
      ],
    ]);
    expect(store.ring.valueAt(ch.SPEED, 0)).toBe(150);
    expect(store.ring.qualityAt(ch.SPEED, 0)).toBe(Quality.Valid);
    expect(store.ring.valueAt(ch.GEAR, 0)).toBeNull();
    expect(store.ring.qualityAt(ch.GEAR, 0)).toBe(Quality.Invalid);

    // Row 1: SPEED unavailable, GEAR valid
    send(0, [
      [
        10.01,
        [
          rec.quality(ch.SPEED, Quality.Unavailable),
          rec.i32(ch.GEAR, 4),
        ],
      ],
    ]);
    expect(store.ring.valueAt(ch.SPEED, 1)).toBeNull();
    expect(store.ring.qualityAt(ch.SPEED, 1)).toBe(Quality.Unavailable);
    expect(store.ring.valueAt(ch.GEAR, 1)).toBe(4);
    expect(store.ring.qualityAt(ch.GEAR, 1)).toBe(Quality.Valid);
  });

  it('DEFERRED 18: store records origin and notifies status listeners on change', () => {
    const { store, frames } = harness({ origin: 'lmu' });
    expect(store.origin).toBe('lmu');
    expect(store.dataOrigin).toBe('lmu');
    expect(store.getStatus().origin).toBe('lmu');

    let statusCalls = 0;
    store.subscribeStatus(() => {
      statusCalls += 1;
    });

    store.setOrigin('synthetic');
    expect(store.origin).toBe('synthetic');
    expect(store.getStatus().origin).toBe('synthetic');
    frames.runFrame();
    expect(statusCalls).toBe(1);

    // Idempotent set does not trigger extra notification
    store.setOrigin('synthetic');
    frames.runFrame();
    expect(statusCalls).toBe(1);
  });
});
