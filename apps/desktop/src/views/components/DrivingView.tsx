import { useMemo, type JSX } from 'react';
import type { VehicleDynamicsState } from '../types';
import { TyreCarTopView } from './TyreCarTopView';

export interface DrivingViewProps {
  readonly state?: VehicleDynamicsState;
  readonly onToggleLimiter?: () => void;
  readonly onChangeEngineMap?: (map: number) => void;
  readonly onChangeBrakeBias?: (delta: number) => void;
}

const DEFAULT_STATE: VehicleDynamicsState = {
  brakeFrac: 0.0,
  throttleFrac: 0.85,
  rpm: 8240,
  rpmMax: 9000,
  torqueNm: 620,
  gear: 4,
  speedKmh: 218.4,
  clutchFrac: 0.0,
  fuelL: 42.5,
  fuelCapacityL: 90.0,
  batterySocFrac: 0.68,
  regenKw: -125.0,
  latG: 1.45,
  longG: 0.62,
  steerAngleDeg: 14.2,
  engineWaterTempC: 89.2,
  engineOilTempC: 98.4,
  clutchRpm: 0,
  tcLevel: 3,
  tcCut: 1,
  tcSlip: 1.4,
  absLevel: 2,
  brakeBiasFrontFrac: 0.545,
  pitLimiterActive: false,
  ignitionActive: true,
  eBoostState: 'DEPLOYING',
  engineMap: 1,
  wheels: {
    fl: {
      outerLeftC: 88.4,
      coreC: 85.2,
      outerRightC: 81.6,
      surfaceC: 86.8,
      carcassC: 84.1,
      pressureKpa: 178.4,
      wearFraction: 0.942,
      brakeDiscTempC: 440,
      brakePeakTempC: 560,
      slipPercentage: 2.1,
      lockupActive: false,
    },
    fr: {
      outerLeftC: 79.4,
      coreC: 83.1,
      outerRightC: 86.8,
      surfaceC: 84.2,
      carcassC: 82.0,
      pressureKpa: 177.1,
      wearFraction: 0.938,
      brakeDiscTempC: 432,
      brakePeakTempC: 554,
      slipPercentage: 1.8,
      lockupActive: false,
    },
    rl: {
      outerLeftC: 91.2,
      coreC: 89.0,
      outerRightC: 87.1,
      surfaceC: 90.4,
      carcassC: 88.2,
      pressureKpa: 176.0,
      wearFraction: 0.915,
      brakeDiscTempC: 385,
      brakePeakTempC: 490,
      slipPercentage: 3.4,
      lockupActive: false,
    },
    rr: {
      outerLeftC: 85.6,
      coreC: 88.4,
      outerRightC: 90.8,
      surfaceC: 89.1,
      carcassC: 87.0,
      pressureKpa: 175.4,
      wearFraction: 0.912,
      brakeDiscTempC: 378,
      brakePeakTempC: 482,
      slipPercentage: 2.9,
      lockupActive: false,
    },
  },
};

// 12 Minisectors for current lap delta inspired by Telemetry Tool v1509
const MINISECTOR_DELTAS = [
  -0.035, -0.062, 0.015, -0.048, 0.008, -0.024,
  -0.018, 0.032, -0.012, -0.054, 0.018, -0.026,
];

