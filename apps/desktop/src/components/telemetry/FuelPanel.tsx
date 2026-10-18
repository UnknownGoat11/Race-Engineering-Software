import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatFuel, formatFuelPerLap, NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
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
] as const;

export function FuelPanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <div className="card" data-testid="panel-fuel">
        <div className="card-header">
          <span className="card-header-title">FUEL CALCULATION</span>
        </div>
        <div className="card-body">
          <span className="card-row-label">No telemetry store active</span>
        </div>
      </div>
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

  const fuelPct = fuel != null && capacity != null && capacity > 0 ? (fuel / capacity) * 100 : null;

  return (
    <div className="card" data-testid="panel-fuel" data-dq={dq ?? undefined}>
      <div className="card-header">
        <span className="card-header-title">FUEL CALCULATION</span>
        <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
          {dq === 'stale' && (
            <span className="status-badge status-badge-stale">STALE</span>
          )}
          {dq === 'untrusted' && (
            <span className="status-badge status-badge-error">UNTRUSTED</span>
          )}
          {lapsRemaining != null && (
            <span
              className={`status-badge ${
                lapsRemaining <= 2
                  ? 'status-badge-error'
                  : lapsRemaining <= 4
                    ? 'status-badge-warn'
                    : 'status-badge-live'
              }`}
            >
              {lapsRemaining.toFixed(1)} LAPS LEFT
            </span>
          )}
        </div>
      </div>
      <div className="card-body">
        {dq === 'untrusted' && (
          <div className="panel-untrusted-note">Untrusted — data format mismatch</div>
        )}

        <div className="fuel-hero" style={{ display: 'flex', alignItems: 'baseline', gap: '8px', marginBottom: '8px' }}>
          <span style={{ fontSize: '26px', fontWeight: 700, fontFamily: 'monospace' }}>
            {formatFuel(fuel)}
          </span>
          <span className="card-row-label">/ {formatFuel(capacity, { withUnit: true })}</span>
          {fuelPct != null && (
            <span className="card-row-label" style={{ marginLeft: 'auto' }}>
              ({fuelPct.toFixed(0)}%)
            </span>
          )}
        </div>

        {/* Visual fuel capacity bar — rendered ONLY when fuel percentage is known (T03) */}
        {fuelPct != null && (
          <div
            data-testid="fuel-bar"
            style={{
              height: '8px',
              backgroundColor: 'var(--color-surface)',
              borderRadius: '4px',
              overflow: 'hidden',
              marginBottom: '12px',
            }}
          >
            <div
              style={{
                height: '100%',
                width: `${Math.min(100, Math.max(0, fuelPct))}%`,
                backgroundColor:
                  fuelPct < 15
                    ? 'var(--color-error)'
                    : fuelPct < 30
                      ? 'var(--color-warn)'
                      : 'var(--color-accent)',
                transition: 'width 0.3s ease',
              }}
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
              <span style={{ display: 'block', fontSize: '10px', color: 'var(--color-text-dim)' }}>
                (reserve not reported)
              </span>
            </span>
            <span className="card-row-value" style={{ fontWeight: 600 }}>
              {fuelToAdd != null ? formatFuel(fuelToAdd, { withUnit: true }) : NO_VALUE}
            </span>
          </div>
        </div>

        {marginLaps != null && (
          <div className="card-row" style={{ marginTop: '8px', borderTop: '1px solid var(--color-border)', paddingTop: '6px' }}>
            <span className="card-row-label">Safety Margin</span>
            <span className="card-row-value">
              {marginLaps >= 0 ? `+${marginLaps.toFixed(1)} laps` : `${marginLaps.toFixed(1)} laps`}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
