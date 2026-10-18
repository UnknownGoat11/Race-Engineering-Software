/**
 * Driver Telemetry Dashboard Tab / Pit Board (spec §88, §1814–1875, REDESIGN.md §3.1 & C7).
 * High-visibility, glanceable display optimized for sim drivers read from ~1 m:
 * Exactly 4 items (C7):
 * 1. Delta to Personal Best (96 px hero, ±0.050 s deadband per C12)
 * 2. Laps left on limiting resource fuel-or-VE (64 px, amber <= 3 laps, red <= 1 lap)
 * 3. Flag summary strip from flags.summary (0x0B0C)
 * 4. Pit limiter status tile (0x0015)
 * Strict requirement: no text < 16 px anywhere on this board!
 */
import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import {
  formatDelta,
  formatFuel,
  formatFuelPerLap,
  formatLapTime,
  NO_VALUE,
} from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';

const DELTA_DEADBAND_S = 0.050;

const DRIVER_BOARD_CHANNELS = [
  ch.DELTA_PERSONAL_BEST,
  ch.DELTA_BEST,
  ch.LAP_TIME,
  ch.LAST_LAP_TIME,
  ch.BEST_LAP_TIME,
  ch.LAP_NUMBER,
  ch.FUEL,
  ch.FUEL_PER_LAP,
  ch.FUEL_LAPS_REMAINING,
  ch.FUEL_TO_ADD,
  ch.VE_LAPS_REMAINING,
  ch.SPEED_LIMITER,
  ch.FLAG_SUMMARY,
  ch.YELLOW_FLAG_STATE,
  ch.BLUE_FLAG,
] as const;

export function DriverTelemetryTab(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  return store ? <DriverTelemetryContent /> : <DriverTelemetryEmpty />;
}

function DriverTelemetryEmpty(): JSX.Element {
  return (
    <div className="pit-board" data-testid="driver-telemetry-empty">
      <div className="pit-board-empty-message">
        No live telemetry store active.
      </div>
    </div>
  );
}

