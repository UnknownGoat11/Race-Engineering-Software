import { type JSX } from 'react';
import { vehicleClassCode } from '../../lib/classes';
import { COPY } from '../../lib/labels';
import { isLiveLooking, mayShowDriverIdentity, type EngineerView } from '../../lib/statusModel';
import { NO_VALUE } from '../../lib/units';
import { ConfirmAction } from '../ui/ConfirmAction';

export interface PitwallBarProps {
  readonly view: EngineerView;
  readonly origin?: 'lmu' | 'synthetic' | null | undefined;
  readonly driverName?: string | null | undefined;
  readonly carName?: string | null | undefined;
  readonly vehicleClass?: string | null | undefined;
  readonly trackName?: string | null | undefined;
  readonly sessionType?: string | null | undefined;
  readonly currentLap?: number | null | undefined;
  readonly currentSector?: number | null | undefined;
  readonly sessionTimeRemainingS?: number | null | undefined;
  readonly fcyActive?: boolean | undefined;
  readonly scActive?: boolean | undefined;
  readonly rttMs?: number | null | undefined;
  readonly streamAgeS?: number | null | undefined;
  readonly isConnected: boolean;
  readonly onRefreshAllData: () => void;
  readonly onLeaveSession: () => void;
  readonly onOpenSettings?: (() => void) | undefined;
}

function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return NO_VALUE;
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const remS = s % 60;
  if (h > 0) {
    return `${h}:${m.toString().padStart(2, '0')}:${remS.toString().padStart(2, '0')}`;
  }
  return `${m}:${remS.toString().padStart(2, '0')}`;
}

export function PitwallBar({
  view,
  origin,
  driverName,
  carName,
  vehicleClass,
  trackName,
  sessionType,
  currentLap,
  currentSector,
  sessionTimeRemainingS,
  fcyActive,
  scActive,
  rttMs,
  streamAgeS,
  isConnected,
  onRefreshAllData,
  onLeaveSession,
  onOpenSettings,
}: PitwallBarProps): JSX.Element {
  const showIdentity = mayShowDriverIdentity(view);
  const isLive = isLiveLooking(view);
  const isSynthetic = origin === 'synthetic' || view.kind === 'synthetic';

  // Stream badge logic strictly enforcing OR-1 (synthetic is never LIVE)
  let badgeEl: JSX.Element;
  if (isSynthetic) {
    badgeEl = (
      <span className="badge badge-dq-synthetic" data-testid="stream-health-badge">
        ▨ SYNTHETIC
      </span>
    );
  } else if (isLive) {
    badgeEl = (
      <span className="badge badge-status-ok" data-testid="stream-health-badge">
        ● LIVE STREAM
      </span>
    );
  } else if (view.kind === 'stale') {
    badgeEl = (
      <span className="badge badge-status-warn" data-testid="stream-health-badge">
        ◆ STALE
      </span>
    );
  } else if (view.kind === 'away') {
    badgeEl = (
      <span className="badge badge-status-warn" data-testid="stream-health-badge">
        ◆ DRIVER AWAY
      </span>
    );
  } else if (view.kind === 'incompatible') {
    badgeEl = (
      <span className="badge badge-status-critical" data-testid="stream-health-badge">
        ■ INCOMPATIBLE
      </span>
    );
  } else if (view.kind === 'ended') {
    badgeEl = (
      <span className="badge badge-status-critical" data-testid="stream-health-badge">
        ■ ENDED
      </span>
    );
  } else {
    badgeEl = (
      <span className="badge badge-subtle" data-testid="stream-health-badge">
        □ NOT CONNECTED
      </span>
    );
  }

  // Lap & Sector string
  const lapSectorText =
    currentLap != null
      ? `L${currentLap}${currentSector != null ? ` S${currentSector}` : ''}`
      : null;

  // Car display with class code
  const classCode = vehicleClass ? vehicleClassCode(vehicleClass) : null;
  const carDisplay = carName
    ? `${carName}${classCode ? ` (${classCode})` : ''}`
    : NO_VALUE;

  return (
    <header className="pitwall-bar" data-testid="pitwall-bar">
      <div className="pitwall-bar-left">
        {badgeEl}

        {view.kind === 'away' && (
          <span className="status-badge status-badge-warn" data-testid="driver-away-badge">
            DRIVER AWAY
          </span>
        )}

        {showIdentity && (
          <>
            <span className="pitwall-meta-item pitwall-meta-driver" data-testid="engineer-driver-name">
              {driverName ?? NO_VALUE}
            </span>
            <span className="pitwall-sep">&bull;</span>
            <span className="pitwall-meta-item pitwall-meta-car" data-testid="engineer-car">
              {carDisplay}
            </span>
            <span className="pitwall-sep">&bull;</span>
            <span className="pitwall-meta-item pitwall-meta-track" data-testid="engineer-track">
              {trackName ?? NO_VALUE}
            </span>
            {sessionType && (
              <>
                <span className="pitwall-sep">&bull;</span>
                <span className="pitwall-meta-item pitwall-meta-session">
                  {sessionType}
                </span>
              </>
            )}
            {lapSectorText && (
              <>
                <span className="pitwall-sep">&bull;</span>
                <span className="pitwall-meta-item pitwall-meta-lap">
                  {lapSectorText}
                </span>
              </>
            )}
            {sessionTimeRemainingS != null && (
              <>
                <span className="pitwall-sep">&bull;</span>
                <span className="pitwall-meta-item pitwall-meta-clock">
                  {formatClock(sessionTimeRemainingS)} left
                </span>
              </>
            )}
            {fcyActive && (
              <span className="badge badge-status-warn pitwall-flag-pill">
                FCY
              </span>
            )}
            {scActive && (
              <span className="badge badge-status-warn pitwall-flag-pill">
                SC
              </span>
            )}
          </>
        )}

        <div className="pitwall-network-meta">
          <span className="pitwall-meta-sub">RTT</span>
          <span className="num pitwall-rtt-value" data-testid="engineer-rtt">
            {rttMs != null ? `${rttMs.toFixed(0)} ms` : NO_VALUE}
          </span>
          <span className="pitwall-meta-sub">age</span>
          <span className="num pitwall-age-value" data-testid="engineer-stream-age">
            {streamAgeS != null ? `${streamAgeS.toFixed(1)}s` : NO_VALUE}
          </span>
        </div>
      </div>

      <div className="pitwall-bar-right">
        {isConnected && (
          <>
            <button
              type="button"
              className="btn btn-sm"
              onClick={onRefreshAllData}
              title="Request fresh state snapshot from driver"
              data-testid="btn-request-snapshot"
            >
              {COPY.refreshAllData}
            </button>
            <ConfirmAction
              label={COPY.leaveSession}
              prompt="Leave this driver's session?"
              consequence={COPY.leaveSessionConsequence}
              variant="danger"
              size="sm"
              onConfirm={onLeaveSession}
              testId="btn-engineer-disconnect"
            />
          </>
        )}
        {onOpenSettings && (
          <button
            type="button"
            className="btn btn-sm pitwall-settings-btn"
            onClick={onOpenSettings}
            title="Open application settings"
            aria-label="Settings"
            data-testid="btn-open-settings"
          >
            ⚙
          </button>
        )}
      </div>
    </header>
  );
}
