import { useState, useMemo, useRef, useCallback, useEffect, type JSX } from 'react';
import type { CompetitorEntryDto, ScoringStateDto } from '../../hooks/useScoring';
import { formatGap, formatLapGap, formatLapTime, NO_VALUE } from '../../lib/units';
import { parseVehicleClass, vehicleClassCode, classColorVar } from '../../lib/classes';
import { useScopeDq, type ScopeDq } from './presentation';
import '../../styles/common.css';
import './tower.css';

export type TowerVariant = 'full' | 'compact' | 'rail';

export interface TimingTowerProps {
  readonly scoring: ScoringStateDto | null;
  readonly selectedSlotId?: number | null;
  readonly onSelectSlot?: (slotId: number) => void;
  /** Optional explicit data quality override; falls back to ScopeDqContext. */
  readonly dq?: ScopeDq;
  /**
   * Tower layout variant:
   * - 'full': 1920 primary layout with all columns (P, CLS, #, DRIVER, INT, GAP, LAST, PIT).
   * - 'compact': 1366 layout with Last & Pit count dropped (P, CLS, #, DRIVER, INT, GAP).
   * - 'rail': < 1280 collapsed 56px rail (P, class stripe, #) that expands as an overlay on click/focus.
   * Defaults to 'full' (with container-query responsive collapsing).
   */
  readonly variant?: TowerVariant;
  /**
   * Optional manual override for testing the own-row pinning mechanism.
   * In normal operation, pinning is computed automatically from scroll position.
   */
  readonly pinnedOverride?: 'none' | 'top' | 'bottom';
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

export function TimingTower({
  scoring,
  selectedSlotId,
  onSelectSlot,
  dq,
  variant = 'full',
  pinnedOverride,
}: TimingTowerProps): JSX.Element {
  const [selectedClass, setSelectedClass] = useState<string>('ALL');
  const [isRailExpanded, setIsRailExpanded] = useState<boolean>(false);
  const [scrollPinned, setScrollPinned] = useState<'none' | 'top' | 'bottom'>('none');
  const effectivePinned = pinnedOverride ?? scrollPinned;

  const tableContainerRef = useRef<HTMLDivElement>(null);
  const overlayRef = useRef<HTMLDivElement>(null);

  const contextDq = useScopeDq();
  const effectiveDq = dq ?? contextDq;

  const isRail = variant === 'rail';
  const showLastAndPit = variant !== 'compact';

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

  const playerVehicle = useMemo(() => filteredVehicles.find((v) => v.is_player), [filteredVehicles]);
  const playerIndex = useMemo(() => filteredVehicles.findIndex((v) => v.is_player), [filteredVehicles]);

  const isRace = useMemo(() => isRaceSession(scoring?.session.session_type), [scoring?.session.session_type]);
  const isFiltered = selectedClass !== 'ALL';

  const { gaps, intervals } = useMemo(
    () => computeGapsAndIntervals(filteredVehicles, isRace, isFiltered),
    [filteredVehicles, isRace, isFiltered],
  );

  const updatePinnedState = useCallback(() => {
    const container = tableContainerRef.current;
    if (!container || !playerVehicle || playerIndex === -1) {
      setScrollPinned('none');
      return;
    }

    const { scrollTop, clientHeight, scrollHeight } = container;
    if (scrollHeight <= clientHeight + 4) {
      setScrollPinned('none');
      return;
    }

    // Try finding the driver row element in tbody
    const driverRowEl = container.querySelector<HTMLTableRowElement>(
      `tbody [data-testid="standings-row-${playerVehicle.slot_id}"]`,
    );

    if (driverRowEl) {
      const rowTop = driverRowEl.offsetTop;
      const rowHeight = driverRowEl.offsetHeight || 22;
      if (rowTop + rowHeight <= scrollTop + 2) {
        setScrollPinned('top');
      } else if (rowTop >= scrollTop + clientHeight - 2) {
        setScrollPinned('bottom');
      } else {
        setScrollPinned('none');
      }
    } else {
      // Fallback estimate assuming 22px row height
      const estRowTop = playerIndex * 22;
      if (estRowTop + 22 <= scrollTop) {
        setScrollPinned('top');
      } else if (estRowTop >= scrollTop + clientHeight) {
        setScrollPinned('bottom');
      } else {
        setScrollPinned('none');
      }
    }
  }, [playerVehicle, playerIndex]);

  // Recalculate pinned state on scroll and vehicle updates
  useEffect(() => {
    updatePinnedState();
  }, [updatePinnedState]);

  // Keyboard Escape listener for rail mode expanded overlay
  useEffect(() => {
    if (!isRailExpanded) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setIsRailExpanded(false);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [isRailExpanded]);

  // Click outside to collapse rail overlay
  useEffect(() => {
    if (!isRailExpanded) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (overlayRef.current && !overlayRef.current.contains(e.target as Node)) {
        setIsRailExpanded(false);
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
    };
  }, [isRailExpanded]);

  const [focusedSlotId, setFocusedSlotId] = useState<number | null>(null);
  const rowRefs = useRef<Map<number, HTMLTableRowElement>>(new Map());
  const railRowRefs = useRef<Map<number, HTMLTableRowElement>>(new Map());

  const activeSlotId = useMemo(() => {
    if (focusedSlotId !== null && filteredVehicles.some((v) => v.slot_id === focusedSlotId)) {
      return focusedSlotId;
    }
    if (selectedSlotId != null && filteredVehicles.some((v) => v.slot_id === selectedSlotId)) {
      return selectedSlotId;
    }
    if (playerVehicle && filteredVehicles.some((v) => v.slot_id === playerVehicle.slot_id)) {
      return playerVehicle.slot_id;
    }
    return filteredVehicles[0]?.slot_id ?? null;
  }, [focusedSlotId, selectedSlotId, playerVehicle, filteredVehicles]);

  const handleRowClick = (slotId: number) => {
    onSelectSlot?.(slotId);
    // If pinned row was clicked, scroll driver into view in tbody
    const targetRow = tableContainerRef.current?.querySelector<HTMLTableRowElement>(
      `tbody [data-testid="standings-row-${slotId}"]`,
    );
    if (typeof targetRow?.scrollIntoView === 'function') {
      targetRow.scrollIntoView({ block: 'center', behavior: 'smooth' });
    }
  };

  const handleRowKeyDown = (
    e: React.KeyboardEvent<HTMLTableRowElement>,
    v: CompetitorEntryDto,
    idx: number,
    isRailRow = false,
  ) => {
    const refs = isRailRow ? railRowRefs : rowRefs;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      const nextIdx = Math.min(idx + 1, filteredVehicles.length - 1);
      const nextSlot = filteredVehicles[nextIdx]?.slot_id;
      if (nextSlot !== undefined) {
        setFocusedSlotId(nextSlot);
        refs.current.get(nextSlot)?.focus();
      }
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      const prevIdx = Math.max(idx - 1, 0);
      const prevSlot = filteredVehicles[prevIdx]?.slot_id;
      if (prevSlot !== undefined) {
        setFocusedSlotId(prevSlot);
        refs.current.get(prevSlot)?.focus();
      }
    } else if (e.key === 'Home') {
      e.preventDefault();
      const firstSlot = filteredVehicles[0]?.slot_id;
      if (firstSlot !== undefined) {
        setFocusedSlotId(firstSlot);
        refs.current.get(firstSlot)?.focus();
      }
    } else if (e.key === 'End') {
      e.preventDefault();
      const lastSlot = filteredVehicles[filteredVehicles.length - 1]?.slot_id;
      if (lastSlot !== undefined) {
        setFocusedSlotId(lastSlot);
        refs.current.get(lastSlot)?.focus();
      }
    } else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      setFocusedSlotId(v.slot_id);
      handleRowClick(v.slot_id);
    }
  };

