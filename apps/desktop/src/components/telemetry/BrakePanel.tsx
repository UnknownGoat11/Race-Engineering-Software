import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatTemperature } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';

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

function getBrakeColor(tempC: number | null | undefined): string {
  if (tempC == null || isNaN(tempC)) return 'var(--color-text-muted)';
  if (tempC < 300) return '#60a5fa'; // Blue (cold)
  if (tempC <= 600) return '#4ade80'; // Green (optimal)
  if (tempC <= 750) return '#fbbf24'; // Amber (hot)
  return '#f87171'; // Red (fading / overheated)
}

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

  const maxTemp = Math.max(tFL ?? 0, tFR ?? 0, tRL ?? 0, tRR ?? 0);
  const isOverheating = maxTemp > 750;

  return (
    <div className="card" data-testid="panel-brakes">
      <div className="card-header">
        <span className="card-header-title">BRAKE SYSTEM</span>
        {biasFront != null && (
          <span className="status-badge status-badge-info">
            BIAS {(biasFront * 100).toFixed(1)}% F
          </span>
        )}
      </div>
      <div className="card-body">
        {isOverheating && (
          <div
            className="status-badge status-badge-error"
            style={{ width: '100%', marginBottom: '8px', textAlign: 'center' }}
          >
            DISC TEMPERATURE CRITICAL (&gt; 750°C)
          </div>
        )}

        <div className="tyres-grid-2x2">
          {/* FL */}
          <div className="tyre-corner-box">
            <div className="tyre-corner-header">
              <span className="tyre-corner-title">FRONT LEFT</span>
              <span className="card-row-label">Peak: {formatTemperature(pFL)}</span>
            </div>
            <div style={{ fontSize: '20px', fontWeight: 700, fontFamily: 'monospace', color: getBrakeColor(tFL), textAlign: 'center', margin: '6px 0' }}>
              {formatTemperature(tFL, { withUnit: true })}
            </div>
          </div>

          {/* FR */}
          <div className="tyre-corner-box">
            <div className="tyre-corner-header">
              <span className="tyre-corner-title">FRONT RIGHT</span>
              <span className="card-row-label">Peak: {formatTemperature(pFR)}</span>
            </div>
            <div style={{ fontSize: '20px', fontWeight: 700, fontFamily: 'monospace', color: getBrakeColor(tFR), textAlign: 'center', margin: '6px 0' }}>
              {formatTemperature(tFR, { withUnit: true })}
            </div>
          </div>

          {/* RL */}
          <div className="tyre-corner-box">
            <div className="tyre-corner-header">
              <span className="tyre-corner-title">REAR LEFT</span>
              <span className="card-row-label">Peak: {formatTemperature(pRL)}</span>
            </div>
            <div style={{ fontSize: '20px', fontWeight: 700, fontFamily: 'monospace', color: getBrakeColor(tRL), textAlign: 'center', margin: '6px 0' }}>
              {formatTemperature(tRL, { withUnit: true })}
            </div>
          </div>

          {/* RR */}
          <div className="tyre-corner-box">
            <div className="tyre-corner-header">
              <span className="tyre-corner-title">REAR RIGHT</span>
              <span className="card-row-label">Peak: {formatTemperature(pRR)}</span>
            </div>
            <div style={{ fontSize: '20px', fontWeight: 700, fontFamily: 'monospace', color: getBrakeColor(tRR), textAlign: 'center', margin: '6px 0' }}>
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
