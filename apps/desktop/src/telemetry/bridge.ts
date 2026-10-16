/**
 * The Tauri side of the telemetry feed (D-023): a `BatchSubscribe` backed by `bridge_subscribe` and a raw
 * `Channel<ArrayBuffer>`, ready for `connectBatchSource`.
 *
 * - Each subscription attempt uses a fresh Channel. Batches may arrive before `bridge_subscribe` resolves (the
 *   backend sends RESET + BACKFILL while subscribing); they are delivered at once.
 * - Ending a subscription silences its channel immediately and calls `bridge_unsubscribe` (also when `subscribe`
 *   resolves only after the end).
 * - A failed `bridge_subscribe` is reported through `onError` (never swallowed) and retried with exponential
 *   back-off, except `backend_unavailable` (no desktop backend: retrying cannot help).
 * - The subscription's `origin` (`lmu` | `synthetic`) is exposed via `onOrigin` / `origin()`, so the store and UI
 *   can label synthetic data (DEFERRED #18). It is the origin at subscribe time; `core:status.origin` tracks
 *   later changes (e.g. the demo starting).
 */
import { Channel } from '@tauri-apps/api/core';
import { IpcError, bridgeSubscribe, bridgeUnsubscribe, type Origin } from '../lib/ipc';
import {
  RESUBSCRIBE_INITIAL_DELAY_MS,
  RESUBSCRIBE_MAX_DELAY_MS,
  connectBatchSource,
  type BatchListener,
  type BatchSubscribe,
} from './connect';
import type { ScheduleTimeout, TelemetryStore } from './store';

const BACKOFF_FACTOR = 2;

export interface BridgeSourceOptions {
  /** Called with the origin of every successful subscription. */
  readonly onOrigin?: (origin: Origin) => void;
  /** Called for every failed `bridge_subscribe` / `bridge_unsubscribe`. */
  readonly onError?: (error: IpcError) => void;
  /** Timer for subscribe retries (default `setTimeout`). */
  readonly scheduleTimeout?: ScheduleTimeout;
}

export interface BridgeSource {
  /** Pass to `connectBatchSource`. */
  readonly subscribe: BatchSubscribe;
  /** Origin of the most recent successful subscription, or null before the first one. */
  readonly origin: () => Origin | null;
}

const defaultScheduleTimeout: ScheduleTimeout = (callback, delayMs) => {
  const handle = setTimeout(callback, delayMs);
  return () => {
    clearTimeout(handle);
  };
};

function asIpcError(error: unknown): IpcError {
  if (error instanceof IpcError) return error;
  const message = error instanceof Error ? error.message : String(error);
  return new IpcError('bridge_subscribe', 'backend_error', message);
}

/** Create the Tauri batch source. */
export function createBridgeSource(options: BridgeSourceOptions = {}): BridgeSource {
  const scheduleTimeout = options.scheduleTimeout ?? defaultScheduleTimeout;
  let lastOrigin: Origin | null = null;

  const report = (error: unknown): void => {
    options.onError?.(asIpcError(error));
  };

  const subscribe: BatchSubscribe = (listener: BatchListener) => {
    let ended = false;
    let attempt = 0;
    let subscriptionId: number | null = null;
    let cancelRetry: (() => void) | null = null;
    let delayMs = RESUBSCRIBE_INITIAL_DELAY_MS;
    let channel: Channel<ArrayBuffer> | null = null;

    const silence = (): void => {
      if (channel !== null) channel.onmessage = () => undefined;
      channel = null;
    };

    const start = (): void => {
      attempt += 1;
      const mine = attempt;
      silence();
      const current = new Channel<ArrayBuffer>();
      current.onmessage = (batch: ArrayBuffer) => {
        if (!ended && mine === attempt) listener(batch);
      };
      channel = current;
      bridgeSubscribe(current).then(
        (response) => {
          if (ended || mine !== attempt) {
            bridgeUnsubscribe(response.subscription_id).catch(report);
            return;
          }
          subscriptionId = response.subscription_id;
          delayMs = RESUBSCRIBE_INITIAL_DELAY_MS;
          lastOrigin = response.origin;
          options.onOrigin?.(response.origin);
        },
        (error: unknown) => {
          if (ended || mine !== attempt) return;
          silence();
          const ipcError = asIpcError(error);
          options.onError?.(ipcError);
          if (ipcError.code === 'backend_unavailable') return;
          const wait = delayMs;
          delayMs = Math.min(delayMs * BACKOFF_FACTOR, RESUBSCRIBE_MAX_DELAY_MS);
          cancelRetry = scheduleTimeout(() => {
            cancelRetry = null;
            if (!ended) start();
          }, wait);
        },
      );
    };

    start();

    return () => {
      if (ended) return;
      ended = true;
      silence();
      cancelRetry?.();
      cancelRetry = null;
      if (subscriptionId !== null) {
        const id = subscriptionId;
        subscriptionId = null;
        bridgeUnsubscribe(id).catch(report);
      }
    };
  };

  return { subscribe, origin: () => lastOrigin };
}

export interface TauriTelemetryConnection {
  /** Ends the subscription and cancels pending retries; the store keeps its data. */
  readonly disconnect: () => void;
  /** Origin of the current subscription, or null before the first one. */
  readonly origin: () => Origin | null;
}

/** Feed `store` from the Tauri bridge (resubscribing on refused batches / resync as `connectBatchSource` does). */
export function connectTauriTelemetry(
  store: TelemetryStore,
  options: BridgeSourceOptions = {},
): TauriTelemetryConnection {
  const source = createBridgeSource(options);
  const connectOptions = options.scheduleTimeout === undefined ? {} : { scheduleTimeout: options.scheduleTimeout };
  const disconnect = connectBatchSource(store, source.subscribe, connectOptions);
  return { disconnect, origin: source.origin };
}
