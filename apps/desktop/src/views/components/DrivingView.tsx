import type { JSX } from 'react';
import type { VehicleDynamicsState } from '../types';
import { TyreCarTopView } from './TyreCarTopView';
import { HypercarVetQuickWidget, DEFAULT_HYPERCAR_VET } from './HypercarVetQuickWidget';
import { DEFAULT_LOCKUP_EVENTS } from './TyreDegQuickSummary';

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
  hypercarVet: DEFAULT_HYPERCAR_VET,
  lockupEvents: DEFAULT_LOCKUP_EVENTS,
};

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

  // G-G circle coordinate mapping (-2.0G to +2.0G mapped to 0..100% within SVG)
  const dotX = 50 + (liveState.latG / 2.5) * 45;
  const dotY = 50 - (liveState.longG / 2.5) * 45;

  return (
    <div className="driving-view-container" data-testid="driving-view">
      {/* 3-Column Cockpit Layout directly inspired by Image 2 */}
      <div className="driving-cockpit-grid">
        {/* Left Column: Powertrain & Pedals + G-G Friction Circle */}
        <div className="driving-col driving-col-left">
          {/* Engine Map Banner */}
          <div className="engine-map-banner">
            <span className="engine-map-label">ENGINE MAP: #{liveState.engineMap}</span>
            <div className="engine-map-buttons">
              <button
                type="button"
                className="btn btn-xs btn-mode-switch"
                onClick={() => onChangeEngineMap?.(liveState.engineMap === 1 ? 2 : 1)}
              >
                {liveState.engineMap === 1 ? 'SWITCH QUAL' : 'SWITCH RACE'}
              </button>
            </div>
          </div>

          {/* Vertical Inputs Bar: BRAKE, THROT, REVS, TORQUE */}
          <div className="pedals-vertical-row">
            {/* Brake */}
            <div className="pedal-gauge-card pedal-gauge-brake">
              <span className="pedal-gauge-title">B<br />R<br />A<br />K<br />E</span>
              <div className="pedal-meter-bar-track">
                <div className="pedal-meter-fill brake-fill" style={{ height: `${liveState.brakeFrac * 100}%` }} />
              </div>
              <span className="pedal-gauge-value">{(liveState.brakeFrac * 100).toFixed(0)}%</span>
            </div>

            {/* Throttle */}
            <div className="pedal-gauge-card pedal-gauge-throt">
              <span className="pedal-gauge-title">T<br />H<br />R<br />O<br />T</span>
              <div className="pedal-meter-bar-track">
                <div className="pedal-meter-fill throt-fill" style={{ height: `${liveState.throttleFrac * 100}%` }} />
              </div>
              <span className="pedal-gauge-value">{(liveState.throttleFrac * 100).toFixed(0)}%</span>
            </div>

            {/* Gear & Center Display */}
            <div className="pedal-gear-cluster">
              <div className="gear-digit-box">
                <span className="gear-digit">{liveState.gear === 0 ? 'N' : liveState.gear === -1 ? 'R' : liveState.gear}</span>
                <span className="speed-sub-label">{liveState.speedKmh.toFixed(0)} km/h</span>
              </div>
              <div className={`clutch-indicator ${liveState.clutchFrac > 0.05 ? 'clutch-active' : ''}`}>
                <span className="clutch-box-text">C<br />L</span>
                <span className="clutch-sub-val">{(liveState.clutchFrac * 100).toFixed(0)}%</span>
              </div>
            </div>

            {/* Revs */}
            <div className="pedal-gauge-card pedal-gauge-revs">
              <span className="pedal-gauge-title">R<br />E<br />V<br />S</span>
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
              <span className="pedal-gauge-title">T<br />O<br />R<br />Q</span>
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

          {/* Hypercar Virtual Energy Tank (VET) Quick Glance Widget */}
          <div className="driving-hypercar-widget-wrap">
            <HypercarVetQuickWidget
              state={liveState.hypercarVet ?? DEFAULT_HYPERCAR_VET}
              speedKmh={liveState.speedKmh}
              compact
            />
          </div>

          {/* G-G Friction Circle (Radar Accelerometer) per Image 2 */}
          <div className="friction-circle-card" data-testid="friction-circle">
            <div className="friction-circle-header">
              <span className="friction-title">G-FORCE & FRICTION ENVELOPE</span>
              <span className="friction-vals">Lat: {liveState.latG.toFixed(2)}G | Long: {liveState.longG.toFixed(2)}G</span>
            </div>
            <div className="friction-circle-viewport">
              <svg viewBox="0 0 100 100" className="friction-circle-svg">
                {/* Crosshairs */}
                <line x1="10" y1="50" x2="90" y2="50" stroke="#475569" strokeWidth="1" strokeDasharray="2 2" />
                <line x1="50" y1="10" x2="50" y2="90" stroke="#475569" strokeWidth="1" strokeDasharray="2 2" />

                {/* Concentric G Rings (0.5G, 1.0G, 1.5G, 2.0G) */}
                <circle cx="50" cy="50" r="10" fill="none" stroke="#334155" strokeWidth="0.8" />
                <circle cx="50" cy="50" r="20" fill="none" stroke="#334155" strokeWidth="0.8" />
                <circle cx="50" cy="50" r="30" fill="none" stroke="#475569" strokeWidth="1" />
                <circle cx="50" cy="50" r="40" fill="none" stroke="#22d3ee" strokeWidth="1.2" strokeOpacity="0.4" />

                {/* Outer Grip Limit Radar Arc */}
                <path d="M 15 50 A 35 35 0 1 1 85 50" fill="none" stroke="#f59e0b" strokeWidth="2.5" strokeOpacity="0.7" />

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

        {/* Right Column: Electronics, Aids & Session Status */}
        <div className="driving-col driving-col-right">
          <div className="aids-status-panel">
            <div className="aids-panel-header">
              <span className="aids-title">DRIVER AIDS & SYSTEMS</span>
            </div>

            <div className="aids-grid">
              <div className="aids-item">
                <span className="aids-item-label">TRACTION CONTROL:</span>
                <span className="aids-item-value num-cyan">TC: {liveState.tcLevel}</span>
              </div>
              <div className="aids-item">
                <span className="aids-item-label">ABS LEVEL:</span>
                <span className="aids-item-value num-cyan">ABS: {liveState.absLevel}</span>
              </div>
              <div className="aids-item">
                <span className="aids-item-label">TC CUT / SLIP:</span>
                <span className="aids-item-value">CUT: {liveState.tcCut} | Slip: {liveState.tcSlip}%</span>
              </div>
              <div className="aids-item">
                <span className="aids-item-label">AI / AGGRESSION:</span>
                <span className="aids-item-value">AI: 0 | Aggr: 0</span>
              </div>
              <div className="aids-item">
                <span className="aids-item-label">FUEL & TYRE MODES:</span>
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
                <span className="system-name">E-Boost Hybrid:</span>
                <span className={`system-status-pill pill-${liveState.eBoostState.toLowerCase()}`}>
                  {liveState.eBoostState}
                </span>
              </div>

              <div className="system-toggle-row">
                <span className="system-name">Pit Lane Limiter:</span>
                <button
                  type="button"
                  className={`btn btn-xs btn-limiter ${liveState.pitLimiterActive ? 'limiter-on' : 'limiter-off'}`}
                  onClick={onToggleLimiter}
                >
                  {liveState.pitLimiterActive ? 'LIMITER ON (60 KM/H)' : 'NO PIT-LIMITER'}
                </button>
              </div>

              <div className="system-toggle-row">
                <span className="system-name">Ignition / Starter:</span>
                <span className={`system-status-pill ${liveState.ignitionActive ? 'pill-active' : 'pill-off'}`}>
                  {liveState.ignitionActive ? 'IGNI: ON' : 'IGNI: OFF'}
                </span>
              </div>
            </div>

            {/* Session Event Feed */}
            <div className="session-events-ticker">
              <span className="session-events-title">-- SESSION EVENTS --</span>
              <div className="session-events-scroll">
                <div className="event-line">P1 lap time: 1:31.428 (GR010)</div>
                <div className="event-line">Sector 2 Yellow Cleared</div>
                <div className="event-line">FL Brake Peak Temp Recorded: 560°C</div>
                <div className="event-line">Virtual Energy Remaining: 68.4%</div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
