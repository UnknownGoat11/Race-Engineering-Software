import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatFuel, formatFuelPerLap, NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { Panel } from '../ui/Panel';
import { StatusBadge } from '../ui/StatusBadge';
import { useScopeDq } from './presentation';
import './telemetry.css';

const FUEL_CHANNELS = [
  ch.FUEL,
  ch.FUEL_CAPACITY,
  ch.FUEL_PER_LAP,
  ch.FUEL_USED_LAST_LAP,
  ch.FUEL_LAPS_REMAINING,
  ch.FUEL_TO_FINISH,
  ch.FUEL_TO_ADD,
  ch.FUEL_MARGIN_LAPS,
  ch.VIRTUAL_ENERGY,
  ch.VE_PER_LAP,
  ch.VE_LAPS_REMAINING,
] as const;

export function FuelPanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <Panel title="FUEL" testId="panel-fuel">
        <span className="card-row-label">No telemetry store active</span>
      </Panel>
    );
  }
  return <FuelPanelContent />;
}

function FuelPanelContent(): JSX.Element {
  const dq = useScopeDq();
  const samples = useChannelValues(FUEL_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < FUEL_CHANNELS.length; i++) {
      const id = FUEL_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  const fuel = num(ch.FUEL);
  const capacity = num(ch.FUEL_CAPACITY);
  const consPerLap = num(ch.FUEL_PER_LAP);
  const lastLapUsed = num(ch.FUEL_USED_LAST_LAP);
  const lapsRemaining = num(ch.FUEL_LAPS_REMAINING);
  const fuelToFinish = num(ch.FUEL_TO_FINISH);
  const fuelToAdd = num(ch.FUEL_TO_ADD);
  const marginLaps = num(ch.FUEL_MARGIN_LAPS);

  // Virtual Energy channels (EN10)
  const ve = num(ch.VIRTUAL_ENERGY);
  const vePerLap = num(ch.VE_PER_LAP);
  const veLapsRemaining = num(ch.VE_LAPS_REMAINING);
  const hasVe = ve != null || vePerLap != null || veLapsRemaining != null;

  const fuelPct = fuel != null && capacity != null && capacity > 0 ? (fuel / capacity) * 100 : null;

  const fillModifier = useMemo(() => {
    if (fuelPct == null) return 'progress-fill--safe';
    if (fuelPct < 15 || (marginLaps != null && marginLaps < 1.0)) {
      return 'progress-fill--crit';
    }
    if (fuelPct < 30 || (marginLaps != null && marginLaps < 2.5)) {
      return 'progress-fill--warn';
    }
    return 'progress-fill--safe';
  }, [fuelPct, marginLaps]);

  const metaContent = lapsRemaining != null ? (
    <StatusBadge
      level={lapsRemaining <= 2 ? 'critical' : lapsRemaining <= 4 ? 'warn' : 'ok'}
      label={`${lapsRemaining.toFixed(1)} LAPS LEFT`}
      glyph={false}
    />
  ) : null;

  return (
    <Panel title="FUEL" dq={dq} meta={metaContent} testId="panel-fuel">
      <div className="fuel-hero">
        <span className="fuel-hero-val">{formatFuel(fuel)}</span>
        <span className="card-row-label">/ {formatFuel(capacity, { withUnit: true })}</span>
        {fuelPct != null && (
          <span className="card-row-label fuel-pct-label">({fuelPct.toFixed(0)}%)</span>
        )}
      </div>

      {/* Visual fuel capacity bar — rendered ONLY when fuel percentage is known (T03) */}
      {fuelPct != null && (
        <div data-testid="fuel-bar" className="progress-track">
          <div
            className={`progress-fill ${fillModifier}`}
            style={{ width: `${Math.min(100, Math.max(0, fuelPct))}%` }}
          />
        </div>
      )}

      <div className="telemetry-grid-2x2">
        <div className="card-row">
          <span className="card-row-label">Avg Cons/Lap</span>
          <span className="card-row-value">{formatFuelPerLap(consPerLap, { withUnit: true })}</span>
        </div>
        <div className="card-row">
          <span className="card-row-label">Last Lap Used</span>
          <span className="card-row-value">{formatFuel(lastLapUsed, { withUnit: true })}</span>
        </div>
        <div className="card-row">
          <span className="card-row-label">Req to Finish</span>
          <span className="card-row-value">
            {fuelToFinish != null ? formatFuel(fuelToFinish, { withUnit: true }) : NO_VALUE}
          </span>
        </div>
        <div className="card-row">
          <span className="card-row-label">
            Fuel to Add
            <span className="reserve-note">(reserve not reported)</span>
          </span>
          <span className="card-row-value">
            {fuelToAdd != null ? formatFuel(fuelToAdd, { withUnit: true }) : NO_VALUE}
          </span>
        </div>
      </div>

      {/* Combined Virtual Energy section when available (EN10) */}
      {hasVe && (
        <div className="ve-combined-section">
          <div className="card-row">
            <span className="card-row-label">VE Laps Remaining</span>
            <span className="card-row-value">
              {veLapsRemaining != null ? `${veLapsRemaining.toFixed(1)} laps` : NO_VALUE}
            </span>
          </div>
          <div className="card-row">
            <span className="card-row-label">VE / Lap</span>
            <span className="card-row-value">
              {vePerLap != null ? `${(vePerLap * 100).toFixed(1)}%` : NO_VALUE}
            </span>
          </div>
        </div>
      )}

      {marginLaps != null && (
        <div className="card-row margin-row">
          <span className="card-row-label">Safety Margin</span>
          <span className="card-row-value">
            {marginLaps >= 0 ? `+${marginLaps.toFixed(1)} laps` : `${marginLaps.toFixed(1)} laps`}
          </span>
        </div>
      )}
    </Panel>
  );
}
