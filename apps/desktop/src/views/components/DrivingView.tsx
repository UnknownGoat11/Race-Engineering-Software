import { type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatDelta, formatFuel, formatLapTime, formatSpeed, NO_VALUE } from '../../lib/units';
import { useLiveReadings } from '../liveData';
import { WHEEL_CHANNELS, readWheels } from '../wheels';
import { TyreCarTopView } from './TyreCarTopView';
import { NOT_CONNECTED_REASON, ViewNoData } from './ViewNoData';

const DRIVING_CHANNELS: readonly number[] = [
  ch.BRAKE,
  ch.THROTTLE,
  ch.RPM,
  ch.RPM_MAX,
  ch.ENGINE_TORQUE,
  ch.GEAR,
  ch.SPEED,
  ch.CLUTCH,
  ch.FUEL,
  ch.FUEL_CAPACITY,
  ch.BATTERY_SOC,
  ch.REGEN_POWER,
  ch.G_LAT,
  ch.G_LONG,
  ch.STEERING_ANGLE,
  ch.WATER_TEMP,
  ch.OIL_TEMP,
  ch.CLUTCH_RPM,
  ch.TC_LEVEL,
  ch.TC_CUT,
  ch.TC_SLIP,
  ch.ABS_LEVEL,
  ch.BRAKE_BIAS_FRONT,
  ch.SPEED_LIMITER_ACTIVE,
  ch.IGNITION_STARTER,
  ch.HYBRID_MOTOR_STATE,
  ch.MOTOR_MAP,
  ch.BEST_LAP_TIME,
  ch.DELTA_BEST,
  ch.FLAG_SUMMARY,
  ...WHEEL_CHANNELS,
];

/** FLAG_SUMMARY codes (registry: 0 green, 1 local yellow, 2 FCY/SC, 3 blue, 4 red, 5 chequered). */
const FLAG_LABELS: Readonly<Record<number, string>> = {
  0: 'GREEN FLAG',
  1: 'LOCAL YELLOW',
  2: 'FCY / SAFETY CAR',
  3: 'BLUE FLAG',
  4: 'RED FLAG',
  5: 'CHEQUERED',
};

/** LMU mIgnitionStarter: 0 off, 1 ignition, 2 ignition + starter. */
const IGNITION_LABELS: Readonly<Record<number, string>> = { 0: 'OFF', 1: 'ON', 2: 'STARTER' };

function pct(frac: number | null, decimals = 0): string {
  return frac === null ? NO_VALUE : `${(frac * 100).toFixed(decimals)}%`;
}

function clampPct(frac: number | null): number {
  return frac === null ? 0 : Math.max(0, Math.min(100, frac * 100));
}

function intText(value: number | null): string {
  return value === null ? NO_VALUE : Math.round(value).toString();
}

function gearText(gear: number | null): string {
  if (gear === null) return NO_VALUE;
  const g = Math.round(gear);
  if (g < 0) return 'R';
  if (g === 0) return 'N';
  return String(g);
}

/** G value with an explicit direction word; G_LAT is positive toward the LEFT (ISO), G_LONG positive accelerating. */
function gText(g: number | null): string {
  return g === null ? NO_VALUE : `${g.toFixed(2)}G`;
}

