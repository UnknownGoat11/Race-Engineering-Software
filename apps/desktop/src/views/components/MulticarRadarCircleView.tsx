import type { JSX } from 'react';
import { ch } from '../../generated/channels';
import type { ScoringStateDto } from '../../hooks/useScoring';
import { formatGap, formatLapGap, NO_VALUE } from '../../lib/units';
import { useLiveReadings } from '../liveData';
import { ViewNoData } from './ViewNoData';

export interface MulticarRadarCircleViewProps {
  /** Live scoring (core:scoring); null when none has arrived. */
  readonly scoring: ScoringStateDto | null;
}

interface Marker {
  readonly slotId: number;
  readonly label: string;
  readonly frac: number;
  readonly isPlayer: boolean;
}

const LIVE_CHANNELS: readonly number[] = [ch.TRACK_LENGTH];

function placeLabel(place: number | null): string {
  return place !== null ? `P${String(place)}` : NO_VALUE;
}

/**
 * Field positions on a circle: each car's live lap distance (scoring) divided by the track length (TRACK_LENGTH).
 * Without scoring or a track length there is nothing real to place, so an explicit no-data state is shown.
 */
export function MulticarRadarCircleView({ scoring }: MulticarRadarCircleViewProps): JSX.Element {
  const r = useLiveReadings(LIVE_CHANNELS);
  const trackLength = r.num(ch.TRACK_LENGTH);
  const vehicles = scoring?.vehicles ?? [];

  const markers: Marker[] = [];
  if (trackLength !== null && trackLength > 0) {
    for (const v of vehicles) {
      if (v.lap_dist_m === null) continue;
      const frac = (((v.lap_dist_m / trackLength) % 1) + 1) % 1;
      markers.push({ slotId: v.slot_id, label: placeLabel(v.place), frac, isPlayer: v.is_player });
    }
  }

  const center = 150;
  const trackRadius = 100;
  const sorted = [...vehicles].sort((a, b) => (a.place ?? Infinity) - (b.place ?? Infinity));

  return (
    <div className="multicar-circle-view" data-testid="multicar-circle-view">
      <div className="circle-view-header">
        <div className="circle-title-group">
          <h2 className="circle-title">Multi-Car Track Radar (Circular Projection)</h2>
          <p className="circle-desc">Each car at its live lap distance as a fraction of the track length.</p>
        </div>
      </div>

      {markers.length === 0 ? (
        <ViewNoData
          title="Field positions"
          reason={
            scoring === null
              ? 'No live scoring received: car positions are unknown.'
              : 'Track length or car lap distances are not reported: positions cannot be placed.'
          }
          testId="circle-no-data"
        />
      ) : (
        <div className="circle-radar-workspace">
          <div className="circle-svg-card">
            <svg viewBox="0 0 300 300" className="polar-radar-svg" role="img" aria-label="Field positions around the lap">
              <circle cx={center} cy={center} r={trackRadius} fill="none" stroke="#334155" strokeWidth="14" />
              <circle cx={center} cy={center} r={trackRadius} fill="none" stroke="#475569" strokeWidth="2" />
              <line
                x1={center}
                y1={center - trackRadius - 12}
                x2={center}
                y2={center - trackRadius + 12}
                stroke="#ffffff"
                strokeWidth="3"
              />
              <text x={center} y={center - trackRadius - 16} fill="#ffffff" fontSize="9" fontWeight="bold" textAnchor="middle">
                S/F LINE
              </text>
              {markers.map((m) => {
                const angle = m.frac * Math.PI * 2 - Math.PI / 2;
                const x = center + trackRadius * Math.cos(angle);
                const y = center + trackRadius * Math.sin(angle);
                return (
                  <g key={m.slotId} data-testid={`circle-car-${String(m.slotId)}`}>
                    <circle
                      cx={x}
                      cy={y}
                      r={m.isPlayer ? 6.5 : 5}
                      fill={m.isPlayer ? '#ffffff' : '#22d3ee'}
                      stroke="#090b0e"
                      strokeWidth="1.5"
                    />
                    <text
                      x={x + 10 * Math.cos(angle)}
                      y={y + 10 * Math.sin(angle) + 3}
                      fill="#e2e8f0"
                      fontSize="8"
                      fontWeight={m.isPlayer ? 'bold' : 'normal'}
                      textAnchor={Math.cos(angle) >= 0 ? 'start' : 'end'}
                    >
                      {m.label}
                      {m.isPlayer ? ' (YOU)' : ''}
                    </text>
                  </g>
                );
              })}
            </svg>
          </div>

          <div className="circle-traffic-table-card">
            <div className="table-card-header">
              <span className="table-card-title">FIELD (LIVE SCORING)</span>
            </div>
            <table className="traffic-table">
              <thead>
                <tr>
                  <th>Pos</th>
                  <th>Class</th>
                  <th>Vehicle</th>
                  <th>Gap to leader</th>
                </tr>
              </thead>
              <tbody>
                {sorted.map((v) => (
                  <tr key={v.slot_id} className={v.is_player ? 'row-highlighted' : ''}>
                    <td>{placeLabel(v.place)}</td>
                    <td>{v.class_name ?? NO_VALUE}</td>
                    <td>{v.vehicle_name ?? NO_VALUE}</td>
                    <td>
                      {v.laps_behind_leader !== null && v.laps_behind_leader > 0
                        ? formatLapGap(v.laps_behind_leader)
                        : formatGap(v.time_behind_leader_s)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
