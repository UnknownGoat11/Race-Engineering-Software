import { type JSX } from 'react';
import type { AlertDto } from '../../lib/ipc';
import { useScopeDq } from './presentation';

export interface AlertsBannerProps {
  readonly alerts: readonly AlertDto[];
  readonly onDismiss: (id: string) => void;
  readonly onClearAll?: () => void;
}

export function AlertsBanner({ alerts, onDismiss, onClearAll }: AlertsBannerProps): JSX.Element | null {
  const scopeDq = useScopeDq();

  // On ended session or when there are no alerts, don't show active alerts
  if (alerts.length === 0 || scopeDq === 'ended') return null;

  const isStale = scopeDq === 'stale';

  return (
    <div
      className={`alerts-container ${isStale ? 'alerts-container-stale' : ''}`}
      data-testid="alerts-banner"
      style={{ marginBottom: '12px' }}
    >
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '6px' }}>
        <span
          className="card-row-label"
          style={{ fontWeight: 600, color: isStale ? 'var(--text-tertiary)' : 'var(--color-warn)' }}
        >
          ACTIVE ALERTS ({alerts.length}){isStale ? ' · STALE' : ''}
        </span>
        {onClearAll && alerts.length > 1 && (
          <button type="button" className="btn btn-xs" onClick={onClearAll}>
            Clear All
          </button>
        )}
      </div>
      <div className="alerts-list">
        {alerts.map((alert) => {
          const badgeClass =
            alert.level === 'critical'
              ? 'status-badge-error'
              : alert.level === 'warning'
                ? 'status-badge-warn'
                : 'status-badge-info';

          return (
            <div
              key={alert.id}
              className={`alert-item alert-item-${alert.level} ${isStale ? 'alert-item-stale' : ''}`}
              data-testid={`alert-${alert.id}`}
              role={alert.level === 'critical' ? 'alert' : 'status'}
            >
              <span className={`status-badge ${badgeClass}`} style={{ textTransform: 'uppercase' }}>
                {alert.level}
              </span>
              <span className="alert-message" style={{ flex: 1, fontSize: '13px' }}>
                <strong>{alert.name}:</strong> {alert.message}
              </span>
              <button
                type="button"
                className="btn btn-xs"
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
    </div>
  );
}

