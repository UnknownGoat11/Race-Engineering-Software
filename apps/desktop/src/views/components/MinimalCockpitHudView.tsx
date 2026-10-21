import type { JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatDelta, formatFuel, formatLapTime, formatSpeed, formatTemperature, NO_VALUE } from '../../lib/units';
import { useLiveReadings } from '../liveData';
import { NOT_CONNECTED_REASON, ViewNoData } from './ViewNoData';

const HUD_CHANNELS: readonly number[] = [
  ch.FLAG_SUMMARY,
  ch.LAP_NUMBER,
  ch.SESSION_MAX_LAPS,
  ch.POSITION,
  ch.DELTA_BEST,
  ch.GEAR,
  ch.SPEED,
  ch.FUEL,
  ch.FUEL_LAPS_REMAINING,
  ch.LAST_LAP_TIME,
  ch.BEST_LAP_TIME,
  ch.TYRE_TEMP_AVG_FL,
  ch.TYRE_TEMP_AVG_FR,
  ch.TYRE_TEMP_AVG_RL,
  ch.TYRE_TEMP_AVG_RR,
];

/** FLAG_SUMMARY codes (registry: 0 green, 1 local yellow, 2 FCY/SC, 3 blue, 4 red, 5 chequered). */
const FLAG_LABELS: Readonly<Record<number, string>> = {
  0: 'GREEN',
  1: 'LOCAL YELLOW',
  2: 'FCY / SAFETY CAR',
  3: 'BLUE',
  4: 'RED',
  5: 'CHEQUERED',
};

function gearText(gear: number | null): string {
  if (gear === null) return NO_VALUE;
  const g = Math.round(gear);
  return g < 0 ? 'R' : g === 0 ? 'N' : String(g);
}

function tempDeg(c: number | null): string {
  const t = formatTemperature(c);
  return t === NO_VALUE ? NO_VALUE : `${t}°`;
}

export function MinimalCockpitHudView(): JSX.Element {
  const r = useLiveReadings(HUD_CHANNELS);
  const flag = r.num(ch.FLAG_SUMMARY);
  const lap = r.num(ch.LAP_NUMBER);
  const maxLaps = r.num(ch.SESSION_MAX_LAPS);
  const position = r.num(ch.POSITION);
  const delta = r.num(ch.DELTA_BEST);
  const fuelLaps = r.num(ch.FUEL_LAPS_REMAINING);

  const flagLabel = flag !== null ? (FLAG_LABELS[Math.round(flag)] ?? `FLAG ${String(Math.round(flag))}`) : `FLAG ${NO_VALUE}`;
  const lapText = `LAP ${lap !== null ? String(Math.round(lap)) : NO_VALUE}${maxLaps !== null && maxLaps > 0 ? ` / ${String(Math.round(maxLaps))}` : ''}`;
  const deltaClass = delta === null ? '' : delta < 0 ? 'text-green' : delta > 0 ? 'text-red' : '';

  return (
    <div className="minimal-hud-view" data-testid="minimal-ui-view">
      {!r.connected && <ViewNoData title="Minimal HUD" reason={NOT_CONNECTED_REASON} testId="hud-not-connected" />}
      <div className="minimal-hud-container">
        <div className="minimal-flag-strip">
          <span className="flag-badge" data-testid="hud-flag">
            {flagLabel}
          </span>
          <span className="session-time-text" data-testid="hud-lap">
            {lapText}
          </span>
          <span className="position-pill" data-testid="hud-position">
            {position !== null ? `P${String(Math.round(position))}` : `P${NO_VALUE}`}
          </span>
        </div>

        <div className="minimal-center-cluster">
          <div className="minimal-delta-box">
            <span className="minimal-delta-label">DELTA (VS BEST LAP)</span>
            <div className={`minimal-delta-number ${deltaClass}`} data-testid="hud-delta">
              {formatDelta(delta)}
            </div>
          </div>

          <div className="minimal-gear-speed-box">
            <div className="minimal-gear" data-testid="hud-gear">
              {gearText(r.num(ch.GEAR))}
            </div>
            <div className="minimal-speed" data-testid="hud-speed">
              {formatSpeed(r.num(ch.SPEED))} <span className="speed-unit">KM/H</span>
            </div>
          </div>
        </div>

        <div className="minimal-bottom-grid">
          <div className="minimal-stat-card">
            <span className="min-stat-label">FUEL REMAINING</span>
            <div className="min-stat-val text-cyan" data-testid="hud-fuel">
              {formatFuel(r.num(ch.FUEL), { withUnit: true })}
            </div>
            <span className="min-stat-sub">
              {fuelLaps !== null ? `${fuelLaps.toFixed(1)} LAPS OF FUEL (ESTIMATED)` : `LAPS OF FUEL: ${NO_VALUE}`}
            </span>
          </div>

          <div className="minimal-stat-card">
            <span className="min-stat-label">LAST LAP / BEST LAP</span>
            <div className="min-stat-val" data-testid="hud-last-lap">
              {formatLapTime(r.num(ch.LAST_LAP_TIME))}
            </div>
            <span className="min-stat-sub text-purple" data-testid="hud-best-lap">
              BEST: {formatLapTime(r.num(ch.BEST_LAP_TIME))}
            </span>
          </div>

          <div className="minimal-stat-card">
            <span className="min-stat-label">TYRE SURFACE AVG (FL / FR / RL / RR)</span>
            <div className="min-stat-val" data-testid="hud-tyre-temps">
              {tempDeg(r.num(ch.TYRE_TEMP_AVG_FL))} • {tempDeg(r.num(ch.TYRE_TEMP_AVG_FR))} •{' '}
              {tempDeg(r.num(ch.TYRE_TEMP_AVG_RL))} • {tempDeg(r.num(ch.TYRE_TEMP_AVG_RR))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
