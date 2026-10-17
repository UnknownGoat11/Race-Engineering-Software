/**
 * Driver Telemetry Dashboard Tab (spec §88, §1814–1875).
 * High-visibility pit board display optimized for drivers on a secondary screen:
 * - Prominent Lap Delta to PB (large green/red indicator)
 * - Lap Timing (current, last, best)
 * - Fuel Calculator (remaining, fuel/lap, laps remaining, fuel to add)
 * - 4-Wheel Tyre Board (Inner/Middle/Outer temps, pressure, wear %)
 * - Brake Temperatures (FL, FR, RL, RR)
 * - Active Flags & Limiter indicator
 */
import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import {
  formatDelta,
  formatFuel,
  formatFuelPerLap,
  formatLapTime,
  formatPressure,
  formatTemperature,
  NO_VALUE,
} from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';

const DRIVER_CHANNELS = [
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
  ch.TYRE_TEMP_INNER_FL,
  ch.TYRE_TEMP_MIDDLE_FL,
  ch.TYRE_TEMP_OUTER_FL,
  ch.TYRE_PRESSURE_FL,
  ch.TYRE_WEAR_FL,
  ch.TYRE_TEMP_INNER_FR,
  ch.TYRE_TEMP_MIDDLE_FR,
  ch.TYRE_TEMP_OUTER_FR,
  ch.TYRE_PRESSURE_FR,
  ch.TYRE_WEAR_FR,
  ch.TYRE_TEMP_INNER_RL,
  ch.TYRE_TEMP_MIDDLE_RL,
  ch.TYRE_TEMP_OUTER_RL,
  ch.TYRE_PRESSURE_RL,
  ch.TYRE_WEAR_RL,
  ch.TYRE_TEMP_INNER_RR,
  ch.TYRE_TEMP_MIDDLE_RR,
  ch.TYRE_TEMP_OUTER_RR,
  ch.TYRE_PRESSURE_RR,
  ch.TYRE_WEAR_RR,
  ch.BRAKE_TEMP_FL,
  ch.BRAKE_TEMP_FR,
  ch.BRAKE_TEMP_RL,
  ch.BRAKE_TEMP_RR,
  ch.YELLOW_FLAG_STATE,
  ch.BLUE_FLAG,
  ch.SPEED_LIMITER,
] as const;

export function DriverTelemetryTab(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  return store ? <DriverTelemetryContent /> : <DriverTelemetryEmpty />;
}

function DriverTelemetryEmpty(): JSX.Element {
  return (
    <div className="telemetry-board" data-testid="driver-telemetry-empty">
      <p className="card-row-label">No live telemetry store active.</p>
    </div>
  );
}

