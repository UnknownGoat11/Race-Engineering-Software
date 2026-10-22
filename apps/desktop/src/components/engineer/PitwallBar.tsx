import { type JSX } from 'react';
import { vehicleClassCode } from '../../lib/classes';
import { COPY } from '../../lib/labels';
import { isLiveLooking, mayShowDriverIdentity, type EngineerView } from '../../lib/statusModel';
import { formatClock, NO_VALUE } from '../../lib/units';
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
  /** Session caution from scoring: `fcy` = full-course yellow / safety car (LMU does not tell them apart). */
  readonly caution?: 'fcy' | 'red' | 'none' | null | undefined;
  readonly rttMs?: number | null | undefined;
  readonly streamAgeS?: number | null | undefined;
  readonly isConnected: boolean;
  readonly onRefreshAllData: () => void;
  readonly onLeaveSession: () => void;
  /** The session has ended: one Close button replaces Resync / Leave (audit U18). */
  readonly ended?: boolean | undefined;
  readonly onClose?: (() => void) | undefined;
  readonly onOpenSettings?: (() => void) | undefined;
  /** Opens the Analysis page (the shell header is hidden while connected, so the bar offers the route). */
  readonly onOpenAnalysis?: (() => void) | undefined;
}

/** Settings gear: one inline stroke icon in currentColor (no emoji, V13). */
function GearIcon(): JSX.Element {
  return (
    <svg
      className="pitwall-gear-icon"
      width="12"
      height="12"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      aria-hidden="true"
      focusable="false"
    >
      <circle cx="8" cy="8" r="2.25" />
      <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4" />
    </svg>
  );
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
  caution,
  rttMs,
  streamAgeS,
  isConnected,
  onRefreshAllData,
  onLeaveSession,
  ended,
  onClose,
  onOpenSettings,
  onOpenAnalysis,
}: PitwallBarProps): JSX.Element {
  const showIdentity = mayShowDriverIdentity(view);
  const isLive = isLiveLooking(view);
  const isSynthetic = origin === 'synthetic' || view.kind === 'synthetic';

  // Stream badge logic strictly enforcing OR-1 (synthetic is never LIVE)
  let badgeEl: JSX.Element;
  if (isSynthetic) {
    badgeEl = (
      <span className="badge badge-dq-synthetic" data-testid="stream-health-badge">
        <span aria-hidden="true">▨</span> SYNTHETIC
      </span>
    );
  } else if (isLive) {
    badgeEl = (
      <span className="badge badge-status-ok" data-testid="stream-health-badge">
        <span aria-hidden="true">●</span> LIVE STREAM
      </span>
    );
  } else if (view.kind === 'stale') {
    badgeEl = (
      <span className="badge badge-status-warn" data-testid="stream-health-badge">
        <span aria-hidden="true">◆</span> STALE
      </span>
    );
  } else if (view.kind === 'away') {
    badgeEl = (
      <span className="badge badge-status-warn" data-testid="stream-health-badge">
        <span aria-hidden="true">◆</span> DRIVER AWAY
      </span>
    );
  } else if (view.kind === 'incompatible') {
    badgeEl = (
      <span className="badge badge-status-critical" data-testid="stream-health-badge">
        <span aria-hidden="true">■</span> INCOMPATIBLE
      </span>
    );
  } else if (view.kind === 'ended') {
    badgeEl = (
      <span className="badge badge-status-critical" data-testid="stream-health-badge">
        <span aria-hidden="true">■</span> ENDED
      </span>
    );
  } else {
    badgeEl = (
      <span className="badge badge-subtle" data-testid="stream-health-badge">
        <span aria-hidden="true">□</span> NOT CONNECTED
      </span>
    );
  }

  // Lap & Sector string
  const lapSectorText =
    currentLap != null
      ? `Lap ${String(currentLap)}${currentSector != null ? ` · S${String(currentSector)}` : ''}`
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
            {caution === 'fcy' && (
              <span className="badge badge-status-warn pitwall-flag-pill" data-testid="pitwall-caution">
                FCY / SC
              </span>
            )}
            {caution === 'red' && (
              <span className="badge badge-status-critical pitwall-flag-pill" data-testid="pitwall-caution">
                Red flag
              </span>
            )}
          </>
        )}

        <div className="pitwall-network-meta">
          <span className="pitwall-meta-sub">RTT</span>
          <span className="num pitwall-rtt-value" data-testid="engineer-rtt">
            {rttMs != null ? `${rttMs.toFixed(0)} ms` : NO_VALUE}
          </span>
          <span className="pitwall-meta-sub">Age</span>
          <span className="num pitwall-age-value" data-testid="engineer-stream-age">
            {streamAgeS != null ? `${streamAgeS.toFixed(1)} s` : NO_VALUE}
          </span>
        </div>
      </div>

      <div className="pitwall-bar-right">
        {isConnected && ended === true && onClose && (
          <button type="button" className="btn btn-sm" onClick={onClose} data-testid="btn-engineer-close">
            Close
          </button>
        )}
        {isConnected && ended !== true && (
          <>
            <button
              type="button"
              className="btn btn-sm"
              onClick={onRefreshAllData}
              title="Ask the driver's app for a full state snapshot"
              data-testid="btn-request-snapshot"
            >
              Resync
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
        {onOpenAnalysis && (
          <button
            type="button"
            className="btn btn-sm"
            onClick={onOpenAnalysis}
            title="Open the analysis views"
            data-testid="btn-open-analysis"
          >
            Analysis
          </button>
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
            <GearIcon />
          </button>
        )}
      </div>
    </header>
  );
}
