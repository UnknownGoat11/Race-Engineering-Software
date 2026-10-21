/**
 * Live data access for the engineering views.
 *
 * Every view reads real telemetry from the nearest `TelemetryStoreContext` (ViewsScreen provides one fed by
 * the Tauri bridge). Without a store, or while a channel has no live value, readings are null and views render
 * NO_VALUE / an explicit "no data" state: nothing here ever substitutes a placeholder number.
 */
import { useContext, useMemo, useSyncExternalStore } from 'react';
import { CHANNEL_BY_ID } from '../generated/channels';
import { formatChannelValue, NO_VALUE } from '../lib/units';
import { TelemetryStoreContext } from '../telemetry/hooks';
import type { ChannelSample, TelemetryStore, TelemetryStoreStatus } from '../telemetry/store';

const EMPTY_SAMPLES: readonly ChannelSample[] = [];
const ID_SEPARATOR = ',';

function noopUnsubscribe(): void {
  // Nothing to release: there is no store to subscribe to.
}

function noopSubscribe(): () => void {
  return noopUnsubscribe;
}

function emptySamples(): readonly ChannelSample[] {
  return EMPTY_SAMPLES;
}

/** The nearest TelemetryStore, or null when the view is rendered without one. */
export function useOptionalTelemetryStore(): TelemetryStore | null {
  return useContext(TelemetryStoreContext);
}

/** Current live readings of a channel set. Every accessor returns null when the channel has no live value. */
export interface LiveReadings {
  /** True when a TelemetryStore is present (the view is connected to the telemetry pipeline). */
  readonly connected: boolean;
  /** True when at least one requested channel currently has a live value. */
  readonly anyValue: boolean;
  readonly num: (id: number) => number | null;
  readonly bool: (id: number) => boolean | null;
  readonly text: (id: number) => string | null;
  readonly sample: (id: number) => ChannelSample | null;
}

function toNumber(value: ChannelSample['value']): number | null {
  if (typeof value === 'number') return Number.isFinite(value) ? value : null;
  if (typeof value === 'bigint') return Number(value);
  return null;
}

/**
 * Live snapshots of `ids` from the nearest store. Unlike `useChannelValues` this does not throw without a
 * store: it then reports `connected: false` and null for every channel.
 */
export function useLiveReadings(ids: readonly number[]): LiveReadings {
  const store = useContext(TelemetryStoreContext);
  const key = ids.join(ID_SEPARATOR);
  const selection = useMemo(
    () => (store === null ? null : store.selectChannels(key === '' ? [] : key.split(ID_SEPARATOR).map(Number))),
    [store, key],
  );
  const subscribe = selection?.subscribe ?? noopSubscribe;
  const getSnapshot = selection?.getSnapshot ?? emptySamples;
  const samples = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  return useMemo(() => {
    const byId = new Map<number, ChannelSample>();
    let anyValue = false;
    for (const s of samples) {
      byId.set(s.id, s);
      if (s.value !== null) anyValue = true;
    }
    const sample = (id: number): ChannelSample | null => byId.get(id) ?? null;
    return {
      connected: store !== null,
      anyValue,
      sample,
      num: (id: number): number | null => {
        const s = byId.get(id);
        return s ? toNumber(s.value) : null;
      },
      bool: (id: number): boolean | null => {
        const v = byId.get(id)?.value;
        if (typeof v === 'boolean') return v;
        if (typeof v === 'number' && Number.isFinite(v)) return v !== 0;
        return null;
      },
      text: (id: number): string | null => {
        const v = byId.get(id)?.value;
        return typeof v === 'string' && v.trim() !== '' ? v : null;
      },
    };
  }, [samples, store]);
}

function nullStatus(): TelemetryStoreStatus | null {
  return null;
}

/** Store diagnostics (counters), or null without a store. Changes with every batch: diagnostics views only. */
export function useOptionalTelemetryStatus(): TelemetryStoreStatus | null {
  const store = useContext(TelemetryStoreContext);
  const subscribe = useMemo(
    () => (store === null ? noopSubscribe : (listener: () => void) => store.subscribeStatus(listener)),
    [store],
  );
  const getSnapshot = useMemo(() => (store === null ? nullStatus : () => store.getStatus()), [store]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * A channel value formatted with the channel's registry unit ("218 km/h", "1.82 bar"); NO_VALUE when the
 * value is missing or the channel unknown.
 */
export function formatReading(id: number, value: number | boolean | string | bigint | null | undefined): string {
  const def = CHANNEL_BY_ID.get(id);
  if (def === undefined || value === null || value === undefined) return NO_VALUE;
  const f = formatChannelValue(def, value);
  if (f.text === NO_VALUE) return NO_VALUE;
  return f.unitLabel === '' ? f.text : `${f.text} ${f.unitLabel}`;
}

/** A fraction (0..1) as a percentage with `decimals` places; NO_VALUE when missing. */
export function fracPct(frac: number | null, decimals = 1): string {
  return frac === null ? NO_VALUE : `${(frac * 100).toFixed(decimals)}%`;
}

/** Real samples of the store's live ring (the last `windowS` seconds of session time). */
export interface LiveRingSeries {
  /** Rows currently held in the ring (0 when there is no store or no data yet). */
  readonly rows: number;
  /** Seconds of session time the ring covers by configuration. */
  readonly windowS: number | null;
  /** Session time (s) of every ring row, oldest first. */
  readonly times: readonly (number | null)[];
  /** Per requested channel: its value at every ring row, oldest first; null for gaps. */
  readonly series: ReadonlyMap<number, readonly (number | null)[]>;
}

function ringKey(store: TelemetryStore | null): string {
  if (store === null) return '';
  const ring = store.ring;
  return `${String(ring.length)}:${String(ring.newestTime())}`;
}

/**
 * The live ring columns of `ids`, recomputed whenever the ring changes (at most once per animation frame,
 * as ring notifications are). Empty without a store.
 */
export function useLiveRing(ids: readonly number[]): LiveRingSeries {
  const store = useContext(TelemetryStoreContext);
  const subscribe = useMemo(
    () => (store === null ? noopSubscribe : (listener: () => void) => store.subscribeRing(listener)),
    [store],
  );
  const getSnapshot = useMemo(() => () => ringKey(store), [store]);
  const version = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  const key = ids.join(ID_SEPARATOR);

  return useMemo(() => {
    const series = new Map<number, (number | null)[]>();
    if (store === null || version === '') {
      return { rows: 0, windowS: store?.liveWindowS ?? null, times: [], series };
    }
    const ring = store.ring;
    const rows = ring.length;
    const times: (number | null)[] = new Array<number | null>(rows);
    for (let i = 0; i < rows; i++) times[i] = ring.timeAt(i);
    const wanted = key === '' ? [] : key.split(ID_SEPARATOR).map(Number);
    for (const id of wanted) {
      const column: (number | null)[] = new Array<number | null>(rows);
      for (let i = 0; i < rows; i++) column[i] = ring.valueAt(id, i);
      series.set(id, column);
    }
    return { rows, windowS: store.liveWindowS, times, series };
  }, [store, version, key]);
}
