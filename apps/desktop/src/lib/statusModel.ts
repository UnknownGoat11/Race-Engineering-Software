/**
 * One truthful derivation of what every data source and status tile shows (REDESIGN.md §3.1 tile table, W1.1;
 * owner rules OR-1 "stale, synthetic, failed or ended data is never shown as live" and OR-2 "unknown is not 0").
 *
 * Pure functions only: inputs are the `CoreStatus` DTO (4 Hz), the telemetry store's freshness and origin, and a
 * small rolling history kept by the caller (`recordStatus`) so "in the last 60 s" rules can be judged. Nothing here
 * reads a clock; `nowMs` is always passed in.
 *
 * Truth values (`SourceTruth`) and their precedence (first match wins):
 *   incompatible > ended > failed > stale > unavailable > synthetic > live
 * `live` is only ever produced for data whose origin is known to be `lmu`; an unknown origin is not live.
 */
import type {
  CoreStatus,
  DropsDto,
  EngineerSlotDto,
  LoginStateDto,
  NetworkStatusDto,
  Origin,
} from './ipc';
import {
  engineerLinkLabel,
  lmuStateLabel,
  networkStateLabel,
  pipelineHealthLabel,
  recordingStateLabel,
  type PipelineHealthState,
} from './labels';
import { NO_VALUE, formatAgeShort, formatByteSize, formatCountdown, formatRtt, joinUnit } from './units';

// ---------------------------------------------------------------------------------------------------------
// Thresholds (named, overridable through DriverTileOptions)
// ---------------------------------------------------------------------------------------------------------

/** Window of the "in the last 60 s" rules (domain audit §3). */
export const ATTENTION_WINDOW_MS = 60_000;
/** More drops than this inside the window are "sustained" (critical). */
export const SUSTAINED_DROPS_PER_WINDOW = 10;
/** LMU telemetry nominal rate (GATE1_REPORT §2, A-LMU-001 VERIFIED ≈ 100 Hz). Overridable; never hard-coded in copy. */
export const NOMINAL_TELEMETRY_HZ = 100;
/** A read rate below this fraction of nominal is a warning. */
export const LOW_RATE_FRACTION = 0.9;
/** Streaming RTT bands (ms): below WARN ok, WARN..CRITICAL warning, above CRITICAL critical. */
export const RTT_WARN_MS = 150;
export const RTT_CRITICAL_MS = 400;
/** Reconnecting for at least this long is critical. */
export const RECONNECT_CRITICAL_MS = 30_000;
/** Recording disk-free bands (decimal GB, as the domain audit states them). */
export const DISK_WARN_BYTES = 20e9;
export const DISK_CRITICAL_BYTES = 5e9;
/** A code expiring in less than this is a warning. */
export const CODE_EXPIRY_WARN_S = 60;

// ---------------------------------------------------------------------------------------------------------
// Data-source truth
// ---------------------------------------------------------------------------------------------------------

export type SourceTruth = 'live' | 'stale' | 'unavailable' | 'synthetic' | 'failed' | 'ended' | 'incompatible';

/** The data sources the UI shows. */
export type DataSourceId = 'lmu' | 'recording' | 'streaming' | 'engineerStream';

export interface SourceStatus {
  readonly source: DataSourceId;
  readonly truth: SourceTruth;
  /** Provenance used for the verdict (null = unknown, never treated as LMU). */
  readonly origin: Origin | null;
  /** Age of the newest data in seconds, when known. */
  readonly ageS: number | null;
  /** Human reason / detail from the backend, when there is one. */
  readonly reason: string | null;
}

/**
 * Runtime check of a backend-provided origin: only the exact value 'lmu' counts (a missing or unexpected value
 * from an older/newer backend is not proof of LMU data).
 */
export function isLmuOrigin(origin: unknown): origin is 'lmu' {
  return origin === 'lmu';
}

/** True only for live data of LMU origin: the only case that may get a live look (green dot, LIVE word). */
export function isLiveTruth(status: SourceStatus): boolean {
  return status.truth === 'live' && status.origin === 'lmu';
}

/** Store freshness of the relevant frequency class (`TelemetryStore.getStreamState`). */
export type StoreFreshness = 'live' | 'degraded' | 'stale' | 'noData';

