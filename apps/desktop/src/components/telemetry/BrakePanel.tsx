import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatTemperature, NO_VALUE } from '../../lib/units';
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

interface BrakeCorner {
  readonly code: string;
  readonly name: string;
  readonly temp: number | null;
  readonly peak: number | null;
  readonly wear: number | null;
  readonly thickness: number | null;
}

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

  const corners: readonly BrakeCorner[] = [
    { code: 'FL', name: 'Front left', temp: tFL, peak: pFL, wear: wFL, thickness: thFL },
    { code: 'FR', name: 'Front right', temp: tFR, peak: pFR, wear: wFR, thickness: thFR },
    { code: 'RL', name: 'Rear left', temp: tRL, peak: pRL, wear: wRL, thickness: thRL },
    { code: 'RR', name: 'Rear right', temp: tRR, peak: pRR, wear: wRR, thickness: thRR },
  ];

  return (
    <Panel title="BRAKES" dq={dq} meta={metaContent} testId="panel-brakes">
      {/* v1.3 V06: one line per corner, "FL 352 °C · peak 754 · wear —" (code only, full name in title/aria) */}
      <div className="brakes-list">
        {corners.map((c) => (
          <div
            key={c.code}
            className="brake-row"
            role="group"
            aria-label={`${c.name} brake`}
            data-testid={`brake-${c.code.toLowerCase()}`}
          >
            <span className="brake-row-code" title={c.name}>
              {c.code}
            </span>
            <span className="brake-row-temp">{formatTemperature(c.temp, { withUnit: true })}</span>
            <span className="brake-row-kv">
              <span className="brake-row-lbl">· peak</span>{' '}
              <span className="brake-row-num">{formatTemperature(c.peak)}</span>
            </span>
            <span
              className="brake-row-kv"
              data-testid={`brake-wear-${c.code.toLowerCase()}`}
              title={c.thickness != null ? `Disc thickness ${(c.thickness * 1000).toFixed(1)} mm` : undefined}
            >
              <span className="brake-row-lbl">wear</span>{' '}
              <span className="brake-row-num">{c.wear != null ? `${(c.wear * 100).toFixed(1)}%` : NO_VALUE}</span>
            </span>
          </div>
        ))}
      </div>

      {tempDiffFR != null && (
        <div className="brake-bias-row">
          <span className="card-row-label">Front–rear temp delta</span>
          <span className="card-row-value brake-row-num">
            {tempDiffFR >= 0 ? `+${tempDiffFR.toFixed(0)} °C` : `${tempDiffFR.toFixed(0)} °C`}
          </span>
        </div>
      )}
    </Panel>
  );
}