  const dqClass = effectiveDq ? `standings-table--${effectiveDq}` : '';

  // Empty / Not Connected State
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

  // Helper to render table row cells for full / compact variants
  const renderRowCells = (v: CompetitorEntryDto, idx: number, isPinned = false) => {
    const isDriver = v.is_player;
    const posDisplay = isFiltered ? (v.class_place ?? idx + 1) : (v.place ?? NO_VALUE);
    const carNum = extractCarNumber(v.vehicle_name);
    const classCode = vehicleClassCode(v.class_name);
    const classColor = classColorVar(v.class_name);
    const gapVal = gaps[idx] ?? NO_VALUE;
    const intVal = intervals[idx] ?? NO_VALUE;

    return (
      <>
        <td
          className="standings-col-pos"
          style={isDriver ? undefined : { borderLeft: `3px solid ${classColor}` }}
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
            {isPinned && <span className="pinned-badge" title="Pinned driver row">📌</span>}
            {v.in_pits && <span className="pit-badge pit-badge-in">PIT</span>}
            {v.in_garage_stall && <span className="pit-badge pit-badge-garage">GAR</span>}
          </div>
        </td>
        <td className="standings-col-gap">
          {intVal === 'LDR' ? <span className="gap-ldr">LDR</span> : intVal}
        </td>
        <td className="standings-col-gap">
          {gapVal === 'LDR' ? <span className="gap-ldr">LDR</span> : gapVal}
        </td>
        {showLastAndPit && (
          <>
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
          </>
        )}
      </>
    );
  };

