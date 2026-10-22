import { isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';

// ---------------------------------------------------------------------------------------------------------------
// Wire shape: `core:scoring` is `serde_json::to_value(ScoringState)` (crates/app-core/src/driver/pipeline.rs and
// engineer/core.rs). Field names and types follow crates/telemetry-model/src/scoring.rs exactly; every LMU enum is
// serialised as its RAW LMU value (crates/telemetry-model/src/enums.rs module docs), never as a string.
// ---------------------------------------------------------------------------------------------------------------

/** `ScoringWeather` (scoring.rs). */
export interface RawScoringWeather {
  readonly ambient_temp_c: number | null;
  readonly track_temp_c: number | null;
  /** Rain intensity 0..1 (`mRaining`). */
  readonly raining_frac: number | null;
  readonly dark_cloud_frac: number | null;
  readonly cloud_coverage_raw: number | null;
  readonly path_wetness_min_frac: number | null;
  readonly path_wetness_max_frac: number | null;
  readonly path_wetness_avg_frac: number | null;
  readonly wind_mps: readonly [number, number, number] | null;
  readonly track_grip_level_raw: number | null;
}

/** `SessionScoring` (scoring.rs). */
export interface RawSessionScoring {
  readonly track_name: string | null;
  /** Raw `mSession` (0 test, 1–4 practice, 5–8 qualifying, 9 warm-up, 10–13 race). */
  readonly session_type: number | null;
  /** Raw `mGamePhase` (6 = full-course yellow / safety car, 7 = session stopped). */
  readonly game_phase: number | null;
  /** Raw `mYellowFlagState` i8 (-1 invalid, 0 none, 1 pending … 6 resume, 7 race halt). */
  readonly yellow_flag_state: number | null;
  readonly sector_flags_raw: readonly [number, number, number] | null;
  readonly start_light: number | null;
  readonly num_red_lights: number | null;
  readonly num_vehicles: number | null;
  readonly current_et_s: number | null;
  readonly end_et_s: number | null;
  readonly time_remaining_s: number | null;
  readonly max_laps: number | null;
  readonly track_length_m: number | null;
  readonly in_realtime: boolean | null;
  readonly game_mode: number | null;
  readonly max_players: number | null;
  readonly time_of_day_s: number | null;
  readonly fixed_setup: boolean | null;
  readonly weather: RawScoringWeather;
}

/** `CompetitorEntry` (scoring.rs). */
export interface RawCompetitorEntry {
  readonly slot_id: number;
  readonly driver: string | null;
  readonly vehicle_name: string | null;
  readonly vehicle_filename: string | null;
  readonly class_name: string | null;
  readonly pit_group: string | null;
  readonly place: number | null;
  readonly class_place: number | null;
  readonly laps: number | null;
  readonly lap_dist_m: number | null;
  readonly sector: number | null;
  readonly best_lap_s: number | null;
  readonly last_lap_s: number | null;
  readonly best_sector1_s: number | null;
  readonly best_sector12_s: number | null;
  readonly last_sector1_s: number | null;
  readonly last_sector12_s: number | null;
  readonly cur_sector1_s: number | null;
  readonly cur_sector12_s: number | null;
  readonly time_behind_next_s: number | null;
  readonly laps_behind_next: number | null;
  readonly time_behind_leader_s: number | null;
  readonly laps_behind_leader: number | null;
  readonly time_into_lap_s: number | null;
  readonly estimated_lap_s: number | null;
  readonly lap_start_et_s: number | null;
  readonly in_pits: boolean | null;
  readonly in_garage_stall: boolean | null;
  /** Raw `mPitState` u8 (observed: 0 none, 1 request, 2 entering, 4 stopped in box, 5 leaving). */
  readonly pit_state: number | null;
  readonly num_pitstops: number | null;
  readonly num_penalties: number | null;
  readonly finish_status: number | null;
  readonly flag: number | null;
  readonly under_yellow: boolean | null;
  readonly individual_phase: number | null;
  readonly count_lap_flag: number | null;
  readonly control: number | null;
  readonly is_player: boolean;
  readonly qualification: number | null;
  readonly energy_or_fuel_frac: number | null;
  readonly pos_x_m: number | null;
  readonly pos_z_m: number | null;
}

/** `ScoringState` (scoring.rs): the `core:scoring` payload. */
export interface RawScoringState {
  readonly session: RawSessionScoring;
  readonly vehicles: readonly RawCompetitorEntry[];
}

// ---------------------------------------------------------------------------------------------------------------
// View shape: what the screens read. Enums are decoded to labels here, once; a value the backend did not send is
// null, never a default.
// ---------------------------------------------------------------------------------------------------------------

export interface CompetitorEntryDto {
  readonly slot_id: number;
  readonly driver: string | null;
  readonly vehicle_name: string | null;
  readonly vehicle_filename: string | null;
  readonly class_name: string | null;
  readonly pit_group: string | null;
  readonly place: number | null;
  readonly class_place: number | null;
  readonly laps: number | null;
  readonly lap_dist_m: number | null;
  readonly sector: number | null;
  readonly best_lap_s: number | null;
  readonly last_lap_s: number | null;
  readonly best_sector1_s: number | null;
  readonly best_sector12_s: number | null;
  readonly last_sector1_s: number | null;
  readonly last_sector12_s: number | null;
  readonly time_behind_next_s: number | null;
  readonly laps_behind_next: number | null;
  readonly time_behind_leader_s: number | null;
  readonly laps_behind_leader: number | null;
  readonly in_pits: boolean | null;
  readonly in_garage_stall: boolean | null;
  /** Decoded pit state label ("None", "Entering", "Stopped in pit box", …, "Unknown (6)"). */
  readonly pit_state: string | null;
  readonly num_pitstops: number | null;
  readonly is_player: boolean;
  readonly pos_x_m: number | null;
  readonly pos_z_m: number | null;
}

/**
 * Session-wide caution derived from the raw yellow flag state and game phase: `fcy` = full-course yellow or
 * safety car (LMU does not tell the two apart), `red` = race halt / session stopped, `none` = measured green,
 * null = not known.
 */
export type CautionState = 'fcy' | 'red' | 'none';

export interface SessionScoringDto {
  readonly track_name: string | null;
  /** Decoded session label, as the core prints it: "Race 1", "Qualifying 2", "Warm-up", "Unknown (14)". */
  readonly session_type: string | null;
  readonly session_time_remaining_s: number | null;
  /** Session end time (`mEndET`), when it is a real end time. */
  readonly end_time_s: number | null;
  readonly lap_limit: number | null;
  /** Not sent by the backend (kept for older callers): always null from real data. */
  readonly time_limit_s: number | null;
  /** Decoded yellow flag label ("No caution", "Pending", "Pits closed", …); null when not known. */
  readonly yellow_flag_state: string | null;
  /** The raw `mYellowFlagState`. */
  readonly yellow_flag_raw?: number | null;
  readonly game_phase?: number | null;
  readonly caution?: CautionState | null;
  readonly sector_flags_raw?: readonly number[] | null;
  readonly dark_cloud?: number | null;
  /** Rain intensity 0..1; 0 only when 0 was measured. */
  readonly raining?: number | null;
  readonly ambient_temp_c?: number | null;
  readonly track_temp_c?: number | null;
}

export interface ScoringStateDto {
  readonly session: SessionScoringDto;
  readonly vehicles: readonly CompetitorEntryDto[];
}

// --- decoding (labels mirror the Rust `label()` / `Display` impls in enums.rs) ---------------------------------

const YELLOW_FLAG_LABELS: Readonly<Record<number, string>> = {
  [-1]: 'Invalid',
  0: 'No caution',
  1: 'Pending',
  2: 'Pits closed',
  3: 'Pit lead lap',
  4: 'Pits open',
  5: 'Last lap',
  6: 'Resume',
  7: 'Race halt',
};

const PIT_STATE_LABELS: Readonly<Record<number, string>> = {
  0: 'None',
  1: 'Requested',
  2: 'Entering',
  3: 'Stopped (header value, not observed)',
  4: 'Stopped in pit box',
  5: 'Leaving pit box / garage',
};

/** Raw yellow states of an active full-course yellow / safety car period (pending … resume). */
const YELLOW_FCY_FIRST = 1;
const YELLOW_FCY_LAST = 6;
const YELLOW_RACE_HALT = 7;
const PHASE_FULL_COURSE_YELLOW = 6;
const PHASE_SESSION_STOPPED = 7;

function num(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

function int(v: unknown): number | null {
  const n = num(v);
  return n !== null && Number.isInteger(n) ? n : null;
}

function str(v: unknown): string | null {
  return typeof v === 'string' ? v : null;
}

function bool(v: unknown): boolean | null {
  return typeof v === 'boolean' ? v : null;
}

function record(v: unknown): Readonly<Record<string, unknown>> | null {
  return typeof v === 'object' && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function enumLabel(raw: number | null, labels: Readonly<Record<number, string>>): string | null {
  if (raw === null) return null;
  return labels[raw] ?? `Unknown (${String(raw)})`;
}

/** `SessionType` Display (enums.rs): raw `mSession` to "Race 1", "Qualifying 2", "Warm-up", … */
export function sessionTypeLabel(raw: number | null): string | null {
  if (raw === null) return null;
  if (raw === 0) return 'Test day';
  if (raw >= 1 && raw <= 4) return `Practice ${String(raw)}`;
  if (raw >= 5 && raw <= 8) return `Qualifying ${String(raw - 4)}`;
  if (raw === 9) return 'Warm-up';
  if (raw >= 10 && raw <= 13) return `Race ${String(raw - 9)}`;
  return `Unknown (${String(raw)})`;
}

/** Session caution from the raw yellow flag state and game phase (see `CautionState`). */
export function cautionOf(yellowRaw: number | null, gamePhase: number | null): CautionState | null {
  if (yellowRaw === YELLOW_RACE_HALT || gamePhase === PHASE_SESSION_STOPPED) return 'red';
  if (
    (yellowRaw !== null && yellowRaw >= YELLOW_FCY_FIRST && yellowRaw <= YELLOW_FCY_LAST) ||
    gamePhase === PHASE_FULL_COURSE_YELLOW
  ) {
    return 'fcy';
  }
  if (yellowRaw === 0) return 'none';
  return null;
}

/** `lmu_session_time_remaining` (scoring.rs): `max(0, end − current)` only for a real end time. */
function timeRemainingCalc(endS: number | null, currentS: number | null): number | null {
  if (endS === null || endS <= 0 || currentS === null) return null;
  return Math.max(0, endS - currentS);
}

function toCompetitor(raw: unknown): CompetitorEntryDto | null {
  const v = record(raw);
  const slot = v === null ? null : int(v['slot_id']);
  if (v === null || slot === null) return null;
  return {
    slot_id: slot,
    driver: str(v['driver']),
    vehicle_name: str(v['vehicle_name']),
    vehicle_filename: str(v['vehicle_filename']),
    class_name: str(v['class_name']),
    pit_group: str(v['pit_group']),
    place: int(v['place']),
    class_place: int(v['class_place']),
    laps: int(v['laps']),
    lap_dist_m: num(v['lap_dist_m']),
    sector: int(v['sector']),
    best_lap_s: num(v['best_lap_s']),
    last_lap_s: num(v['last_lap_s']),
    best_sector1_s: num(v['best_sector1_s']),
    best_sector12_s: num(v['best_sector12_s']),
    last_sector1_s: num(v['last_sector1_s']),
    last_sector12_s: num(v['last_sector12_s']),
    time_behind_next_s: num(v['time_behind_next_s']),
    laps_behind_next: int(v['laps_behind_next']),
    time_behind_leader_s: num(v['time_behind_leader_s']),
    laps_behind_leader: int(v['laps_behind_leader']),
    in_pits: bool(v['in_pits']),
    in_garage_stall: bool(v['in_garage_stall']),
    pit_state: enumLabel(int(v['pit_state']), PIT_STATE_LABELS),
    num_pitstops: int(v['num_pitstops']),
    is_player: v['is_player'] === true,
    pos_x_m: num(v['pos_x_m']),
    pos_z_m: num(v['pos_z_m']),
  };
}

/**
 * Maps the `core:scoring` JSON (serde of `ScoringState`) to the view shape. Tolerates missing or mistyped fields
 * (they become null, never a crash or a default); returns null when the payload is not a scoring state at all.
 */
export function toScoringView(raw: unknown): ScoringStateDto | null {
  const root = record(raw);
  const s = root === null ? null : record(root['session']);
  if (root === null || s === null) return null;
  const weather: Readonly<Record<string, unknown>> = record(s['weather']) ?? {};
  const yellowRaw = int(s['yellow_flag_state']);
  const gamePhase = int(s['game_phase']);
  const endS = num(s['end_et_s']);
  const reported = num(s['time_remaining_s']);
  const sectorFlags = Array.isArray(s['sector_flags_raw'])
    ? (s['sector_flags_raw'] as unknown[]).map(int).filter((x): x is number => x !== null)
    : null;
  const vehicles = Array.isArray(root['vehicles'])
    ? (root['vehicles'] as unknown[]).map(toCompetitor).filter((x): x is CompetitorEntryDto => x !== null)
    : [];
  return {
    session: {
      track_name: str(s['track_name']),
      session_type: sessionTypeLabel(int(s['session_type'])),
      session_time_remaining_s:
        reported !== null && reported >= 0 ? reported : timeRemainingCalc(endS, num(s['current_et_s'])),
      end_time_s: endS !== null && endS > 0 ? endS : null,
      lap_limit: int(s['max_laps']),
      time_limit_s: null,
      yellow_flag_state: enumLabel(yellowRaw, YELLOW_FLAG_LABELS),
      yellow_flag_raw: yellowRaw,
      game_phase: gamePhase,
      caution: cautionOf(yellowRaw, gamePhase),
      sector_flags_raw: sectorFlags,
      dark_cloud: num(weather['dark_cloud_frac']),
      raining: num(weather['raining_frac']),
      ambient_temp_c: num(weather['ambient_temp_c']),
      track_temp_c: num(weather['track_temp_c']),
    },
    vehicles,
  };
}

/**
 * Hook to subscribe to `core:scoring` (5 Hz) emitted across the Tauri bridge, adapted to the view shape.
 * Cleanly unlistens on unmount.
 */
export function useScoring(): ScoringStateDto | null {
  const [scoring, setScoring] = useState<ScoringStateDto | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: UnlistenFn | null = null;
    let disposed = false;

    listen<unknown>('core:scoring', (event) => {
      if (!disposed) {
        const view = toScoringView(event.payload);
        if (view !== null) setScoring(view);
      }
    })
      .then((fn) => {
        if (disposed) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((err: unknown) => {
        console.warn('Failed to listen to core:scoring', err);
      });

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
        unlisten = null;
      }
    };
  }, []);

  return scoring;
}