function DriverTelemetryContent(): JSX.Element {
  const samples = useChannelValues(DRIVER_BOARD_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < DRIVER_BOARD_CHANNELS.length; i++) {
      const id = DRIVER_BOARD_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  // 1. Delta to Personal Best
  const deltaPb = num(ch.DELTA_PERSONAL_BEST) ?? num(ch.DELTA_BEST);
  const lapTime = num(ch.LAP_TIME);
  const lastLapTime = num(ch.LAST_LAP_TIME);
  const bestLapTime = num(ch.BEST_LAP_TIME);
  const lapNumber = num(ch.LAP_NUMBER);

  // 2. Limiting resource: Fuel vs VE
  const fuelL = num(ch.FUEL);
  const fuelPerLap = num(ch.FUEL_PER_LAP);
  const fuelLapsRemaining = num(ch.FUEL_LAPS_REMAINING);
  const fuelToAdd = num(ch.FUEL_TO_ADD);
  const veLapsLeft = num(ch.VE_LAPS_REMAINING);

  // Determine limiting resource (C7)
  const isVeLimiting =
    veLapsLeft !== null &&
    (fuelLapsRemaining === null || veLapsLeft < fuelLapsRemaining);
  const limitingResourceLabel = isVeLimiting ? 'VE' : 'FUEL';
  const limitingLapsRemaining = isVeLimiting ? veLapsLeft : fuelLapsRemaining;

  // Limiting resource alert level
  const lapsLevelClass = (() => {
    if (limitingLapsRemaining === null) return 'pit-board-level-off';
    if (limitingLapsRemaining <= 1.0) return 'pit-board-level-critical';
    if (limitingLapsRemaining <= 3.0) return 'pit-board-level-warn';
    return 'pit-board-level-ok';
  })();

  // 3. Flags
  const flagSummary = num(ch.FLAG_SUMMARY);
  const yellowState = num(ch.YELLOW_FLAG_STATE);
  const blueFlag = num(ch.BLUE_FLAG);

  // 4. Pit Limiter
  const pitLimiter = num(ch.SPEED_LIMITER);
  const isLimiterOn = pitLimiter !== null && pitLimiter > 0;

  // Delta deadband & qualifier (C12)
  const deltaInfo = (() => {
    if (deltaPb === null) {
      return {
        className: 'delta-neutral',
        qualifier: 'NO DELTA',
        text: '—',
      };
    }
    if (Math.abs(deltaPb) <= DELTA_DEADBAND_S) {
      return {
        className: 'delta-zero',
        qualifier: 'ON PACE',
        text: formatDelta(deltaPb, { withUnit: true }),
      };
    }
    if (deltaPb < -DELTA_DEADBAND_S) {
      return {
        className: 'delta-negative',
        qualifier: '▼ FASTER',
        text: formatDelta(deltaPb, { withUnit: true }),
      };
    }
    return {
      className: 'delta-positive',
      qualifier: '▲ SLOWER',
      text: formatDelta(deltaPb, { withUnit: true }),
    };
  })();

  return (
    <div className="pit-board" data-testid="driver-telemetry-board">
      {/* Item 3: Flag Summary Strip (full width) */}
      <div className="pit-board-flag-banner" data-testid="driver-flag-banner">
        {flagSummary === 1 && (
          <div className="flag-strip flag-strip-warn">
            ⚠️ LOCAL YELLOW FLAG
          </div>
        )}
        {flagSummary === 2 && (
          <div className="flag-strip flag-strip-warn">
            ⚠️ FULL COURSE YELLOW / SAFETY CAR
          </div>
        )}
        {flagSummary === 3 && (
          <div className="flag-strip flag-strip-blue">
            ◆ BLUE FLAG — YIELD POSITION
          </div>
        )}
        {flagSummary === 4 && (
          <div className="flag-strip flag-strip-critical">
            🛑 RED FLAG — SESSION STOPPED
          </div>
        )}
        {flagSummary === 5 && (
          <div className="flag-strip flag-strip-chequered">
            🏁 CHEQUERED FLAG
          </div>
        )}
        {flagSummary === null && yellowState !== null && yellowState > 0 && (
          <div className="flag-strip flag-strip-warn">
            ⚠️ YELLOW FLAG
          </div>
        )}
        {flagSummary === null && blueFlag !== null && blueFlag > 0 && (
          <div className="flag-strip flag-strip-blue">
            ◆ BLUE FLAG
          </div>
        )}
        {flagSummary === null &&
          (yellowState === null || yellowState === 0) &&
          (blueFlag === null || blueFlag === 0) && (
            <div className="flag-strip flag-strip-clear">
              ● GREEN FLAG · TRACK CLEAR
            </div>
          )}
      </div>

      {/* Item 1: Delta to Personal Best (96 px hero) */}
      <div className="pit-board-hero" data-testid="driver-delta-hero">
        <div className="pit-board-hero-header">
          <span className="pit-board-hero-title">DELTA TO PERSONAL BEST</span>
          <span className={`pit-board-hero-badge ${deltaInfo.className}`}>
            {deltaInfo.qualifier}
          </span>
        </div>

        <div className={`pit-board-hero-value ${deltaInfo.className}`} data-testid="driver-delta-value">
          {deltaInfo.text}
        </div>

        <div className="pit-board-hero-footer">
          <span>Lap {lapNumber !== null ? Math.round(lapNumber) : NO_VALUE}</span>
          <span>Current: {formatLapTime(lapTime, { allowZero: true })}</span>
          <span>Best: {formatLapTime(bestLapTime)}</span>
        </div>
      </div>

      {/* Grid: Item 2 (Fuel/VE Laps) & Item 4 (Pit Limiter) */}
      <div className="pit-board-grid">
        {/* Item 2: Laps Remaining on Limiting Resource */}
        <div className={`pit-board-tile ${lapsLevelClass}`} data-testid="driver-fuel-tile">
          <div className="pit-board-tile-header">
            <span className="pit-board-tile-title">
              LAPS REMAINING ({limitingResourceLabel})
            </span>
            <span className="pit-board-tile-glyph" aria-hidden="true">
              {limitingLapsRemaining !== null && limitingLapsRemaining <= 1.0
                ? '■'
                : limitingLapsRemaining !== null && limitingLapsRemaining <= 3.0
                  ? '◆'
                  : '●'}
            </span>
          </div>

          <div className="pit-board-tile-body">
            <div className="pit-board-tile-value" data-testid="driver-laps-remaining">
              {limitingLapsRemaining !== null
                ? limitingLapsRemaining.toFixed(1)
                : NO_VALUE}
            </div>
          </div>

          <div className="pit-board-tile-footer">
            <div className="pit-board-tile-detail">
              Fuel: {formatFuel(fuelL, { withUnit: true })} · Avg: {formatFuelPerLap(fuelPerLap, { withUnit: true })}
            </div>
            <div className="pit-board-tile-subtext">
              Target Refuel: {formatFuel(fuelToAdd, { withUnit: true })}
            </div>
          </div>
        </div>

        {/* Item 4: Pit Limiter */}
        <div
          className={`pit-board-tile ${isLimiterOn ? 'pit-board-level-ok' : 'pit-board-level-off'}`}
          data-testid="driver-limiter-tile"
        >
          <div className="pit-board-tile-header">
            <span className="pit-board-tile-title">PIT LIMITER</span>
            <span className="pit-board-tile-glyph" aria-hidden="true">
              {isLimiterOn ? '●' : '□'}
            </span>
          </div>

          <div className="pit-board-tile-body">
            <div className="pit-board-tile-value pit-board-limiter-value" data-testid="driver-limiter-state">
              {isLimiterOn ? 'ACTIVE' : 'OFF'}
            </div>
          </div>

          <div className="pit-board-tile-footer">
            <div className="pit-board-tile-detail">
              {isLimiterOn ? 'Pit Limiter Engaged' : 'Pit Limiter Disengaged'}
            </div>
            <div className="pit-board-tile-subtext">
              Last Lap: {formatLapTime(lastLapTime)}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
