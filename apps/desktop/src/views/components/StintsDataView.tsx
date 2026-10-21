import type { JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatClock, formatFuelPerLap, NO_VALUE } from '../../lib/units';
import { useLiveReadings } from '../liveData';
import { LiveCell, NO_HISTORY_REASON, NOT_CONNECTED_REASON, ViewNoData } from './ViewNoData';

const STINT_CHANNELS: readonly number[] = [
  ch.STINT_NUMBER,
  ch.STINT_LAPS,
  ch.STINT_TIME,
  ch.TYRE_SET_LAPS,
  ch.PIT_STOPS,
  ch.FUEL_PER_LAP,
  ch.TYRE_COMPOUND_FRONT,
  ch.TYRE_COMPOUND_REAR,
];

function count(value: number | null): string {
  return value === null ? NO_VALUE : Math.round(value).toString();
}

/** Current stint from live calculated channels; the ledger of past stints needs history the app does not keep. */
export function StintsDataView(): JSX.Element {
  const r = useLiveReadings(STINT_CHANNELS);
  const stintTime = r.num(ch.STINT_TIME);

  return (
    <div className="stints-data-view" data-testid="stints-data-view">
      <div className="stints-header">
        <h2 className="stints-title">Stints & Tyre Wear Ledger</h2>
        <p className="stints-desc">Current stint from live telemetry. Past stints are not recorded yet.</p>
      </div>

      {!r.connected && <ViewNoData title="Current stint" reason={NOT_CONNECTED_REASON} testId="stints-not-connected" />}

      <div className="view-live-grid" data-testid="stints-current">
        <LiveCell label="STINT" value={count(r.num(ch.STINT_NUMBER))} />
        <LiveCell label="LAPS IN STINT" value={count(r.num(ch.STINT_LAPS))} />
        <LiveCell label="TIME IN STINT" value={stintTime !== null ? formatClock(stintTime) : NO_VALUE} />
        <LiveCell label="LAPS ON TYRE SET" value={count(r.num(ch.TYRE_SET_LAPS))} />
        <LiveCell label="PIT STOPS" value={count(r.num(ch.PIT_STOPS))} />
        <LiveCell label="FUEL / LAP (AVG)" value={formatFuelPerLap(r.num(ch.FUEL_PER_LAP), { withUnit: true })} />
        <LiveCell label="COMPOUND F / R" value={`${r.text(ch.TYRE_COMPOUND_FRONT) ?? NO_VALUE} / ${r.text(ch.TYRE_COMPOUND_REAR) ?? NO_VALUE}`} />
      </div>

      <div className="view-live-section">
        <span className="view-live-section-title">STINT LEDGER</span>
        <ViewNoData title="Past stints" reason={NO_HISTORY_REASON} testId="stints-ledger-no-data" />
      </div>
    </div>
  );
}
