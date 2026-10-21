import type { JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatLapTime } from '../../lib/units';
import { useLiveReadings } from '../liveData';
import { LiveCell, NO_FIELD_DATA_REASON, NO_HISTORY_REASON, ViewNoData } from './ViewNoData';

const BEST_CHANNELS: readonly number[] = [
  ch.BEST_LAP_TIME,
  ch.LAST_LAP_TIME,
  ch.BEST_SECTOR1,
  ch.BEST_SECTOR2,
  ch.LAST_SECTOR1,
  ch.LAST_SECTOR2,
  ch.LAST_SECTOR3,
];

/**
 * Pace statistics. Best / last lap and sector times are live scoring values; mean, median, spread and
 * theoretical optimum need lap history the app does not record, so they show an explicit no-data state.
 */
export function StatisticsTableView(): JSX.Element {
  const r = useLiveReadings(BEST_CHANNELS);
  return (
    <div className="statistics-table-view" data-testid="statistics-table-view">
      <div className="stat-header">
        <h2 className="stat-title">Session & Stint Statistics Table</h2>
      </div>

      <div className="view-live-grid" data-testid="statistics-live">
        <LiveCell label="BEST LAP" value={formatLapTime(r.num(ch.BEST_LAP_TIME))} />
        <LiveCell label="LAST LAP" value={formatLapTime(r.num(ch.LAST_LAP_TIME))} />
        <LiveCell label="BEST S1" value={formatLapTime(r.num(ch.BEST_SECTOR1))} />
        <LiveCell label="BEST S2 (approx.)" value={formatLapTime(r.num(ch.BEST_SECTOR2))} />
        <LiveCell label="LAST S1" value={formatLapTime(r.num(ch.LAST_SECTOR1))} />
        <LiveCell label="LAST S2" value={formatLapTime(r.num(ch.LAST_SECTOR2))} />
        <LiveCell label="LAST S3" value={formatLapTime(r.num(ch.LAST_SECTOR3))} />
      </div>

      <div className="view-live-section">
        <span className="view-live-section-title">DISTRIBUTION STATISTICS</span>
        <ViewNoData title="Mean / median / spread" reason={NO_HISTORY_REASON} testId="statistics-no-data" />
      </div>
    </div>
  );
}

/** Lap-time trend chart: needs lap history the app does not record, so it shows an explicit no-data state. */
export function StatisticsTableGraphsView(): JSX.Element {
  return (
    <div className="statistics-graphs-view" data-testid="statistics-table-graphs-view">
      <div className="stat-header">
        <h2 className="stat-title">Statistics Table Graphs (Lap-Time Trend)</h2>
      </div>
      <ViewNoData title="Lap-time trend" reason={NO_HISTORY_REASON} testId="statistics-graphs-no-data" />
    </div>
  );
}

/** Head-to-head benchmark vs the field: needs per-car sector history, so it shows an explicit no-data state. */
export function StatisticsRedBlueView(): JSX.Element {
  return (
    <div className="statistics-red-blue-view" data-testid="statistics-red-blue-view">
      <div className="stat-header">
        <h2 className="stat-title">Statistics: RED, BLUE &amp; SES Benchmark</h2>
      </div>
      <ViewNoData title="Field benchmark" reason={NO_FIELD_DATA_REASON} testId="statistics-red-blue-no-data" />
    </div>
  );
}
