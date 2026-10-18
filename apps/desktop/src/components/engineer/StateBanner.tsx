import { type JSX } from 'react';
import { endedReasonLabel, incompatibleCopy } from '../../lib/labels';
import type { EngineerView } from '../../lib/statusModel';
import { formatAgeShort } from '../../lib/units';

export interface StateBannerProps {
  readonly view: EngineerView;
  readonly driverName?: string | null | undefined;
  readonly fcyActive?: boolean | undefined;
  readonly scActive?: boolean | undefined;
  readonly onRejoin?: (() => void) | undefined;
}

export function StateBanner({
  view,
  driverName,
  fcyActive,
  scActive,
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

  if (fcyActive || scActive) {
    const title = fcyActive ? 'FULL COURSE YELLOW' : 'SAFETY CAR';
    const text = fcyActive
      ? 'FCY active · Neutralized pace across entire track'
      : 'Safety Car deployed · Form queue behind SC';
    return (
      <div
        className="pitwall-state-banner pitwall-state-banner-caution"
        data-testid="state-banner-caution"
        role="status"
      >
        <div className="state-banner-content">
          <span className="badge badge-status-warn">{title}</span>
          <span className="state-banner-text">{text}</span>
        </div>
      </div>
    );
  }

  return null;
}
