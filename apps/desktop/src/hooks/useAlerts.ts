import { isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useCallback, useEffect, useReducer } from 'react';
import type { AlertDto } from '../lib/ipc';

/** An alert currently raised (the strip), with the wall time this app received it (for its age). */
export interface ActiveAlert extends AlertDto {
  readonly receivedAtMs: number;
}

export type AlertLogKind = 'raised' | 'updated' | 'cleared' | 'dismissed';

/** One entry of the append-only session log. */
export interface AlertLogEntry {
  readonly seq: number;
  readonly kind: AlertLogKind;
  /** Wall time (ms) of the entry: when the event arrived or the engineer dismissed it. */
  readonly atMs: number;
  readonly alert: AlertDto;
}

export interface AlertsState {
  readonly active: readonly ActiveAlert[];
  /** Oldest first, at most `ALERT_LOG_CAP` entries (the oldest are dropped). */
  readonly log: readonly AlertLogEntry[];
  readonly nextSeq: number;
}

/** Log entries kept (audit U11). */
export const ALERT_LOG_CAP = 200;

export const INITIAL_ALERTS_STATE: AlertsState = { active: [], log: [], nextSeq: 1 };

export type AlertsAction =
  | { readonly type: 'event'; readonly alert: AlertDto; readonly atMs: number }
  | { readonly type: 'dismiss'; readonly id: string; readonly atMs: number }
  /** `log: false` empties the strip without log entries (session end: the log keeps its history). */
  | { readonly type: 'clearAll'; readonly atMs: number; readonly log: boolean };

function append(state: AlertsState, entries: readonly Omit<AlertLogEntry, 'seq'>[]): Pick<AlertsState, 'log' | 'nextSeq'> {
  let seq = state.nextSeq;
  const added = entries.map((e) => ({ ...e, seq: seq++ }));
  const log = [...state.log, ...added];
  return { log: log.length > ALERT_LOG_CAP ? log.slice(log.length - ALERT_LOG_CAP) : log, nextSeq: seq };
}

function sameContent(a: AlertDto, b: AlertDto): boolean {
  return a.level === b.level && a.name === b.name && a.message === b.message && a.timestamp_ns === b.timestamp_ns;
}

/**
 * Alerts state machine. Dismiss and clear-all only change the active list (plus a log entry); a raise or clear
 * from the core is always logged; nothing ever removes a log entry except the cap.
 */
export function alertsReducer(state: AlertsState, action: AlertsAction): AlertsState {
  switch (action.type) {
    case 'event': {
      const { alert, atMs } = action;
      const index = state.active.findIndex((a) => a.id === alert.id);
      if (!alert.raised) {
        const last = [...state.log].reverse().find((e) => e.alert.id === alert.id);
        const active = index >= 0 ? state.active.filter((a) => a.id !== alert.id) : state.active;
        if (last?.kind === 'cleared' && index < 0) return state;
        return { active, ...append(state, [{ kind: 'cleared', atMs, alert }]) };
      }
      const current = index >= 0 ? state.active[index] : undefined;
      // A repeat of the same raise (a late listener replay) changes nothing.
      if (current !== undefined && sameContent(current, alert)) return state;
      const entry: ActiveAlert = { ...alert, receivedAtMs: current?.receivedAtMs ?? atMs };
      const active = index >= 0 ? state.active.map((a, i) => (i === index ? entry : a)) : [...state.active, entry];
      return { active, ...append(state, [{ kind: current === undefined ? 'raised' : 'updated', atMs, alert }]) };
    }
    case 'dismiss': {
      const alert = state.active.find((a) => a.id === action.id);
      if (alert === undefined) return state;
      return {
        active: state.active.filter((a) => a.id !== action.id),
        ...append(state, [{ kind: 'dismissed', atMs: action.atMs, alert }]),
      };
    }
    case 'clearAll': {
      if (state.active.length === 0) return state;
      if (!action.log) return { ...state, active: [] };
      return {
        active: [],
        ...append(
          state,
          state.active.map((alert) => ({ kind: 'dismissed' as const, atMs: action.atMs, alert })),
        ),
      };
    }
  }
}

export interface UseAlertsResult {
  /** Active alerts (the strip). */
  readonly alerts: readonly ActiveAlert[];
  /** Append-only session log (oldest first, capped at `ALERT_LOG_CAP`). */
  readonly log: readonly AlertLogEntry[];
  readonly dismissAlert: (id: string) => void;
  /** Empties the strip; `{ log: false }` (session end) adds no log entries. The log itself is kept. */
  readonly clearAllAlerts: (options?: { readonly log?: boolean }) => void;
}

/**
 * Hook to listen to `core:alert` events: the active alerts plus an append-only log of every raise, clear and
 * dismiss (audit U11).
 */
export function useAlerts(): UseAlertsResult {
  const [state, dispatch] = useReducer(alertsReducer, INITIAL_ALERTS_STATE);

  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: UnlistenFn | null = null;
    let disposed = false;

    listen<AlertDto>('core:alert', (event) => {
      if (disposed) return;
      dispatch({ type: 'event', alert: event.payload, atMs: Date.now() });
    })
      .then((fn) => {
        if (disposed) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((err: unknown) => {
        console.warn('Failed to listen to core:alert', err);
      });

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
        unlisten = null;
      }
    };
  }, []);

  const dismissAlert = useCallback((id: string) => {
    dispatch({ type: 'dismiss', id, atMs: Date.now() });
  }, []);

  const clearAllAlerts = useCallback((options?: { readonly log?: boolean }) => {
    dispatch({ type: 'clearAll', atMs: Date.now(), log: options?.log ?? true });
  }, []);

  return { alerts: state.active, log: state.log, dismissAlert, clearAllAlerts };
}
