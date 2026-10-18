import { useState, useMemo, type JSX } from 'react';
import type { CompetitorEntryDto, ScoringStateDto } from '../../hooks/useScoring';
import { formatGap, formatLapGap, formatLapTime, NO_VALUE } from '../../lib/units';
import { parseVehicleClass, vehicleClassCode, classColorVar } from '../../lib/classes';
import { useScopeDq, type ScopeDq } from './presentation';
import '../../styles/common.css';
import './tower.css';

export interface TimingTowerProps {
  readonly scoring: ScoringStateDto | null;
  readonly selectedSlotId?: number | null;
  readonly onSelectSlot?: (slotId: number) => void;
  /** Optional explicit data quality override; falls back to ScopeDqContext. */
  readonly dq?: ScopeDq;
}

/**
 * Extracts car number from vehicle name string.
 * Common formats: "#51 Ferrari 499P", "Oreca 07 #81", "#92", or standalone number.
 */
function extractCarNumber(vehicleName: string | null | undefined): string {
  if (!vehicleName) return NO_VALUE;
  const match = /#(\d+)/.exec(vehicleName);
  if (match?.[1]) return match[1];
  const numMatch = /\b\d{1,3}\b/.exec(vehicleName);
  if (numMatch?.[0]) return numMatch[0];
  return NO_VALUE;
}

/**
 * Determines whether the scoring session is a race.
 * Handles both preview string session types ('Race', 'Qualifying') and real LMU raw mSession numbers (10..13 = Race).
 */
function isRaceSession(sessionType: string | null | undefined): boolean {
  if (!sessionType) return true;
  const s = sessionType.trim().toLowerCase();
  if (s.includes('race')) return true;
  if (s.includes('qual') || s.includes('prac') || s.includes('test') || s.includes('warm')) return false;
  const num = parseInt(s, 10);
  if (!isNaN(num)) {
    return num >= 10 && num <= 13;
  }
  return true;
}

interface ComputedTowerGaps {
  readonly gaps: readonly string[];
  readonly intervals: readonly string[];
}

/**
 * Computes gap to leader and interval to car ahead according to REDESIGN.md C8, C9, T22, and T23:
 * - Leader in any mode shows 'LDR' for both Gap and Int.
 * - Non-race (qualifying/practice): gaps and intervals calculated from best_lap_s deltas (T23).
 * - Class filter active: Gap is to the class leader, Int is to the class car ahead (T22).
 * - Race overall: uses laps/time behind overall leader and next car.
 */
