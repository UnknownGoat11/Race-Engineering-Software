/**
 * Connects a TelemetryStore to a source of UiBatch messages (D-023). The source is a plain subscribe function,
 * so this module knows nothing about Tauri: app-core wires the `tauri::ipc::Channel` to it later.
 *
 * The subscription is dropped and a new one is made, which the bridge starts with RESET + BACKFILL (D-023 §2),
 * when the store refuses a batch (structural error or schema mismatch) or reports `resyncNeeded` (a gap lost
 * deltas, so cached values may be outdated until the full state is resent). Consecutive restarts back off
 * exponentially so a misbehaving or overloaded source cannot cause a resubscribe storm; the delay resets once a
 * subscription has run for RESUBSCRIBE_STABLE_AFTER_MS without needing a restart. Batches that arrive on a
 * dropped subscription are ignored.
 */
import type { ScheduleTimeout, TelemetryStore } from './store';

/** Receives one batch exactly as sent (an ArrayBuffer for a Tauri raw channel message). */
export type BatchListener = (batch: ArrayBuffer | ArrayBufferView) => void;
/** Starts a subscription and returns the function that ends it. */
export type BatchSubscribe = (listener: BatchListener) => () => void;

/** First resubscribe delay after a refused batch or a resync request. */
export const RESUBSCRIBE_INITIAL_DELAY_MS = 100;
/** Upper bound of the resubscribe delay. */
export const RESUBSCRIBE_MAX_DELAY_MS = 5_000;
/**
 * A subscription that has run this long without a refusal or resync resets the delay to the initial one. It is
 * at least the maximum delay, so restarts can never come faster than the back-off allows.
 */
export const RESUBSCRIBE_STABLE_AFTER_MS = RESUBSCRIBE_MAX_DELAY_MS;
const BACKOFF_FACTOR = 2;

export interface ConnectOptions {
  /** Timer used for resubscribe delays (default `setTimeout`). */
  readonly scheduleTimeout?: ScheduleTimeout;
}

const defaultScheduleTimeout: ScheduleTimeout = (callback, delayMs) => {
  const handle = setTimeout(callback, delayMs);
  return () => {
    clearTimeout(handle);
  };
};

/**
 * Feeds every batch from `subscribe` into `store`, resubscribing after a refused batch or a resync request.
 * Returns a function that ends the subscription and cancels any pending resubscribe; the store keeps its data.
 */
export function connectBatchSource(
  store: TelemetryStore,
  subscribe: BatchSubscribe,
  options: ConnectOptions = {},
): () => void {
  const scheduleTimeout = options.scheduleTimeout ?? defaultScheduleTimeout;
  let generation = 0;
  let unsubscribe: (() => void) | null = null;
  let cancelRetry: (() => void) | null = null;
  let cancelStable: (() => void) | null = null;
  let delayMs = RESUBSCRIBE_INITIAL_DELAY_MS;
  let closed = false;

  const stopStableTimer = (): void => {
    cancelStable?.();
    cancelStable = null;
  };

  const start = (): void => {
    generation += 1;
    const mine = generation;
    const end = subscribe((batch) => {
      if (closed || mine !== generation) return;
      const result = store.applyBatch(batch);
      if (result.ok && !result.value.resyncNeeded) return;
      restart();
    });
    // A source may deliver (and the store refuse) a batch before `subscribe` returns; that subscription is
    // already superseded and must be ended here.
    if (closed || mine !== generation) {
      end();
      return;
    }
    unsubscribe = end;
    stopStableTimer();
    cancelStable = scheduleTimeout(() => {
      cancelStable = null;
      delayMs = RESUBSCRIBE_INITIAL_DELAY_MS;
    }, RESUBSCRIBE_STABLE_AFTER_MS);
  };

  const restart = (): void => {
    // Invalidate the current subscription first, so nothing it still delivers is applied.
    generation += 1;
    stopStableTimer();
    const ending = unsubscribe;
    unsubscribe = null;
    ending?.();
    if (cancelRetry !== null) return;
    const wait = delayMs;
    delayMs = Math.min(delayMs * BACKOFF_FACTOR, RESUBSCRIBE_MAX_DELAY_MS);
    cancelRetry = scheduleTimeout(() => {
      cancelRetry = null;
      if (!closed) start();
    }, wait);
  };

  start();
  return () => {
    closed = true;
    generation += 1;
    cancelRetry?.();
    cancelRetry = null;
    stopStableTimer();
    const ending = unsubscribe;
    unsubscribe = null;
    ending?.();
  };
}
