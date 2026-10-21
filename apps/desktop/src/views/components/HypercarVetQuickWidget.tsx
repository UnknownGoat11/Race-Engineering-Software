import type { JSX } from 'react';
import { ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { fracPct, useLiveReadings } from '../liveData';

export interface HypercarVetQuickWidgetProps {
  readonly onOpenFullView?: (() => void) | undefined;
  readonly compact?: boolean | undefined;
}

const VET_WIDGET_CHANNELS: readonly number[] = [
  ch.VIRTUAL_ENERGY,
  ch.VE_PER_LAP,
  ch.VE_USED_LAST_LAP,
  ch.VE_LAPS_REMAINING,
  ch.BATTERY_SOC,
  ch.REGEN_POWER,
  ch.HYBRID_MOTOR_TORQUE,
];

/**
 * Compact virtual-energy summary from live channels: VIRTUAL_ENERGY (fraction of the allowance), per-lap
 * usage, laps remaining, battery state of charge, regeneration power and hybrid motor torque. LMU reports
 * virtual energy as a fraction, so no MJ figures are shown.
 */
export function HypercarVetQuickWidget({ onOpenFullView, compact = false }: HypercarVetQuickWidgetProps): JSX.Element {
  const r = useLiveReadings(VET_WIDGET_CHANNELS);
  const ve = r.num(ch.VIRTUAL_ENERGY);
  const vePerLap = r.num(ch.VE_PER_LAP);
  const veLastLap = r.num(ch.VE_USED_LAST_LAP);
  const veLaps = r.num(ch.VE_LAPS_REMAINING);
  const soc = r.num(ch.BATTERY_SOC);
  const regenW = r.num(ch.REGEN_POWER);
  const motorTorque = r.num(ch.HYBRID_MOTOR_TORQUE);

  const vePct = ve !== null ? ve * 100 : null;
  // Last lap minus the window average, in percentage points of the allowance (both measured / calculated).
  const deltaPts = veLastLap !== null && vePerLap !== null ? (veLastLap - vePerLap) * 100 : null;

  return (
    <div className={`hypercar-vet-quick-card ${compact ? 'vet-compact' : ''}`} data-testid="hypercar-vet-quick-widget">
      <div className="vet-quick-header">
        <div className="vet-title-wrap">
          <span className="vet-glyph">⚡</span>
          <span className="vet-title">VIRTUAL ENERGY (VE)</span>
        </div>
        <div className="vet-header-right">
          {onOpenFullView && (
            <button
              type="button"
              className="btn btn-xs btn-outline"
              onClick={onOpenFullView}
              title="Open full Hypercar Hybrid telemetry screen"
            >
              FULL VIEW ↗
            </button>
          )}
        </div>
      </div>

      <div className="vet-quick-body">
        <div className="vet-tank-meter">
          <div className="vet-tank-info">
            <span className="vet-tank-label">VIRTUAL ENERGY REMAINING</span>
            <span className="vet-tank-val num-cyan" data-testid="vet-remaining">
              {fracPct(ve)}
            </span>
          </div>

          {vePct !== null && (
            <div
              className="vet-meter-track"
              role="progressbar"
              aria-valuenow={vePct}
              aria-valuemin={0}
              aria-valuemax={100}
            >
              <div className="vet-meter-fill" style={{ width: `${Math.min(100, Math.max(0, vePct))}%` }} />
            </div>
          )}

          <div className="vet-meter-sub">
            <span className="vet-laps-rem" data-testid="vet-laps-remaining">
              {veLaps !== null ? `${veLaps.toFixed(1)} laps left (estimated)` : `Laps left: ${NO_VALUE}`}
            </span>
          </div>
        </div>

        <div className="vet-delta-box">
          <span className="vet-delta-label">LAST LAP VS AVERAGE</span>
          <div className="vet-delta-pill" data-testid="vet-lap-delta">
            <span className="vet-delta-sign">
              {deltaPts !== null ? `${deltaPts > 0 ? '+' : ''}${deltaPts.toFixed(2)} pts` : NO_VALUE}
            </span>
          </div>
          <span className="vet-target-info">
            Avg/lap: {fracPct(vePerLap)} | Last: {fracPct(veLastLap)}
          </span>
        </div>
      </div>

      <div className="vet-mguk-strip">
        <div className="mguk-power-item">
          <span className="mguk-power-label">REGEN:</span>
          <span className="mguk-power-val num-cyan" data-testid="vet-regen">
            {regenW !== null ? `${(regenW / 1000).toFixed(0)} kW` : NO_VALUE}
          </span>
        </div>
        <div className="mguk-power-item">
          <span className="mguk-power-label">MOTOR TORQUE:</span>
          <span className="mguk-power-val num-green">
            {motorTorque !== null ? `${motorTorque.toFixed(0)} Nm` : NO_VALUE}
          </span>
        </div>
        <div className="mguk-power-item">
          <span className="mguk-power-label">BATT SOC:</span>
          <span className="mguk-power-val" data-testid="vet-soc">
            {fracPct(soc)}
          </span>
        </div>
      </div>
    </div>
  );
}
