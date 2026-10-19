import type { JSX } from 'react';
import type { LockupEvent, TyreSectionTemperatures } from '../types';

export interface TyreDegQuickSummaryProps {
  readonly wheels?: {
    readonly fl: TyreSectionTemperatures;
    readonly fr: TyreSectionTemperatures;
    readonly rl: TyreSectionTemperatures;
    readonly rr: TyreSectionTemperatures;
  } | undefined;
  readonly lockupEvents?: readonly LockupEvent[] | undefined;
  readonly currentLap?: number | undefined;
  readonly onOpenFullView?: (() => void) | undefined;
  readonly compact?: boolean | undefined;
}

export const DEFAULT_LOCKUP_EVENTS: readonly LockupEvent[] = [
  {
    id: 'lck-1',
    timestampMs: Date.now() - 145000,
    lapNumber: 12,
    cornerName: 'T1 (La Source)',
    wheel: 'FL',
    durationMs: 380,
    peakSlipPct: 42.5,
    severity: 'MODERATE',
    flatspotDamagePct: 14.0,
    estimatedVibrationIndex: 3.8,
  },
  {
    id: 'lck-2',
    timestampMs: Date.now() - 320000,
    lapNumber: 9,
    cornerName: 'T18 (Bus Stop Chicane)',
    wheel: 'FR',
    durationMs: 190,
    peakSlipPct: 28.2,
    severity: 'MILD',
    flatspotDamagePct: 6.5,
    estimatedVibrationIndex: 1.5,
  },
  {
    id: 'lck-3',
    timestampMs: Date.now() - 540000,
    lapNumber: 5,
    cornerName: 'T5 (Les Combes)',
    wheel: 'FL',
    durationMs: 220,
    peakSlipPct: 31.0,
    severity: 'MILD',
    flatspotDamagePct: 5.0,
    estimatedVibrationIndex: 1.2,
  },
];