function computeGapsAndIntervals(
  vehicles: readonly CompetitorEntryDto[],
  isRace: boolean,
  isFiltered: boolean,
): ComputedTowerGaps {
  const n = vehicles.length;
  const gaps: string[] = Array.from({ length: n }, () => NO_VALUE);
  const intervals: string[] = Array.from({ length: n }, () => NO_VALUE);

  if (n === 0) return { gaps, intervals };

  // Leader is always LDR
  gaps[0] = 'LDR';
  intervals[0] = 'LDR';

  if (!isRace) {
    // Practice / Qualifying: based on best_lap_s difference (T23)
    const leaderBest = vehicles[0]?.best_lap_s ?? null;
    for (let i = 1; i < n; i++) {
      const current = vehicles[i];
      const prev = vehicles[i - 1];

      // Gap to leader
      if (current?.best_lap_s != null && leaderBest != null) {
        const delta = current.best_lap_s - leaderBest;
        gaps[i] = delta <= 0 ? 'LDR' : formatGap(delta);
      } else {
        gaps[i] = NO_VALUE;
      }

      // Interval to car ahead
      if (current?.best_lap_s != null && prev?.best_lap_s != null) {
        const delta = current.best_lap_s - prev.best_lap_s;
        intervals[i] = delta <= 0 ? 'LDR' : formatGap(delta);
      } else {
        intervals[i] = NO_VALUE;
      }
    }
  } else {
    // Race session
    if (isFiltered) {
      // Under a class filter: Gap to class leader, Int to class car ahead (T22)
      const classLeader = vehicles[0];
      for (let i = 1; i < n; i++) {
        const current = vehicles[i];
        const prev = vehicles[i - 1];

        // Gap to class leader
        const leaderLaps = classLeader?.laps ?? 0;
        const currentLaps = current?.laps ?? 0;
        const lapDiff = leaderLaps - currentLaps;

        if (lapDiff > 0) {
          gaps[i] = formatLapGap(lapDiff);
        } else if (current?.time_behind_leader_s != null && classLeader?.time_behind_leader_s != null) {
          const delta = current.time_behind_leader_s - classLeader.time_behind_leader_s;
          gaps[i] = delta <= 0 ? 'LDR' : formatGap(delta);
        } else {
          gaps[i] = NO_VALUE;
        }

        // Interval to class car ahead
        const prevLaps = prev?.laps ?? 0;
        const intLapDiff = prevLaps - currentLaps;

        if (intLapDiff > 0) {
          intervals[i] = formatLapGap(intLapDiff);
        } else if (current?.time_behind_leader_s != null && prev?.time_behind_leader_s != null) {
          const delta = current.time_behind_leader_s - prev.time_behind_leader_s;
          intervals[i] = delta <= 0 ? 'LDR' : formatGap(delta);
        } else if (current?.time_behind_next_s != null) {
          intervals[i] = formatGap(current.time_behind_next_s);
        } else {
          intervals[i] = NO_VALUE;
        }
      }
    } else {
      // Overall (ALL classes)
      for (let i = 1; i < n; i++) {
        const current = vehicles[i];

        // Overall Gap
        if (current?.laps_behind_leader != null && current.laps_behind_leader > 0) {
          gaps[i] = formatLapGap(current.laps_behind_leader);
        } else if (current?.time_behind_leader_s != null) {
          gaps[i] = formatGap(current.time_behind_leader_s);
        } else {
          gaps[i] = NO_VALUE;
        }

        // Overall Interval
        if (current?.laps_behind_next != null && current.laps_behind_next > 0) {
          intervals[i] = formatLapGap(current.laps_behind_next);
        } else if (current?.time_behind_next_s != null) {
          intervals[i] = formatGap(current.time_behind_next_s);
        } else {
          intervals[i] = NO_VALUE;
        }
      }
    }
  }

  return { gaps, intervals };
}

