import type { JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatFuel, formatFuelPerLap, formatSpeed, formatTemperature, NO_VALUE } from '../../lib/units';
import { fracPct, useLiveReadings } from '../liveData';
import { HypercarVetQuickWidget } from './HypercarVetQuickWidget';
import { LiveCell, NOT_CONNECTED_REASON, ViewNoData } from './ViewNoData';

const HYBRID_CHANNELS: readonly number[] = [
  ch.VIRTUAL_ENERGY,
  ch.VE_PER_LAP,
  ch.VE_USED_THIS_LAP,
  ch.VE_LAPS_REMAINING,
  ch.FUEL,
  ch.FUEL_CAPACITY,
  ch.FUEL_PER_LAP,
  ch.FUEL_LAPS_REMAINING,
  ch.SPEED,
  ch.BATTERY_SOC,
  ch.BATTERY_CHARGE,
  ch.REGEN_POWER,
  ch.HYBRID_MOTOR_TORQUE,
  ch.HYBRID_MOTOR_RPM,
  ch.HYBRID_MOTOR_TEMP,
  ch.HYBRID_WATER_TEMP,
  ch.HYBRID_MOTOR_STATE,
];

function laps(value: number | null): string {
  return value === null ? NO_VALUE : `${value.toFixed(1)} laps`;
}

export function HypercarHybridView(): JSX.Element {
  const r = useLiveReadings(HYBRID_CHANNELS);

  const ve = r.num(ch.VIRTUAL_ENERGY);
  const veLaps = r.num(ch.VE_LAPS_REMAINING);
  const fuel = r.num(ch.FUEL);
  const fuelCapacity = r.num(ch.FUEL_CAPACITY);
  const fuelLaps = r.num(ch.FUEL_LAPS_REMAINING);
  const regenW = r.num(ch.REGEN_POWER);
  const motorRpm = r.num(ch.HYBRID_MOTOR_RPM);
  const motorTorque = r.num(ch.HYBRID_MOTOR_TORQUE);
  const motorState = r.num(ch.HYBRID_MOTOR_STATE);

  // Which allowance runs out first, from the two calculated laps-remaining channels; unknown unless both exist.
  const limiter =
    veLaps !== null && fuelLaps !== null ? (veLaps < fuelLaps ? 'VIRTUAL ENERGY' : 'FUEL') : null;
  const fuelPct = fuel !== null && fuelCapacity !== null && fuelCapacity > 0 ? (fuel / fuelCapacity) * 100 : null;

  return (
    <div className="view-card hypercar-hybrid-view" data-testid="hypercar-hybrid-view">
      <div className="view-card-header">
        <div className="view-card-title-group">
          <span className="view-card-glyph">⚡</span>
          <h2 className="view-card-title">Hypercar Virtual Energy & Hybrid (Live)</h2>
          <span className="badge badge-accent">LIVE CHANNELS</span>
        </div>
      </div>

      {!r.connected && <ViewNoData title="Hybrid & virtual energy" reason={NOT_CONNECTED_REASON} testId="hybrid-not-connected" />}

      <div className="hypercar-quick-banner">
        <HypercarVetQuickWidget />
      </div>

      <div className="hypercar-main-grid">
        <div className="hypercar-col-left">
          <div className="stat-card">
            <div className="stat-card-label">STINT LIMITER (VIRTUAL ENERGY VS FUEL)</div>
            <div className="limiter-hero-row">
              <span className="limiter-title">RUNS OUT FIRST:</span>
              <span className="limiter-name badge badge-accent" data-testid="hybrid-stint-limiter">
                {limiter ?? NO_VALUE}
              </span>
            </div>

            <div className="limiter-comparison-grid">
              <div className="limiter-box">
                <span className="limiter-metric-label">VIRTUAL ENERGY REMAINING</span>
                <span className="limiter-metric-val num-cyan" data-testid="hybrid-ve-remaining">
                  {fracPct(ve)}
                </span>
                <span className="limiter-sub">
                  {laps(veLaps)} @ {fracPct(r.num(ch.VE_PER_LAP))}/lap (estimated)
                </span>
              </div>

              <div className="limiter-box">
                <span className="limiter-metric-label">FUEL REMAINING</span>
                <span className="limiter-metric-val num-green" data-testid="hybrid-fuel-remaining">
                  {formatFuel(fuel, { withUnit: true })}
                  {fuelPct !== null ? ` (${fuelPct.toFixed(0)}%)` : ''}
                </span>
                <span className="limiter-sub">
                  {laps(fuelLaps)} @ {formatFuelPerLap(r.num(ch.FUEL_PER_LAP), { withUnit: true })} (estimated)
                </span>
              </div>
            </div>
          </div>
        </div>

        <div className="hypercar-col-right">
          <div className="stat-card">
            <div className="stat-card-label">HYBRID SYSTEM (LIVE)</div>
            <div className="view-live-grid" data-testid="hybrid-live-grid">
              <LiveCell label="Speed" value={formatSpeed(r.num(ch.SPEED), { withUnit: true })} />
              <LiveCell label="Battery SoC" value={fracPct(r.num(ch.BATTERY_SOC))} />
              <LiveCell label="Battery charge" value={fracPct(r.num(ch.BATTERY_CHARGE))} />
              <LiveCell label="Regen power" value={regenW !== null ? `${(regenW / 1000).toFixed(1)} kW` : NO_VALUE} />
              <LiveCell label="Motor torque" value={motorTorque !== null ? `${motorTorque.toFixed(1)} Nm` : NO_VALUE} />
              <LiveCell label="Motor speed" value={motorRpm !== null ? `${Math.round(motorRpm).toString()} rpm` : NO_VALUE} />
              <LiveCell label="Motor temp" value={formatTemperature(r.num(ch.HYBRID_MOTOR_TEMP), { withUnit: true })} />
              <LiveCell label="Cooler temp" value={formatTemperature(r.num(ch.HYBRID_WATER_TEMP), { withUnit: true })} />
              <LiveCell
                label="Motor state (raw)"
                value={motorState !== null ? String(Math.round(motorState)) : NO_VALUE}
              />
              <LiveCell label="VE used this lap" value={fracPct(r.num(ch.VE_USED_THIS_LAP))} />
            </div>
          </div>

          <ViewNoData
            title="Per-corner regeneration & lift-and-coast"
            reason="Per-braking-zone energy harvest needs lap-distance history this app does not record yet."
            testId="hybrid-regen-zones-no-data"
          />
        </div>
      </div>
    </div>
  );
}