/** The parts of `TelemetryStoreStatus` the model needs. */
export interface StoreSnapshot {
  readonly state: 'waiting' | 'receiving' | 'rejected';
  readonly origin: Origin | null;
  readonly lastRejection:
    | { readonly kind: 'schemaMismatch'; readonly found: number; readonly expected: number }
    | { readonly kind: string }
    | null;
}

function sourceStatus(
  source: DataSourceId,
  truth: SourceTruth,
  origin: Origin | null,
  ageS: number | null = null,
  reason: string | null = null,
): SourceStatus {
  return { source, truth, origin, ageS, reason };
}

/** Driver-side LMU source. A LIVE state with a synthetic origin is the demo: `synthetic`, never `live`. */
export function lmuSourceTruth(status: CoreStatus | null): SourceStatus {
  if (status === null) return sourceStatus('lmu', 'unavailable', null);
  const origin = status.origin;
  const detail = status.lmu.detail;
  switch (status.lmu.state) {
    case 'ERROR':
      return sourceStatus('lmu', 'failed', origin, null, detail);
    case 'STALE':
    case 'PAUSED':
      return sourceStatus('lmu', 'stale', origin, null, detail);
    case 'LIVE':
      if (origin === 'synthetic') return sourceStatus('lmu', 'synthetic', origin, null, detail);
      return sourceStatus('lmu', isLmuOrigin(origin) ? 'live' : 'unavailable', origin, null, detail);
    default:
      // NOT_RUNNING, CONNECTING, UNKNOWN and states this build does not know.
      return sourceStatus('lmu', 'unavailable', origin, null, detail);
  }
}

/** Driver recording (local data capture; it records LMU data only, so "live" means writing now). */
export function recordingTruth(status: CoreStatus | null): SourceStatus {
  if (status === null) return sourceStatus('recording', 'unavailable', null);
  const r = status.recording;
  const origin = status.origin;
  if (origin === 'synthetic') return sourceStatus('recording', 'unavailable', origin, null, 'demo is not recorded');
  switch (r.state) {
    case 'active':
      return sourceStatus('recording', isLmuOrigin(origin) ? 'live' : 'unavailable', origin, null, null);
    case 'failed':
      return sourceStatus('recording', 'failed', origin, null, r.error);
    case 'suspended':
      return sourceStatus('recording', 'failed', origin, null, r.error ?? 'disk space critically low');
    default:
      return sourceStatus('recording', 'unavailable', origin, null, r.error);
  }
}

/** Driver streaming (relay link carrying LMU data). */
export function streamingTruth(status: CoreStatus | null): SourceStatus {
  if (status === null) return sourceStatus('streaming', 'unavailable', null);
  const n = status.network;
  const origin = status.origin;
  if (origin === 'synthetic') return sourceStatus('streaming', 'unavailable', origin, null, 'demo is not streamed');
  if (n.relogin_required || n.state === 'FAILED') {
    return sourceStatus('streaming', 'failed', origin, null, n.failure ?? n.last_error);
  }
  if (!n.streaming) return sourceStatus('streaming', 'unavailable', origin, null, null);
  switch (n.state) {
    case 'CONNECTED':
    case 'DEGRADED': {
      // Streaming with nothing to send (LMU not live, or 0 frames/s) is not live either.
      const lmu = lmuSourceTruth(status);
      if (lmu.truth !== 'live') return sourceStatus('streaming', 'stale', origin, null, 'no LMU data to send');
      if (status.rates.frames_sent_hz === 0) {
        return sourceStatus('streaming', 'stale', origin, null, 'no frames sent');
      }
      return sourceStatus('streaming', isLmuOrigin(origin) ? 'live' : 'unavailable', origin, null, n.degraded_reason);
    }
    case 'STALE':
    case 'RECONNECTING':
      return sourceStatus('streaming', 'stale', origin, null, n.degraded_reason ?? n.last_error);
    default:
      return sourceStatus('streaming', 'unavailable', origin, null, n.degraded_reason ?? n.last_error);
  }
}

/** Schema versions from a store schema-mismatch rejection, when that is why batches are refused. */
export function schemaMismatch(store: StoreSnapshot | null | undefined): { found: number; expected: number } | null {
  const rej = store?.lastRejection;
  if (store?.state !== 'rejected' || rej?.kind !== 'schemaMismatch') return null;
  if (!('found' in rej) || !('expected' in rej)) return null;
  return { found: rej.found, expected: rej.expected };
}

// ---------------------------------------------------------------------------------------------------------
// Engineer view
// ---------------------------------------------------------------------------------------------------------

