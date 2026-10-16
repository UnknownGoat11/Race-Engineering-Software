import { invoke, isTauri } from '@tauri-apps/api/core';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ch } from '../generated/channels';
import { IpcError, type BridgeSubscribeResponse, type Origin } from '../lib/ipc';
import { FakeClock, FakeFrames } from '../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec } from '../test/tlvEncoder';
import { connectTauriTelemetry, createBridgeSource } from './bridge';
import { RESUBSCRIBE_INITIAL_DELAY_MS } from './connect';
import { TelemetryStore } from './store';
import { UI_BATCH_FLAG_RESET } from './uiBatch';

vi.mock('@tauri-apps/api/core', () => {
  class Channel<T> {
    static readonly created: Channel<unknown>[] = [];
    onmessage: (message: T) => void = () => undefined;
    constructor() {
      Channel.created.push(this as Channel<unknown>);
    }
  }
  return { Channel, invoke: vi.fn(), isTauri: vi.fn(() => true) };
});

interface MockChannel {
  onmessage: (message: ArrayBuffer) => void;
}

/** Channels created by the code under test, in creation order. */
async function channels(): Promise<MockChannel[]> {
  const core = (await import('@tauri-apps/api/core')) as unknown as { Channel: { created: MockChannel[] } };
  return core.Channel.created;
}

interface Pending {
  resolve: (value: BridgeSubscribeResponse) => void;
  reject: (error: unknown) => void;
}

/** Scriptable backend: bridge_subscribe calls stay pending until resolved by the test. */
class Backend {
  readonly subscribes: Pending[] = [];
  readonly unsubscribed: number[] = [];

  install(): void {
    vi.mocked(isTauri).mockReturnValue(true);
    vi.mocked(invoke).mockImplementation((command: string, args?: unknown) => {
      if (command === 'bridge_subscribe') {
        const onBatch = (args as { on_batch?: unknown } | undefined)?.on_batch;
        if (onBatch === undefined) return Promise.reject(new Error('on_batch missing'));
        return new Promise((resolve, reject) => {
          this.subscribes.push({ resolve, reject });
        });
      }
      if (command === 'bridge_unsubscribe') {
        this.unsubscribed.push((args as { subscription_id: number }).subscription_id);
        return Promise.resolve(null);
      }
      return Promise.reject(new Error(`unexpected command ${command}`));
    });
  }

  accept(index: number, subscriptionId: number, origin: Origin = 'lmu'): void {
    const pending = this.subscribes[index];
    if (pending === undefined) throw new Error(`no subscribe call ${index}`);
    pending.resolve({ subscription_id: subscriptionId, origin });
  }

  refuse(index: number, code: string, message = 'refused'): void {
    const pending = this.subscribes[index];
    if (pending === undefined) throw new Error(`no subscribe call ${index}`);
    pending.reject({ code, message });
  }
}

const flush = async (): Promise<void> => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};

const toArrayBuffer = (bytes: Uint8Array): ArrayBuffer => bytes.slice().buffer;

const batch = (flags: number, seq: number, speed: number): ArrayBuffer =>
  toArrayBuffer(encodeUiBatch(flags, seq, [encodeTlvBlock({ sourceEtS: seq + 1 }, [rec.f32(ch.SPEED, speed)])]));

function newStore(clock: FakeClock): TelemetryStore {
  return new TelemetryStore({
    now: clock.now,
    scheduleFrame: new FakeFrames().schedule,
    scheduleTimeout: clock.scheduleTimeout,
  });
}

