import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatPressure, formatTemperature, NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { useScopeDq } from './presentation';
import './telemetry.css';

const TYRE_CHANNELS = [
  // FL
  ch.TYRE_TEMP_INNER_FL,
  ch.TYRE_TEMP_MIDDLE_FL,
  ch.TYRE_TEMP_OUTER_FL,
  ch.TYRE_CARCASS_TEMP_FL,
  ch.TYRE_PRESSURE_FL,
  ch.TYRE_WEAR_FL,
  // FR
  ch.TYRE_TEMP_INNER_FR,
  ch.TYRE_TEMP_MIDDLE_FR,
  ch.TYRE_TEMP_OUTER_FR,
  ch.TYRE_CARCASS_TEMP_FR,
  ch.TYRE_PRESSURE_FR,
  ch.TYRE_WEAR_FR,
  // RL
  ch.TYRE_TEMP_INNER_RL,
  ch.TYRE_TEMP_MIDDLE_RL,
  ch.TYRE_TEMP_OUTER_RL,
  ch.TYRE_CARCASS_TEMP_RL,
  ch.TYRE_PRESSURE_RL,
  ch.TYRE_WEAR_RL,
  // RR
  ch.TYRE_TEMP_INNER_RR,
  ch.TYRE_TEMP_MIDDLE_RR,
  ch.TYRE_TEMP_OUTER_RR,
  ch.TYRE_CARCASS_TEMP_RR,
  ch.TYRE_PRESSURE_RR,
  ch.TYRE_WEAR_RR,
  // Balance
  ch.TYRE_TEMP_BALANCE_FRONT_REAR,
  ch.TYRE_TEMP_BALANCE_LEFT_RIGHT,
] as const;

interface CornerData {
  inner: number | null | undefined;
  mid: number | null | undefined;
  outer: number | null | undefined;
  carcass: number | null | undefined;
  pressure: number | null | undefined;
  wear: number | null | undefined;
}

function getTempColor(tempC: number | null | undefined): string {
  if (tempC == null || isNaN(tempC)) return 'var(--color-text-muted)';
  if (tempC < 70) return '#60a5fa'; // Blue (cold)
  if (tempC < 100) return '#4ade80'; // Green (optimal)
  if (tempC < 115) return '#fbbf24'; // Amber (warm)
  return '#f87171'; // Red (overheating)
}

function renderCorner(title: string, data: CornerData, testId: string): JSX.Element {
  return (
    <div className="tyre-corner-box" data-testid={testId}>
      <div className="tyre-corner-header">
        <span className="tyre-corner-title">{title}</span>
        <span className="tyre-corner-pressure">
          {formatPressure(data.pressure, { withUnit: true })}
        </span>
      </div>
      <div className="tyre-tread-grid">
        <div className="tyre-tread-slot" style={{ color: getTempColor(data.inner) }}>
          <span className="tyre-slot-label">I</span>
          <span className="tyre-slot-val">{formatTemperature(data.inner)}</span>
        </div>
        <div className="tyre-tread-slot" style={{ color: getTempColor(data.mid) }}>
          <span className="tyre-slot-label">M</span>
          <span className="tyre-slot-val">{formatTemperature(data.mid)}</span>
        </div>
        <div className="tyre-tread-slot" style={{ color: getTempColor(data.outer) }}>
          <span className="tyre-slot-label">O</span>
          <span className="tyre-slot-val">{formatTemperature(data.outer)}</span>
        </div>
      </div>
      <div className="tyre-footer-row">
        <span className="card-row-label">Carcass: {formatTemperature(data.carcass, { withUnit: true })}</span>
        <span className="card-row-label">
          Wear (raw): {data.wear != null ? data.wear.toFixed(2) : NO_VALUE}
        </span>
      </div>
    </div>
  );
}

