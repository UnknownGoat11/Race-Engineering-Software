import { describe, expect, it } from 'vitest';
import { Quality, ch } from '../generated/channels';
import { FakeClock, FakeFrames } from '../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec } from '../test/tlvEncoder';
import {
  RESUBSCRIBE_INITIAL_DELAY_MS,
  RESUBSCRIBE_MAX_DELAY_MS,
  RESUBSCRIBE_STABLE_AFTER_MS,
  connectBatchSource,
  type BatchListener,
} from './connect';
import { TelemetryStore } from './store';
import { UI_BATCH_FLAG_GAP_BEFORE, UI_BATCH_FLAG_RESET } from './uiBatch';

class FakeSource {
  readonly active = new Set<BatchListener>();
  subscriptions = 0;
  unsubscriptions = 0;
  /** Batches delivered synchronously inside `subscribe` (a source replaying on subscribe). */
  onSubscribe: Uint8Array[] = [];

  readonly subscribe = (listener: BatchListener): (() => void) => {
    this.subscriptions += 1;
    this.active.add(listener);
    for (const batch of this.onSubscribe) listener(batch);
    return () => {
      this.unsubscriptions += 1;
      this.active.delete(listener);
    };
  };

  deliver(batch: Uint8Array): void {
    for (const listener of [...this.active]) listener(batch);
  }
}

const GOOD = (seq: number, speed: number): Uint8Array =>
  encodeUiBatch(seq === 0 ? UI_BATCH_FLAG_RESET : 0, seq, [
    encodeTlvBlock({ sourceEtS: seq + 1 }, [rec.f32(ch.SPEED, speed)]),
  ]);
const BAD = new Uint8Array([0x4c, 0x52, 0x55, 0x58]);

function setup(): { store: TelemetryStore; source: FakeSource; clock: FakeClock } {
  const clock = new FakeClock();
  const store = new TelemetryStore({
    now: clock.now,
    scheduleFrame: new FakeFrames().schedule,
    scheduleTimeout: clock.scheduleTimeout,
  });
  return { store, source: new FakeSource(), clock };
}

