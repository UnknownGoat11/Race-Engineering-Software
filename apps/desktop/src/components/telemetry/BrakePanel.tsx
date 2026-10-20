import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatTemperature } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { Panel } from '../ui/Panel';
import { StatusBadge } from '../ui/StatusBadge';
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
  ch.BRAKE_WEAR_FL,
  ch.BRAKE_WEAR_FR,
  ch.BRAKE_WEAR_RL,
  ch.BRAKE_WEAR_RR,
  ch.BRAKE_THICKNESS_FL,
  ch.BRAKE_THICKNESS_FR,
  ch.BRAKE_THICKNESS_RL,
  ch.BRAKE_THICKNESS_RR,
  ch.BRAKE_CORE_TEMP_FL,
  ch.BRAKE_CORE_TEMP_FR,
  ch.BRAKE_CORE_TEMP_RL,
  ch.BRAKE_CORE_TEMP_RR,
] as const;

export function BrakePanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <Panel title="BRAKES" testId="panel-brakes">
        <span className="card-row-label">No telemetry store active</span>
      </Panel>
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

  const wFL = num(ch.BRAKE_WEAR_FL);
  const wFR = num(ch.BRAKE_WEAR_FR);
  const wRL = num(ch.BRAKE_WEAR_RL);
  const wRR = num(ch.BRAKE_WEAR_RR);

  const thFL = num(ch.BRAKE_THICKNESS_FL);
  const thFR = num(ch.BRAKE_THICKNESS_FR);
  const thRL = num(ch.BRAKE_THICKNESS_RL);
  const thRR = num(ch.BRAKE_THICKNESS_RR);

  const biasFront = num(ch.BRAKE_BIAS_FRONT);
  const tempDiffFR = num(ch.BRAKE_TEMP_FRONT_REAR);

  const metaContent = (
    <>
      {biasFront != null && (
        <StatusBadge level="info" glyph={false}>
          BIAS {(biasFront * 100).toFixed(1)}% F
        </StatusBadge>
      )}
      <StatusBadge level="off" glyph={false}>
        No threshold set
      </StatusBadge>
    </>
  );

  return (
    <Panel title="BRAKES" dq={dq} meta={metaContent} testId="panel-brakes">
      <div className="brakes-grid-2x2">
        {/* FL */}
        <div className="brake-corner-box">
          <div className="brake-corner-header">
            <span className="brake-corner-title">FL · FRONT LEFT</span>
            <span className="brake-peak-label">Peak: {formatTemperature(pFL)}</span>
          </div>
          <div className="brake-temp-value">
            {formatTemperature(tFL, { withUnit: true })}
          </div>
          <div className="brake-sub-metrics">
            <span className="brake-sub-wear">
              Wear: {wFL != null ? `${(wFL * 100).toFixed(1)}%` : '0.0%'}
            </span>
            {thFL != null && (
              <span className="brake-sub-thick">{(thFL * 1000).toFixed(1)} mm</span>
            )}
          </div>
        </div>

        {/* FR */}
        <div className="brake-corner-box">
          <div className="brake-corner-header">
            <span className="brake-corner-title">FR · FRONT RIGHT</span>
            <span className="brake-peak-label">Peak: {formatTemperature(pFR)}</span>
          </div>
          <div className="brake-temp-value">
            {formatTemperature(tFR, { withUnit: true })}
          </div>
          <div className="brake-sub-metrics">
            <span className="brake-sub-wear">
              Wear: {wFR != null ? `${(wFR * 100).toFixed(1)}%` : '0.0%'}
            </span>
            {thFR != null && (
              <span className="brake-sub-thick">{(thFR * 1000).toFixed(1)} mm</span>
            )}
          </div>
        </div>

        {/* RL */}
        <div className="brake-corner-box">
          <div className="brake-corner-header">
            <span className="brake-corner-title">RL · REAR LEFT</span>
            <span className="brake-peak-label">Peak: {formatTemperature(pRL)}</span>
          </div>
          <div className="brake-temp-value">
            {formatTemperature(tRL, { withUnit: true })}
          </div>
          <div className="brake-sub-metrics">
            <span className="brake-sub-wear">
              Wear: {wRL != null ? `${(wRL * 100).toFixed(1)}%` : '0.0%'}
            </span>
            {thRL != null && (
              <span className="brake-sub-thick">{(thRL * 1000).toFixed(1)} mm</span>
            )}
          </div>
        </div>

        {/* RR */}
        <div className="brake-corner-box">
          <div className="brake-corner-header">
            <span className="brake-corner-title">RR · REAR RIGHT</span>
            <span className="brake-peak-label">Peak: {formatTemperature(pRR)}</span>
          </div>
          <div className="brake-temp-value">
            {formatTemperature(tRR, { withUnit: true })}
          </div>
          <div className="brake-sub-metrics">
            <span className="brake-sub-wear">
              Wear: {wRR != null ? `${(wRR * 100).toFixed(1)}%` : '0.0%'}
            </span>
            {thRR != null && (
              <span className="brake-sub-thick">{(thRR * 1000).toFixed(1)} mm</span>
            )}
          </div>
        </div>
      </div>

      {tempDiffFR != null && (
        <div className="brake-bias-row">
          <span className="card-row-label">Front - Rear Temp Delta</span>
          <span className="card-row-value">
            {tempDiffFR >= 0 ? `+${tempDiffFR.toFixed(0)}°C` : `${tempDiffFR.toFixed(0)}°C`}
          </span>
        </div>
      )}
    </Panel>
  );
}
