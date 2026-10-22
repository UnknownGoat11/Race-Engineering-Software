// `core:scoring` adapter (audit U02): the JSON the backend really sends (serde of the Rust `ScoringState`,
// crates/telemetry-model/src/scoring.rs + enums.rs: raw LMU enum values, `time_remaining_s`, nested `weather`) maps
// to the view shape the screens read. The fixture is built field by field from the Rust struct definitions; the
// names are invented.
import { describe, expect, it } from 'vitest';
import { SyntheticField } from '../preview/synthetic/field';
import { buildLapProfile } from '../preview/synthetic/car';
import { cautionOf, sessionTypeLabel, toScoringView, type RawScoringState } from './useScoring';

/** `serde_json::to_value(ScoringState)` of a race under a pending full-course yellow. */
const REAL_SCORING_JSON = `{
  "session": {
    "track_name": "Sebring International Raceway",
    "session_type": 10,
    "game_phase": 6,
    "yellow_flag_state": 1,
    "sector_flags_raw": [1, 1, 11],
    "start_light": null,
    "num_red_lights": 5,
    "num_vehicles": 2,
    "current_et_s": 4210.5,
    "end_et_s": 10800.0,
    "time_remaining_s": 6589.5,
    "max_laps": null,
    "track_length_m": 6019.0,
    "in_realtime": true,
    "game_mode": 2,
    "max_players": 32,
    "time_of_day_s": 50400.0,
    "fixed_setup": false,
    "weather": {
      "ambient_temp_c": 27.4,
      "track_temp_c": 38.9,
      "raining_frac": 0.0,
      "dark_cloud_frac": 0.2,
      "cloud_coverage_raw": 3,
      "path_wetness_min_frac": 0.0,
      "path_wetness_max_frac": 0.0,
      "path_wetness_avg_frac": 0.0,
      "wind_mps": [1.0, 0.0, -2.0],
      "track_grip_level_raw": 2
    }
  },
  "vehicles": [
    {
      "slot_id": 4, "driver": "A. Tester", "vehicle_name": "#92 Porsche 911 GT3 R LMGT3",
      "vehicle_filename": null, "class_name": "LMGT3", "pit_group": "Team One", "place": 1, "class_place": 1,
      "laps": 41, "lap_dist_m": 1234.5, "sector": 1, "best_lap_s": 118.234, "last_lap_s": 119.01,
      "best_sector1_s": 38.1, "best_sector12_s": 80.2, "last_sector1_s": 38.4, "last_sector12_s": 80.9,
      "cur_sector1_s": null, "cur_sector12_s": null, "time_behind_next_s": 0.0, "laps_behind_next": 0,
      "time_behind_leader_s": 0.0, "laps_behind_leader": 0, "time_into_lap_s": 12.3, "estimated_lap_s": 119.0,
      "lap_start_et_s": 4198.2, "in_pits": false, "in_garage_stall": false, "pit_state": 0, "num_pitstops": 1,
      "num_penalties": 0, "finish_status": 0, "flag": 0, "under_yellow": true, "individual_phase": 6,
      "count_lap_flag": 2, "control": 2, "is_player": false, "qualification": 3, "energy_or_fuel_frac": 0.41,
      "pos_x_m": 120.5, "pos_z_m": -340.25
    },
    {
      "slot_id": 9, "driver": "B. Example", "vehicle_name": "#31 Ferrari 296 LMGT3",
      "vehicle_filename": null, "class_name": "LMGT3", "pit_group": null, "place": 2, "class_place": 2,
      "laps": 41, "lap_dist_m": null, "sector": null, "best_lap_s": null, "last_lap_s": null,
      "best_sector1_s": null, "best_sector12_s": null, "last_sector1_s": null, "last_sector12_s": null,
      "cur_sector1_s": null, "cur_sector12_s": null, "time_behind_next_s": 1.42, "laps_behind_next": 0,
      "time_behind_leader_s": 1.42, "laps_behind_leader": 0, "time_into_lap_s": null, "estimated_lap_s": null,
      "lap_start_et_s": null, "in_pits": true, "in_garage_stall": false, "pit_state": 4, "num_pitstops": 2,
      "num_penalties": 0, "finish_status": 0, "flag": 6, "under_yellow": true, "individual_phase": 6,
      "count_lap_flag": 2, "control": 0, "is_player": true, "qualification": 5, "energy_or_fuel_frac": null,
      "pos_x_m": null, "pos_z_m": null
    }
  ]
}`;

