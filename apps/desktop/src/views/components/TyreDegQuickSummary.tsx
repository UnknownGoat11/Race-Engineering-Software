import type { JSX } from 'react';
import { ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { useLiveReadings } from '../liveData';
import { ViewNoData } from './ViewNoData';

export interface TyreDegQuickSummaryProps {
  readonly onOpenFullView?: (() => void) | undefined;
  readonly compact?: boolean | undefined;
}

const WEAR_CHANNELS = [
  { pos: 'FL', id: ch.TYRE_WEAR_FL },
  { pos: 'FR', id: ch.TYRE_WEAR_FR },
  { pos: 'RL', id: ch.TYRE_WEAR_RL },
  { pos: 'RR', id: ch.TYRE_WEAR_RR },
] as const;

const SUMMARY_CHANNELS: readonly number[] = [...WEAR_CHANNELS.map((w) => w.id), ch.TYRE_SET_LAPS, ch.LAP_NUMBER];

/**
 * Current tyre wear per wheel (raw LMU value, as the Tyre panel shows it) and laps on the current set.
 * Degradation rate, cliff projection and lock-up detection need history the app does not record, so they show
 * an explicit no-data state.
 */
export function TyreDegQuickSummary({ onOpenFullView, compact = false }: TyreDegQuickSummaryProps): JSX.Element {
  const r = useLiveReadings(SUMMARY_CHANNELS);
  const setLaps = r.num(ch.TYRE_SET_LAPS);
  const lap = r.num(ch.LAP_NUMBER);

  return (
    <div className={`tyre-deg-quick-card ${compact ? 'deg-compact' : ''}`} data-testid="tyre-deg-quick-summary">
      <div className="tyre-deg-header">
        <div className="deg-title-wrap">
          <span className="deg-glyph">🛞</span>
          <span className="deg-title">TYRE WEAR (LIVE)</span>
        </div>
        <div className="deg-header-right">
          {onOpenFullView && (
            <button
              type="button"
              className="btn btn-xs btn-outline"
              onClick={onOpenFullView}
              title="Open full Tyre Degradation view"
            >
              FULL VIEW ↗
            </button>
          )}
        </div>
      </div>

      <div className="deg-wheels-grid">
        {WEAR_CHANNELS.map(({ pos, id }) => {
          const wear = r.num(id);
          return (
            <div key={pos} className={`deg-wheel-box deg-wheel-${pos.toLowerCase()}`}>
              <div className="deg-wheel-row">
                <span className="deg-wheel-pos">{pos}</span>
                <span className="deg-wheel-wear num-cyan" data-testid={`deg-wear-${pos.toLowerCase()}`}>
                  Wear (raw): {wear !== null ? wear.toFixed(2) : NO_VALUE}
                </span>
              </div>
            </div>
          );
        })}
      </div>

      <div className="deg-stint-summary-bar">
        <div className="stint-stat-item">
          <span className="stint-stat-label">LAPS ON SET:</span>
          <span className="stint-stat-val num-cyan" data-testid="deg-set-laps">
            {setLaps !== null ? Math.round(setLaps).toString() : NO_VALUE}
          </span>
        </div>
        <div className="stint-stat-item">
          <span className="stint-stat-label">CURRENT LAP:</span>
          <span className="stint-stat-val" data-testid="deg-current-lap">
            {lap !== null ? Math.round(lap).toString() : NO_VALUE}
          </span>
        </div>
      </div>

      <ViewNoData
        title="Degradation rate & cliff projection"
        reason="Needs per-lap wear history, which the app does not record yet."
        testId="deg-rate-no-data"
      />
    </div>
  );
}