describe('connectBatchSource', () => {
  it('applies every batch the source delivers', () => {
    const { store, source, clock } = setup();
    connectBatchSource(store, source.subscribe, { scheduleTimeout: clock.scheduleTimeout });
    source.deliver(GOOD(0, 10));
    source.deliver(GOOD(1, 11));
    expect(store.getSample(ch.SPEED).value).toBe(11);
    expect(source.subscriptions).toBe(1);
  });

  it('drops the subscription after a refused batch and resubscribes after a delay', () => {
    const { store, source, clock } = setup();
    connectBatchSource(store, source.subscribe, { scheduleTimeout: clock.scheduleTimeout });
    source.deliver(GOOD(0, 10));
    source.deliver(BAD);
    expect(source.unsubscriptions).toBe(1);
    expect(source.active.size).toBe(0);
    expect(store.getStatus().state).toBe('rejected');
    clock.advance(RESUBSCRIBE_INITIAL_DELAY_MS - 1);
    expect(source.subscriptions).toBe(1);
    clock.advance(1);
    expect(source.subscriptions).toBe(2);
    source.deliver(GOOD(0, 20)); // the new subscription starts with RESET
    expect(store.getSample(ch.SPEED).value).toBe(20);
    expect(store.getStatus().state).toBe('receiving');
  });

  it('ignores batches still delivered on a dropped subscription', () => {
    const { store, source, clock } = setup();
    const captured: { first: BatchListener | null } = { first: null };
    connectBatchSource(
      store,
      (listener) => {
        captured.first ??= listener;
        return source.subscribe(listener);
      },
      { scheduleTimeout: clock.scheduleTimeout },
    );
    source.deliver(GOOD(0, 10));
    source.deliver(BAD);
    clock.advance(RESUBSCRIBE_INITIAL_DELAY_MS);
    const dropped = captured.first;
    if (dropped === null) throw new Error('no first subscription');
    dropped(GOOD(1, 99));
    expect(store.getSample(ch.SPEED).value).toBe(10);
  });

  it('backs off exponentially on consecutive refusals and resets the delay once a subscription has been stable', () => {
    const { store, source, clock } = setup();
    connectBatchSource(store, source.subscribe, { scheduleTimeout: clock.scheduleTimeout });
    const refuseAndMeasure = (): number => {
      const before = source.subscriptions;
      source.deliver(BAD);
      let waited = 0;
      while (source.subscriptions === before) {
        clock.advance(1);
        waited += 1;
      }
      return waited;
    };
    const delays: number[] = [];
    for (let i = 0; i < 8; i++) delays.push(refuseAndMeasure());
    expect(delays.slice(0, 4)).toEqual([100, 200, 400, 800]);
    expect(Math.max(...delays)).toBe(RESUBSCRIBE_MAX_DELAY_MS);
    // One good batch is not enough: a source that accepts one batch and then fails again must not restart at
    // the initial delay (that would allow a resubscribe storm).
    source.deliver(GOOD(0, 1));
    expect(refuseAndMeasure()).toBe(RESUBSCRIBE_MAX_DELAY_MS);
    // A subscription that has run for RESUBSCRIBE_STABLE_AFTER_MS resets the delay.
    source.deliver(GOOD(0, 1));
    clock.advance(RESUBSCRIBE_STABLE_AFTER_MS);
    expect(refuseAndMeasure()).toBe(RESUBSCRIBE_INITIAL_DELAY_MS);
  });

  it('resubscribes when a gap leaves the store needing a resync, and the new RESET resolves it (review F2)', () => {
    const { store, source, clock } = setup();
    connectBatchSource(store, source.subscribe, { scheduleTimeout: clock.scheduleTimeout });
    source.deliver(
      encodeUiBatch(UI_BATCH_FLAG_RESET, 0, [
        encodeTlvBlock({ sourceEtS: 1 }, [rec.i32(ch.GEAR, 3), rec.f32(ch.SPEED, 50)]),
      ]),
    );
    // Batch 1 (which changed GEAR) was lost; batch 2 is a delta that does not carry it.
    source.deliver(encodeUiBatch(0, 2, [encodeTlvBlock({ sourceEtS: 1.02 }, [rec.f32(ch.SPEED, 51)])]));
    expect(store.getStatus().resyncNeeded).toBe(true);
    expect(store.getSample(ch.GEAR).state).toBe('stale');
    expect(source.unsubscriptions).toBe(1);
    expect(source.active.size).toBe(0);
    // A GAP_BEFORE batch without RESET has the same effect.
    clock.advance(RESUBSCRIBE_INITIAL_DELAY_MS);
    expect(source.subscriptions).toBe(2);
    source.deliver(GOOD(0, 60));
    expect(source.subscriptions).toBe(2);
    source.deliver(encodeUiBatch(UI_BATCH_FLAG_GAP_BEFORE, 1, [encodeTlvBlock({ sourceEtS: 2 }, [])]));
    expect(store.getStatus().resyncNeeded).toBe(true);
    expect(source.unsubscriptions).toBe(2);
    clock.advance(2 * RESUBSCRIBE_INITIAL_DELAY_MS);
    expect(source.subscriptions).toBe(3);
    // The new subscription starts with RESET + full state: everything is confirmed again.
    source.deliver(
      encodeUiBatch(UI_BATCH_FLAG_RESET, 0, [
        encodeTlvBlock({ sourceEtS: 3 }, [rec.i32(ch.GEAR, 4), rec.f32(ch.SPEED, 70)]),
      ]),
    );
    expect(store.getStatus().resyncNeeded).toBe(false);
    expect(store.getSample(ch.GEAR)).toMatchObject({ state: 'live', value: 4 });
    expect(source.active.size).toBe(1);
  });

  it('ends a subscription superseded while subscribe() was still running', () => {
    const { store, source, clock } = setup();
    source.onSubscribe = [BAD];
    connectBatchSource(store, source.subscribe, { scheduleTimeout: clock.scheduleTimeout });
    expect(source.active.size).toBe(0);
    expect(source.unsubscriptions).toBe(1);
  });

  it('disconnect ends the subscription, cancels a pending resubscribe and keeps the data', () => {
    const { store, source, clock } = setup();
    const disconnect = connectBatchSource(store, source.subscribe, { scheduleTimeout: clock.scheduleTimeout });
    source.deliver(GOOD(0, 10));
    source.deliver(BAD);
    disconnect();
    clock.advance(RESUBSCRIBE_MAX_DELAY_MS);
    expect(source.subscriptions).toBe(1);
    expect(source.active.size).toBe(0);
    // The data is kept (and, with nothing arriving, has gone STALE rather than being discarded).
    expect(store.getSample(ch.SPEED)).toMatchObject({ quality: Quality.Valid, state: 'stale', value: null });
    expect(store.ring.length).toBe(1);
  });
});
