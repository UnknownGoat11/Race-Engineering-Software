/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). Shared timeline of the SYNTHETIC race every engineer
 * and driver scenario shows: a 4 h race, page loaded in the final stint (~23 min to go), so fuel-to-finish and the
 * safety margin are meaningful numbers rather than a multi-stint deficit.
 */
import { buildLapProfile, type CarVariant } from './car';

/** Race length (s). */
export const RACE_LENGTH_S = 4 * 3600;
/** Session time (s) at page load. */
export const SESSION_TIME_AT_LOAD_S = 13_020;
/** Seconds of history the telemetry subscription backfills (the live window is 60 s). */
export const BACKFILL_S = 60;
/** Session time (s) of the first backfilled telemetry row (the player crosses the line there). */
export const STREAM_START_S = SESSION_TIME_AT_LOAD_S - BACKFILL_S;
/** Laps of the current stint already completed at the first backfilled row. */
export const STINT_LAPS_DONE = 9;
/** Stint number of the current stint. */
export const CURRENT_STINT = 9;

export interface RaceTimeline {
  readonly lapTimeS: number;
  /** Laps the player completed before the first backfilled row. */
  readonly startLap: number;
  /** Laps the race will run for the player (time-certain race: the lap after the clock expires is the last). */
  readonly raceLaps: number;
  /** Session time (s) at which the current stint started. */
  readonly stintStartS: number;
  /** Player's personal best (s). */
  readonly bestLapS: number;
}

const cache = new Map<CarVariant, RaceTimeline>();

export function raceTimeline(variant: CarVariant): RaceTimeline {
  const cached = cache.get(variant);
  if (cached !== undefined) return cached;
  const lapTimeS = buildLapProfile(variant).lapTimeS;
  // Average race lap including pit stops and the start is a little slower than the reference lap.
  const startLap = Math.floor(STREAM_START_S / (lapTimeS * 1.035));
  const raceLaps = startLap + Math.ceil((RACE_LENGTH_S - STREAM_START_S) / lapTimeS);
  const timeline: RaceTimeline = {
    lapTimeS,
    startLap,
    raceLaps,
    stintStartS: STREAM_START_S - STINT_LAPS_DONE * lapTimeS,
    bestLapS: lapTimeS - 0.214,
  };
  cache.set(variant, timeline);
  return timeline;
}

/** `m:ss.mmm` (for alert texts). */
export function formatLap(s: number): string {
  const m = Math.floor(s / 60);
  const rest = s - m * 60;
  return `${m}:${rest.toFixed(3).padStart(6, '0')}`;
}

/** Which part of a session a scenario's stream shows. */
export type TimelineKind = 'final-stint' | 'race-start' | 'qualifying';

/** Qualifying session length (s) and the session time at page load of a qualifying scenario. */
export const QUALI_LENGTH_S = 20 * 60;
const QUALI_TIME_AT_LOAD_S = 11 * 60;
/** Session time (s) at which the player crosses the line at the rolling start (after the formation lap). */
const RACE_START_LINE_S = 180;

/** Fraction of a lap already driven at the first backfilled row (final stint), per car (D14: lap clocks differ). */
const FINAL_STINT_LAP_FRACTION: Readonly<Record<CarVariant, number>> = { gt3: 0.27, hypercar: 0.58 };

export interface StreamTimeline {
  readonly kind: TimelineKind;
  readonly lapTimeS: number;
  /** Seconds of history the subscription backfills. */
  readonly backfillS: number;
  /** Session time (s) of the first backfilled row. */
  readonly streamStartS: number;
  /** Session time (s) at page load (= streamStartS + backfillS). */
  readonly sessionTimeAtLoadS: number;
  /** Laps completed before the first backfilled row. */
  readonly startLap: number;
  /** Seconds into the current lap at the first backfilled row. */
  readonly lapOffsetS: number;
  /** Race laps for the player (race timelines; NaN in qualifying). */
  readonly raceLaps: number;
  /** Session length (s). */
  readonly sessionLengthS: number;
  readonly bestLapS: number;
  readonly stint: number;
  /** Laps of the current stint completed at the first backfilled row. */
  readonly stintLapsDone: number;
  readonly stintStartS: number;
  /** Core session type display string (`SessionType` Display in telemetry-model: "Race 1", "Qualifying 1"). */
  readonly sessionType: string;
}

/**
 * Timeline of one scenario's stream. `final-stint` is the shared 4 h race timeline above; `race-start` begins on
 * the player's first crossing of the line after the rolling start (no lap of history yet unless `backfillS`
 * covers one); `qualifying` is the middle of a 20 min qualifying session.
 */
export function streamTimeline(variant: CarVariant, kind: TimelineKind = 'final-stint', backfillS?: number): StreamTimeline {
  const race = raceTimeline(variant);
  const lapTimeS = race.lapTimeS;
  switch (kind) {
    case 'final-stint': {
      const back = backfillS ?? BACKFILL_S;
      const streamStartS = SESSION_TIME_AT_LOAD_S - back;
      return {
        kind,
        lapTimeS,
        backfillS: back,
        streamStartS,
        sessionTimeAtLoadS: SESSION_TIME_AT_LOAD_S,
        startLap: race.startLap,
        lapOffsetS: FINAL_STINT_LAP_FRACTION[variant] * lapTimeS,
        raceLaps: race.raceLaps,
        sessionLengthS: RACE_LENGTH_S,
        bestLapS: race.bestLapS,
        stint: CURRENT_STINT,
        stintLapsDone: STINT_LAPS_DONE,
        stintStartS: race.stintStartS,
        sessionType: 'Race 1',
      };
    }
    case 'race-start': {
      const back = backfillS ?? 25;
      return {
        kind,
        lapTimeS,
        backfillS: back,
        streamStartS: RACE_START_LINE_S,
        sessionTimeAtLoadS: RACE_START_LINE_S + back,
        startLap: 0,
        lapOffsetS: 0,
        raceLaps: Math.ceil(RACE_LENGTH_S / lapTimeS),
        sessionLengthS: RACE_LENGTH_S,
        bestLapS: Number.NaN,
        stint: 1,
        stintLapsDone: 0,
        stintStartS: RACE_START_LINE_S,
        sessionType: 'Race 1',
      };
    }
    case 'qualifying': {
      const back = backfillS ?? BACKFILL_S;
      return {
        kind,
        lapTimeS,
        backfillS: back,
        streamStartS: QUALI_TIME_AT_LOAD_S - back,
        sessionTimeAtLoadS: QUALI_TIME_AT_LOAD_S,
        startLap: 3,
        lapOffsetS: 0.41 * lapTimeS,
        raceLaps: Number.NaN,
        sessionLengthS: QUALI_LENGTH_S,
        bestLapS: race.bestLapS - 0.65,
        stint: 2,
        stintLapsDone: 1,
        stintStartS: QUALI_TIME_AT_LOAD_S - back - (1 + 0.41) * lapTimeS,
        sessionType: 'Qualifying 1',
      };
    }
  }
}