export function TimingTower({ scoring, selectedSlotId, onSelectSlot, dq }: TimingTowerProps): JSX.Element {
  const [selectedClass, setSelectedClass] = useState<string>('ALL');
  const contextDq = useScopeDq();
  const effectiveDq = dq ?? contextDq;

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

  const isRace = useMemo(() => isRaceSession(scoring?.session.session_type), [scoring?.session.session_type]);
  const isFiltered = selectedClass !== 'ALL';

  const { gaps, intervals } = useMemo(
    () => computeGapsAndIntervals(filteredVehicles, isRace, isFiltered),
    [filteredVehicles, isRace, isFiltered],
  );

  const dqClass = effectiveDq ? `standings-table--${effectiveDq}` : '';

  if (!scoring || scoring.vehicles.length === 0) {
    return (
      <div className="standings-tower" data-testid="timing-tower">
        <div className="standings-header">
          <div className="standings-title-row">
            <span className="card-header-title">STANDINGS TOWER</span>
            {effectiveDq === 'stale' && (
              <span className="status-badge-stale" data-testid="tower-stale-badge">STALE</span>
            )}
            {effectiveDq === 'ended' && (
              <span className="status-badge-ended" data-testid="tower-ended-badge">ENDED</span>
            )}
            {effectiveDq === 'untrusted' && (
              <span className="status-badge-untrusted" data-testid="tower-untrusted-badge">UNTRUSTED</span>
            )}
          </div>
        </div>
        <div className="standings-empty">
          <span className="card-row-label">
            {effectiveDq === 'ended' ? 'Session ended' : 'Awaiting live scoring data...'}
          </span>
        </div>
      </div>
    );
  }

  return (
    <div className="standings-tower" data-testid="timing-tower">
      <div className="standings-header">
        <div className="standings-title-row">
          <span className="card-header-title">STANDINGS TOWER</span>
          {effectiveDq === 'stale' && (
            <span className="status-badge-stale" data-testid="tower-stale-badge">STALE</span>
          )}
          {effectiveDq === 'ended' && (
            <span className="status-badge-ended" data-testid="tower-ended-badge">ENDED</span>
          )}
          {effectiveDq === 'untrusted' && (
            <span className="status-badge-untrusted" data-testid="tower-untrusted-badge">UNTRUSTED</span>
          )}
        </div>
        {classes.length > 1 && (
          <div className="standings-class-filter" role="group" aria-label="Class Filter">
            <button
              type="button"
              className={`btn btn-xs ${selectedClass === 'ALL' ? 'btn-primary' : ''}`}
              aria-pressed={selectedClass === 'ALL'}
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
                aria-pressed={selectedClass === cls}
                onClick={() => {
                  setSelectedClass(cls);
                }}
              >
                {vehicleClassCode(cls)}
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="standings-table-container">
        <table className={`standings-table ${dqClass}`}>
          <thead>
            <tr>
              <th className="standings-col-pos" title="Position">P</th>
              <th className="standings-col-class" title="Class">CLS</th>
              <th className="standings-col-num" title="Car Number">#</th>
              <th className="standings-col-driver">DRIVER</th>
              <th className="standings-col-gap" title="Interval to car ahead">INT</th>
              <th className="standings-col-gap" title="Gap to leader">GAP</th>
              <th className="standings-col-last standings-col-time" title="Last lap time">LAST</th>
              <th className="standings-col-pit" title="Pit status">PIT</th>
            </tr>
          </thead>
          <tbody>
            {filteredVehicles.map((v, idx) => {
              const isSelected = selectedSlotId === v.slot_id;
              const isDriver = v.is_player;
              const rowClass = [
                'standings-row',
                isDriver ? 'standings-row-driver' : '',
                isSelected ? 'standings-row-selected' : '',
                effectiveDq === 'stale' ? 'standings-row--stale' : '',
              ]
                .filter(Boolean)
                .join(' ');

              const posDisplay = isFiltered ? (v.class_place ?? idx + 1) : (v.place ?? NO_VALUE);
              const carNum = extractCarNumber(v.vehicle_name);
              const classCode = vehicleClassCode(v.class_name);
              const classColor = classColorVar(v.class_name);
              const gapVal = gaps[idx] ?? NO_VALUE;
              const intVal = intervals[idx] ?? NO_VALUE;

              return (
                <tr
                  key={v.slot_id}
                  className={rowClass}
                  data-testid={`standings-row-${v.slot_id}`}
                  onClick={() => onSelectSlot?.(v.slot_id)}
                >
                  <td
                    className="standings-col-pos"
                    style={{ borderLeft: `3px solid ${classColor}` }}
                  >
                    {posDisplay}
                  </td>
                  <td className="standings-col-class">
                    <span
                      className="class-chip"
                      data-class={parseVehicleClass(v.class_name)}
                      style={{ color: classColor, borderColor: classColor }}
                    >
                      {classCode}
                    </span>
                  </td>
                  <td className="standings-col-num" title={v.vehicle_name ?? undefined}>
                    {carNum}
                  </td>
                  <td className="standings-col-driver">
                    <div className="standings-driver-cell" title={v.driver ?? 'Unknown'}>
                      <span className="standings-driver-name">{v.driver ?? 'Unknown'}</span>
                      {isDriver && <span className="driver-pill" data-testid="driver-pill">DRIVER</span>}
                      {v.in_pits && <span className="pit-badge pit-badge-in">PIT</span>}
                      {v.in_garage_stall && <span className="pit-badge pit-badge-garage">GAR</span>}
                    </div>
                  </td>
                  <td className="standings-col-gap">
                    {intVal === 'LDR' ? (
                      <span className="gap-ldr">LDR</span>
                    ) : (
                      intVal
                    )}
                  </td>
                  <td className="standings-col-gap">
                    {gapVal === 'LDR' ? (
                      <span className="gap-ldr">LDR</span>
                    ) : (
                      gapVal
                    )}
                  </td>
                  <td className="standings-col-last standings-col-time">{formatLapTime(v.last_lap_s)}</td>
                  <td className="standings-col-pit">
                    {v.in_pits ? (
                      <span className="pit-badge pit-badge-in">PIT</span>
                    ) : v.in_garage_stall ? (
                      <span className="pit-badge pit-badge-garage">GAR</span>
                    ) : (
                      <span className="pit-stops-count">{v.num_pitstops ?? NO_VALUE}</span>
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