describe('toScoringView (real core:scoring JSON)', () => {
  const view = toScoringView(JSON.parse(REAL_SCORING_JSON) as unknown);

  it('maps the session block: decoded session type, time remaining, nested weather', () => {
    expect(view).not.toBeNull();
    const s = view?.session;
    expect(s?.track_name).toBe('Sebring International Raceway');
    expect(s?.session_type).toBe('Race 1');
    expect(s?.session_time_remaining_s).toBe(6589.5);
    expect(s?.end_time_s).toBe(10800);
    expect(s?.lap_limit).toBeNull();
    expect(s?.ambient_temp_c).toBe(27.4);
    expect(s?.track_temp_c).toBe(38.9);
    expect(s?.raining).toBe(0);
    expect(s?.dark_cloud).toBe(0.2);
    expect(s?.sector_flags_raw).toEqual([1, 1, 11]);
  });

  it('decodes the raw i8 yellow flag state (never calls string methods on a number) and derives FCY', () => {
    expect(view?.session.yellow_flag_state).toBe('Pending');
    expect(view?.session.yellow_flag_raw).toBe(1);
    expect(view?.session.caution).toBe('fcy');
  });

  it('maps the vehicles, decoding the raw pit state', () => {
    expect(view?.vehicles).toHaveLength(2);
    const [a, b] = view?.vehicles ?? [];
    expect(a?.slot_id).toBe(4);
    expect(a?.vehicle_name).toBe('#92 Porsche 911 GT3 R LMGT3');
    expect(a?.pit_state).toBe('None');
    expect(a?.pos_x_m).toBe(120.5);
    expect(b?.is_player).toBe(true);
    expect(b?.pit_state).toBe('Stopped in pit box');
    expect(b?.lap_dist_m).toBeNull();
    expect(b?.best_lap_s).toBeNull();
  });

  it('missing values stay null (no invented defaults); a non-scoring payload is rejected', () => {
    const bare = toScoringView({ session: { weather: {} }, vehicles: [] });
    expect(bare?.session).toMatchObject({
      session_type: null,
      session_time_remaining_s: null,
      yellow_flag_state: null,
      caution: null,
      raining: null,
      ambient_temp_c: null,
      track_temp_c: null,
    });
    expect(toScoringView(null)).toBeNull();
    expect(toScoringView('nope')).toBeNull();
    // A mistyped field never throws.
    expect(toScoringView({ session: { yellow_flag_state: 'fcy', weather: null }, vehicles: [{ slot_id: 'x' }] }))
      .toMatchObject({ session: { yellow_flag_state: null }, vehicles: [] });
  });

  it('falls back to end − current for the time remaining, only with a real end time', () => {
    const at = (end: number | null, current: number | null): number | null =>
      toScoringView({ session: { end_et_s: end, current_et_s: current, weather: {} }, vehicles: [] })?.session
        .session_time_remaining_s ?? null;
    expect(at(3600, 100)).toBe(3500);
    expect(at(3600, 3700)).toBe(0);
    expect(at(0, 100)).toBeNull();
    expect(at(3600, null)).toBeNull();
  });
});

describe('decoders', () => {
  it('session type labels follow the Rust SessionType Display', () => {
    expect([0, 1, 4, 5, 6, 9, 10, 13, 14].map(sessionTypeLabel)).toEqual([
      'Test day', 'Practice 1', 'Practice 4', 'Qualifying 1', 'Qualifying 2', 'Warm-up', 'Race 1', 'Race 4',
      'Unknown (14)',
    ]);
    expect(sessionTypeLabel(null)).toBeNull();
  });

  it('caution: raw yellow 1..6 or phase 6 = fcy, 7 = red, measured 0 = none, unknown = null', () => {
    for (const raw of [1, 2, 3, 4, 5, 6]) expect(cautionOf(raw, null)).toBe('fcy');
    expect(cautionOf(0, 6)).toBe('fcy');
    expect(cautionOf(7, null)).toBe('red');
    expect(cautionOf(0, 7)).toBe('red');
    expect(cautionOf(0, 5)).toBe('none');
    expect(cautionOf(-1, null)).toBeNull();
    expect(cautionOf(null, null)).toBeNull();
  });
});

describe('preview field emits the real backend shape (H03)', () => {
  it('round-trips through JSON and the adapter like real data', () => {
    const field = new SyntheticField({
      kind: 'gt3',
      playerVehicle: 'Porsche 911 GT3 R LMGT3',
      playerClass: 'GT3',
      playerDriver: 'Sam Driver',
      playerPlace: 3,
      playerProfile: buildLapProfile('gt3'),
      playerLapsDone: 10,
      sessionType: 'Race 1',
      raceLengthS: 14_400,
      referenceSessionTimeS: 3_600,
      fcy: true,
    });
    const raw: RawScoringState = field.snapshot(10);
    expect(typeof raw.session.yellow_flag_state).toBe('number');
    expect(typeof raw.session.session_type).toBe('number');
    const view = toScoringView(JSON.parse(JSON.stringify(raw)) as unknown);
    expect(view?.session.session_type).toBe('Race 1');
    expect(view?.session.caution).toBe('fcy');
    expect(view?.session.session_time_remaining_s).toBe(14_400 - 3_610);
    expect(view?.vehicles).toHaveLength(25);
    expect(view?.vehicles.every((v) => v.pit_state === 'None' || v.pit_state === 'Stopped in pit box')).toBe(true);
  });
});
