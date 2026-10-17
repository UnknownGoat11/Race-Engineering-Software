/**
 * Staleness thresholds — the single place they are defined (spec §45 "stale detection", §126 "STALE STATE").
 *
 * Spec §126: the engineer application computes `age = now − last received sample` and shows
 *   LIVE < 250 ms ≤ DEGRADED ≤ 1 s < STALE,
 * and "these thresholds should be configurable" (TelemetryStore option `freshnessThresholds`).
 *
 * What "age" means here, per frequency class:
 * - Link age: the bridge sends deltas of complete frames (D-012, D-023), so a channel missing from a block is
 *   unchanged as of that block, and every live batch confirms every channel the stream has not lost (a gap
 *   leaves channels unconfirmed until re-sent; store.ts). The link age is the time since the newest live
 *   (non-BACKFILL) batch.
 * - Source age: a batch only proves the Rust core is still publishing, not that LMU is still refreshing its
 *   buffers. For classes fed by an LMU buffer the class age adds the source age the core reports in-band
 *   (`SOURCE_AGE_CHANNELS`: physics ← `source.telemetry_age_s`, scoring and environment ←
 *   `source.scoring_age_s`): age = link age + source age.
 * - While that source-age channel has no VALID, confirmed value (the Rust core does not publish it yet), the
 *   class age is the link age alone and the class's freshness basis is 'linkOnly'; stream indicators say so.
 *   The core may also mark a sample STALE in-band (quality STALE), which always shows STALE.
 */
import { ch, type FrequencyClass } from '../generated/channels';

/** Freshness of a confirmed sample. */
export type Freshness = 'live' | 'degraded' | 'stale';

/**
 * What a class's age covers: 'endToEnd' when it includes the LMU source age (or the class has no upstream
 * refresh to measure: static, event and diagnostic data), 'linkOnly' when only the link to the Rust core is
 * measured because the source age is not reported.
 */
export type FreshnessBasis = 'endToEnd' | 'linkOnly';

/** Age limits for one frequency class, in milliseconds. `Infinity` means "never". */
export interface FreshnessThreshold {
  /** At this age (inclusive) a sample becomes DEGRADED; `Infinity` goes from LIVE straight to STALE. */
  readonly degradedAfterMs: number;
  /** Above this age (exclusive) a sample becomes STALE. */
  readonly staleAfterMs: number;
}

/** Thresholds for every frequency class. */
export type FreshnessThresholds = Readonly<Record<FrequencyClass, FreshnessThreshold>>;

/** Spec §126: LIVE below 250 ms. */
export const SPEC_DEGRADED_AFTER_MS = 250;
/** Spec §126: STALE above 1 s. */
export const SPEC_STALE_AFTER_MS = 1_000;
/**
 * Environment data (weather, track conditions) changes over minutes and is sent at most once per second
 * (docs/PROTOCOL.md WEATHER_UPDATE); a value a few seconds old is still accurate, so it tolerates longer
 * stream stalls before being flagged.
 */
export const ENVIRONMENT_DEGRADED_AFTER_MS = 2_000;
export const ENVIRONMENT_STALE_AFTER_MS = 5_000;
/**
 * Static channels (per-session constants such as the vehicle name) do not change while the link is up, so
 * they are never DEGRADED; once the link has been silent for longer than the spec §126 STALE threshold they
 * are STALE (previously valid, not known to be current, spec §107): the session may have changed meanwhile.
 */
export const STATIC_STALE_AFTER_MS = SPEC_STALE_AFTER_MS;

const SPEC_126: FreshnessThreshold = {
  degradedAfterMs: SPEC_DEGRADED_AFTER_MS,
  staleAfterMs: SPEC_STALE_AFTER_MS,
};

/**
 * Default thresholds per frequency class. Physics, scoring, event and diagnostic channels use the spec §126
 * values verbatim (diagnostics describe the link's current health, so they must not look live during a
 * stall); environment data tolerates longer stalls; static data goes STALE when the link is lost.
 */
export const DEFAULT_FRESHNESS_THRESHOLDS: FreshnessThresholds = {
  Physics: SPEC_126,
  Scoring: SPEC_126,
  Environment: { degradedAfterMs: ENVIRONMENT_DEGRADED_AFTER_MS, staleAfterMs: ENVIRONMENT_STALE_AFTER_MS },
  Event: SPEC_126,
  Static: { degradedAfterMs: Number.POSITIVE_INFINITY, staleAfterMs: STATIC_STALE_AFTER_MS },
  Diagnostic: SPEC_126,
};

/**
 * The in-band source-age channel (seconds) whose value adds to a class's age. Every environment channel LMU
 * provides comes from the scoring buffer (generated registry), so it shares the scoring age. Classes not
 * listed have no upstream refresh to measure.
 */
export const SOURCE_AGE_CHANNELS: Readonly<Partial<Record<FrequencyClass, number>>> = {
  Physics: ch.SRC_TELEMETRY_AGE,
  Scoring: ch.SRC_SCORING_AGE,
  Environment: ch.SRC_SCORING_AGE,
};

/** Freshness of a sample confirmed `ageMs` ago (Infinity = never confirmed by a live batch). */
export function freshnessForAge(ageMs: number, threshold: FreshnessThreshold): Freshness {
  if (ageMs > threshold.staleAfterMs) return 'stale';
  if (Number.isFinite(threshold.degradedAfterMs) && ageMs >= threshold.degradedAfterMs) return 'degraded';
  return 'live';
}

/** Throws if a threshold is negative, NaN, or (when finite) degrades after it goes stale. */
export function validateThreshold(threshold: FreshnessThreshold): void {
  const { degradedAfterMs, staleAfterMs } = threshold;
  if (Number.isNaN(degradedAfterMs) || Number.isNaN(staleAfterMs) || degradedAfterMs < 0 || staleAfterMs < 0) {
    throw new RangeError('freshness thresholds must be non-negative numbers');
  }
  if (Number.isFinite(degradedAfterMs) && degradedAfterMs > staleAfterMs) {
    throw new RangeError('degradedAfterMs must not exceed staleAfterMs');
  }
}
