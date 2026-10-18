import { useState, type JSX } from 'react';
import type { AlertDto } from '../../lib/ipc';
import { useScopeDq } from './presentation';

export interface AlertsBannerProps {
  readonly alerts: readonly AlertDto[];
  readonly onDismiss: (id: string) => void;
  readonly onClearAll?: () => void;
}

export function AlertsBanner({ alerts, onDismiss, onClearAll }: AlertsBannerProps): JSX.Element | null {
  const scopeDq = useScopeDq();
  const [expanded, setExpanded] = useState(false);

  // On ended session or when there are no alerts, don't show active alerts
  if (alerts.length === 0 || scopeDq === 'ended') return null;

  const isStale = scopeDq === 'stale';
  const visibleAlerts = expanded ? alerts : alerts.slice(0, 2);
  const overflowCount = alerts.length - 2;

  return (
    <div
      className={`pitwall-alert-strip ${isStale ? 'alert-strip-stale' : ''}`}
      data-testid="alerts-banner"
    >
      <div className="alert-strip-items">
        {visibleAlerts.map((alert) => {
          const badgeClass =
            alert.level === 'critical'
              ? 'badge-status-critical'
              : alert.level === 'warning'
                ? 'badge-status-warn'
                : 'badge-subtle';

          const glyph =
            alert.level === 'critical' ? '■' : alert.level === 'warning' ? '◆' : '●';

          return (
            <div
              key={alert.id}
              className={`alert-strip-row alert-strip-row-${alert.level} ${isStale ? 'alert-row-stale' : ''}`}
              data-testid={`alert-${alert.id}`}
              role={alert.level === 'critical' ? 'alert' : 'status'}
            >
              <span className={`badge ${badgeClass} alert-strip-badge`}>
                {glyph} {alert.level.toUpperCase()}
              </span>
              <span className="alert-strip-message">
                <strong>{alert.name}:</strong> {alert.message}
              </span>
              <button
                type="button"
                className="btn btn-xs alert-dismiss-btn"
                onClick={() => {
                  onDismiss(alert.id);
                }}
                aria-label={`Dismiss alert ${alert.name}`}
              >
                Dismiss
              </button>
            </div>
          );
        })}
      </div>

      {overflowCount > 0 && (
        <button
          type="button"
          className="btn btn-xs alert-overflow-badge"
          onClick={() => {
            setExpanded(!expanded);
          }}
          data-testid="alert-overflow-btn"
        >
          {expanded ? 'Show less' : `+${overflowCount} more`}
        </button>
      )}

      {onClearAll && alerts.length > 1 && (
        <button
          type="button"
          className="btn btn-xs alert-clear-all-btn"
          onClick={onClearAll}
        >
          Clear all
        </button>
      )}
    </div>
  );
}