describe('createBridgeSource', () => {
  let backend: Backend;
  let clock: FakeClock;

  beforeEach(async () => {
    (await channels()).length = 0;
    backend = new Backend();
    backend.install();
    clock = new FakeClock();
  });

  it('subscribes with a raw channel, delivers batches (also before subscribe resolves) and reports the origin', async () => {
    const origins: Origin[] = [];
    const source = createBridgeSource({ onOrigin: (o) => origins.push(o), scheduleTimeout: clock.scheduleTimeout });
    const received: unknown[] = [];
    source.subscribe((b) => received.push(b));

    expect(invoke).toHaveBeenCalledWith('bridge_subscribe', { on_batch: (await channels())[0] });
    const [channel] = await channels();
    if (channel === undefined) throw new Error('no channel');
    const early = batch(UI_BATCH_FLAG_RESET, 0, 1);
    channel.onmessage(early); // backfill arrives while bridge_subscribe is still pending
    expect(received).toEqual([early]);
    expect(source.origin()).toBeNull();

    backend.accept(0, 7, 'synthetic');
    await flush();
    expect(source.origin()).toBe('synthetic');
    expect(origins).toEqual(['synthetic']);
  });

  it('unsubscribe calls bridge_unsubscribe with the id and silences the channel', async () => {
    const source = createBridgeSource({ scheduleTimeout: clock.scheduleTimeout });
    const received: unknown[] = [];
    const end = source.subscribe((b) => received.push(b));
    backend.accept(0, 42);
    await flush();
    const [channel] = await channels();
    if (channel === undefined) throw new Error('no channel');

    end();
    end(); // idempotent
    await flush();
    expect(backend.unsubscribed).toEqual([42]);
    channel.onmessage(batch(0, 1, 2));
    expect(received).toEqual([]);
  });

  it('a subscription ended before bridge_subscribe resolves is unsubscribed once it resolves', async () => {
    const origins: Origin[] = [];
    const source = createBridgeSource({ onOrigin: (o) => origins.push(o), scheduleTimeout: clock.scheduleTimeout });
    const end = source.subscribe(() => undefined);
    end();
    expect(backend.unsubscribed).toEqual([]);
    backend.accept(0, 9);
    await flush();
    expect(backend.unsubscribed).toEqual([9]);
    expect(origins).toEqual([]);
  });

  it('reports a failed subscribe and retries with back-off on a new channel', async () => {
    const errors: IpcError[] = [];
    const source = createBridgeSource({ onError: (e) => errors.push(e), scheduleTimeout: clock.scheduleTimeout });
    const received: unknown[] = [];
    source.subscribe((b) => received.push(b));
    backend.refuse(0, 'bridge_closed');
    await flush();
    expect(errors).toHaveLength(1);
    expect(errors[0]).toBeInstanceOf(IpcError);
    expect(errors[0]?.code).toBe('bridge_closed');

    clock.advance(RESUBSCRIBE_INITIAL_DELAY_MS - 1);
    expect(backend.subscribes).toHaveLength(1);
    clock.advance(1);
    expect(backend.subscribes).toHaveLength(2);
    const created = await channels();
    expect(created).toHaveLength(2);
    created[0]?.onmessage(batch(0, 0, 1)); // the failed attempt's channel is silent
    expect(received).toEqual([]);

    backend.refuse(1, 'bridge_closed');
    await flush();
    clock.advance(2 * RESUBSCRIBE_INITIAL_DELAY_MS - 1);
    expect(backend.subscribes).toHaveLength(2);
    clock.advance(1);
    expect(backend.subscribes).toHaveLength(3);
  });

  it('does not retry when there is no desktop backend, and ending cancels a pending retry', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    const errors: IpcError[] = [];
    const source = createBridgeSource({ onError: (e) => errors.push(e), scheduleTimeout: clock.scheduleTimeout });
    source.subscribe(() => undefined);
    await flush();
    expect(errors.map((e) => e.code)).toEqual(['backend_unavailable']);
    clock.advance(10_000);
    expect(invoke).not.toHaveBeenCalled();

    vi.mocked(isTauri).mockReturnValue(true);
    const end = source.subscribe(() => undefined);
    backend.refuse(0, 'bridge_closed');
    await flush();
    end();
    clock.advance(10_000);
    expect(backend.subscribes).toHaveLength(1);
  });

  it('reports a failed unsubscribe instead of swallowing it', async () => {
    const errors: IpcError[] = [];
    const source = createBridgeSource({ onError: (e) => errors.push(e), scheduleTimeout: clock.scheduleTimeout });
    const end = source.subscribe(() => undefined);
    backend.accept(0, 3);
    await flush();
    vi.mocked(invoke).mockRejectedValueOnce({ code: 'core_shutdown', message: 'gone' });
    end();
    await flush();
    expect(errors.map((e) => e.code)).toEqual(['core_shutdown']);
  });
});

describe('connectTauriTelemetry', () => {
  let backend: Backend;
  let clock: FakeClock;

  beforeEach(async () => {
    (await channels()).length = 0;
    backend = new Backend();
    backend.install();
    clock = new FakeClock();
  });

  it('batches from the channel reach the store; a refused batch resubscribes through the bridge', async () => {
    const store = newStore(clock);
    const connection = connectTauriTelemetry(store, { scheduleTimeout: clock.scheduleTimeout });
    backend.accept(0, 1, 'lmu');
    await flush();
    expect(connection.origin()).toBe('lmu');

    const first = (await channels())[0];
    if (first === undefined) throw new Error('no channel');
    first.onmessage(batch(UI_BATCH_FLAG_RESET, 0, 10));
    first.onmessage(batch(0, 1, 11));
    expect(store.getSample(ch.SPEED).value).toBe(11);

    first.onmessage(toArrayBuffer(new Uint8Array([0x4c, 0x52, 0x55, 0x58]))); // refused by the store
    await flush();
    expect(backend.unsubscribed).toEqual([1]);
    clock.advance(RESUBSCRIBE_INITIAL_DELAY_MS);
    expect(backend.subscribes).toHaveLength(2);
    backend.accept(1, 2, 'synthetic');
    await flush();
    expect(connection.origin()).toBe('synthetic');

    const second = (await channels())[1];
    if (second === undefined) throw new Error('no second channel');
    second.onmessage(batch(UI_BATCH_FLAG_RESET, 0, 20));
    expect(store.getSample(ch.SPEED).value).toBe(20);

    connection.disconnect();
    await flush();
    expect(backend.unsubscribed).toEqual([1, 2]);
  });
});