export function DrivingView(): JSX.Element {
  const r = useLiveReadings(DRIVING_CHANNELS);

  const brake = r.num(ch.BRAKE);
  const throttle = r.num(ch.THROTTLE);
  const rpm = r.num(ch.RPM);
  const rpmMax = r.num(ch.RPM_MAX);
  const torque = r.num(ch.ENGINE_TORQUE);
  const gear = r.num(ch.GEAR);
  const speed = r.num(ch.SPEED);
  const clutch = r.num(ch.CLUTCH);
  const fuel = r.num(ch.FUEL);
  const fuelCapacity = r.num(ch.FUEL_CAPACITY);
  const soc = r.num(ch.BATTERY_SOC);
  const regenW = r.num(ch.REGEN_POWER);
  const latG = r.num(ch.G_LAT);
  const longG = r.num(ch.G_LONG);
  const steeringRad = r.num(ch.STEERING_ANGLE);
  const motorMap = r.num(ch.MOTOR_MAP);
  const bestLap = r.num(ch.BEST_LAP_TIME);
  const deltaBest = r.num(ch.DELTA_BEST);
  const flag = r.num(ch.FLAG_SUMMARY);
  const limiter = r.bool(ch.SPEED_LIMITER_ACTIVE);
  const ignition = r.num(ch.IGNITION_STARTER);
  const hybridState = r.num(ch.HYBRID_MOTOR_STATE);
  const brakeBiasFront = r.num(ch.BRAKE_BIAS_FRONT);

  const fuelPct = fuel !== null && fuelCapacity !== null && fuelCapacity > 0 ? (fuel / fuelCapacity) * 100 : null;
  const rpmPct = rpm !== null && rpmMax !== null && rpmMax > 0 ? Math.min(100, (rpm / rpmMax) * 100) : null;
  const regenKw = regenW !== null ? regenW / 1000 : null;
  const steeringDeg = steeringRad !== null ? (steeringRad * 180) / Math.PI : null;

  // G-G dot: -2.5 G .. +2.5 G mapped to 10..90. G_LAT is positive to the LEFT, so left plots left (smaller x).
  const hasG = latG !== null && longG !== null;
  const dotX = latG !== null ? Math.max(10, Math.min(90, 50 - (latG / 2.5) * 40)) : 50;
  const dotY = longG !== null ? Math.max(10, Math.min(90, 50 - (longG / 2.5) * 40)) : 50;

  const flagLabel = flag !== null ? (FLAG_LABELS[Math.round(flag)] ?? `FLAG ${Math.round(flag)}`) : null;
  const ignitionLabel = ignition !== null ? (IGNITION_LABELS[Math.round(ignition)] ?? String(Math.round(ignition))) : NO_VALUE;
  const deltaClass = deltaBest === null ? '' : deltaBest < 0 ? 'delta-faster' : deltaBest > 0 ? 'delta-slower' : '';

  return (
    <div className="driving-view-container" data-testid="driving-view">
      {!r.connected && <ViewNoData title="Driving" reason={NOT_CONNECTED_REASON} testId="driving-not-connected" />}

      <div className="driving-header-bar">
        <div className="driving-header-left">
          <div className="engine-map-banner">
            <span className="engine-map-label" data-testid="driving-motor-map">
              MOTOR MAP: {motorMap !== null ? `#${Math.round(motorMap)}` : NO_VALUE}
            </span>
          </div>
        </div>

        <div className="driving-lap-status">
          <span className="lap-status-best" data-testid="driving-best-lap">
            BEST: {formatLapTime(bestLap)}
          </span>
          <span className={`lap-status-delta ${deltaClass}`} data-testid="driving-delta-best">
            {deltaBest !== null ? `${formatDelta(deltaBest)}s` : NO_VALUE}
          </span>
        </div>

        <div className="driving-header-right">
          <div className={`session-status-pill ${flagLabel !== null ? 'pill-active' : 'pill-off'}`} data-testid="driving-flag">
            {flagLabel ?? `FLAG: ${NO_VALUE}`}
          </div>
          <span
            className={`system-status-pill ${limiter === true ? 'limiter-on' : 'limiter-off'}`}
            data-testid="driving-pit-limiter"
          >
            PIT LIMITER: {limiter === null ? NO_VALUE : limiter ? 'ON' : 'OFF'}
          </span>
        </div>
      </div>

      <div className="driving-cockpit-grid">
        <div className="driving-col driving-col-left">
          <div className="pedals-vertical-row">
            <div className="pedal-gauge-card pedal-gauge-brake">
              <span className="pedal-gauge-title">BRAKE</span>
              <div className="pedal-meter-bar-track">
                <div className="pedal-meter-fill brake-fill" style={{ height: `${clampPct(brake)}%` }} />
              </div>
              <span className="pedal-gauge-value" data-testid="driving-brake">
                {pct(brake)}
              </span>
            </div>

            <div className="pedal-gauge-card pedal-gauge-throt">
              <span className="pedal-gauge-title">THROT</span>
              <div className="pedal-meter-bar-track">
                <div className="pedal-meter-fill throt-fill" style={{ height: `${clampPct(throttle)}%` }} />
              </div>
              <span className="pedal-gauge-value" data-testid="driving-throttle">
                {pct(throttle)}
              </span>
            </div>

            <div className="pedal-gear-cluster">
              <div className="gear-digit-box">
                <span className="gear-digit" data-testid="driving-gear">
                  {gearText(gear)}
                </span>
                <span className="speed-sub-label" data-testid="driving-speed">
                  {formatSpeed(speed, { withUnit: true })}
                </span>
              </div>
              <div className={`clutch-indicator ${clutch !== null && clutch > 0.05 ? 'clutch-active' : ''}`}>
                <span className="clutch-box-text">CLUTCH</span>
                <span className="clutch-sub-val">{pct(clutch)}</span>
              </div>
            </div>

            <div className="pedal-gauge-card pedal-gauge-revs">
              <span className="pedal-gauge-title">RPM</span>
              <div className="pedal-meter-bar-track">
                {rpmPct !== null && (
                  <div
                    className={`pedal-meter-fill revs-fill ${rpmPct > 90 ? 'revs-shift-light' : ''}`}
                    style={{ height: `${rpmPct}%` }}
                  />
                )}
              </div>
              <span className="pedal-gauge-value" data-testid="driving-rpm">
                {intText(rpm)}
              </span>
            </div>

            <div className="pedal-gauge-card pedal-gauge-torque">
              <span className="pedal-gauge-title">TORQ Nm</span>
              <div className="pedal-meter-bar-track" />
              <span className="pedal-gauge-value" data-testid="driving-torque">
                {intText(torque)}
              </span>
            </div>
          </div>

          <div className="energy-status-panel">
            <div className="energy-stat-row">
              <span className="energy-label">Fuel:</span>
              <span className="energy-val num-cyan" data-testid="driving-fuel">
                {formatFuel(fuel, { withUnit: true })}
                {fuelPct !== null ? ` (${fuelPct.toFixed(1)}%)` : ''}
              </span>
            </div>
            <div className="energy-stat-row">
              <span className="energy-label">SoC (Battery):</span>
              <span className="energy-val num-cyan" data-testid="driving-soc">
                {pct(soc)}
              </span>
            </div>
            <div className="energy-stat-row">
              <span className="energy-label">Regen power:</span>
              <span className="energy-val" data-testid="driving-regen">
                {regenKw !== null ? `${regenKw.toFixed(1)} kW` : NO_VALUE}
              </span>
            </div>
          </div>

          <div className="friction-circle-card" data-testid="friction-circle">
            <div className="friction-circle-header">
              <span className="friction-title">G-FORCE (LIVE)</span>
              <span className="friction-vals" data-testid="driving-g-values">
                Lat: {gText(latG)}
                {latG !== null && latG !== 0 ? (latG > 0 ? ' L' : ' R') : ''} | Long: {gText(longG)}
              </span>
            </div>
            <div className="friction-circle-viewport">
              <svg viewBox="0 0 100 100" className="friction-circle-svg" role="img" aria-label="Live G-force position">
                <line x1="10" y1="50" x2="90" y2="50" stroke="#334155" strokeWidth="1" strokeDasharray="2 2" />
                <line x1="50" y1="10" x2="50" y2="90" stroke="#334155" strokeWidth="1" strokeDasharray="2 2" />
                {/* Scale rings at 0.625 G steps (2.5 G = outer ring). */}
                <circle cx="50" cy="50" r="10" fill="none" stroke="#1e293b" strokeWidth="0.8" />
                <circle cx="50" cy="50" r="20" fill="none" stroke="#1e293b" strokeWidth="0.8" />
                <circle cx="50" cy="50" r="30" fill="none" stroke="#334155" strokeWidth="1" />
                <circle cx="50" cy="50" r="40" fill="none" stroke="#0284c7" strokeWidth="1.2" strokeOpacity="0.4" />
                <text x="12" y="48" fill="#64748b" fontSize="5">L</text>
                <text x="85" y="48" fill="#64748b" fontSize="5">R</text>
                {hasG && <circle cx={dotX} cy={dotY} r="3.5" fill="#ef4444" stroke="#ffffff" strokeWidth="1" />}
              </svg>
            </div>
            <div className="steering-angle-strip">
              <span className="steer-label">STEERING ANGLE:</span>
              <span className="steer-value" data-testid="driving-steering">
                {steeringDeg !== null ? `${steeringDeg.toFixed(1)}°` : NO_VALUE}
              </span>
            </div>
          </div>
        </div>

        <div className="driving-col driving-col-center">
          <TyreCarTopView
            wheels={readWheels(r)}
            engineWaterTempC={r.num(ch.WATER_TEMP)}
            engineOilTempC={r.num(ch.OIL_TEMP)}
            clutchRpm={r.num(ch.CLUTCH_RPM)}
          />
        </div>

        <div className="driving-col driving-col-right">
          <div className="aids-status-panel">
            <div className="aids-panel-header">
              <span className="aids-title">DRIVER AIDS & SYSTEMS</span>
            </div>

            <div className="aids-grid">
              <div className="aids-item">
                <span className="aids-item-label">TC / ABS:</span>
                <span className="aids-item-value num-cyan" data-testid="driving-tc-abs">
                  TC: {intText(r.num(ch.TC_LEVEL))} | ABS: {intText(r.num(ch.ABS_LEVEL))}
                </span>
              </div>
              <div className="aids-item">
                <span className="aids-item-label">TC CUT / SLIP:</span>
                <span className="aids-item-value">
                  CUT: {intText(r.num(ch.TC_CUT))} | SLIP: {intText(r.num(ch.TC_SLIP))}
                </span>
              </div>
            </div>

            <div className="brakebias-control-card">
              <div className="brakebias-header">
                <span className="brakebias-label">BRAKE BIAS</span>
                <span className="brakebias-val num-cyan" data-testid="driving-brake-bias">
                  {brakeBiasFront !== null
                    ? `F ${(brakeBiasFront * 100).toFixed(1)}% / R ${((1 - brakeBiasFront) * 100).toFixed(1)}%`
                    : NO_VALUE}
                </span>
              </div>
            </div>

            <div className="systems-toggle-list">
              <div className="system-toggle-row">
                <span className="system-name">Hybrid motor state:</span>
                <span className="system-status-pill" data-testid="driving-hybrid-state">
                  {hybridState !== null ? String(Math.round(hybridState)) : NO_VALUE}
                </span>
              </div>
              <div className="system-toggle-row">
                <span className="system-name">Starter / Ignition:</span>
                <span className="system-status-pill" data-testid="driving-ignition">
                  {ignitionLabel}
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