export type EngineerView =
  /** No status received yet. */
  | { readonly kind: 'unavailable' }
  /** Not joined and the login for the engineer role is missing. */
  | { readonly kind: 'notSignedIn' }
  | { readonly kind: 'notConnected' }
  | { readonly kind: 'joining' }
  /** Identity card shown; no identity may appear elsewhere until confirmed (OR-3). */
  | { readonly kind: 'awaitingConfirmation' }
  /** Joined, nothing received yet. */
  | { readonly kind: 'waiting' }
  | { readonly kind: 'live'; readonly ageS: number | null; readonly degraded: boolean }
  | { readonly kind: 'synthetic'; readonly ageS: number | null }
  | {
      readonly kind: 'stale';
      readonly ageS: number | null;
      readonly reason: string | null;
      readonly synthetic: boolean;
    }
  | { readonly kind: 'away'; readonly sinceMs: number | null; readonly ageS: number | null; readonly synthetic: boolean }
  | { readonly kind: 'incompatible'; readonly versions: { readonly found: number; readonly expected: number } | null }
  | { readonly kind: 'ended'; readonly reason: string | null };

export interface EngineerViewInput {
  readonly status: CoreStatus | null;
  /** Login state for the engineer role; null = not known yet (does not block). */
  readonly login?: LoginStateDto | null;
  /**
   * Provenance of the received stream. In engineer mode `CoreStatus.origin` is always `lmu` (app-core
   * core.rs), so pass the telemetry store's origin; null = unknown (never live).
   */
  readonly origin: Origin | null;
  /** Store freshness of the physics class (or the class the screen relies on). */
  readonly storeFreshness?: StoreFreshness | null;
  readonly store?: StoreSnapshot | null;
  /** When `driver_away` first became true (see `nextSince`), for the "away since" banner. */
  readonly awaySinceMs?: number | null;
}

/** Tracks when a boolean condition started: the previous start while it holds, `nowMs` when it starts, else null. */
export function nextSince(previous: number | null, active: boolean, nowMs: number): number | null {
  if (!active) return null;
  return previous ?? nowMs;
}

export function deriveEngineerView(input: EngineerViewInput): EngineerView {
  const { status } = input;
  if (status === null) return { kind: 'unavailable' };
  const eng = status.engineer;
  const phase = eng?.phase ?? 'not_started';
  const signedOut = input.login !== undefined && input.login !== null && !input.login.authenticated;

  switch (phase) {
    case 'ended':
      return { kind: 'ended', reason: eng?.ended_reason ?? null };
    case 'joining':
      return { kind: 'joining' };
    case 'awaiting_confirmation':
      return { kind: 'awaitingConfirmation' };
    case 'monitoring':
      break;
    default:
      // idle, not_started and unknown phases: not watching anything.
      return signedOut ? { kind: 'notSignedIn' } : { kind: 'notConnected' };
  }

  // Monitoring: incompatible > away > stale > waiting > synthetic > live.
  const mismatch = schemaMismatch(input.store);
  if (eng?.incompatible_schema === true || mismatch !== null) {
    return { kind: 'incompatible', versions: mismatch };
  }
  const synthetic = input.origin === 'synthetic';
  const ageS = eng?.stream_age_s ?? null;
  if (eng?.driver_away === true || status.network.driver_away) {
    return { kind: 'away', sinceMs: input.awaySinceMs ?? null, ageS, synthetic };
  }
  const health = eng?.stream_health ?? 'no_data';
  const fresh = input.storeFreshness ?? null;
  if (health === 'stale' || fresh === 'stale') {
    return { kind: 'stale', ageS, reason: status.network.degraded_reason, synthetic };
  }
  if (health !== 'healthy' || fresh === 'noData') return { kind: 'waiting' };
  if (synthetic) return { kind: 'synthetic', ageS };
  if (input.origin !== 'lmu') {
    // Healthy data of unknown provenance: never live-looking (OR-1).
    return { kind: 'stale', ageS, reason: 'data source not confirmed', synthetic: false };
  }
  return { kind: 'live', ageS, degraded: fresh === 'degraded' };
}

/** True only for `live` (which deriveEngineerView produces only for LMU-origin data). */
export function isLiveLooking(view: EngineerView): boolean {
  return view.kind === 'live';
}

