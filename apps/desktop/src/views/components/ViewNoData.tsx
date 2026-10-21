import type { JSX, ReactNode } from 'react';
import { NO_VALUE } from '../../lib/units';
import '../viewsLive.css';

export interface ViewNoDataProps {
  /** What is missing, e.g. "Lap history". */
  readonly title: string;
  /** Why there is nothing to show (which source is missing). */
  readonly reason: string;
  readonly testId?: string;
  readonly children?: ReactNode;
}

/** Explicit "no data" state: a view shows this instead of demo or placeholder numbers. */
export function ViewNoData({ title, reason, testId, children }: ViewNoDataProps): JSX.Element {
  return (
    <div className="view-no-data" data-testid={testId ?? 'view-no-data'} role="status">
      <div className="view-no-data-head">
        <span className="view-no-data-dash" aria-hidden="true">
          {NO_VALUE}
        </span>
        <span className="view-no-data-label">NO DATA</span>
        <span className="view-no-data-title">{title}</span>
      </div>
      <p className="view-no-data-reason">{reason}</p>
      {children}
    </div>
  );
}

export interface LiveCellProps {
  readonly label: string;
  readonly value: string;
  readonly note?: string | undefined;
  readonly testId?: string | undefined;
}

/** One labelled live value; `value` is already formatted (NO_VALUE when missing). */
export function LiveCell({ label, value, note, testId }: LiveCellProps): JSX.Element {
  return (
    <div className="view-live-cell" data-testid={testId}>
      <span className="view-live-label">{label}</span>
      <span className="view-live-value">{value}</span>
      {note !== undefined && <span className="view-live-note">{note}</span>}
    </div>
  );
}

/** Reason shown when the view is not connected to the telemetry pipeline at all. */
export const NOT_CONNECTED_REASON = 'No telemetry connection: this view shows live data only while the app receives telemetry.';

/** Reason shown by views that need lap/session history the app does not record. */
export const NO_HISTORY_REASON =
  'This view needs lap-by-lap history, which the app does not record yet. Nothing is shown rather than example numbers.';

/** Reason shown by multi-car views that need per-car data no source provides here. */
export const NO_FIELD_DATA_REASON =
  'Per-car data for this view is not provided by the live scoring feed. Nothing is shown rather than example numbers.';
