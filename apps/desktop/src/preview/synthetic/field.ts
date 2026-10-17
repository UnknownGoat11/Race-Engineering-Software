/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). SYNTHETIC race field for `core:scoring`.
 *
 * ~25 cars with invented driver names (no real player names, HANDOFF rule 3), constant per-car pace, a few pit
 * stops and lapped cars. The player's car follows the synthetic car model (car.ts) so its lap distance matches
 * the telemetry stream. Shapes are exactly `ScoringStateDto` / `CompetitorEntryDto` (src/hooks/useScoring.ts).
 */
import type { CompetitorEntryDto, ScoringStateDto } from '../../hooks/useScoring';
import { distanceAtTime, type LapProfile } from './car';
import { TRACK_LENGTH_M, TRACK_NAME, sectorAt, trackPosition } from './track';

interface FieldCar {
  readonly slot: number;
  readonly driver: string;
  readonly vehicle: string;
  readonly className: string;
  /** Lap time pace (s). */
  readonly paceS: number;
  /** Race progress (laps, fractional) at the reference time. */
  readonly startProgressLaps: number;
  readonly pitstops: number;
  readonly inPits: boolean;
  readonly isPlayer: boolean;
}

/** Invented names (deliberately generic; not taken from any recording). */
const FIRST = ['A.', 'B.', 'C.', 'D.', 'E.', 'F.', 'G.', 'H.', 'J.', 'K.', 'L.', 'M.', 'N.', 'P.', 'R.', 'S.', 'T.', 'V.'];
const LAST = [
  'Varga', 'Lindqvist', 'Moreau', 'Okafor', 'Castell', 'Brandt', 'Halloran', 'Navarro', 'Ishikawa', 'Petrescu',
  'Delacroix', 'Kowal', 'Ferreira', 'Halvorsen', 'Mancini', 'Duarte', 'Novak', 'Achterberg', 'Sorensen', 'Ruiz',
  'Whitlock', 'Ambrose', 'Keller', 'Oyelaran', 'Tremblay', 'Vasquez', 'Lund',
];

const GT3_MODELS = [
  'Porsche 911 GT3 R LMGT3', 'Ferrari 296 LMGT3', 'BMW M4 LMGT3', 'Aston Martin Vantage AMR LMGT3',
  'Lexus RC F LMGT3', 'Ford Mustang LMGT3', 'Corvette Z06 LMGT3.R', 'McLaren 720S LMGT3 Evo',
  'Lamborghini Huracan LMGT3 Evo2', 'Mercedes-AMG LMGT3',
];
const HYPERCAR_MODELS = [
  'Toyota GR010 Hybrid', 'Ferrari 499P', 'Porsche 963', 'Cadillac V-Series.R', 'BMW M Hybrid V8', 'Alpine A424',
  'Peugeot 9X8',
];
const LMP2_MODELS = ['Oreca 07 Gibson'];

export type FieldKind = 'gt3' | 'mixed';

export interface FieldOptions {
  readonly kind: FieldKind;
  readonly playerVehicle: string;
  readonly playerClass: string;
  readonly playerDriver: string;
  /** Player overall position at the reference time (1-based). */
  readonly playerPlace: number;
  readonly playerProfile: LapProfile;
  /** Laps the player has completed at the reference time. */
  readonly playerLapsDone: number;
  readonly sessionType: string;
  readonly raceLengthS: number;
  /** Session time (s) at the reference time (`snapshot(0)`). */
  readonly referenceSessionTimeS: number;
}

function carNumber(i: number): number {
  return [92, 31, 55, 27, 78, 88, 81, 59, 85, 60, 77, 54, 44, 95, 46, 87, 33, 61, 91, 23, 10, 12, 71, 18, 64, 6][i] ?? 100 + i;
}

export class SyntheticField {
  readonly #cars: FieldCar[] = [];
  readonly #opts: FieldOptions;

