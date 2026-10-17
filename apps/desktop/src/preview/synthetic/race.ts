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
