import { type JSX } from 'react';
import type { AlertDto } from '../../lib/ipc';

export interface RaceEvent {
  readonly id: string;
  readonly timeText: string;
  readonly kind: 'flag' | 'lap' | 'pit' | 'alert' | 'session';
  readonly message: string;
}

export interface EventsLogProps {
  readonly alerts?: readonly AlertDto[];
  readonly events?: readonly RaceEvent[];
  readonly onDismissAlert?: (id: string) => void;
}

export function EventsLog({
  alerts = [],
  events = [],
}: EventsLogProps): JSX.Element {
  // Combine alert entries and race events, sorted newest first
  const combinedItems: {
    readonly id: string;
    readonly time: string;
    readonly kind: string;
    readonly message: string;
    readonly badgeClass: string;
  }[] = [];

  for (const alert of alerts) {
    combinedItems.push({
      id: `alert-${alert.id}`,
      time: 'LIVE',
      kind: alert.level.toUpperCase(),
      message: `${alert.name}: ${alert.message}`,
      badgeClass:
        alert.level === 'critical'
          ? 'badge-status-critical'
          : alert.level === 'warning'
            ? 'badge-status-warn'
            : 'badge-subtle',
    });
  }

  for (const ev of events) {
    combinedItems.push({
      id: `ev-${ev.id}`,
      time: ev.timeText,
      kind: ev.kind.toUpperCase(),
      message: ev.message,
      badgeClass: 'badge-subtle',
    });
  }

  return (
    <div className="card events-log-panel" data-testid="events-log">
      <div className="card-header events-log-header">
        <span className="card-header-title">SESSION EVENTS &amp; LOG</span>
        <span className="card-row-label">
          {combinedItems.length} {combinedItems.length === 1 ? 'EVENT' : 'EVENTS'}
        </span>
      </div>
      <div className="events-log-body">
        {combinedItems.length === 0 ? (
          <div className="events-log-empty">
            No events recorded yet for this session.
          </div>
        ) : (
          <ul className="events-log-list" role="log" aria-label="Race events log">
            {combinedItems.map((item) => (
              <li key={item.id} className="events-log-item">
                <span className="num events-log-time">{item.time}</span>
                <span className={`badge ${item.badgeClass} events-log-badge`}>
                  {item.kind}
                </span>
                <span className="events-log-msg">{item.message}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
