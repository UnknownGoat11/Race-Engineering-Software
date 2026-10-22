import { type JSX } from 'react';
import type { AlertLogEntry, AlertLogKind } from '../../hooks/useAlerts';
import { formatTimeOfDay } from '../../lib/units';

export interface RaceEvent {
  readonly id: string;
  readonly timeText: string;
  readonly kind: 'flag' | 'lap' | 'pit' | 'alert' | 'session';
  readonly message: string;
}

export interface EventsLogProps {
  /** Append-only alert log (useAlerts), oldest first. */
  readonly log?: readonly AlertLogEntry[];
  readonly events?: readonly RaceEvent[];
}

const KIND_LABEL: Readonly<Record<AlertLogKind, string>> = {
  raised: 'Raised',
  updated: 'Updated',
  cleared: 'Cleared',
  dismissed: 'Dismissed',
};

/** Local wall-clock time of an entry, `hh:mm:ss`. */
function wallClock(ms: number): string {
  const d = new Date(ms);
  return formatTimeOfDay(d.getHours() * 3600 + d.getMinutes() * 60 + d.getSeconds());
}

/** Session log (audit U11): every alert raise, clear and dismissal of the session, newest first. */
export function EventsLog({ log = [], events = [] }: EventsLogProps): JSX.Element {
  const items: {
    readonly id: string;
    readonly time: string;
    readonly kind: string;
    readonly message: string;
    readonly badgeClass: string;
  }[] = [];

  for (const entry of [...log].reverse()) {
    const { alert } = entry;
    const raised = entry.kind === 'raised' || entry.kind === 'updated';
    items.push({
      id: `log-${String(entry.seq)}`,
      time: wallClock(entry.atMs),
      kind: KIND_LABEL[entry.kind],
      message: `${alert.name}: ${alert.message}`,
      badgeClass: !raised
        ? 'badge-subtle'
        : alert.level === 'critical'
          ? 'badge-status-critical'
          : alert.level === 'warning'
            ? 'badge-status-warn'
            : 'badge-subtle',
    });
  }

  for (const ev of events) {
    items.push({
      id: `ev-${ev.id}`,
      time: ev.timeText,
      kind: ev.kind.charAt(0).toUpperCase() + ev.kind.slice(1),
      message: ev.message,
      badgeClass: 'badge-subtle',
    });
  }

  return (
    <div className="card events-log-panel" data-testid="events-log">
      <div className="card-header events-log-header">
        <span className="card-header-title">Session log</span>
        <span className="card-row-label">
          <span className="num">{items.length}</span> {items.length === 1 ? 'entry' : 'entries'}
        </span>
      </div>
      <div className="events-log-body">
        {items.length === 0 ? (
          <div className="events-log-empty">No events recorded yet for this session.</div>
        ) : (
          <ul className="events-log-list" role="log" aria-label="Session log">
            {items.map((item) => (
              <li key={item.id} className="events-log-item">
                <span className="num events-log-time">{item.time}</span>
                <span className={`badge ${item.badgeClass} events-log-badge`}>{item.kind}</span>
                <span className="events-log-msg">{item.message}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
