import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatTemperature } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { useScopeDq } from './presentation';
import './telemetry.css';

const BRAKE_CHANNELS = [
  ch.BRAKE_TEMP_FL,
  ch.BRAKE_TEMP_FR,
  ch.BRAKE_TEMP_RL,
  ch.BRAKE_TEMP_RR,
  ch.BRAKE_TEMP_PEAK_FL,
  ch.BRAKE_TEMP_PEAK_FR,
  ch.BRAKE_TEMP_PEAK_RL,
  ch.BRAKE_TEMP_PEAK_RR,
  ch.BRAKE_BIAS_FRONT,
  ch.BRAKE_BIAS_REAR,
  ch.BRAKE_TEMP_FRONT_REAR,
] as const;

export function BrakePanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <div className="card" data-testid="panel-brakes">
        <div className="card-header">
          <span className="card-header-title">BRAKE SYSTEM</span>
        </div>
        <div className="card-body">
          <span className="card-row-label">No telemetry store active</span>
        </div>
      </div>
    );
  }
  return <BrakePanelContent />;
}

function BrakePanelContent(): JSX.Element {
  const dq = useScopeDq();
  const samples = useChannelValues(BRAKE_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < BRAKE_CHANNELS.length; i++) {
      const id = BRAKE_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  const tFL = num(ch.BRAKE_TEMP_FL);
  const tFR = num(ch.BRAKE_TEMP_FR);
  const tRL = num(ch.BRAKE_TEMP_RL);
  const tRR = num(ch.BRAKE_TEMP_RR);

  const pFL = num(ch.BRAKE_TEMP_PEAK_FL);
  const pFR = num(ch.BRAKE_TEMP_PEAK_FR);
  const pRL = num(ch.BRAKE_TEMP_PEAK_RL);
  const pRR = num(ch.BRAKE_TEMP_PEAK_RR);

  const biasFront = num(ch.BRAKE_BIAS_FRONT);
  const tempDiffFR = num(ch.BRAKE_TEMP_FRONT_REAR);

  return (
    <div className="card" data-testid="panel-brakes" data-dq={dq ?? undefined}>
      <div className="card-header">
        <span className="card-header-title">BRAKE SYSTEM</span>
        <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
          {dq === 'stale' && (
            <span className="status-badge status-badge-stale">STALE</span>
          )}
          {dq === 'untrusted' && (
            <span className="status-badge status-badge-error">UNTRUSTED</span>
          )}
          {biasFront != null && (
            <span className="status-badge status-badge-info">
              BIAS {(biasFront * 100).toFixed(1)}% F
            </span>
          )}
          <span className="status-badge status-badge-neutral">
            No threshold set
          </span>
        </div>
      </div>
      <div className="card-body">
        {dq === 'untrusted' && (
          <div className="panel-untrusted-note">Untrusted — data format mismatch</div>
        )}

        <div className="brakes-grid-2x2">
          {/* FL */}
          <div className="brake-corner-box">
            <div className="brake-corner-header">
              <span className="brake-corner-title">FRONT LEFT</span>
              <span className="brake-peak-label">Peak: {formatTemperature(pFL)}</span>
            </div>
            <div className="brake-temp-value">
              {formatTemperature(tFL, { withUnit: true })}
            </div>
          </div>

          {/* FR */}
          <div className="brake-corner-box">
            <div className="brake-corner-header">
              <span className="brake-corner-title">FRONT RIGHT</span>
              <span className="brake-peak-label">Peak: {formatTemperature(pFR)}</span>
            </div>
            <div className="brake-temp-value">
              {formatTemperature(tFR, { withUnit: true })}
            </div>
          </div>

          {/* RL */}
          <div className="brake-corner-box">
            <div className="brake-corner-header">
              <span className="brake-corner-title">REAR LEFT</span>
              <span className="brake-peak-label">Peak: {formatTemperature(pRL)}</span>
            </div>
            <div className="brake-temp-value">
              {formatTemperature(tRL, { withUnit: true })}
            </div>
          </div>

          {/* RR */}
          <div className="brake-corner-box">
            <div className="brake-corner-header">
              <span className="brake-corner-title">REAR RIGHT</span>
              <span className="brake-peak-label">Peak: {formatTemperature(pRR)}</span>
            </div>
            <div className="brake-temp-value">
              {formatTemperature(tRR, { withUnit: true })}
            </div>
          </div>
        </div>

        {tempDiffFR != null && (
          <div className="card-row" style={{ marginTop: '8px', borderTop: '1px solid var(--color-border)', paddingTop: '6px' }}>
            <span className="card-row-label">Front - Rear Temp Delta</span>
            <span className="card-row-value">
              {tempDiffFR >= 0 ? `+${tempDiffFR.toFixed(0)}°C` : `${tempDiffFR.toFixed(0)}°C`}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