export function DrivingView({
  state = DEFAULT_STATE,
  onToggleLimiter,
  onChangeEngineMap,
  onChangeBrakeBias,
}: DrivingViewProps): JSX.Element {
  const liveState = state;

  const fuelPct = ((liveState.fuelL / liveState.fuelCapacityL) * 100).toFixed(1);
  const socPct = (liveState.batterySocFrac * 100).toFixed(0);
  const rpmPct = Math.min(100, (liveState.rpm / liveState.rpmMax) * 100);

  // Suggested gear logic inspired by Telemetry Tool v1509:
  // If upshift recommended -> green, if downshift recommended -> red
  const suggestedGear = useMemo(() => {
    if (liveState.gear > 0 && liveState.gear < 7 && liveState.rpm > 8400 && liveState.throttleFrac > 0.7) {
      return liveState.gear + 1;
    }
    if (liveState.gear > 1 && (liveState.rpm < 4600 || liveState.brakeFrac > 0.4)) {
      return liveState.gear - 1;
    }
    return liveState.gear;
  }, [liveState.gear, liveState.rpm, liveState.throttleFrac, liveState.brakeFrac]);

  // G-G circle coordinate mapping (-2.5G to +2.5G mapped to 10..90 within SVG viewBox)
  const dotX = Math.max(10, Math.min(90, 50 + (liveState.latG / 2.5) * 40));
  const dotY = Math.max(10, Math.min(90, 50 - (liveState.longG / 2.5) * 40));

  // Cumulative lap delta from minisectors
  const totalLapDeltaS = MINISECTOR_DELTAS.reduce((acc, v) => acc + v, 0);

  return (
    <div className="driving-view-container" data-testid="driving-view">
      {/* Top Banner: Minisector timing bar & delta display inspired by Telemetry Tool v1509 */}
      <div className="driving-header-bar">
        <div className="driving-header-left">
          <div className="engine-map-banner">
            <span className="engine-map-label">ENGINE MAP: #{liveState.engineMap}</span>
            <div className="engine-map-buttons">
              <button
                type="button"
                className="btn btn-xs btn-mode-switch"
                onClick={() => onChangeEngineMap?.(liveState.engineMap === 1 ? 2 : 1)}
                title="Toggle between Qualifying and Race engine map"
              >
                {liveState.engineMap === 1 ? 'SWITCH QUAL' : 'SWITCH RACE'}
              </button>
            </div>
          </div>
        </div>

        {/* Minisector Timing Bar (12 sectors with live delta to session reference) */}
        <div className="driving-minisector-strip" title="12 Minisectors delta vs reference lap (Telemetry Tool specification)">
          <div className="minisector-strip-header">
            <span className="minisector-title">LAP DELTA (Δt):</span>
            <span className={`minisector-delta-badge ${totalLapDeltaS < 0 ? 'delta-faster' : 'delta-slower'}`}>
              {totalLapDeltaS < 0 ? '' : '+'}{totalLapDeltaS.toFixed(3)}s
            </span>
            <span className="minisector-sub">BEST: 1:34.563</span>
          </div>
          <div className="minisector-blocks-row">
            {MINISECTOR_DELTAS.map((delta, idx) => {
              const isFaster = delta < 0;
              const isPurple = idx === 1 || idx === 9;
              return (
                <div
                  key={idx}
                  className={`minisector-block ${isPurple ? 'ms-purple' : isFaster ? 'ms-faster' : 'ms-slower'}`}
                  title={`Sector ${idx + 1}: ${delta < 0 ? '' : '+'}${delta.toFixed(3)}s`}
                >
                  <span className="ms-num">{idx + 1}</span>
                </div>
              );
            })}
          </div>
        </div>

        <div className="driving-header-right">
          <div className="session-status-pill pill-active">GREEN FLAG</div>
          <button
            type="button"
            className={`btn btn-xs btn-limiter ${liveState.pitLimiterActive ? 'limiter-on' : 'limiter-off'}`}
            onClick={onToggleLimiter}
          >
            {liveState.pitLimiterActive ? 'PIT LIMITER: ON' : 'PIT LIMITER: OFF'}
          </button>
        </div>
      </div>

      {/* 3-Column Cockpit Layout: Fully responsive, zero right-edge clipping */}
      <div className="driving-cockpit-grid">
        {/* Left Column: Powertrain & Pedals + G-G Friction Circle */}
        <div className="driving-col driving-col-left">
          {/* Vertical Inputs Bar: BRAKE, THROT, REVS, TORQUE (Clean technical layout, no character hacks) */}
          <div className="pedals-vertical-row">
            {/* Brake */}
            <div className="pedal-gauge-card pedal-gauge-brake">
              <span className="pedal-gauge-title">BRAKE</span>
              <div className="pedal-meter-bar-track">
                <div className="pedal-meter-fill brake-fill" style={{ height: `${liveState.brakeFrac * 100}%` }} />
              </div>
              <span className="pedal-gauge-value">{(liveState.brakeFrac * 100).toFixed(0)}%</span>
            </div>

            {/* Throttle */}
            <div className="pedal-gauge-card pedal-gauge-throt">
              <span className="pedal-gauge-title">THROT</span>
              <div className="pedal-meter-bar-track">
                <div className="pedal-meter-fill throt-fill" style={{ height: `${liveState.throttleFrac * 100}%` }} />
              </div>
              <span className="pedal-gauge-value">{(liveState.throttleFrac * 100).toFixed(0)}%</span>
            </div>

            {/* Gear & Center Display with Suggested Gear (Telemetry Tool style) */}
            <div className="pedal-gear-cluster">
              <div className="gear-digit-box">
                {suggestedGear !== liveState.gear && (
                  <span
                    className={`suggested-gear-chip ${suggestedGear > liveState.gear ? 'suggest-upshift' : 'suggest-downshift'}`}
                    title={suggestedGear > liveState.gear ? `Shift up to Gear ${suggestedGear}` : `Shift down to Gear ${suggestedGear}`}
                  >
                    {suggestedGear > liveState.gear ? `▲ ${suggestedGear}` : `▼ ${suggestedGear}`}
                  </span>
                )}
                <span className="gear-digit">{liveState.gear === 0 ? 'N' : liveState.gear === -1 ? 'R' : liveState.gear}</span>
                <span className="speed-sub-label">{liveState.speedKmh.toFixed(0)} km/h</span>
              </div>
              <div className={`clutch-indicator ${liveState.clutchFrac > 0.05 ? 'clutch-active' : ''}`}>
                <span className="clutch-box-text">CLUTCH</span>
                <span className="clutch-sub-val">{(liveState.clutchFrac * 100).toFixed(0)}%</span>
              </div>
            </div>

            {/* Revs */}
            <div className="pedal-gauge-card pedal-gauge-revs">
              <span className="pedal-gauge-title">RPM</span>
              <div className="pedal-meter-bar-track">
                <div
                  className={`pedal-meter-fill revs-fill ${rpmPct > 90 ? 'revs-shift-light' : ''}`}
                  style={{ height: `${rpmPct}%` }}
                />
              </div>
              <span className="pedal-gauge-value">{liveState.rpm.toFixed(0)}</span>
            </div>

            {/* Torque */}
            <div className="pedal-gauge-card pedal-gauge-torque">
              <span className="pedal-gauge-title">TORQ</span>
              <div className="pedal-meter-bar-track">
                <div className="pedal-meter-fill torque-fill" style={{ height: `${Math.min(100, (liveState.torqueNm / 800) * 100)}%` }} />
              </div>
              <span className="pedal-gauge-value">{liveState.torqueNm.toFixed(0)}</span>
            </div>
          </div>

          {/* Fuel & Hybrid Energy Status */}
          <div className="energy-status-panel">
            <div className="energy-stat-row">
              <span className="energy-label">Fuel:</span>
              <span className="energy-val num-cyan">{liveState.fuelL.toFixed(1)}L ({fuelPct}%)</span>
            </div>
            <div className="energy-stat-row">
              <span className="energy-label">SoC (Battery):</span>
              <span className="energy-val num-cyan">{socPct}%</span>
            </div>
            <div className="energy-stat-row">
              <span className="energy-label">Regen / Deploy:</span>
              <span className={`energy-val ${liveState.regenKw < 0 ? 'regen-active' : 'deploy-active'}`}>
                {liveState.regenKw.toFixed(1)} kW
              </span>
            </div>
          </div>

          {/* G-G Friction Circle (Radar Accelerometer) */}
          <div className="friction-circle-card" data-testid="friction-circle">
            <div className="friction-circle-header">
              <span className="friction-title">G-FORCE & FRICTION ENVELOPE</span>
              <span className="friction-vals">Lat: {liveState.latG.toFixed(2)}G | Long: {liveState.longG.toFixed(2)}G</span>
            </div>
            <div className="friction-circle-viewport">
              <svg viewBox="0 0 100 100" className="friction-circle-svg">
                {/* Crosshairs */}
                <line x1="10" y1="50" x2="90" y2="50" stroke="#334155" strokeWidth="1" strokeDasharray="2 2" />
                <line x1="50" y1="10" x2="50" y2="90" stroke="#334155" strokeWidth="1" strokeDasharray="2 2" />

                {/* Concentric G Rings (0.5G, 1.0G, 1.5G, 2.0G) */}
                <circle cx="50" cy="50" r="10" fill="none" stroke="#1e293b" strokeWidth="0.8" />
                <circle cx="50" cy="50" r="20" fill="none" stroke="#1e293b" strokeWidth="0.8" />
                <circle cx="50" cy="50" r="30" fill="none" stroke="#334155" strokeWidth="1" />
                <circle cx="50" cy="50" r="40" fill="none" stroke="#0284c7" strokeWidth="1.2" strokeOpacity="0.4" />

                {/* Outer Grip Limit Envelope */}
                <path d="M 15 50 A 35 35 0 1 1 85 50" fill="none" stroke="#f59e0b" strokeWidth="2" strokeOpacity="0.7" />

                {/* Live G Position Indicator Dot */}
                <circle cx={dotX} cy={dotY} r="3.5" fill="#ef4444" stroke="#ffffff" strokeWidth="1" />
              </svg>
            </div>
            {/* Steering angle indicator */}
            <div className="steering-angle-strip">
              <span className="steer-label">STEERING ANGLE:</span>
              <span className="steer-value">{liveState.steerAngleDeg.toFixed(1)}°</span>
            </div>
          </div>
        </div>

        {/* Center Column: 4-Wheel Top View Tyre Thermals + Drivetrain */}
        <div className="driving-col driving-col-center">
          <TyreCarTopView
            wheels={liveState.wheels}
            engineWaterTempC={liveState.engineWaterTempC}
            engineOilTempC={liveState.engineOilTempC}
            clutchRpm={liveState.clutchRpm}
            lockupEvents={liveState.lockupEvents}
          />
        </div>

        {/* Right Column: Electronics, Aids & Telemetry Data (Telemetry Tool inspired) */}
        <div className="driving-col driving-col-right">
          <div className="aids-status-panel">
            <div className="aids-panel-header">
              <span className="aids-title">DRIVER AIDS & SYSTEMS</span>
            </div>

            <div className="aids-grid">
              <div className="aids-item">
                <span className="aids-item-label">TC / ABS:</span>
                <span className="aids-item-value num-cyan">TC: {liveState.tcLevel} | ABS: {liveState.absLevel}</span>
              </div>
              <div className="aids-item">
                <span className="aids-item-label">TC CUT / SLIP:</span>
                <span className="aids-item-value">CUT: {liveState.tcCut} | Slip: {liveState.tcSlip}%</span>
              </div>
              <div className="aids-item">
                <span className="aids-item-label">AI / AGGR:</span>
                <span className="aids-item-value">AI: 100% | Aggr: High</span>
              </div>
              <div className="aids-item">
                <span className="aids-item-label">FUEL / TYRE RATE:</span>
                <span className="aids-item-value">Fuel: X1 | Tyre: X1</span>
              </div>
            </div>

            {/* Brake Bias with Adjusters */}
            <div className="brakebias-control-card">
              <div className="brakebias-header">
                <span className="brakebias-label">BRAKE BIAS</span>
                <span className="brakebias-val num-cyan">
                  F {(liveState.brakeBiasFrontFrac * 100).toFixed(1)}% / R {((1 - liveState.brakeBiasFrontFrac) * 100).toFixed(1)}%
                </span>
              </div>
              <div className="brakebias-buttons">
                <button
                  type="button"
                  className="btn btn-xs"
                  onClick={() => onChangeBrakeBias?.(-0.005)}
                  title="Shift brake bias rearward (-0.5%)"
                >
                  ◀ REAR (-0.5%)
                </button>
                <button
                  type="button"
                  className="btn btn-xs"
                  onClick={() => onChangeBrakeBias?.(0.005)}
                  title="Shift brake bias forward (+0.5%)"
                >
                  FRONT (+0.5%) ▶
                </button>
              </div>
            </div>

            {/* E-Boost & Pit Limiter */}
            <div className="systems-toggle-list">
              <div className="system-toggle-row">
                <span className="system-name">Hybrid:</span>
                <span className={`system-status-pill pill-${liveState.eBoostState.toLowerCase()}`}>
                  {liveState.eBoostState}
                </span>
              </div>

              <div className="system-toggle-row">
                <span className="system-name">Starter / Ignition:</span>
                <span className={`system-status-pill ${liveState.ignitionActive ? 'pill-active' : 'pill-off'}`}>
                  {liveState.ignitionActive ? 'IGNI: ON' : 'IGNI: OFF'}
                </span>
              </div>
            </div>

            {/* Corner Ride Heights & Damper Travel (Telemetry Tool v1509 feature) */}
            <div className="corner-telemetry-card">
              <div className="corner-telemetry-header">
                <span className="corner-telemetry-title">CORNER RIDE HEIGHTS & DAMAGE</span>
              </div>
              <div className="corner-telemetry-grid">
                <div className="corner-item">
                  <span className="corner-label">FL HEIGHT:</span>
                  <span className="corner-val num-cyan">48.2 mm</span>
                </div>
                <div className="corner-item">
                  <span className="corner-label">FR HEIGHT:</span>
                  <span className="corner-val num-cyan">48.8 mm</span>
                </div>
                <div className="corner-item">
                  <span className="corner-label">RL HEIGHT:</span>
                  <span className="corner-val num-cyan">62.4 mm</span>
                </div>
                <div className="corner-item">
                  <span className="corner-label">RR HEIGHT:</span>
                  <span className="corner-val num-cyan">62.1 mm</span>
                </div>
              </div>
              <div className="corner-damage-row">
                <span className="damage-label">SUSPENSION DAMAGE:</span>
                <span className="damage-val text-green">0.0% (INTACT)</span>
              </div>
            </div>

            {/* Brake Wear & Peak Temperatures */}
            <div className="brake-wear-card">
              <div className="brake-wear-header">
                <span className="brake-wear-title">BRAKE DISC WEAR (REST API)</span>
              </div>
              <div className="brake-wear-grid">
                <div className="brake-wear-item">FL: <strong>1.2%</strong> (560°C peak)</div>
                <div className="brake-wear-item">FR: <strong>1.1%</strong> (554°C peak)</div>
                <div className="brake-wear-item">RL: <strong>0.8%</strong> (490°C peak)</div>
                <div className="brake-wear-item">RR: <strong>0.8%</strong> (482°C peak)</div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