/** The data-source truth of an engineer view. */
export function engineerViewTruth(view: EngineerView): SourceTruth {
  switch (view.kind) {
    case 'live':
      return 'live';
    case 'synthetic':
      return 'synthetic';
    case 'stale':
    case 'away':
      return 'stale';
    case 'incompatible':
      return 'incompatible';
    case 'ended':
      return 'ended';
    default:
      return 'unavailable';
  }
}

/** The identity of the driver may be shown only after the engineer confirmed the identity card (OR-3). */
export function mayShowDriverIdentity(view: EngineerView): boolean {
  switch (view.kind) {
    case 'live':
    case 'synthetic':
    case 'stale':
    case 'away':
    case 'waiting':
    case 'incompatible':
      return true;
    default:
      return false;
  }
}

// ---------------------------------------------------------------------------------------------------------
// Rolling history (for "in the last 60 s" rules)
// ---------------------------------------------------------------------------------------------------------

export const DROP_KEYS = [
  'reader_overrun',
  'pipeline_overrun',
  'recorder_overrun',
  'net_dropped_obsolete',
  'net_events_dropped',
  'ui_dropped',
] as const satisfies readonly (keyof DropsDto)[];

export interface StatusSample {
  readonly atMs: number;
  readonly drops: DropsDto;
  readonly engineerIds: readonly number[];
}

export interface StatusHistory {
  /** Oldest first; one sample at or before `now − window` is kept as the baseline. */
  readonly samples: readonly StatusSample[];
  /** When the streaming link entered RECONNECTING (null while it is not reconnecting). */
  readonly reconnectingSinceMs: number | null;
}

export const EMPTY_HISTORY: StatusHistory = { samples: [], reconnectingSinceMs: null };

/** Adds `status` to the history (call once per status update) and trims samples outside the window. */
export function recordStatus(
  history: StatusHistory,
  status: CoreStatus | null,
  nowMs: number,
  windowMs: number = ATTENTION_WINDOW_MS,
): StatusHistory {
  if (status === null) return history;
  const sample: StatusSample = {
    atMs: nowMs,
    drops: status.drops,
    engineerIds: status.network.engineers.map((e) => e.connection_id),
  };
  const all = [...history.samples, sample];
  // Keep the newest sample at or before the window start (baseline) and everything after it.
  let first = 0;
  for (let i = 0; i < all.length; i += 1) {
    const s = all[i];
    if (s !== undefined && s.atMs <= nowMs - windowMs) first = i;
  }
  const reconnecting = status.network.streaming && status.network.state === 'RECONNECTING';
  return {
    samples: all.slice(first),
    reconnectingSinceMs: nextSince(history.reconnectingSinceMs, reconnecting, nowMs),
  };
}

/** Counter increase of `pick` between the window baseline and `current`. A counter that went down was reset. */
interface WindowDelta {
  readonly delta: number;
  /** How much of the window the history covers (ms). */
  readonly coveredMs: number;
}

function windowBaseline(history: StatusHistory, nowMs: number, windowMs: number): StatusSample | null {
  let baseline: StatusSample | null = null;
  for (const s of history.samples) {
    if (s.atMs <= nowMs - windowMs) baseline = s;
  }
  return baseline ?? history.samples[0] ?? null;
}

function counterDelta(
  current: number,
  history: StatusHistory,
  nowMs: number,
  windowMs: number,
  pick: (d: DropsDto) => number,
): WindowDelta {
  const base = windowBaseline(history, nowMs, windowMs);
  if (base === null) return { delta: 0, coveredMs: 0 };
  const before = pick(base.drops);
  const delta = current >= before ? current - before : current;
  return { delta, coveredMs: Math.min(windowMs, Math.max(0, nowMs - base.atMs)) };
}

function totalDrops(d: DropsDto): number {
  return DROP_KEYS.reduce((sum, k) => sum + d[k], 0);
}

// ---------------------------------------------------------------------------------------------------------
// Pipeline health
// ---------------------------------------------------------------------------------------------------------

export type Level = 'ok' | 'warning' | 'critical' | 'off' | 'synthetic';

export interface PipelineHealth {
  readonly state: PipelineHealthState;
  readonly level: Level;
  readonly word: string;
  readonly label: string;
  /** Drops inside the window, or null when there is no status. */
  readonly dropsInWindow: number | null;
  /** Total drops since start, per counter, or null when there is no status / nothing runs. */
  readonly totals: Readonly<Record<(typeof DROP_KEYS)[number], number>> | null;
  /** How much of the window the history covers (s). */
  readonly windowCoveredS: number;
}