export function TyrePanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <div className="card" data-testid="panel-tyres">
        <div className="card-header">
          <span className="card-header-title">TYRE CORNERS (4-WHEEL)</span>
        </div>
        <div className="card-body">
          <span className="card-row-label">No telemetry store active</span>
        </div>
      </div>
    );
  }
  return <TyrePanelContent />;
}

function TyrePanelContent(): JSX.Element {
  const dq = useScopeDq();
  const samples = useChannelValues(TYRE_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < TYRE_CHANNELS.length; i++) {
      const id = TYRE_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  const fl: CornerData = {
    inner: num(ch.TYRE_TEMP_INNER_FL),
    mid: num(ch.TYRE_TEMP_MIDDLE_FL),
    outer: num(ch.TYRE_TEMP_OUTER_FL),
    carcass: num(ch.TYRE_CARCASS_TEMP_FL),
    pressure: num(ch.TYRE_PRESSURE_FL),
    wear: num(ch.TYRE_WEAR_FL),
  };

  const fr: CornerData = {
    inner: num(ch.TYRE_TEMP_INNER_FR),
    mid: num(ch.TYRE_TEMP_MIDDLE_FR),
    outer: num(ch.TYRE_TEMP_OUTER_FR),
    carcass: num(ch.TYRE_CARCASS_TEMP_FR),
    pressure: num(ch.TYRE_PRESSURE_FR),
    wear: num(ch.TYRE_WEAR_FR),
  };

  const rl: CornerData = {
    inner: num(ch.TYRE_TEMP_INNER_RL),
    mid: num(ch.TYRE_TEMP_MIDDLE_RL),
    outer: num(ch.TYRE_TEMP_OUTER_RL),
    carcass: num(ch.TYRE_CARCASS_TEMP_RL),
    pressure: num(ch.TYRE_PRESSURE_RL),
    wear: num(ch.TYRE_WEAR_RL),
  };

  const rr: CornerData = {
    inner: num(ch.TYRE_TEMP_INNER_RR),
    mid: num(ch.TYRE_TEMP_MIDDLE_RR),
    outer: num(ch.TYRE_TEMP_OUTER_RR),
    carcass: num(ch.TYRE_CARCASS_TEMP_RR),
    pressure: num(ch.TYRE_PRESSURE_RR),
    wear: num(ch.TYRE_WEAR_RR),
  };

  const balFR = num(ch.TYRE_TEMP_BALANCE_FRONT_REAR);
  const balLR = num(ch.TYRE_TEMP_BALANCE_LEFT_RIGHT);

  return (
    <div className="card" data-testid="panel-tyres" data-dq={dq ?? undefined}>
      <div className="card-header">
        <span className="card-header-title">TYRE CORNERS (4-WHEEL)</span>
        <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
          {dq === 'stale' && (
            <span className="status-badge status-badge-stale">STALE</span>
          )}
          {dq === 'untrusted' && (
            <span className="status-badge status-badge-error">UNTRUSTED</span>
          )}
          {(balFR != null || balLR != null) && (
            <div style={{ display: 'flex', gap: '8px' }}>
              {balFR != null && (
                <span className="status-badge status-badge-neutral">
                  F/R {balFR > 0 ? '+' : ''}{balFR.toFixed(1)}°C
                </span>
              )}
              {balLR != null && (
                <span className="status-badge status-badge-neutral">
                  L/R {balLR > 0 ? '+' : ''}{balLR.toFixed(1)}°C
                </span>
              )}
            </div>
          )}
        </div>
      </div>
      <div className="card-body">
        {dq === 'untrusted' && (
          <div className="panel-untrusted-note">Untrusted — data format mismatch</div>
        )}
        <div className="tyres-grid-2x2">
          {renderCorner('FRONT LEFT', fl, 'tyre-fl')}
          {renderCorner('FRONT RIGHT', fr, 'tyre-fr')}
          {renderCorner('REAR LEFT', rl, 'tyre-rl')}
          {renderCorner('REAR RIGHT', rr, 'tyre-rr')}
        </div>
      </div>
    </div>
  );
}
