import { useState, useMemo, type JSX } from 'react';
import type { CompetitorEntryDto, ScoringStateDto } from '../../hooks/useScoring';
import { formatGap, formatLapGap, formatLapTime, NO_VALUE } from '../../lib/units';
import '../../styles/common.css';
import './tower.css';

export interface TimingTowerProps {
  readonly scoring: ScoringStateDto | null;
  readonly selectedSlotId?: number | null;
  readonly onSelectSlot?: (slotId: number) => void;
}

export function TimingTower({ scoring, selectedSlotId, onSelectSlot }: TimingTowerProps): JSX.Element {
  const [selectedClass, setSelectedClass] = useState<string>('ALL');

  const classes = useMemo(() => {
    if (!scoring) return [];
    const set = new Set<string>();
    for (const v of scoring.vehicles) {
      if (v.class_name) set.add(v.class_name);
    }
    return Array.from(set).sort();
  }, [scoring]);

  const filteredVehicles = useMemo(() => {
    if (!scoring) return [];
    let list = [...scoring.vehicles];
    if (selectedClass !== 'ALL') {
      list = list.filter((v) => v.class_name === selectedClass);
    }
    // Sort primarily by place (1-based), unplaced at the end
    list.sort((a, b) => {
      if (a.place == null && b.place == null) return 0;
      if (a.place == null) return 1;
      if (b.place == null) return -1;
      return a.place - b.place;
    });
    return list;
  }, [scoring, selectedClass]);

  if (!scoring || scoring.vehicles.length === 0) {
    return (
      <div className="standings-tower" data-testid="timing-tower">
        <div className="standings-header">
          <span className="card-header-title">STANDINGS TOWER</span>
        </div>
        <div className="standings-empty">
          <span className="card-row-label">Awaiting live scoring data...</span>
        </div>
      </div>
    );
  }

  return (
    <div className="standings-tower" data-testid="timing-tower">
      <div className="standings-header">
        <span className="card-header-title">STANDINGS TOWER</span>
        {classes.length > 1 && (
          <div className="standings-class-filter">
            <button
              type="button"
              className={`btn btn-xs ${selectedClass === 'ALL' ? 'btn-primary' : ''}`}
              onClick={() => {
                setSelectedClass('ALL');
              }}
            >
              ALL
            </button>
            {classes.map((cls) => (
              <button
                key={cls}
                type="button"
                className={`btn btn-xs ${selectedClass === cls ? 'btn-primary' : ''}`}
                onClick={() => {
                  setSelectedClass(cls);
                }}
              >
                {cls}
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="standings-table-container">
        <table className="standings-table">
          <thead>
            <tr>
              <th title="Overall Position">P</th>
              <th title="Class Position">CP</th>
              <th>Driver / Team</th>
              <th>Class</th>
              <th title="Gap to Leader">Gap</th>
              <th title="Last Lap Time">Last</th>
              <th title="Best Lap Time">Best</th>
              <th title="Pit Status">Pit</th>
            </tr>
          </thead>
          <tbody>
            {filteredVehicles.map((v) => {
              const isSelected = selectedSlotId === v.slot_id;
              const isDriver = v.is_player;
              const rowClass = [
                isDriver ? 'standings-row-driver' : '',
                isSelected ? 'standings-row-selected' : '',
              ]
                .filter(Boolean)
                .join(' ');

              return (
                <tr
                  key={v.slot_id}
                  className={rowClass}
                  data-testid={`standings-row-${v.slot_id}`}
                  onClick={() => onSelectSlot?.(v.slot_id)}
                >
                  <td className="standings-col-pos">{v.place ?? NO_VALUE}</td>
                  <td className="standings-col-cpos">{v.class_place ?? NO_VALUE}</td>
                  <td className="standings-col-driver">
                    <div className="standings-driver-name" title={v.driver ?? 'Unknown'}>
                      <span className="standings-driver-name-text">{v.driver ?? 'Unknown'}</span>
                      {isDriver && <span className="driver-pill">YOU</span>}
                    </div>
                    <div className="standings-sub-info">
                      {v.vehicle_name ?? v.pit_group ?? NO_VALUE}
                    </div>
                  </td>
                  <td className="standings-col-class">
                    <span className="class-badge" data-class={v.class_name ?? 'UNKNOWN'}>
                      {v.class_name ?? NO_VALUE}
                    </span>
                  </td>
                  <td className="standings-col-gap">
                    {renderGap(v)}
                  </td>
                  <td className="standings-col-time">{formatLapTime(v.last_lap_s)}</td>
                  <td className="standings-col-time">{formatLapTime(v.best_lap_s)}</td>
                  <td className="standings-col-pit">
                    {v.in_pits ? (
                      <span className="pit-badge pit-badge-in">PIT</span>
                    ) : v.in_garage_stall ? (
                      <span className="pit-badge pit-badge-garage">GAR</span>
                    ) : (
                      <span className="pit-stops-count">{v.num_pitstops ?? 0}</span>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function renderGap(v: CompetitorEntryDto): string {
  if (v.place === 1) return 'LEADER';
  if (v.laps_behind_leader != null && v.laps_behind_leader > 0) {
    return formatLapGap(v.laps_behind_leader);
  }
  if (v.time_behind_leader_s != null) {
    return formatGap(v.time_behind_leader_s);
  }
  return NO_VALUE;
}