function DriverTelemetryContent(): JSX.Element {
  const samples = useChannelValues(DRIVER_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < DRIVER_CHANNELS.length; i++) {
      const id = DRIVER_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  // Delta to Personal Best
  const deltaPb = num(ch.DELTA_PERSONAL_BEST) ?? num(ch.DELTA_BEST);
  const lapTime = num(ch.LAP_TIME);
  const lastLapTime = num(ch.LAST_LAP_TIME);
  const bestLapTime = num(ch.BEST_LAP_TIME);
  const lapNumber = num(ch.LAP_NUMBER);

  // Fuel
  const fuelL = num(ch.FUEL);
  const fuelPerLap = num(ch.FUEL_PER_LAP);
  const fuelLapsRemaining = num(ch.FUEL_LAPS_REMAINING);
  const fuelToAdd = num(ch.FUEL_TO_ADD);

  // Tyres FL
  const tyreInFL = num(ch.TYRE_TEMP_INNER_FL);
  const tyreMidFL = num(ch.TYRE_TEMP_MIDDLE_FL);
  const tyreOutFL = num(ch.TYRE_TEMP_OUTER_FL);
  const tyrePressFL = num(ch.TYRE_PRESSURE_FL);
  const tyreWearFL = num(ch.TYRE_WEAR_FL);

  // Tyres FR
  const tyreInFR = num(ch.TYRE_TEMP_INNER_FR);
  const tyreMidFR = num(ch.TYRE_TEMP_MIDDLE_FR);
  const tyreOutFR = num(ch.TYRE_TEMP_OUTER_FR);
  const tyrePressFR = num(ch.TYRE_PRESSURE_FR);
  const tyreWearFR = num(ch.TYRE_WEAR_FR);

  // Tyres RL
  const tyreInRL = num(ch.TYRE_TEMP_INNER_RL);
  const tyreMidRL = num(ch.TYRE_TEMP_MIDDLE_RL);
  const tyreOutRL = num(ch.TYRE_TEMP_OUTER_RL);
  const tyrePressRL = num(ch.TYRE_PRESSURE_RL);
  const tyreWearRL = num(ch.TYRE_WEAR_RL);

  // Tyres RR
  const tyreInRR = num(ch.TYRE_TEMP_INNER_RR);
  const tyreMidRR = num(ch.TYRE_TEMP_MIDDLE_RR);
  const tyreOutRR = num(ch.TYRE_TEMP_OUTER_RR);
  const tyrePressRR = num(ch.TYRE_PRESSURE_RR);
  const tyreWearRR = num(ch.TYRE_WEAR_RR);

  // Brakes
  const brakeFL = num(ch.BRAKE_TEMP_FL);
  const brakeFR = num(ch.BRAKE_TEMP_FR);
  const brakeRL = num(ch.BRAKE_TEMP_RL);
  const brakeRR = num(ch.BRAKE_TEMP_RR);

  // Flags & limiter
  const yellowState = num(ch.YELLOW_FLAG_STATE);
  const blueFlag = num(ch.BLUE_FLAG);
  const pitLimiter = num(ch.SPEED_LIMITER);

  const deltaClass =
    deltaPb === null ? 'delta-zero' : deltaPb < 0 ? 'delta-negative' : deltaPb > 0 ? 'delta-positive' : 'delta-zero';

  return (
    <div className="telemetry-board" data-testid="driver-telemetry-board">
      {/* Flags / Alerts Banner */}
      {(yellowState !== null && yellowState > 0) || (blueFlag !== null && blueFlag > 0) || (pitLimiter !== null && pitLimiter > 0) ? (
        <div className="screen-header-banner" style={{ backgroundColor: 'var(--color-warn-bg)', borderColor: 'var(--color-warn)' }}>
          {yellowState !== null && yellowState > 0 && (
            <span className="status-badge status-badge-warn">⚠️ YELLOW FLAG</span>
          )}
          {blueFlag !== null && blueFlag > 0 && (
            <span className="status-badge" style={{ backgroundColor: 'rgba(59, 130, 246, 0.2)', color: '#3b82f6' }}>
              🔵 BLUE FLAG
            </span>
          )}
          {pitLimiter !== null && pitLimiter > 0 && (
            <span className="status-badge status-badge-live">PIT LIMITER ON</span>
          )}
        </div>
      ) : null}

      {/* Delta to Personal Best */}
      <div className="delta-hero" data-testid="driver-delta-hero">
        <div>
          <div className="delta-label">DELTA TO PERSONAL BEST</div>
          <div className="card-row-label">
            Lap {lapNumber !== null ? Math.round(lapNumber) : NO_VALUE} &bull; Current: {formatLapTime(lapTime, { allowZero: true })}
          </div>
        </div>
        <div className={`delta-value ${deltaClass}`} data-testid="driver-delta-value">
          {formatDelta(deltaPb, { withUnit: true })}
        </div>
      </div>

      <div className="cards-grid">
        {/* Lap Timing */}
        <div className="card">
          <div className="card-header">Lap Timing</div>
          <div className="card-body">
            <div className="card-row">
              <span className="card-row-label">Current Lap</span>
              <span className="card-row-value">{formatLapTime(lapTime, { allowZero: true })}</span>
            </div>
            <div className="card-row">
              <span className="card-row-label">Last Lap</span>
              <span className="card-row-value">{formatLapTime(lastLapTime)}</span>
            </div>
            <div className="card-row">
              <span className="card-row-label">Personal Best</span>
              <span className="card-row-value">{formatLapTime(bestLapTime)}</span>
            </div>
          </div>
        </div>

        {/* Fuel Calculator */}
        <div className="card">
          <div className="card-header">Fuel Calculator</div>
          <div className="card-body">
            <div className="card-row">
              <span className="card-row-label">Remaining Fuel</span>
              <span className="card-row-value">{formatFuel(fuelL, { withUnit: true })}</span>
            </div>
            <div className="card-row">
              <span className="card-row-label">Avg Consumption</span>
              <span className="card-row-value">{formatFuelPerLap(fuelPerLap, { withUnit: true })}</span>
            </div>
            <div className="card-row">
              <span className="card-row-label">Estimated Laps Left</span>
              <span className="card-row-value">
                {fuelLapsRemaining !== null ? `${fuelLapsRemaining.toFixed(1)} laps` : NO_VALUE}
              </span>
            </div>
            <div className="card-row">
              <span className="card-row-label">Target Refuel</span>
              <span className="card-row-value">{formatFuel(fuelToAdd, { withUnit: true })}</span>
            </div>
          </div>
        </div>
      </div>

      {/* 4-Wheel Tyre Board */}
      <div className="card">
        <div className="card-header">
          <span>Tyres (Inner / Middle / Outer)</span>
          <span>Pressure &bull; Wear</span>
        </div>
        <div className="card-body">
          <div className="tyre-car-footprint">
            {/* Front Left */}
            <div className="wheel-card" data-testid="wheel-fl">
              <div className="wheel-card-header">
                <span>FL</span>
                <span>{formatPressure(tyrePressFL, { withUnit: true })}</span>
              </div>
              <div className="wheel-temps-trio">
                <span>{formatTemperature(tyreInFL)}</span>
                <span>{formatTemperature(tyreMidFL)}</span>
                <span>{formatTemperature(tyreOutFL)}</span>
              </div>
              <div className="wheel-metric-row">
                <span>Wear: {tyreWearFL !== null ? `${(tyreWearFL * 100).toFixed(0)}%` : NO_VALUE}</span>
                <span>Brake: {formatTemperature(brakeFL, { withUnit: true })}</span>
              </div>
            </div>

            {/* Front Right */}
            <div className="wheel-card" data-testid="wheel-fr">
              <div className="wheel-card-header">
                <span>FR</span>
                <span>{formatPressure(tyrePressFR, { withUnit: true })}</span>
              </div>
              <div className="wheel-temps-trio">
                <span>{formatTemperature(tyreOutFR)}</span>
                <span>{formatTemperature(tyreMidFR)}</span>
                <span>{formatTemperature(tyreInFR)}</span>
              </div>
              <div className="wheel-metric-row">
                <span>Wear: {tyreWearFR !== null ? `${(tyreWearFR * 100).toFixed(0)}%` : NO_VALUE}</span>
                <span>Brake: {formatTemperature(brakeFR, { withUnit: true })}</span>
              </div>
            </div>

            {/* Rear Left */}
            <div className="wheel-card" data-testid="wheel-rl">
              <div className="wheel-card-header">
                <span>RL</span>
                <span>{formatPressure(tyrePressRL, { withUnit: true })}</span>
              </div>
              <div className="wheel-temps-trio">
                <span>{formatTemperature(tyreInRL)}</span>
                <span>{formatTemperature(tyreMidRL)}</span>
                <span>{formatTemperature(tyreOutRL)}</span>
              </div>
              <div className="wheel-metric-row">
                <span>Wear: {tyreWearRL !== null ? `${(tyreWearRL * 100).toFixed(0)}%` : NO_VALUE}</span>
                <span>Brake: {formatTemperature(brakeRL, { withUnit: true })}</span>
              </div>
            </div>

            {/* Rear Right */}
            <div className="wheel-card" data-testid="wheel-rr">
              <div className="wheel-card-header">
                <span>RR</span>
                <span>{formatPressure(tyrePressRR, { withUnit: true })}</span>
              </div>
              <div className="wheel-temps-trio">
                <span>{formatTemperature(tyreOutRR)}</span>
                <span>{formatTemperature(tyreMidRR)}</span>
                <span>{formatTemperature(tyreInRR)}</span>
              </div>
              <div className="wheel-metric-row">
                <span>Wear: {tyreWearRR !== null ? `${(tyreWearRR * 100).toFixed(0)}%` : NO_VALUE}</span>
                <span>Brake: {formatTemperature(brakeRR, { withUnit: true })}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