/**
 * Pipeline health from ALL drop counters (not only reader overruns). Healthy only when nothing was ever dropped,
 * or when a full window of history shows no new drop. Drops of unknown time (history shorter than the window)
 * count as recent, so the label never says healthy too early. Nothing running (lmu state UNKNOWN: the core
 * reports its idle status with zeroed counters) is `not_running`, never healthy.
 */
export function pipelineHealth(
  status: CoreStatus | null,
  history: StatusHistory,
  nowMs: number,
  windowMs: number = ATTENTION_WINDOW_MS,
): PipelineHealth {
  const make = (
    state: PipelineHealthState,
    level: Level,
    dropsInWindow: number | null,
    totals: PipelineHealth['totals'],
    coveredMs: number,
  ): PipelineHealth => {
    const c = pipelineHealthLabel(state);
    return { state, level, word: c.word, label: c.label, dropsInWindow, totals, windowCoveredS: coveredMs / 1000 };
  };
  if (status === null) return make('unavailable', 'off', null, null, 0);
  if (status.lmu.state === 'UNKNOWN') return make('not_running', 'off', null, null, 0);
  const d = status.drops;
  const totals = Object.fromEntries(DROP_KEYS.map((k) => [k, d[k]])) as Record<(typeof DROP_KEYS)[number], number>;
  const total = totalDrops(d);
  const { delta, coveredMs } = counterDelta(total, history, nowMs, windowMs, totalDrops);
  if (total === 0) return make('healthy', 'ok', 0, totals, coveredMs);
  if (delta > SUSTAINED_DROPS_PER_WINDOW) return make('dropping_sustained', 'critical', delta, totals, coveredMs);
  if (delta > 0) return make('dropping', 'warning', delta, totals, coveredMs);
  // No new drop seen, but the history does not cover the whole window: the earlier drops may be recent.
  if (coveredMs < windowMs) return make('dropping', 'warning', delta, totals, coveredMs);
  return make('healthy', 'ok', 0, totals, coveredMs);
}

// ---------------------------------------------------------------------------------------------------------
// Driver tiles
// ---------------------------------------------------------------------------------------------------------

export type TileId = 'lmu' | 'recording' | 'streaming' | 'engineers' | 'code';

/** An action a tile or attention row offers. */
export type TileAction = 'sign_in' | 'open_recordings' | 'remove_engineer' | 'new_code';

export interface Tile {
  readonly id: TileId;
  readonly level: Level;
  /** Big state word (from labels.ts; never a raw enum). */
  readonly word: string;
  /** Detail line (rate, bytes, RTT, names …) or null. */
  readonly detail: string | null;
  /** Why the tile is warning/critical, or a helper line; null when nothing to say. */
  readonly reason: string | null;
  readonly action: TileAction | null;
}

export interface CodeTile extends Tile {
  readonly id: 'code';
  /** The engineer code, or null (render empty slots). Shown to the driver only. */
  readonly code: string | null;
  /** Seconds until the code expires, when known. */
  readonly expiresInS: number | null;
  /** All engineer slots are taken: a new code cannot be issued. */
  readonly sessionFull: boolean;
}

export interface AttentionItem {
  readonly id: string;
  readonly level: 'warning' | 'critical';
  readonly title: string;
  readonly detail: string | null;
  readonly action: TileAction | null;
}

export interface DriverTiles {
  readonly lmu: Tile;
  readonly recording: Tile;
  readonly streaming: Tile;
  readonly engineers: Tile;
  readonly code: CodeTile;
  readonly pipeline: PipelineHealth;
  /** Critical first, then warning; in tile order within a level. */
  readonly attention: readonly AttentionItem[];
}

export interface DriverTileOptions {
  readonly nowMs: number;
  /** Wall clock (Unix seconds) for the code countdown; default `nowMs / 1000`. */
  readonly nowUnixS?: number;
  readonly nominalTelemetryHz?: number;
  readonly windowMs?: number;
}

function tile(
  id: TileId,
  level: Level,
  word: string,
  detail: string | null = null,
  reason: string | null = null,
  action: TileAction | null = null,
): Tile {
  return { id, level, word, detail, reason, action };
}

function hz(value: number | null): string {
  return value === null || !Number.isFinite(value) ? NO_VALUE : joinUnit(String(Math.round(value)), 'Hz');
}

