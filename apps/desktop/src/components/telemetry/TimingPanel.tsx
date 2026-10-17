import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatDelta, formatLapTime, NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';

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
      <div className="card" data-testid="panel-timing">
        <div className="card-header">
          <span className="card-header-title">LAP TIMING & POSITION</span>
        </div>
        <div className="card-body">
          <span className="card-row-label">No telemetry store active</span>
        </div>
      </div>
    );
  }
  return <TimingPanelContent />;
}

function TimingPanelContent(): JSX.Element {
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
  const classPos = num(ch.CLASS_POSITION);
  const overallPos = num(ch.POSITION);

  const deltaStyle = useMemo(() => {
    if (deltaPb == null || isNaN(deltaPb)) {
      return { text: NO_VALUE, className: 'delta-chip delta-neutral' };
    }
    const text = formatDelta(deltaPb);
    if (deltaPb < -0.005) {
      return { text, className: 'delta-chip delta-ahead' };
    }
    if (deltaPb > 0.005) {
      return { text, className: 'delta-chip delta-behind' };
    }
    return { text, className: 'delta-chip delta-neutral' };
  }, [deltaPb]);

  return (
    <div className="card" data-testid="panel-timing">
      <div className="card-header">
        <span className="card-header-title">LAP TIMING & POSITION</span>
        <div style={{ display: 'flex', gap: '8px' }}>
          <span className="status-badge status-badge-info">
            P{overallPos != null ? Math.round(overallPos) : NO_VALUE} (CP
            {classPos != null ? Math.round(classPos) : NO_VALUE})
          </span>
          <span className="status-badge status-badge-neutral">
            L{lapNum != null ? Math.round(lapNum) : NO_VALUE} / S
            {sector != null ? Math.round(sector) : NO_VALUE}
          </span>
        </div>
      </div>
      <div className="card-body">
        <div className="delta-hero-container" style={{ margin: '4px 0 12px 0' }}>
          <div className="delta-hero-label">DELTA TO PERSONAL BEST</div>
          <div className={`delta-hero-value ${deltaStyle.className}`} data-testid="timing-delta-pb">
            {deltaStyle.text}
          </div>
        </div>

        <div className="telemetry-grid-2x2">
          <div className="card-row">
            <span className="card-row-label">Current Lap</span>
            <span className="card-row-value">{formatLapTime(lapTime)}</span>
          </div>
          <div className="card-row">
            <span className="card-row-label">Predicted</span>
            <span className="card-row-value">{formatLapTime(predicted)}</span>
          </div>
          <div className="card-row">
            <span className="card-row-label">Last Lap</span>
            <span className="card-row-value">{formatLapTime(lastLap)}</span>
          </div>
          <div className="card-row">
            <span className="card-row-label">Best Lap</span>
            <span className="card-row-value" style={{ color: 'var(--color-ahead)' }}>
              {formatLapTime(bestLap)}
            </span>
          </div>
        </div>

        <div className="sectors-bar" style={{ marginTop: '12px' }}>
          <div className="card-row-label" style={{ marginBottom: '4px' }}>
            LAST LAP SECTOR SPLITS
          </div>
          <div style={{ display: 'flex', gap: '8px' }}>
            <div className="meta-chip" style={{ flex: 1 }}>
              <span className="meta-chip-label">S1</span>
              <span className="meta-chip-value">{formatLapTime(lastS1)}</span>
            </div>
            <div className="meta-chip" style={{ flex: 1 }}>
              <span className="meta-chip-label">S2</span>
              <span className="meta-chip-value">{formatLapTime(lastS2)}</span>
            </div>
            <div className="meta-chip" style={{ flex: 1 }}>
              <span className="meta-chip-label">S3</span>
              <span className="meta-chip-value">{formatLapTime(lastS3)}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
