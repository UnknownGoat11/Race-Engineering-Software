import type { JSX } from 'react';
import type { HypercarVetState } from '../types';

export interface HypercarVetQuickWidgetProps {
  readonly state?: HypercarVetState | undefined;
  readonly speedKmh?: number | undefined;
  readonly onOpenFullView?: (() => void) | undefined;
  readonly compact?: boolean | undefined;
}

export const DEFAULT_HYPERCAR_VET: HypercarVetState = {
  vetCapacityMj: 900.0,
  vetRemainingMj: 615.6,
  targetPerLapMj: 26.50,
  actualLastLapMj: 26.18,
  deltaLastLapMj: -0.32,
  stintLapsRemaining: 23.2,
  mgukDeployKw: 180.0,
  mgukRegenKw: -125.0,
  mgukGateSpeedKmh: 190.0,
  mgukGateActive: true,
  batterySocFrac: 0.784,
  batteryTempC: 44.2,
  regenHarvestedLapMj: 3.42,
  deployMode: 'BALANCED',
};

export function HypercarVetQuickWidget({
  state = DEFAULT_HYPERCAR_VET,
  speedKmh = 218.4,
  onOpenFullView,
  compact = false,
}: HypercarVetQuickWidgetProps): JSX.Element {
  const vetPct = (state.vetRemainingMj / state.vetCapacityMj) * 100;
  const isOverConsuming = state.deltaLastLapMj > 0.15;
  const isUnderConsuming = state.deltaLastLapMj < -0.15;
  const isGateActive = speedKmh >= state.mgukGateSpeedKmh;

  return (
    <div
      className={`hypercar-vet-quick-card ${compact ? 'vet-compact' : ''}`}
      data-testid="hypercar-vet-quick-widget"
    >
      <div className="vet-quick-header">
        <div className="vet-title-wrap">
          <span className="vet-glyph">⚡</span>
          <span className="vet-title">HYPERCAR VIRTUAL ENERGY (VET)</span>
        </div>
        <div className="vet-header-right">
          <span className="badge badge-subtle">{state.deployMode} MODE</span>
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

      {/* Main Virtual Energy Tank Gauge & Target Status */}
      <div className="vet-quick-body">
        {/* VET Progress & Quantity */}
        <div className="vet-tank-meter">
          <div className="vet-tank-info">
            <span className="vet-tank-label">STINT ENERGY REMAINING</span>
            <span className="vet-tank-val num-cyan">
              {state.vetRemainingMj.toFixed(1)} / {state.vetCapacityMj.toFixed(0)} MJ
            </span>
          </div>

          <div className="vet-meter-track" role="progressbar" aria-valuenow={vetPct} aria-valuemin={0} aria-valuemax={100}>
            <div
              className="vet-meter-fill"
              style={{
                width: `${Math.min(100, Math.max(0, vetPct))}%`,
                backgroundColor: vetPct < 20 ? '#ef4444' : vetPct < 40 ? '#f59e0b' : '#06b6d4',
              }}
            />
          </div>

          <div className="vet-meter-sub">
            <span className="vet-pct-text">{vetPct.toFixed(1)}% Tank</span>
            <span className="vet-laps-rem">~{state.stintLapsRemaining.toFixed(1)} Laps Left</span>
          </div>
        </div>

        {/* Delta vs Target Pill */}
        <div className="vet-delta-box">
          <span className="vet-delta-label">LAP ENERGY DELTA</span>
          <div
            className={`vet-delta-pill ${
              isOverConsuming ? 'delta-over' : isUnderConsuming ? 'delta-under' : 'delta-on-target'
            }`}
          >
            <span className="vet-delta-sign">
              {state.deltaLastLapMj > 0 ? `+${state.deltaLastLapMj.toFixed(2)}` : state.deltaLastLapMj.toFixed(2)} MJ
            </span>
            <span className="vet-delta-tag">
              {isOverConsuming
                ? '⚠️ LIFT & COAST'
                : isUnderConsuming
                ? '✅ PUSH ALLOWED'
                : '🎯 ON TARGET'}
            </span>
          </div>
          <span className="vet-target-info">
            Target: {state.targetPerLapMj.toFixed(2)} MJ | Last: {state.actualLastLapMj.toFixed(2)} MJ
          </span>
        </div>
      </div>

      {/* MGU-K Hybrid Deployment & Axle Gate Status */}
      <div className="vet-mguk-strip">
        <div className="mguk-gate-chip">
          <span className="gate-label">MGU-K 190 KM/H GATE:</span>
          <span className={`gate-status-pill ${isGateActive ? 'gate-active' : 'gate-inhibited'}`}>
            {isGateActive ? `ACTIVE (${speedKmh.toFixed(0)} KM/H)` : `INHIBITED (<190)`}
          </span>
        </div>

        <div className="mguk-power-item">
          <span className="mguk-power-label">DEPLOY:</span>
          <span className="mguk-power-val num-green">{state.mgukDeployKw.toFixed(0)} kW</span>
        </div>

        <div className="mguk-power-item">
          <span className="mguk-power-label">REGEN:</span>
          <span className="mguk-power-val num-cyan">{state.mgukRegenKw.toFixed(0)} kW</span>
        </div>

        <div className="mguk-power-item">
          <span className="mguk-power-label">BATT SOC:</span>
          <span className="mguk-power-val">{(state.batterySocFrac * 100).toFixed(1)}%</span>
        </div>
      </div>
    </div>
  );
}
