import type { JSX } from 'react';
import { CHANNEL_BY_ID, QUALITY_NAMES, VALUE_TYPE_NAMES, ch } from '../../generated/channels';
import { formatCount, NO_VALUE } from '../../lib/units';
import { formatReading, useLiveReadings, useOptionalTelemetryStatus } from '../liveData';
import { NOT_CONNECTED_REASON, ViewNoData } from './ViewNoData';

/** Channels listed by the inspector (registry ids; constant names and types come from the generated table). */
const INSPECTED_CHANNELS: readonly number[] = [
  ch.SPEED,
  ch.THROTTLE,
  ch.BRAKE,
  ch.CLUTCH,
  ch.STEERING_ANGLE,
  ch.GEAR,
  ch.RPM,
  ch.G_LAT,
  ch.G_LONG,
  ch.TYRE_TEMP_MIDDLE_FL,
  ch.TYRE_TEMP_MIDDLE_FR,
  ch.TYRE_TEMP_MIDDLE_RL,
  ch.TYRE_TEMP_MIDDLE_RR,
  ch.BRAKE_TEMP_FL,
  ch.BRAKE_TEMP_PEAK_FL,
  ch.FUEL,
  ch.FUEL_PER_LAP,
  ch.BATTERY_SOC,
  ch.VIRTUAL_ENERGY,
  ch.SRC_TELEMETRY_RATE,
];

function hexId(id: number): string {
  return `0x${id.toString(16).toUpperCase().padStart(4, '0')}`;
}

/** Live channel inspector: real values, sample states and wire qualities from the TelemetryStore. */
export function RawDataInspectorView(): JSX.Element {
  const r = useLiveReadings(INSPECTED_CHANNELS);
  const status = useOptionalTelemetryStatus();

  return (
    <div className="raw-data-view" data-testid="raw-data-view">
      <div className="raw-header">
        <h2 className="raw-title">~ Raw Data ~ (Channel Inspector)</h2>
        <p className="raw-desc">
          Current decoded value, sample state and wire quality of selected registry channels, plus store counters.
        </p>
      </div>

      {status === null ? (
        <ViewNoData title="Telemetry store" reason={NOT_CONNECTED_REASON} testId="raw-not-connected" />
      ) : (
        <div className="raw-stats-bar" data-testid="raw-stats">
          <div className="raw-stat">
            <span className="raw-stat-label">STORE STATE:</span> <strong>{status.state.toUpperCase()}</strong>
          </div>
          <div className="raw-stat">
            <span className="raw-stat-label">SOURCE TELEMETRY RATE:</span>{' '}
            <strong>{formatReading(ch.SRC_TELEMETRY_RATE, r.num(ch.SRC_TELEMETRY_RATE))}</strong>
          </div>
          <div className="raw-stat">
            <span className="raw-stat-label">BATCHES APPLIED:</span>{' '}
            <strong data-testid="raw-batches-applied">{formatCount(status.batchesApplied)}</strong>
          </div>
          <div className="raw-stat">
            <span className="raw-stat-label">BATCHES REJECTED:</span>{' '}
            <strong data-testid="raw-batches-rejected">{formatCount(status.batchesRejected)}</strong>
          </div>
          <div className="raw-stat">
            <span className="raw-stat-label">GAPS:</span> <strong>{formatCount(status.gaps)}</strong>
          </div>
          <div className="raw-stat">
            <span className="raw-stat-label">RESYNC NEEDED:</span>{' '}
            <strong>{status.resyncNeeded ? 'YES' : 'NO'}</strong>
          </div>
        </div>
      )}

      <div className="raw-table-card">
        <table className="raw-table">
          <thead>
            <tr>
              <th>Channel ID</th>
              <th>Constant Name</th>
              <th>Type</th>
              <th>Current Decoded Value</th>
              <th>State</th>
              <th>Wire Quality</th>
              <th>Nominal Class</th>
            </tr>
          </thead>
          <tbody>
            {INSPECTED_CHANNELS.map((id) => {
              const def = CHANNEL_BY_ID.get(id);
              const sample = r.sample(id);
              return (
                <tr key={id} data-testid={`raw-row-${hexId(id)}`}>
                  <td className="font-mono text-cyan">{hexId(id)}</td>
                  <td className="font-bold">{def?.constName ?? NO_VALUE}</td>
                  <td className="font-mono">{def ? VALUE_TYPE_NAMES[def.valueType] : NO_VALUE}</td>
                  <td className="font-mono text-white">{formatReading(id, sample?.value)}</td>
                  <td>{sample ? sample.state.toUpperCase() : NO_VALUE}</td>
                  <td className="font-mono">{sample?.quality != null ? QUALITY_NAMES[sample.quality] : NO_VALUE}</td>
                  <td className="font-mono">{def?.frequency ?? NO_VALUE}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
