import { type JSX } from 'react';
import { endedReasonLabel, incompatibleCopy } from '../../lib/labels';
import type { EngineerView } from '../../lib/statusModel';
import { formatAgeShort } from '../../lib/units';

export interface StateBannerProps {
  readonly view: EngineerView;
  readonly driverName?: string | null | undefined;
  /** Session caution from scoring (`fcy` = full-course yellow or safety car; LMU does not tell them apart). */
  readonly caution?: 'fcy' | 'red' | 'none' | null | undefined;
  /** Decoded LMU yellow flag state ("Pending", "Pits closed", …), shown as the caution detail. */
  readonly cautionDetail?: string | null | undefined;
  readonly onRejoin?: (() => void) | undefined;
}

export function StateBanner({
  view,
  driverName,
  caution,
  cautionDetail,
  onRejoin,
}: StateBannerProps): JSX.Element | null {
  if (view.kind === 'incompatible') {
    const copy = incompatibleCopy(
      view.versions?.found ?? null,
      view.versions?.expected ?? null,
    );
    return (
      <div
        className="pitwall-state-banner pitwall-state-banner-incompatible"
        data-testid="state-banner-incompatible"
        role="alert"
      >
        <div className="state-banner-content">
          <span className="badge badge-status-critical">{copy.word}</span>
          <span className="state-banner-text">
            {copy.label}
            {copy.helper ? ` · ${copy.helper}` : ''}
          </span>
        </div>
      </div>
    );
  }

  if (view.kind === 'ended') {
    const copy = endedReasonLabel(view.reason, driverName);
    return (
      <div
        className="pitwall-state-banner pitwall-state-banner-ended"
        data-testid="state-banner-ended"
        role="alert"
      >
        <div className="state-banner-content">
          <span className="badge badge-status-critical">{copy.word}</span>
          <span className="state-banner-text">
            {copy.label}
            {copy.helper ? ` · ${copy.helper}` : ''}
          </span>
        </div>
        {onRejoin && (
          <div className="state-banner-action">
            <button
              type="button"
              className="btn btn-sm btn-primary"
              onClick={onRejoin}
              data-testid="btn-rejoin"
            >
              Join with new code
            </button>
          </div>
        )}
      </div>
    );
  }

  if (view.kind === 'away') {
    const elapsedText =
      view.sinceMs != null
        ? ` for ${formatAgeShort(view.sinceMs / 1000)}`
        : '';
    return (
      <div
        className="pitwall-state-banner pitwall-state-banner-away"
        data-testid="state-banner-away"
        role="status"
      >
        <div className="state-banner-content">
          <span className="badge badge-status-warn">DRIVER AWAY</span>
          <span className="state-banner-text">
            Driver away · Driver is in garage / away{elapsedText}. Live updates paused.
          </span>
        </div>
      </div>
    );
  }

  if (view.kind === 'stale') {
    const ageText = view.ageS != null ? `${view.ageS.toFixed(1)}s` : '';
    const reasonText = view.reason ? ` (${view.reason})` : '';
    return (
      <div
        className="pitwall-state-banner pitwall-state-banner-stale"
        data-testid="state-banner-stale"
        role="status"
      >
        <div className="state-banner-content">
          <span className="badge badge-status-warn">STALE</span>
          <span className="state-banner-text">
            Telemetry stale{ageText ? ` for ${ageText}` : ''}{reasonText}.
          </span>
        </div>
      </div>
    );
  }

  if (caution === 'fcy' || caution === 'red') {
    const red = caution === 'red';
    const detail = cautionDetail != null ? ` · ${cautionDetail}` : '';
    return (
      <div
        className={`pitwall-state-banner ${red ? 'pitwall-state-banner-red' : 'pitwall-state-banner-caution'}`}
        data-testid="state-banner-caution"
        role="status"
      >
        <div className="state-banner-content">
          <span className={`badge ${red ? 'badge-status-critical' : 'badge-status-warn'}`}>{red ? 'RED FLAG' : 'FCY / SC'}</span>
          <span className="state-banner-text">
            {red ? 'Race halted' : 'Full-course yellow or safety car'}
            {detail}
          </span>
        </div>
      </div>
    );
  }

  return null;
}