  // Rail Mode: Collapsed 56px Rail
  if (isRail && !isRailExpanded) {
    return (
      <div
        className="standings-tower standings-tower--rail"
        data-testid="timing-tower-rail"
        onClick={() => {
          setIsRailExpanded(true);
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            setIsRailExpanded(true);
          }
        }}
        tabIndex={0}
        role="region"
        aria-label="Timing Tower (Rail Mode, press Enter or click to expand)"
      >
        <div className="standings-header standings-header--rail">
          <button
            type="button"
            className="standings-rail-expand-btn"
            aria-label="Expand standings tower"
            title="Expand standings tower"
            onClick={(e) => {
              e.stopPropagation();
              setIsRailExpanded(true);
            }}
          >
            <span className="standings-rail-glyph">▶</span>
            <span className="standings-rail-hdr-title"><abbr title="Position">POS</abbr></span>
          </button>
        </div>

        <div className="standings-table-container">
          <table className={`standings-table standings-table--rail ${dqClass}`}>
            <thead>
              <tr>
                <th className="standings-col-pos" scope="col"><abbr title="Position">P</abbr></th>
                <th className="standings-col-num" scope="col"><abbr title="Car Number">#</abbr></th>
              </tr>
            </thead>
            <tbody>
              {filteredVehicles.map((v, idx) => {
                const isSelected = selectedSlotId === v.slot_id;
                const isDriver = v.is_player;
                const isFocused = v.slot_id === activeSlotId;
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
                const classColor = classColorVar(v.class_name);

                return (
                  <tr
                    key={v.slot_id}
                    ref={(el) => {
                      if (el) railRowRefs.current.set(v.slot_id, el);
                      else railRowRefs.current.delete(v.slot_id);
                    }}
                    className={rowClass}
                    role="row"
                    aria-selected={isSelected}
                    tabIndex={isFocused ? 0 : -1}
                    data-testid={`standings-row-${v.slot_id}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      setFocusedSlotId(v.slot_id);
                      onSelectSlot?.(v.slot_id);
                    }}
                    onFocus={() => {
                      setFocusedSlotId(v.slot_id);
                    }}
                    onKeyDown={(e) => {
                      handleRowKeyDown(e, v, idx, true);
                    }}
                  >
                    <td
                      className="standings-col-pos"
                      style={isDriver ? undefined : { borderLeft: `3px solid ${classColor}` }}
                    >
                      {posDisplay}
                    </td>
                    <td className="standings-col-num" title={v.vehicle_name ?? undefined}>
                      {carNum}
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

  // Full or Compact or Rail-Expanded Tower
  const towerClassNames = [
    'standings-tower',
    variant === 'compact' ? 'standings-tower--compact' : '',
    isRailExpanded ? 'standings-tower--rail-expanded' : '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <div
      className={towerClassNames}
      data-testid={isRailExpanded ? 'timing-tower-rail-expanded' : 'timing-tower'}
      ref={isRailExpanded ? overlayRef : undefined}
    >
      <div className="standings-header">
        <div className="standings-title-row">
          {isRailExpanded && (
            <button
              type="button"
              className="standings-rail-collapse-btn"
              onClick={() => {
                setIsRailExpanded(false);
              }}
              aria-label="Collapse tower"
              title="Collapse tower"
            >
              ◀
            </button>
          )}
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
              className={`standings-filter-chip btn btn-xs ${selectedClass === 'ALL' ? 'standings-filter-chip--active btn-primary' : ''}`}
              aria-pressed={selectedClass === 'ALL'}
              onClick={() => {
                setSelectedClass('ALL');
              }}
            >
              ALL
            </button>
            {classes.map((cls) => {
              const code = vehicleClassCode(cls);
              const isActive = selectedClass === cls;
              return (
                <button
                  key={cls}
                  type="button"
                  className={`standings-filter-chip btn btn-xs ${isActive ? 'standings-filter-chip--active btn-primary' : ''}`}
                  aria-pressed={isActive}
                  onClick={() => {
                    setSelectedClass(cls);
                  }}
                >
                  {code}
                </button>
              );
            })}
          </div>
        )}
      </div>

      <div
        className="standings-table-container"
        ref={tableContainerRef}
        onScroll={updatePinnedState}
      >
        <table className={`standings-table ${dqClass}`}>
          <thead>
            <tr>
              <th className="standings-col-pos" scope="col"><abbr title="Position">P</abbr></th>
              <th className="standings-col-class" scope="col"><abbr title="Class">CLS</abbr></th>
              <th className="standings-col-num" scope="col"><abbr title="Car Number">#</abbr></th>
              <th className="standings-col-driver" scope="col">DRIVER</th>
              <th className="standings-col-gap" scope="col"><abbr title="Interval to car ahead">INT</abbr></th>
              <th className="standings-col-gap" scope="col"><abbr title="Gap to leader">GAP</abbr></th>
              {showLastAndPit && (
                <>
                  <th className="standings-col-last standings-col-time" scope="col"><abbr title="Last lap time">LAST</abbr></th>
                  <th className="standings-col-pit" scope="col"><abbr title="Pit status">PIT</abbr></th>
                </>
              )}
            </tr>

            {/* Pinned driver row at top when scrolled below player */}
            {effectivePinned === 'top' && playerVehicle && (
              <tr
                className="standings-row standings-row-driver standings-row-pinned standings-row-pinned--top"
                data-testid="standings-pinned-driver-top"
                data-pinned="top"
                role="row"
                tabIndex={0}
                aria-label="Driver pinned to top — click or press Enter to jump to position"
                title="Driver (pinned to top) — Click to jump"
                onClick={() => {
                  handleRowClick(playerVehicle.slot_id);
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    handleRowClick(playerVehicle.slot_id);
                  }
                }}
              >
                {renderRowCells(playerVehicle, playerIndex, true)}
              </tr>
            )}
          </thead>

          <tbody>
            {filteredVehicles.map((v, idx) => {
              const isSelected = selectedSlotId === v.slot_id;
              const isDriver = v.is_player;
              const isFocused = v.slot_id === activeSlotId;
              const rowClass = [
                'standings-row',
                isDriver ? 'standings-row-driver' : '',
                isSelected ? 'standings-row-selected' : '',
                effectiveDq === 'stale' ? 'standings-row--stale' : '',
              ]
                .filter(Boolean)
                .join(' ');

              return (
                <tr
                  key={v.slot_id}
                  ref={(el) => {
                    if (el) rowRefs.current.set(v.slot_id, el);
                    else rowRefs.current.delete(v.slot_id);
                  }}
                  className={rowClass}
                  role="row"
                  aria-selected={isSelected}
                  tabIndex={isFocused ? 0 : -1}
                  data-testid={`standings-row-${v.slot_id}`}
                  onClick={() => {
                    setFocusedSlotId(v.slot_id);
                    handleRowClick(v.slot_id);
                  }}
                  onFocus={() => {
                    setFocusedSlotId(v.slot_id);
                  }}
                  onKeyDown={(e) => {
                    handleRowKeyDown(e, v, idx, false);
                  }}
                >
                  {renderRowCells(v, idx, false)}
                </tr>
              );
            })}
          </tbody>

          {/* Pinned driver row at bottom when scrolled above player */}
          {effectivePinned === 'bottom' && playerVehicle && (
            <tfoot>
              <tr
                className="standings-row standings-row-driver standings-row-pinned standings-row-pinned--bottom"
                data-testid="standings-pinned-driver-bottom"
                data-pinned="bottom"
                role="row"
                tabIndex={0}
                aria-label="Driver pinned to bottom — click or press Enter to jump to position"
                title="Driver (pinned to bottom) — Click to jump"
                onClick={() => {
                  handleRowClick(playerVehicle.slot_id);
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    handleRowClick(playerVehicle.slot_id);
                  }
                }}
              >
                {renderRowCells(playerVehicle, playerIndex, true)}
              </tr>
            </tfoot>
          )}
        </table>
      </div>
    </div>
  );
}
