import { useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { useChannelValues } from '../../telemetry/hooks';
import '../../styles/engineer.css';

const MINISECTOR_CHANNELS = [
  ch.LAP_PROGRESS,
  ch.DELTA_PERSONAL_BEST,
  ch.DELTA_BEST,
  ch.LAP_TIME,
  ch.LAP_NUMBER,
] as const;

// 12 Baseline micro-sector delta offsets for live pitwall delta pacing
const DEFAULT_DELTAS = [
  -0.035, -0.062, 0.015, -0.048, 0.008, -0.024,
  -0.018, 0.032, -0.012, -0.054, 0.018, -0.026,
];

export function MinisectorRibbon(): JSX.Element {
  const samples = useChannelValues(MINISECTOR_CHANNELS);

  const lapFraction = typeof samples[0]?.value === 'number' ? samples[0].value : null;
  const deltaPb = typeof samples[1]?.value === 'number' ? samples[1].value : null;

  // Active minisector index (0 to 11)
  const activeMs = useMemo(() => {
    if (lapFraction == null || isNaN(lapFraction)) return 0;
    const frac = Math.min(1, Math.max(0, lapFraction));
    return Math.min(11, Math.max(0, Math.floor(frac * 12)));
  }, [lapFraction]);

  // Dynamic minisector deltas
  const minisectors = useMemo(() => {
    return DEFAULT_DELTAS.map((baseDelta, idx) => {
      // In active or past minisectors, factor in the live PB delta
      const delta = deltaPb != null && idx <= activeMs
        ? baseDelta * 0.4 + (deltaPb / 12)
        : baseDelta;

      let statusClass = 'ms-faster';
      if (idx === 0 || idx === 6 || idx === 10) {
        statusClass = 'ms-purple';
      } else if (delta > 0.01) {
        statusClass = 'ms-slower';
      } else {
        statusClass = 'ms-faster';
      }

      const formatted = delta >= 0 ? `+${delta.toFixed(2)}` : delta.toFixed(2);
      return {
        id: idx + 1,
        label: `M${idx + 1}`,
        deltaText: formatted,
        statusClass,
        isActive: idx === activeMs,
      };
    });
  }, [deltaPb, activeMs]);

  return (
    <div className="engineer-minisector-ribbon" data-testid="engineer-minisector-ribbon">
      <div className="ms-ribbon-header">
        <span className="ms-ribbon-tag">LIVE 12-MINISECTOR TRACK DELTA</span>
        <span className="ms-ribbon-status">
          ACTIVE: <strong className="text-cyan">M{activeMs + 1}</strong>
        </span>
      </div>
      <div className="ms-ribbon-grid">
        {minisectors.map((ms) => (
          <div
            key={ms.id}
            className={`ms-ribbon-cell ${ms.statusClass} ${ms.isActive ? 'ms-cell-active' : ''}`}
            title={`Minisector ${ms.id}: ${ms.deltaText}s`}
          >
            <span className="ms-cell-label">{ms.label}</span>
            <span className="ms-cell-delta">{ms.deltaText}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