export function lmuTile(status: CoreStatus | null, nominalHz: number = NOMINAL_TELEMETRY_HZ): Tile {
  if (status === null) return tile('lmu', 'off', NO_VALUE, null, 'No status from the app yet.');
  const c = lmuStateLabel(status.lmu.state, status.origin);
  const detail = status.lmu.detail;
  if (status.origin === 'synthetic') {
    return tile('lmu', 'synthetic', 'DEMO', null, 'Display only — not recorded or streamed.');
  }
  switch (status.lmu.state) {
    case 'LIVE': {
      if (!isLmuOrigin(status.origin)) return tile('lmu', 'warning', c.word, null, c.helper);
      const rate = status.rates.telemetry_read_hz;
      if (rate !== null && Number.isFinite(rate) && rate < nominalHz * LOW_RATE_FRACTION) {
        return tile('lmu', 'warning', c.word, hz(rate), `Read rate below ${Math.round(nominalHz)} Hz nominal.`);
      }
      return tile('lmu', 'ok', c.word, hz(rate), null);
    }
    case 'CONNECTING':
    case 'PAUSED':
      return tile('lmu', 'warning', c.word, null, detail ?? c.helper);
    case 'STALE':
    case 'ERROR':
      return tile('lmu', 'critical', c.word, null, detail ?? c.helper);
    default:
      // NOT_RUNNING, UNKNOWN (checking) and unknown states: neutral.
      return tile('lmu', 'off', c.word, null, detail ?? c.helper);
  }
}

export function recordingTile(
  status: CoreStatus | null,
  history: StatusHistory = EMPTY_HISTORY,
  nowMs = 0,
  windowMs: number = ATTENTION_WINDOW_MS,
): Tile {
  if (status === null) return tile('recording', 'off', NO_VALUE, null, 'No status from the app yet.');
  const r = status.recording;
  // Demo: the band and the LMU tile already say "not recorded or streamed" (C04).
  if (status.origin === 'synthetic') return tile('recording', 'off', NO_VALUE, null, null);
  const c = recordingStateLabel(r.state);
  const free = r.disk_free_bytes;
  const freeText = free === null ? null : `${formatByteSize(free)} free`;
  switch (r.state) {
    case 'failed':
      return tile('recording', 'critical', c.word, freeText, r.error ?? c.label, 'open_recordings');
    case 'suspended':
      return tile('recording', 'critical', c.word, freeText, r.error ?? c.helper, 'open_recordings');
    case 'active': {
      const bytes = formatByteSize(r.bytes_written);
      const detail = freeText === null ? bytes : `${bytes} · ${freeText}`;
      // `drops.recorder_overrun` mirrors `recording.frames_lost` (app-core pipeline.rs); the history stores drops.
      const lost = status.drops.recorder_overrun;
      const { delta, coveredMs } = counterDelta(lost, history, nowMs, windowMs, (d) => d.recorder_overrun);
      if (free !== null && free < DISK_CRITICAL_BYTES) {
        return tile('recording', 'critical', c.word, detail, 'Disk almost full.', 'open_recordings');
      }
      if (delta > 0) return tile('recording', 'critical', c.word, detail, 'Recording dropped data.', 'open_recordings');
      if (lost > 0 && coveredMs < windowMs) {
        // Lost earlier, time unknown (history shorter than the window): not confirmed recent, still not OK.
        return tile('recording', 'warning', c.word, detail, 'Recording dropped data earlier.');
      }
      if (free !== null && free < DISK_WARN_BYTES) return tile('recording', 'warning', c.word, detail, 'Disk space low.');
      return tile('recording', 'ok', c.word, detail, null);
    }
    default:
      return tile('recording', 'off', c.word, freeText, c.helper);
  }
}

