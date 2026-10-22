// useAlerts state machine (audit U11): the strip shows active alerts; the session log is append-only and keeps
// raises, clears and dismissals (capped at 200). Dismiss and clear only affect the active list.
import { describe, expect, it } from 'vitest';
import type { AlertDto } from '../lib/ipc';
import { ALERT_LOG_CAP, INITIAL_ALERTS_STATE, alertsReducer, type AlertsAction, type AlertsState } from './useAlerts';

const brake: AlertDto = {
  id: 'brake_temp_fl',
  name: 'Brake temperature',
  level: 'warning',
  raised: true,
  message: 'Front-left brake peak above 650 °C.',
  timestamp_ns: 1_000_000_000,
};
const pb: AlertDto = { id: 'pb', name: 'Personal best', level: 'info', raised: true, message: 'Lap 12', timestamp_ns: 2e9 };

function run(actions: readonly AlertsAction[], from: AlertsState = INITIAL_ALERTS_STATE): AlertsState {
  return actions.reduce(alertsReducer, from);
}

describe('alertsReducer', () => {
  it('a raise is active and logged; a repeat of the same raise changes nothing', () => {
    const s = run([
      { type: 'event', alert: brake, atMs: 10 },
      { type: 'event', alert: brake, atMs: 20 },
    ]);
    expect(s.active.map((a) => a.id)).toEqual(['brake_temp_fl']);
    expect(s.active[0]?.receivedAtMs).toBe(10);
    expect(s.log.map((e) => e.kind)).toEqual(['raised']);
  });

  it('dismiss removes it from the strip only; the log keeps the raise and adds the dismissal', () => {
    const s = run([
      { type: 'event', alert: brake, atMs: 10 },
      { type: 'event', alert: pb, atMs: 11 },
      { type: 'dismiss', id: 'brake_temp_fl', atMs: 30 },
    ]);
    expect(s.active.map((a) => a.id)).toEqual(['pb']);
    expect(s.log.map((e) => `${e.kind}:${e.alert.id}`)).toEqual(['raised:brake_temp_fl', 'raised:pb', 'dismissed:brake_temp_fl']);
  });

  it('a clear from the core removes the alert and is logged', () => {
    const s = run([
      { type: 'event', alert: brake, atMs: 10 },
      { type: 'event', alert: { ...brake, raised: false }, atMs: 40 },
    ]);
    expect(s.active).toEqual([]);
    expect(s.log.map((e) => e.kind)).toEqual(['raised', 'cleared']);
  });

  it('session end empties the strip and keeps the whole log', () => {
    const s = run([
      { type: 'event', alert: brake, atMs: 10 },
      { type: 'event', alert: pb, atMs: 11 },
      { type: 'clearAll', atMs: 50, log: false },
    ]);
    expect(s.active).toEqual([]);
    expect(s.log).toHaveLength(2);
  });

  it('clear all (engineer) logs one dismissal per active alert', () => {
    const s = run([
      { type: 'event', alert: brake, atMs: 10 },
      { type: 'event', alert: pb, atMs: 11 },
      { type: 'clearAll', atMs: 50, log: true },
    ]);
    expect(s.active).toEqual([]);
    expect(s.log.map((e) => e.kind)).toEqual(['raised', 'raised', 'dismissed', 'dismissed']);
  });

  it('the log is capped at 200 entries, oldest dropped, sequence numbers keep rising', () => {
    const actions: AlertsAction[] = [];
    for (let i = 0; i < ALERT_LOG_CAP + 25; i++) {
      actions.push({ type: 'event', alert: { ...brake, id: `a${String(i)}` }, atMs: i });
    }
    const s = run(actions);
    expect(s.log).toHaveLength(ALERT_LOG_CAP);
    expect(s.log[0]?.alert.id).toBe('a25');
    expect(s.log.at(-1)?.seq).toBe(ALERT_LOG_CAP + 25);
  });

  it('an update of an active alert is logged as updated and keeps its first receipt time', () => {
    const s = run([
      { type: 'event', alert: brake, atMs: 10 },
      { type: 'event', alert: { ...brake, level: 'critical', message: 'above 750 °C' }, atMs: 60 },
    ]);
    expect(s.active[0]).toMatchObject({ level: 'critical', receivedAtMs: 10 });
    expect(s.log.map((e) => e.kind)).toEqual(['raised', 'updated']);
  });
});