  constructor(opts: FieldOptions) {
    this.#opts = opts;
    const total = 25;
    const playerLapS = opts.playerProfile.lapTimeS;
    for (let i = 0; i < total; i++) {
      const place = i + 1;
      const isPlayer = place === opts.playerPlace;
      let className = 'GT3';
      let models = GT3_MODELS;
      let paceBase = playerLapS;
      if (opts.kind === 'mixed') {
        if (i < 9) {
          className = 'Hypercar';
          models = HYPERCAR_MODELS;
        } else if (i < 13) {
          className = 'LMP2';
          models = LMP2_MODELS;
          paceBase = playerLapS + 6.5;
        } else {
          className = 'GT3';
          models = GT3_MODELS;
          paceBase = playerLapS + 13;
        }
      }
      const model = isPlayer ? opts.playerVehicle : (models[i % models.length] ?? 'Unknown');
      const gapS = (place - opts.playerPlace) * 3.4 + (place > 20 ? 50 : 0);
      const lapped = place >= 22 ? 1 : 0;
      this.#cars.push({
        slot: i + 3,
        driver: isPlayer ? opts.playerDriver : `${FIRST[(i * 7) % FIRST.length] ?? 'A.'} ${LAST[(i * 5) % LAST.length] ?? 'Driver'}`,
        vehicle: `#${carNumber(i)} ${model}`,
        className: isPlayer ? opts.playerClass : className,
        paceS: isPlayer ? playerLapS : paceBase + (place - opts.playerPlace) * 0.11 + ((i * 37) % 7) * 0.05,
        startProgressLaps: opts.playerLapsDone - gapS / playerLapS - lapped,
        pitstops: place % 5 === 0 ? 2 : 1,
        inPits: place === 17,
        isPlayer,
      });
    }
  }

  /** Scoring state `elapsedS` seconds after the reference time. */
  snapshot(elapsedS: number): ScoringStateDto {
    const L = TRACK_LENGTH_M;
    const prof = this.#opts.playerProfile;
    // Every car follows the player's reference lap profile, time-scaled to its own pace, so positions on track
    // (and therefore the running order) are consistent with the player's telemetry stream.
    const positions = this.#cars.map((car) => {
      const rate = car.inPits ? 0.02 : 1;
      const progress = car.startProgressLaps + (elapsedS / car.paceS) * rate;
      const laps = Math.floor(progress);
      const dist = laps * L + distanceAtTime(prof, (progress - laps) * prof.lapTimeS);
      return { car, dist };
    });
    positions.sort((a, b) => b.dist - a.dist);
    const leader = positions[0]?.dist ?? 0;
    const classCount = new Map<string, number>();
    const vehicles: CompetitorEntryDto[] = positions.map(({ car, dist }, idx) => {
      const classPlace = (classCount.get(car.className) ?? 0) + 1;
      classCount.set(car.className, classPlace);
      const ahead = idx > 0 ? (positions[idx - 1]?.dist ?? dist) : dist;
      const lapDist = ((dist % L) + L) % L;
      const laps = Math.floor(dist / L);
      const lapsBehindLeader = Math.floor(leader / L) - laps > 0 && leader - dist > L ? Math.floor((leader - dist) / L) : 0;
      const pos = trackPosition(lapDist);
      const best = car.paceS - 0.35 - ((car.slot * 13) % 5) * 0.07;
      return {
        slot_id: car.slot,
        driver: car.driver,
        vehicle_name: car.vehicle,
        vehicle_filename: null,
        class_name: car.className,
        pit_group: null,
        place: idx + 1,
        class_place: classPlace,
        laps,
        lap_dist_m: lapDist,
        sector: sectorAt(lapDist) + 1,
        best_lap_s: best,
        last_lap_s: car.inPits ? car.paceS + 38.2 : car.paceS + ((car.slot * 7) % 9) * 0.06,
        best_sector1_s: best * 0.33,
        best_sector12_s: best * 0.68,
        last_sector1_s: car.paceS * 0.331,
        last_sector12_s: car.paceS * 0.684,
        time_behind_next_s: idx === 0 ? 0 : ((ahead - dist) / L) * car.paceS,
        laps_behind_next: idx === 0 ? 0 : Math.floor((ahead - dist) / L),
        time_behind_leader_s: idx === 0 ? 0 : ((leader - dist) / L) * car.paceS,
        laps_behind_leader: lapsBehindLeader,
        in_pits: car.inPits,
        in_garage_stall: false,
        pit_state: car.inPits ? 'STOPPED' : 'NONE',
        num_pitstops: car.pitstops,
        is_player: car.isPlayer,
        pos_x_m: pos.x,
        pos_z_m: pos.z,
      };
    });
    return {
      session: {
        track_name: TRACK_NAME,
        session_type: this.#opts.sessionType,
        session_time_remaining_s: Math.max(0, this.#opts.raceLengthS - this.#opts.referenceSessionTimeS - elapsedS),
        end_time_s: this.#opts.raceLengthS,
        lap_limit: null,
        time_limit_s: this.#opts.raceLengthS,
        yellow_flag_state: 'NONE',
        sector_flag: ['GREEN', 'GREEN', 'GREEN'],
        dark_cloud: 0.1,
        raining: 0,
        ambient_temp_c: 29,
        track_temp_c: 41,
      },
      vehicles,
    };
  }
}