/** Streaming tile. No hard-coded rates: the detail shows the measured RTT / send rate or "—". */
export function streamingSummary(
  status: CoreStatus | null,
  history: StatusHistory = EMPTY_HISTORY,
  nowMs = 0,
): Tile {
  if (status === null) return tile('streaming', 'off', NO_VALUE, null, 'No status from the app yet.');
  const n: NetworkStatusDto = status.network;
  // Demo: no reason line, the band says it (C04).
  if (status.origin === 'synthetic') return tile('streaming', 'off', NO_VALUE, null, null);
  const c = networkStateLabel(n.state);
  const rtt = n.rtt_ms === null ? NO_VALUE : `RTT ${joinUnit(formatRtt(n.rtt_ms).trim(), 'ms')}`;
  if (n.relogin_required) {
    return tile('streaming', 'critical', 'FAILED', null, n.failure ?? 'Your login expired. Sign in again to stream.', 'sign_in');
  }
  switch (n.state) {
    case 'FAILED':
      return tile('streaming', 'critical', c.word, null, n.failure ?? n.last_error ?? c.label, null);
    case 'NOT_CONFIGURED':
      return tile('streaming', 'off', c.word, null, c.helper);
    case 'UNKNOWN':
      return tile('streaming', 'off', c.word, null, null);
    default:
      break;
  }
  if (!n.streaming) {
    const off = networkStateLabel('DISCONNECTED');
    return tile('streaming', 'off', off.word, null, off.helper);
  }
  switch (n.state) {
    case 'CONNECTING':
    case 'AUTHENTICATING':
      return tile('streaming', 'warning', c.word, null, null);
    case 'RECONNECTING': {
      const since = history.reconnectingSinceMs;
      const forMs = since === null ? 0 : nowMs - since;
      const reason = [n.degraded_reason, c.helper].filter((s): s is string => s !== null && s !== '').join(' · ');
      return tile('streaming', forMs >= RECONNECT_CRITICAL_MS ? 'critical' : 'warning', c.word, null, reason || null);
    }
    case 'STALE':
      return tile('streaming', 'warning', c.word, rtt, n.degraded_reason ?? c.label);
    case 'DEGRADED':
      return tile('streaming', 'warning', c.word, rtt, n.degraded_reason ?? c.label);
    case 'CONNECTED': {
      const lmu = lmuSourceTruth(status);
      if (lmu.truth !== 'live') return tile('streaming', 'warning', 'IDLE', rtt, 'No LMU data to send.');
      if (status.rates.frames_sent_hz === 0) return tile('streaming', 'warning', 'IDLE', rtt, 'No frames sent.');
      const r = n.rtt_ms;
      if (r !== null && r > RTT_CRITICAL_MS) return tile('streaming', 'critical', c.word, rtt, 'Very high latency.');
      if (r !== null && r >= RTT_WARN_MS) return tile('streaming', 'warning', c.word, rtt, 'High latency.');
      return tile('streaming', 'ok', c.word, rtt, null);
    }
    default:
      return tile('streaming', 'off', c.word, null, null);
  }
}

function engineerName(e: EngineerSlotDto): string {
  return e.display_name.trim() !== '' ? e.display_name : e.username;
}

export function engineersTile(
  status: CoreStatus | null,
  history: StatusHistory = EMPTY_HISTORY,
  nowMs = 0,
  windowMs: number = ATTENTION_WINDOW_MS,
): Tile {
  if (status === null) return tile('engineers', 'off', NO_VALUE, null, 'No status from the app yet.');
  const n = status.network;
  const max = Number.isInteger(n.engineers_max) && n.engineers_max > 0 ? String(n.engineers_max) : NO_VALUE;
  const word = `${n.engineers_connected} / ${max}`;
  if (status.origin === 'synthetic') return tile('engineers', 'off', NO_VALUE, null, null);
  if (n.state === 'NOT_CONFIGURED') return tile('engineers', 'off', word, null, null);
  const names = n.engineers.map((e) => `${engineerName(e)} · ${engineerLinkLabel(e.link).label}`).join(', ');
  const weak = n.engineers.filter((e) => e.link !== 'healthy');
  const base = windowBaseline(history, nowMs, windowMs);
  const nowIds = new Set(n.engineers.map((e) => e.connection_id));
  const dropped = base === null ? [] : history.samples.flatMap((s) => s.engineerIds).filter((id) => !nowIds.has(id));
  if (weak.length > 0) {
    return tile('engineers', 'warning', word, names, `${weak.map(engineerName).join(', ')}: ${engineerLinkLabel(weak[0]?.link).label.toLowerCase()}.`);
  }
  if (dropped.length > 0) return tile('engineers', 'warning', word, names || null, 'An engineer left in the last minute.');
  if (n.engineers_connected === 0) {
    return tile('engineers', 'off', word, null, n.streaming ? 'Waiting for engineer' : 'Start streaming to share.');
  }
  return tile('engineers', 'ok', word, names, null);
}

