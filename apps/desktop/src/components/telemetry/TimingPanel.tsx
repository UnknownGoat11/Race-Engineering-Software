import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatDelta, formatLapTime, NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { Panel } from '../ui/Panel';
import { StatusBadge } from '../ui/StatusBadge';
import { useScopeDq } from './presentation';
import './telemetry.css';

const TIMING_CHANNELS = [
  ch.LAP_NUMBER,
  ch.LAP_TIME,
  ch.LAST_LAP_TIME,
  ch.BEST_LAP_TIME,
  ch.DELTA_PERSONAL_BEST,
  ch.PREDICTED_LAP_TIME,
  ch.SECTOR,
  ch.CUR_SECTOR1,
  ch.LAST_SECTOR1,
  ch.LAST_SECTOR2,
  ch.LAST_SECTOR3,
  ch.BEST_SECTOR1,
  ch.BEST_SECTOR2,
  ch.CLASS_POSITION,
  ch.POSITION,
] as const;

export function TimingPanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <Panel title="TIMING" testId="panel-timing">
        <span className="card-row-label">No telemetry store active</span>
      </Panel>
    );
  }
  return <TimingPanelContent />;
}

function TimingPanelContent(): JSX.Element {
  const dq = useScopeDq();
  const samples = useChannelValues(TIMING_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < TIMING_CHANNELS.length; i++) {
      const id = TIMING_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  const lapNum = num(ch.LAP_NUMBER);
  const lapTime = num(ch.LAP_TIME);
  const lastLap = num(ch.LAST_LAP_TIME);
  const bestLap = num(ch.BEST_LAP_TIME);
  const deltaPb = num(ch.DELTA_PERSONAL_BEST);
  const predicted = num(ch.PREDICTED_LAP_TIME);
  const sector = num(ch.SECTOR);
  const lastS1 = num(ch.LAST_SECTOR1);
  const lastS2 = num(ch.LAST_SECTOR2);
  const lastS3 = num(ch.LAST_SECTOR3);
  const bestS1 = num(ch.BEST_SECTOR1);
  const bestS2 = num(ch.BEST_SECTOR2);
  const classPos = num(ch.CLASS_POSITION);
  const overallPos = num(ch.POSITION);

  // Delta hero per C12: deadband ±0.050 s, faster green with glyph, slower red with glyph
  const deltaPresentation = useMemo(() => {
    if (deltaPb == null || isNaN(deltaPb)) {
      return { text: NO_VALUE, cue: '', className: 'delta-chip delta-neutral' };
    }
    const valText = formatDelta(deltaPb);
    if (deltaPb < -0.050) {
      return { text: valText, cue: '▲ FASTER', className: 'delta-chip delta-ahead' };
    }
    if (deltaPb > 0.050) {
      return { text: valText, cue: '▼ SLOWER', className: 'delta-chip delta-behind' };
    }
    return { text: valText, cue: '', className: 'delta-chip delta-neutral' };
  }, [deltaPb]);

  const getSectorChipClass = (lastSec: number | null, bestSec: number | null): string => {
    if (lastSec == null || isNaN(lastSec) || bestSec == null || isNaN(bestSec)) {
      return 'meta-chip';
    }
    // Slower than personal best by more than 0.010 s
    if (lastSec > bestSec + 0.010) {
      return 'meta-chip meta-chip--slower';
    }
    // Matches or beats personal best
    if (lastSec <= bestSec + 0.010) {
      return 'meta-chip meta-chip--best-personal';
    }
    return 'meta-chip';
  };

  const metaContent = (
    <>
      <StatusBadge level="info" glyph={false}>
        P{overallPos != null ? Math.round(overallPos) : NO_VALUE} (CP
        {classPos != null ? Math.round(classPos) : NO_VALUE})
      </StatusBadge>
      <StatusBadge level="off" glyph={false}>
        L{lapNum != null ? Math.round(lapNum) : NO_VALUE} / S
        {sector != null ? Math.round(sector) + 1 : NO_VALUE}
      </StatusBadge>
    </>
  );

  return (
    <Panel title="TIMING" dq={dq} meta={metaContent} testId="panel-timing">
      <div className="delta-hero-container">
        <div className="delta-hero-label">DELTA TO PERSONAL BEST</div>
        <div className={`delta-hero-value ${deltaPresentation.className}`} data-testid="timing-delta-pb">
          <span>{deltaPresentation.text}</span>
          {deltaPresentation.cue && <span className="delta-cue">{deltaPresentation.cue}</span>}
        </div>
      </div>

      {/* v1.3 V07: one label/value pair per row, never wrapping in the 280 px column */}
      <dl className="tel-kv">
        <div className="tel-kv-row">
          <dt>Current lap</dt>
          <dd>{formatLapTime(lapTime)}</dd>
        </div>
        <div className="tel-kv-row">
          <dt>Predicted</dt>
          <dd>{formatLapTime(predicted)}</dd>
        </div>
        <div className="tel-kv-row">
          <dt>Last lap</dt>
          <dd>{formatLapTime(lastLap)}</dd>
        </div>
        <div className="tel-kv-row">
          <dt>Best lap</dt>
          <dd className="timing-best-val">{formatLapTime(bestLap)}</dd>
        </div>
      </dl>

      <div className="sectors-bar">
        <div className="tel-kv-caption">Last lap sector splits</div>
        <div className="sectors-grid">
          <div className={getSectorChipClass(lastS1, bestS1)}>
            <span className="meta-chip-label">S1</span>
            <span className="meta-chip-value">{formatLapTime(lastS1)}</span>
          </div>
          <div className={getSectorChipClass(lastS2, bestS2)}>
            <span className="meta-chip-label">S2</span>
            <span className="meta-chip-value">{formatLapTime(lastS2)}</span>
          </div>
          <div className="meta-chip">
            <span className="meta-chip-label">S3</span>
            <span className="meta-chip-value">{formatLapTime(lastS3)}</span>
          </div>
        </div>
      </div>
    </Panel>
  );
}
