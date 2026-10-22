import { useEffect, useState, type JSX } from 'react';
import { formatAgoText } from '../../lib/units';
import type { AlertDto } from '../../lib/ipc';
import { useScopeDq } from './presentation';

/** An alert for the strip; `receivedAtMs` (wall time it arrived) gives its age. */
export type StripAlert = AlertDto & { readonly receivedAtMs?: number };

export interface AlertsBannerProps {
  readonly alerts: readonly StripAlert[];
  readonly onDismiss: (id: string) => void;
  /** Dismisses the alerts the strip shows (warning / critical only; info alerts live in the log). */
  readonly onClearAll?: (shownIds: readonly string[]) => void;
  /** Wall clock (ms) for the alert age; defaults to `Date.now()`. */
  readonly nowMs?: number;
}

/** Re-render period of the alert age (ms). */
const AGE_REFRESH_MS = 15_000;

/** Wall clock that refreshes every `periodMs` (the alert age), or the fixed `fixedMs` when given. */
function useWallClock(fixedMs: number | undefined, periodMs: number): number {
  const [now, setNow] = useState(() => fixedMs ?? Date.now());
  useEffect(() => {
    if (fixedMs !== undefined) return undefined;
    const timer = setInterval(() => {
      setNow(Date.now());
    }, periodMs);
    return () => {
      clearInterval(timer);
    };
  }, [fixedMs, periodMs]);
  return fixedMs ?? now;
}

/**
 * Active alert strip (audit E04 / V08): warning and critical only (info goes to the session log), one row by
 * default plus "+N more", the alert age, and Clear all inside the strip.
 */
export function AlertsBanner({ alerts, onDismiss, onClearAll, nowMs }: AlertsBannerProps): JSX.Element | null {
  const scopeDq = useScopeDq();
  const [expanded, setExpanded] = useState(false);
  const now = useWallClock(nowMs, AGE_REFRESH_MS);

  const shown = alerts.filter((a) => a.level === 'warning' || a.level === 'critical');
  // Critical first, then by arrival.
  const ordered = [...shown].sort((a, b) => (a.level === b.level ? 0 : a.level === 'critical' ? -1 : 1));

  // On ended session or when there are no alerts, don't show active alerts
  if (ordered.length === 0 || scopeDq === 'ended') return null;

  const isStale = scopeDq === 'stale';
  const visibleAlerts = expanded ? ordered : ordered.slice(0, 1);
  const overflowCount = ordered.length - 1;

  return (
    <div
      className={`pitwall-alert-strip ${isStale ? 'alert-strip-stale' : ''}`}
      data-testid="alerts-banner"
    >
      <div className="alert-strip-items">
        {visibleAlerts.map((alert, idx) => {
          const badgeClass = alert.level === 'critical' ? 'badge-status-critical' : 'badge-status-warn';
          const glyph = alert.level === 'critical' ? '■' : '◆';
          const age = formatAgoText(alert.receivedAtMs, now);

          return (
            <div
              key={alert.id}
              className={`alert-strip-row alert-strip-row-${alert.level} ${isStale ? 'alert-row-stale' : ''}`}
              data-testid={`alert-${alert.id}`}
              role={alert.level === 'critical' ? 'alert' : 'status'}
            >
              <span className={`badge ${badgeClass} alert-strip-badge`}>
                <span aria-hidden="true">{glyph} </span>{alert.level === 'critical' ? 'Critical' : 'Warning'}
              </span>
              <span className="alert-strip-message">
                <strong>{alert.name}:</strong> {alert.message}
              </span>
              {age !== null && <span className="alert-strip-age">{age}</span>}
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
              {idx === 0 && overflowCount > 0 && (
                <button
                  type="button"
                  className="btn btn-xs alert-overflow-badge"
                  aria-expanded={expanded}
                  onClick={() => {
                    setExpanded(!expanded);
                  }}
                  data-testid="alert-overflow-btn"
                >
                  {expanded ? 'Show less' : `+${String(overflowCount)} more`}
                </button>
              )}
              {idx === 0 && onClearAll && ordered.length > 1 && (
                <button type="button" className="btn btn-xs alert-clear-all-btn" onClick={() => {
                    onClearAll(ordered.map((a) => a.id));
                  }}>
                  Clear all
                </button>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