export function codeTile(status: CoreStatus | null, nowUnixS: number): CodeTile {
  const make = (
    level: Level,
    word: string,
    code: string | null,
    expiresInS: number | null,
    sessionFull: boolean,
    reason: string | null,
  ): CodeTile => ({ id: 'code', level, word, detail: null, reason, action: null, code, expiresInS, sessionFull });
  if (status === null) return make('off', NO_VALUE, null, null, false, 'No status from the app yet.');
  const n = status.network;
  if (status.origin === 'synthetic') return make('off', NO_VALUE, null, null, false, null);
  const full = n.engineers_max > 0 && n.engineers_connected >= n.engineers_max;
  if (full) {
    return make('off', 'SESSION FULL', null, null, true, `Session full (${n.engineers_connected}/${n.engineers_max}). Remove an engineer to add another.`);
  }
  const code = n.connect_code !== null && n.connect_code.trim() !== '' ? n.connect_code : null;
  if (code === null) {
    return make('off', NO_VALUE, null, null, false, n.streaming ? 'Request a new engineer code.' : 'Start streaming to get a code');
  }
  const expires = n.code_expires_at_unix_s;
  const left = expires === null ? null : expires - nowUnixS;
  if (left !== null && left <= 0) return make('warning', 'EXPIRED', null, 0, false, 'The code expired. Request a new one.');
  if (left !== null && left < CODE_EXPIRY_WARN_S) {
    return make('warning', code, code, left, false, `Expires in ${formatCountdown(left).trim()}`);
  }
  const reason = left === null ? 'Single use: one code per engineer.' : `Expires in ${formatCountdown(left).trim()} · single use`;
  return make('ok', code, code, left, false, reason);
}

/**
 * Detail line of the pipeline attention row. Never "0 dropped" (U12): when no new drop was seen but the history
 * does not cover the whole window yet, the earlier drops have no known time, so the line says so.
 */
export function pipelineAttentionDetail(pipeline: PipelineHealth, windowMs: number = ATTENTION_WINDOW_MS): string | null {
  const inWindow = pipeline.dropsInWindow;
  if (inWindow !== null && inWindow > 0) return `${inWindow} dropped in the last ${Math.round(windowMs / 1000)} s`;
  const total = pipeline.totals === null ? 0 : DROP_KEYS.reduce((sum, k) => sum + (pipeline.totals?.[k] ?? 0), 0);
  if (total > 0) return `${total} dropped since start (timing unknown)`;
  return null;
}

const TILE_TITLES: Readonly<Record<TileId, string>> = {
  lmu: 'LMU',
  recording: 'Recording',
  streaming: 'Streaming',
  engineers: 'Engineers',
  code: 'Engineer code',
};

/** The five driver tiles, pipeline health and the attention rows (worst first). */
export function deriveDriverTiles(
  status: CoreStatus | null,
  history: StatusHistory,
  options: DriverTileOptions,
): DriverTiles {
  const { nowMs } = options;
  const windowMs = options.windowMs ?? ATTENTION_WINDOW_MS;
  const lmu = lmuTile(status, options.nominalTelemetryHz ?? NOMINAL_TELEMETRY_HZ);
  const recording = recordingTile(status, history, nowMs, windowMs);
  const streaming = streamingSummary(status, history, nowMs);
  const engineers = engineersTile(status, history, nowMs, windowMs);
  const code = codeTile(status, options.nowUnixS ?? nowMs / 1000);
  const pipeline = pipelineHealth(status, history, nowMs, windowMs);

  const attention: AttentionItem[] = [];
  for (const t of [lmu, recording, streaming, engineers, code]) {
    if (t.level === 'warning' || t.level === 'critical') {
      attention.push({
        id: t.id,
        level: t.level,
        title: `${TILE_TITLES[t.id]}: ${t.word}`,
        detail: t.reason,
        action: t.action,
      });
    }
  }
  if (pipeline.level === 'warning' || pipeline.level === 'critical') {
    attention.push({
      id: 'pipeline',
      level: pipeline.level,
      title: pipeline.label,
      detail: pipelineAttentionDetail(pipeline, windowMs),
      action: null,
    });
  }
  attention.sort((a, b) => (a.level === b.level ? 0 : a.level === 'critical' ? -1 : 1));
  return { lmu, recording, streaming, engineers, code, pipeline, attention };
}

/** Age text for stale tags ("STALE 6.2 s"), "STALE" when the age is unknown. */
export function staleTag(ageS: number | null): string {
  const age = formatAgeShort(ageS);
  return age === NO_VALUE ? 'STALE' : `STALE ${age}`;
}