export function TyreDegQuickSummary({
  wheels,
  lockupEvents = DEFAULT_LOCKUP_EVENTS,
  currentLap = 14,
  onOpenFullView,
  compact = false,
}: TyreDegQuickSummaryProps): JSX.Element {
  // Aggregate flatspot damage per wheel from lockup events
  const flatspotByWheel: Record<'FL' | 'FR' | 'RL' | 'RR', { totalDamage: number; lastCorner: string; severity: 'NONE' | 'MILD' | 'MODERATE' | 'SEVERE' }> = {
    FL: { totalDamage: 0, lastCorner: '', severity: 'NONE' },
    FR: { totalDamage: 0, lastCorner: '', severity: 'NONE' },
    RL: { totalDamage: 0, lastCorner: '', severity: 'NONE' },
    RR: { totalDamage: 0, lastCorner: '', severity: 'NONE' },
  };

  for (const ev of lockupEvents) {
    const w = flatspotByWheel[ev.wheel];
    w.totalDamage = Math.min(100, w.totalDamage + ev.flatspotDamagePct);
    if (!w.lastCorner) w.lastCorner = ev.cornerName;
    if (ev.severity === 'SEVERE' || (ev.severity === 'MODERATE' && w.severity !== 'SEVERE')) {
      w.severity = ev.severity;
    } else if (ev.severity === 'MILD' && w.severity === 'NONE') {
      w.severity = 'MILD';
    }
  }

  const flWear = ((wheels?.fl.wearFraction ?? 0.942) * 100).toFixed(1);
  const frWear = ((wheels?.fr.wearFraction ?? 0.938) * 100).toFixed(1);
  const rlWear = ((wheels?.rl.wearFraction ?? 0.915) * 100).toFixed(1);
  const rrWear = ((wheels?.rr.wearFraction ?? 0.912) * 100).toFixed(1);

  // Stint wear slope calculation: e.g. 0.82% per lap
  const degRatePct = 0.82;
  const cliffWearThreshold = 75.0; // Grip cliff below 75%
  const lowestWear = Math.min(Number(flWear), Number(frWear), Number(rlWear), Number(rrWear));
  const lapsToCliff = Math.max(0, Math.floor((lowestWear - cliffWearThreshold) / degRatePct));
  const cliffLap = currentLap + lapsToCliff;
  const doubleStintProjectedWear = Math.max(0, 100 - (56 * degRatePct));
  const doubleStintViable = doubleStintProjectedWear > cliffWearThreshold;

  return (
    <div
      className={`tyre-deg-quick-card ${compact ? 'deg-compact' : ''}`}
      data-testid="tyre-deg-quick-summary"
    >
      <div className="tyre-deg-header">
        <div className="deg-title-wrap">
          <span className="deg-glyph">🛞</span>
          <span className="deg-title">TYRE DEGRADATION & FLATSPOT HEALTH</span>
        </div>
        <div className="deg-header-right">
          <span className={`badge ${doubleStintViable ? 'badge-green' : 'badge-amber'}`}>
            DOUBLE-STINT: {doubleStintViable ? 'VIABLE' : 'RISKY'}
          </span>
          {onOpenFullView && (
            <button
              type="button"
              className="btn btn-xs btn-outline"
              onClick={onOpenFullView}
              title="Open full Tyre Degradation and Flatspot Logger"
            >
              FULL VIEW ↗
            </button>
          )}
        </div>
      </div>

      {/* 4-Wheel Health Status & Flatspot Badges */}
      <div className="deg-wheels-grid">
        {(['FL', 'FR', 'RL', 'RR'] as const).map((pos) => {
          const wear = pos === 'FL' ? flWear : pos === 'FR' ? frWear : pos === 'RL' ? rlWear : rrWear;
          const fs = flatspotByWheel[pos];
          const hasFlatspot = fs.totalDamage > 0;

          return (
            <div key={pos} className={`deg-wheel-box deg-wheel-${pos.toLowerCase()}`}>
              <div className="deg-wheel-row">
                <span className="deg-wheel-pos">{pos}</span>
                <span className="deg-wheel-wear num-cyan">{wear}%</span>
              </div>
              <div className="deg-wheel-progress">
                <div
                  className="deg-wheel-fill"
                  style={{
                    width: `${wear}%`,
                    backgroundColor: Number(wear) < 80 ? '#ef4444' : Number(wear) < 90 ? '#f59e0b' : '#10b981',
                  }}
                />
              </div>
              <div className="deg-flatspot-row">
                {hasFlatspot ? (
                  <span className={`flatspot-badge badge-${fs.severity.toLowerCase()}`}>
                    ⚠️ {fs.totalDamage.toFixed(0)}% Flatspot ({fs.lastCorner})
                  </span>
                ) : (
                  <span className="flatspot-badge badge-clean">✅ Clean Tyre</span>
                )}
              </div>
            </div>
          );
        })}
      </div>

      {/* Stint Degradation & Cliff Lap Horizon */}
      <div className="deg-stint-summary-bar">
        <div className="stint-stat-item">
          <span className="stint-stat-label">DEGRADATION RATE:</span>
          <span className="stint-stat-val num-cyan">~{degRatePct.toFixed(2)}% / Lap</span>
        </div>
        <div className="stint-stat-item">
          <span className="stint-stat-label">PROJECTED CLIFF:</span>
          <span className="stint-stat-val num-amber">Lap {cliffLap} (~{lapsToCliff} laps remaining)</span>
        </div>
        <div className="stint-stat-item">
          <span className="stint-stat-label">RECENT LOCKUP:</span>
          <span className="stint-stat-val num-red">
            {lockupEvents.length > 0
              ? `${lockupEvents[0]?.wheel} @ ${lockupEvents[0]?.cornerName} (Lap ${lockupEvents[0]?.lapNumber})`
              : 'None'}
          </span>
        </div>
      </div>
    </div>
  );
}
