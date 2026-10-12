/**
 * React bindings for the TelemetryStore (spec §121): narrow `useSyncExternalStore` subscriptions with stable
 * snapshots, so a component re-renders only when what it shows changed, at most once per animation frame.
 * High-rate data never passes through React state.
 */
import { createContext, useCallback, useContext, useEffect, useMemo, useState, useSyncExternalStore } from 'react';
import type { FrequencyClass } from '../generated/channels';
import type { FreshnessBasis } from './freshness';
import type { ChannelSample, StreamState, TelemetryStore, TelemetryStoreStatus } from './store';

/** Provides the TelemetryStore to the hooks below: `<TelemetryStoreContext value={store}>`. */
export const TelemetryStoreContext = createContext<TelemetryStore | null>(null);

/** How often an age display refreshes while data is not live (D-023 freshness status rate: 4 Hz). */
export const AGE_REFRESH_INTERVAL_MS = 250;

/** The store from the nearest `TelemetryStoreContext`; throws when there is none. */
export function useTelemetryStore(): TelemetryStore {
  const store = useContext(TelemetryStoreContext);
  if (store === null) {
    throw new Error('useTelemetryStore needs a <TelemetryStoreContext value={store}> ancestor');
  }
  return store;
}

/** The current snapshot of one channel; re-renders only when that snapshot changes. */
export function useChannelSample(id: number): ChannelSample {
  const store = useTelemetryStore();
  const subscribe = useCallback((onChange: () => void) => store.subscribeChannel(id, onChange), [store, id]);
  const getSnapshot = useCallback(() => store.getSample(id), [store, id]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** Separator of the id list key (ids are integers, so it cannot occur inside one). */
const ID_KEY_SEPARATOR = ',';

/**
 * Snapshots of several channels, in the order given. The returned array keeps its identity until one of the
 * channels changes, so callers may pass a new `ids` array on every render.
 */
export function useChannelValues(ids: readonly number[]): readonly ChannelSample[] {
  const store = useTelemetryStore();
  const key = ids.join(ID_KEY_SEPARATOR);
  const selection = useMemo(
    () => store.selectChannels(key === '' ? [] : key.split(ID_KEY_SEPARATOR).map(Number)),
    [store, key],
  );
  return useSyncExternalStore(selection.subscribe, selection.getSnapshot, selection.getSnapshot);
}

/** Stream state (LIVE / DEGRADED / STALE / NO DATA) of one frequency class. */
export function useStreamState(frequency: FrequencyClass): StreamState {
  const store = useTelemetryStore();
  const subscribe = useCallback((onChange: () => void) => store.subscribeStream(onChange), [store]);
  const getSnapshot = useCallback(() => store.getStreamState(frequency), [store, frequency]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * What the class's stream state measures: 'endToEnd' (link age plus the LMU source age the Rust core reports)
 * or 'linkOnly' (only the link to the Rust core, while the source age is not reported). Indicators must say
 * when it is 'linkOnly' (freshness.ts).
 */
export function useStreamBasis(frequency: FrequencyClass): FreshnessBasis {
  const store = useTelemetryStore();
  const subscribe = useCallback((onChange: () => void) => store.subscribeStream(onChange), [store]);
  const getSnapshot = useCallback(() => store.getStreamBasis(frequency), [store, frequency]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** Store diagnostics (counters, last rejection). Changes with every batch: use it for diagnostics views only. */
export function useTelemetryStatus(): TelemetryStoreStatus {
  const store = useTelemetryStore();
  const subscribe = useCallback((onChange: () => void) => store.subscribeStatus(onChange), [store]);
  const getSnapshot = useCallback(() => store.getStatus(), [store]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * Refreshes `read()` every `intervalMs` while `active`; null while inactive. The clock is read in timer
 * callbacks, never during render.
 */
function usePolledAge(active: boolean, intervalMs: number, read: () => number | null): number | null {
  const [age, setAge] = useState<number | null>(null);
  useEffect(() => {
    if (!active) return undefined;
    const tick = (): void => {
      setAge(read());
    };
    const first = setTimeout(tick, 0);
    const timer = setInterval(tick, intervalMs);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
      // Forget the age of this period, so a later activation never shows it before its first tick.
      setAge(null);
    };
  }, [active, intervalMs, read]);
  return active ? age : null;
}

/**
 * Age in ms of the newest live batch, refreshed every `intervalMs` while `active` (typically while a stream is
 * DEGRADED or STALE); null while inactive or before the first live batch.
 */
export function useLiveBatchAgeMs(active: boolean, intervalMs: number = AGE_REFRESH_INTERVAL_MS): number | null {
  const store = useTelemetryStore();
  const read = useCallback(() => store.liveBatchAgeMs(), [store]);
  return usePolledAge(active, intervalMs, read);
}

/**
 * Age in ms that a frequency class's stream state is judged by (`TelemetryStore.streamAgeMs`: link age plus
 * the reported LMU source age), refreshed every `intervalMs` while `active`; null while inactive or before the
 * first live batch.
 */
export function useStreamAgeMs(
  frequency: FrequencyClass,
  active: boolean,
  intervalMs: number = AGE_REFRESH_INTERVAL_MS,
): number | null {
  const store = useTelemetryStore();
  const read = useCallback(() => store.streamAgeMs(frequency), [store, frequency]);
  return usePolledAge(active, intervalMs, read);
}
