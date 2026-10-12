/**
 * TelemetryStore — the external (non-React) store behind every live telemetry view (spec §121–126, D-011,
 * D-023).
 *
 * - `applyBatch(bytes)` takes one UiBatch (uiBatch.ts) exactly as the Rust bridge sent it. Every block is
 *   decoded and checked against this build's schema BEFORE anything is applied: any envelope error, TLV error
 *   or schema mismatch rejects the whole batch (an error state, never a partial apply). Records are applied
 *   like `TelemetryFrame::apply_tlv`: unknown channel ids are skipped, records that contradict the registry
 *   (wrong type, VALID without a value, VALID non-finite float) become INVALID, non-VALID records carry no
 *   value.
 * - Per channel it keeps the latest sample (value, quality, the TLV header timestamps of the block that set
 *   it, local receive time); provenance and units come from the generated channel metadata.
 * - The live ring (ring.ts) holds `liveWindowS` seconds (spec §63) on the LMU session-time axis: every block
 *   with a finite source ET appends one row of the current numeric values (D-023 §5). RESET clears it,
 *   GAP_BEFORE and batch_seq jumps insert a gap row, an ET regression (new session) clears it.
 * - The bridge sends deltas, so a gap (GAP_BEFORE or a batch_seq jump without RESET) may have lost changes:
 *   every cached sample is then unconfirmed — STALE, and NaN in new ring rows — until a record re-confirms it
 *   or a RESET rebuilds the state; batch reports say `resyncNeeded` meanwhile and `connectBatchSource`
 *   resubscribes (RESET + BACKFILL).
 * - Subscribers (per channel, per channel set, stream state, status, ring) are notified at most once per
 *   animation frame; snapshots are immutable and replaced only when what they show changes, so
 *   `useSyncExternalStore` re-renders nothing when nothing changed.
 * - Staleness (freshness.ts) is evaluated against an injectable clock, on every batch and by a timer that is
 *   always due at the next transition: per frequency class, age = link age + reported LMU source age.
 *
 * The store never invents values: a channel without a record since the last reset is NO DATA, and only
 * VALID, fresh samples expose a value.
 */
import {
  ALL_FREQUENCY_CLASSES,
  Availability,
  CHANNELS,
  FrequencyClass,
  Quality,
  RELAY_NS_UNKNOWN,
  TELEMETRY_SCHEMA_VERSION,
  ValueType,
  type ChannelDef,
} from '../generated/channels';
import {
  DEFAULT_FRESHNESS_THRESHOLDS,
  SOURCE_AGE_CHANNELS,
  freshnessForAge,
  validateThreshold,
  type Freshness,
  type FreshnessBasis,
  type FreshnessThreshold,
} from './freshness';
import { CHANNEL_COUNT, channelIndex, isNumericChannel } from './registry';
import { err, ok, type Result } from './result';
import { TelemetryRingBuffer } from './ring';
import { readTlvBlock, type CodecError, type TlvHeader, type TlvRecordVisitor, type TlvScalar } from './tlv';
import { parseUiBatch, type UiBatchError } from './uiBatch';

// -------------------------------------------------------------------------------------------------------------
// Public types
// -------------------------------------------------------------------------------------------------------------

/**
 * What a channel shows right now. Precedence: NO DATA (nothing since the last reset), NOT SUPPORTED, STALE
 * (unconfirmed since a stream gap, the class's age is above its STALE threshold, or the sample is STALE
 * in-band), INVALID, UNAVAILABLE, then LIVE / DEGRADED for VALID samples by the class's age (spec §126).
 */
export type SampleState = 'live' | 'degraded' | 'stale' | 'invalid' | 'unavailable' | 'notSupported' | 'noData';

/** A sample value: number (floats and ≤32-bit integers), boolean, string or bigint (U64, exact). */
export type SampleValue = number | bigint | boolean | string;

/** Immutable snapshot of one channel. A new object is created only when something it shows changes. */
export interface ChannelSample {
  readonly id: number;
  /** Channel metadata (unit, provenance, …); null for ids this build does not know. */
  readonly def: ChannelDef | null;
  readonly state: SampleState;
  /** Wire quality of the current sample; null when nothing arrived since the last reset. */
  readonly quality: Quality | null;
  /** The value, present only when `state` is 'live' or 'degraded'. Never a placeholder. */
  readonly value: SampleValue | null;
  /**
   * Local receive time (store clock, ms) of the batch whose record set this sample. This and the capture
   * timestamps below belong to the block that last CHANGED the sample (value or quality): the bridge sends
   * deltas, so an unchanged channel is confirmed by every block without a record, and an identical re-send
   * (e.g. in a snapshot) does not move them either. Freshness is judged per frequency class from the newest
   * live batch plus the reported LMU source age (`getStreamState`, freshness.ts), not from these.
   */
  readonly receivedAtMs: number | null;
  /** Driver monotonic capture time (ns) from the TLV header of the block that set this sample. */
  readonly captureMonoNs: bigint | null;
  /** Driver's relay-clock estimate at capture (ns since the Unix epoch); null when the driver did not know it. */
  readonly captureRelayNsEst: bigint | null;
  /** LMU session time of that block (s); null when it was not finite. */
  readonly sourceEtS: number | null;
}

/** Freshness of a frequency class as a whole; 'noData' until one of its channels has a sample. */
export type StreamState = Freshness | 'noData';

/** Why a batch was refused. Nothing of a refused batch is applied. */
export type BatchRejection =
  | { readonly kind: 'envelope'; readonly error: UiBatchError }
  | { readonly kind: 'block'; readonly blockIndex: number; readonly error: CodecError }
  | {
      readonly kind: 'schemaMismatch';
      readonly blockIndex: number;
      readonly found: number;
      readonly expected: number;
    };

/** What an accepted batch did (counts summed over its blocks, as `DecodeReport`). */
export interface BatchReport {
  readonly blocks: number;
  /** Records stored (including those stored as INVALID). */
  readonly applied: number;
  /** Records skipped because this build does not know the channel id. */
  readonly unknownSkipped: number;
  /** Records that contradicted the registry and were stored as INVALID. */
  readonly invalidated: number;
  /** True if the batch followed dropped data (GAP_BEFORE or a batch_seq jump). */
  readonly gapBefore: boolean;
  /**
   * True while a gap since the last RESET (in this batch or an earlier one) may have lost changes: values not
   * re-confirmed since then read STALE. Resubscribing (RESET + BACKFILL) restores the full state;
   * `connectBatchSource` does that.
   */
  readonly resyncNeeded: boolean;
}

/** Store health for diagnostics. */
export interface TelemetryStoreStatus {
  /** 'waiting' before the first batch, 'rejected' from a refused batch until the next accepted one. */
  readonly state: 'waiting' | 'receiving' | 'rejected';
  readonly batchesApplied: number;
  readonly batchesRejected: number;
  readonly lastRejection: BatchRejection | null;
  readonly lastBatchSeq: number | null;
  /** Batches that followed dropped data (GAP_BEFORE flags plus batch_seq jumps). */
  readonly gaps: number;
  /** A gap since the last RESET left values unconfirmed (see `BatchReport.resyncNeeded`). */
  readonly resyncNeeded: boolean;
  readonly seqJumps: number;
  readonly resets: number;
  readonly recordsApplied: number;
  readonly unknownSkipped: number;
  readonly invalidated: number;
  /** Times the ring was cleared because the session time went backwards (a new session). */
  readonly ringClears: number;
}

/** Read-only view of the live ring for graph renderers. */
export type LiveRingView = Pick<
  TelemetryRingBuffer,
  | 'capacity'
  | 'length'
  | 'isEmpty'
  | 'channels'
  | 'timeAt'
  | 'valueAt'
  | 'oldestTime'
  | 'newestTime'
  | 'newest'
  | 'oldest'
  | 'times'
  | 'rangeIndices'
  | 'range'
  | 'copyRange'
  | 'nearestIndex'
  | 'nearest'
  | 'sampleAtTime'
  | 'segments'
>;

/** A store change callback. */
export type Listener = () => void;
/** Runs `callback` before the next repaint; returns a function that cancels it. */
export type ScheduleFrame = (callback: () => void) => () => void;
/** Runs `callback` after `delayMs`; returns a function that cancels it. */
export type ScheduleTimeout = (callback: () => void, delayMs: number) => () => void;

/** Subscription over a fixed set of channels with a stable snapshot array. */
export interface ChannelSelection {
  readonly ids: readonly number[];
  readonly subscribe: (listener: Listener) => () => void;
  /** Same array instance until one of the channels' snapshots changes. */
  readonly getSnapshot: () => readonly ChannelSample[];
}

export interface TelemetryStoreOptions {
  /** Monotonic clock in ms (default `performance.now`). */
  readonly now?: () => number;
  /** Frame scheduler for coalesced notifications (default `requestAnimationFrame`). */
  readonly scheduleFrame?: ScheduleFrame;
  /** Timer for staleness transitions (default `setTimeout`). */
  readonly scheduleTimeout?: ScheduleTimeout;
  /** Live graph window in seconds, 30–120 (spec §63; default 60). */
  readonly liveWindowS?: number;
  /** Per-class overrides of the staleness thresholds (spec §126: configurable). */
  readonly freshnessThresholds?: Partial<Record<FrequencyClass, FreshnessThreshold>>;
  /** Channels kept in the live ring (default `defaultRingChannels()`). */
  readonly ringChannels?: readonly number[];
}

// -------------------------------------------------------------------------------------------------------------
// Constants
// -------------------------------------------------------------------------------------------------------------

/** Spec §63: at least 30 s of live high-resolution telemetry; 60 s preferred and configurable. */
export const DEFAULT_LIVE_WINDOW_S = 60;
export const MIN_LIVE_WINDOW_S = 30;
export const MAX_LIVE_WINDOW_S = 120;
/** A-LMU-001: LMU telemetry is nominally about 100 Hz. Used only for the initial ring size, never assumed. */
export const NOMINAL_ROW_RATE_HZ = 100;
/** A-LMU-001: ring buffers are sized for up to 400 Hz; the ring grows to the measured rate up to this. */
export const MAX_ROW_RATE_HZ = 400;
/** Headroom when the ring grows to a measured row rate (avoids growing again for small rate jitter). */
const RING_GROWTH_HEADROOM = 1.1;
/** The STALE boundary is exclusive (age > staleAfterMs): the timer fires just after it. */
const STALE_TIMER_SLACK_MS = 1;
/** Fallback frame period when `requestAnimationFrame` is unavailable (60 Hz). */
const FALLBACK_FRAME_MS = 16;
const MS_PER_S = 1_000;

const CLASS_COUNT = ALL_FREQUENCY_CLASSES.length;
const CLASS_INDEX: ReadonlyMap<FrequencyClass, number> = new Map(ALL_FREQUENCY_CLASSES.map((c, i) => [c, i] as const));

/** Rows needed to keep `windowS` seconds at `rateHz` (both ends inclusive). */
function rowsFor(windowS: number, rateHz: number): number {
  return Math.ceil(windowS * rateHz) + 1;
}

/**
 * Default live-ring channels: every numeric channel some source provides and that can change during a
 * session (static channels and channels no source provides would only hold constants or gaps).
 */
export function defaultRingChannels(): number[] {
  return CHANNELS.filter(
    (def) =>
      isNumericChannel(def) &&
      def.availability !== Availability.NotProvided &&
      def.frequency !== FrequencyClass.Static,
  ).map((def) => def.id);
}

const defaultScheduleFrame: ScheduleFrame = (callback) => {
  if (typeof requestAnimationFrame === 'function') {
    const handle = requestAnimationFrame(() => {
      callback();
    });
    return () => {
      cancelAnimationFrame(handle);
    };
  }
  const handle = setTimeout(callback, FALLBACK_FRAME_MS);
  return () => {
    clearTimeout(handle);
  };
};

const defaultScheduleTimeout: ScheduleTimeout = (callback, delayMs) => {
  const handle = setTimeout(callback, delayMs);
  return () => {
    clearTimeout(handle);
  };
};

/** `Value::as_f64`: the numeric view graphs use (BOOL → 0/1, U64 → nearest double, text → none). */
function numericView(value: SampleValue | null): number {
  switch (typeof value) {
    case 'number':
      return value;
    case 'boolean':
      return value ? 1 : 0;
    case 'bigint':
      return Number(value);
    default:
      return Number.NaN;
  }
}

/** Timestamps of one applied block, shared by every sample it set. */
interface BlockStamp {
  readonly receivedAtMs: number;
  readonly captureMonoNs: bigint;
  readonly captureRelayNsEst: bigint | null;
  readonly sourceEtS: number | null;
}

interface BatchCounts {
  applied: number;
  unknownSkipped: number;
  invalidated: number;
}

// -------------------------------------------------------------------------------------------------------------
// Store
// -------------------------------------------------------------------------------------------------------------

/** The live telemetry store. Create one per telemetry stream and connect it with `connectBatchSource`. */
export class TelemetryStore {
  readonly #now: () => number;
  readonly #scheduleFrame: ScheduleFrame;
  readonly #scheduleTimeout: ScheduleTimeout;
  readonly #thresholds: readonly FreshnessThreshold[];

  // Per channel, indexed by registry position.
  readonly #has = new Uint8Array(CHANNEL_COUNT);
  /** Wire quality of the current sample (meaningful only where `#has` is 1). */
  readonly #quality: Quality[] = new Array<Quality>(CHANNEL_COUNT).fill(Quality.Unavailable);
  /** Valid numeric value or NaN: the source of ring rows. */
  readonly #numeric = new Float64Array(CHANNEL_COUNT).fill(Number.NaN);
  readonly #value: (SampleValue | null)[] = new Array<SampleValue | null>(CHANNEL_COUNT).fill(null);
  readonly #stamp: (BlockStamp | null)[] = new Array<BlockStamp | null>(CHANNEL_COUNT).fill(null);
  readonly #version = new Uint32Array(CHANNEL_COUNT);
  readonly #classOf = new Uint8Array(CHANNEL_COUNT);
  readonly #snapshots: (ChannelSample | undefined)[] = new Array<ChannelSample | undefined>(CHANNEL_COUNT);
  readonly #snapshotVersion = new Uint32Array(CHANNEL_COUNT);
  readonly #unknownSnapshots = new Map<number, ChannelSample>();

  // Per frequency class.
  readonly #classFreshness: Freshness[];
  readonly #classSamples = new Uint32Array(CLASS_COUNT);
  /** Registry index of each class's source-age channel, or -1 (freshness.ts SOURCE_AGE_CHANNELS). */
  readonly #sourceAgeIndex: Int32Array;
  /** Source age (ms) included in each class's age at the last evaluation; 0 when not reported. */
  readonly #classSourceAgeMs = new Float64Array(CLASS_COUNT);
  readonly #classBasis: FreshnessBasis[];
  #lastLiveAtMs: number | null = null;
  #cancelFreshnessTimer: (() => void) | null = null;
  /** Store-clock time the pending freshness timer fires at; null when none is pending. */
  #freshnessDueAt: number | null = null;

  // Delta-stream continuity (see #desync).
  #desynced = false;
  /** Incremented at every gap; a channel is confirmed when its record arrived in the current epoch. */
  #syncEpoch = 0;
  readonly #confirmedEpoch = new Float64Array(CHANNEL_COUNT);

  // Live ring.
  #ring: TelemetryRingBuffer;
  readonly #ringSource: Int32Array;
  #liveWindowS: number;
  #pendingGap = false;

  // Sequence.
  #lastSeq: number | null = null;

  // Subscriptions and coalesced notification.
  readonly #channelListeners = new Map<number, Set<Listener>>();
  readonly #streamListeners = new Set<Listener>();
  readonly #statusListeners = new Set<Listener>();
  readonly #ringListeners = new Set<Listener>();
  readonly #dirty = new Uint8Array(CHANNEL_COUNT);
  readonly #dirtyList: number[] = [];
  /** Listeners of the frame being flushed (reused every frame). */
  readonly #flushListeners = new Set<Listener>();
  #streamDirty = false;
  #statusDirty = false;
  #ringDirty = false;
  #cancelFrame: (() => void) | null = null;

  // Status.
  #state: TelemetryStoreStatus['state'] = 'waiting';
  #lastRejection: BatchRejection | null = null;
  readonly #counters = {
    batchesApplied: 0,
    batchesRejected: 0,
    gaps: 0,
    seqJumps: 0,
    resets: 0,
    recordsApplied: 0,
    unknownSkipped: 0,
    invalidated: 0,
    ringClears: 0,
  };
  #statusSnapshot: TelemetryStoreStatus | null = null;

  // Batch application.
  /** Headers of the batch being applied (reused). */
  readonly #headers: TlvHeader[] = [];
  /** Header and receive time of the block being applied; its stamp is created only if it changes a sample. */
  #blockHeader: TlvHeader | null = null;
  #blockReceivedAtMs = 0;
  #currentStamp: BlockStamp | null = null;
  readonly #batchCounts: BatchCounts = { applied: 0, unknownSkipped: 0, invalidated: 0 };
  readonly #visitor: TlvRecordVisitor;

  constructor(options: TelemetryStoreOptions = {}) {
    this.#now = options.now ?? (() => performance.now());
    this.#scheduleFrame = options.scheduleFrame ?? defaultScheduleFrame;
    this.#scheduleTimeout = options.scheduleTimeout ?? defaultScheduleTimeout;
    this.#thresholds = ALL_FREQUENCY_CLASSES.map((c) => {
      const threshold = options.freshnessThresholds?.[c] ?? DEFAULT_FRESHNESS_THRESHOLDS[c];
      validateThreshold(threshold);
      return threshold;
    });
    // Nothing confirmed yet: every class with a finite STALE threshold starts STALE.
    this.#classFreshness = this.#thresholds.map((t) => freshnessForAge(Number.POSITIVE_INFINITY, t));
    this.#sourceAgeIndex = Int32Array.from(ALL_FREQUENCY_CLASSES, (c) => {
      const id = SOURCE_AGE_CHANNELS[c];
      return id === undefined ? -1 : channelIndex(id);
    });
    this.#classBasis = ALL_FREQUENCY_CLASSES.map((c) => (SOURCE_AGE_CHANNELS[c] === undefined ? 'endToEnd' : 'linkOnly'));
    CHANNELS.forEach((def, index) => {
      this.#classOf[index] = CLASS_INDEX.get(def.frequency) ?? 0;
    });

    this.#liveWindowS = checkWindow(options.liveWindowS ?? DEFAULT_LIVE_WINDOW_S);
    const ringChannels = options.ringChannels ?? defaultRingChannels();
    const created = TelemetryRingBuffer.create(rowsFor(this.#liveWindowS, NOMINAL_ROW_RATE_HZ), ringChannels);
    if (!created.ok) throw new Error(`live ring: ${created.error.kind}`);
    this.#ring = created.value;
    this.#ringSource = Int32Array.from(ringChannels, (id) => channelIndex(id));

    this.#visitor = {
      record: (channel, quality, valueType, value) => {
        this.#applyRecord(channel, quality, valueType, value);
      },
    };
  }

  // ----- applying batches -------------------------------------------------------------------------------------

  /**
   * Applies one UiBatch (D-023). Returns what was applied, or why the whole batch was refused; a refused
   * batch changes no value and no ring row (the caller resubscribes, which starts with RESET + BACKFILL).
   */
  applyBatch(input: ArrayBuffer | ArrayBufferView): Result<BatchReport, BatchRejection> {
    const bytes = ArrayBuffer.isView(input)
      ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
      : new Uint8Array(input);
    const now = this.#now();
    const parsed = parseUiBatch(bytes);
    if (!parsed.ok) return this.#reject({ kind: 'envelope', error: parsed.error });
    const batch = parsed.value;
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);

    // Pass 1: every block must decode and announce this build's schema (frame.rs apply_tlv). Nothing is
    // applied unless all of them do.
    const headers = this.#headers;
    headers.length = 0;
    const blocks = batch.blocks;
    for (let blockIndex = 0; blockIndex < blocks.length; blockIndex++) {
      const block = blocks[blockIndex];
      if (block === undefined) continue;
      const scan = readTlvBlock(view, block.offset, block.length, null);
      if (!scan.ok) return this.#reject({ kind: 'block', blockIndex, error: scan.error });
      const found = scan.value.header.schemaVersion;
      if (found !== TELEMETRY_SCHEMA_VERSION) {
        return this.#reject({ kind: 'schemaMismatch', blockIndex, found, expected: TELEMETRY_SCHEMA_VERSION });
      }
      headers.push(scan.value.header);
    }

    // Pass 2: apply.
    const jump = !batch.reset && this.#lastSeq !== null && batch.batchSeq !== ((this.#lastSeq + 1) >>> 0);
    const gapBefore = batch.gapBefore || jump;
    this.#lastSeq = batch.batchSeq;
    if (batch.reset) {
      this.#clearState();
      this.#counters.resets += 1;
    }
    if (jump) this.#counters.seqJumps += 1;
    if (gapBefore) {
      this.#counters.gaps += 1;
      this.#pendingGap = true;
      // The bridge sends deltas: blocks lost before this batch may have changed any channel. A RESET batch
      // rebuilds the whole state, so only a gap without RESET leaves the cached values unconfirmed.
      if (!batch.reset) this.#desync();
    }
    const counts = this.#batchCounts;
    counts.applied = 0;
    counts.unknownSkipped = 0;
    counts.invalidated = 0;
    for (let i = 0; i < blocks.length; i++) {
      const block = blocks[i];
      const header = headers[i];
      if (block === undefined || header === undefined) continue;
      this.#blockHeader = header;
      this.#blockReceivedAtMs = now;
      this.#currentStamp = null;
      const applied = readTlvBlock(view, block.offset, block.length, this.#visitor);
      // Pass 1 decoded the same bytes successfully, so this cannot fail.
      if (!applied.ok) throw new Error(`TLV block ${i} failed after validation: ${applied.error.kind}`);
      if (Number.isFinite(header.sourceEtS)) this.#appendRingRow(header.sourceEtS);
    }
    this.#blockHeader = null;
    this.#currentStamp = null;
    headers.length = 0;

    if (!batch.backfill) this.#lastLiveAtMs = now;
    this.#evaluateFreshness(now);
    this.#armFreshnessTimer(now);

    const c = this.#counters;
    c.batchesApplied += 1;
    c.recordsApplied += counts.applied;
    c.unknownSkipped += counts.unknownSkipped;
    c.invalidated += counts.invalidated;
    this.#state = 'receiving';
    this.#statusChanged();
    return ok({
      blocks: blocks.length,
      applied: counts.applied,
      unknownSkipped: counts.unknownSkipped,
      invalidated: counts.invalidated,
      gapBefore,
      resyncNeeded: this.#desynced,
    });
  }

  #reject(rejection: BatchRejection): Result<BatchReport, BatchRejection> {
    this.#counters.batchesRejected += 1;
    this.#lastRejection = rejection;
    this.#state = 'rejected';
    this.#statusChanged();
    return err(rejection);
  }

  /** `frame.rs` `check_sample`, then the store update. */
  #applyRecord(channel: number, quality: Quality, valueType: ValueType, value: TlvScalar | undefined): void {
    const counts = this.#batchCounts;
    const index = channelIndex(channel);
    const def = CHANNELS[index];
    if (def === undefined) {
      counts.unknownSkipped += 1;
      return;
    }
    let stored: Quality = quality;
    let storedValue: SampleValue | null = null;
    if (quality === Quality.Valid) {
      const contradicts =
        value === undefined ||
        valueType !== def.valueType ||
        (typeof value === 'number' &&
          (valueType === ValueType.F32 || valueType === ValueType.F64) &&
          !Number.isFinite(value));
      if (contradicts) {
        stored = Quality.Invalid;
        counts.invalidated += 1;
      } else {
        storedValue = value;
      }
    }
    counts.applied += 1;
    this.#setSample(index, stored, storedValue);
  }

  #setSample(index: number, quality: Quality, value: SampleValue | null): void {
    const had = this.#has[index] === 1;
    const wasConfirmed = this.#isConfirmed(index);
    this.#confirmedEpoch[index] = this.#syncEpoch;
    if (had && this.#quality[index] === quality && Object.is(this.#value[index], value)) {
      if (!wasConfirmed) {
        // Re-confirmed after a gap with the value it already had: the sample is unchanged (and keeps its
        // timestamps), but it is current again, so it shows and graphs again.
        this.#numeric[index] = numericView(value);
        this.#markDirty(index);
      }
      return;
    }
    if (!had) {
      this.#has[index] = 1;
      const cls = this.#classOf[index] ?? 0;
      const before = this.#classSamples[cls] ?? 0;
      this.#classSamples[cls] = before + 1;
      if (before === 0) this.#streamChanged();
    }
    this.#quality[index] = quality;
    this.#value[index] = value;
    this.#numeric[index] = numericView(value);
    this.#stamp[index] = this.#blockStamp();
    this.#version[index] = (this.#version[index] ?? 0) + 1;
    this.#markDirty(index);
  }

  /** Timestamps of the block being applied, created once per block and shared by every sample it changes. */
  #blockStamp(): BlockStamp | null {
    const header = this.#blockHeader;
    if (header === null) return null;
    this.#currentStamp ??= {
      receivedAtMs: this.#blockReceivedAtMs,
      captureMonoNs: header.captureMonoNs,
      captureRelayNsEst: header.captureRelayNsEst === RELAY_NS_UNKNOWN ? null : header.captureRelayNsEst,
      sourceEtS: Number.isFinite(header.sourceEtS) ? header.sourceEtS : null,
    };
    return this.#currentStamp;
  }

  /**
   * True when channel `index`'s cached sample is known to be current: always, except after a gap (GAP_BEFORE
   * or a batch_seq jump) until a record for the channel arrives or a RESET rebuilds the state.
   */
  #isConfirmed(index: number): boolean {
    return !this.#desynced || this.#confirmedEpoch[index] === this.#syncEpoch;
  }

  /**
   * A gap in the delta stream: every cached sample becomes unconfirmed (shown STALE, spec §107 "previously
   * valid, not current") until a record re-confirms it, and the ring receives NaN for it, so neither a
   * readout nor a graph shows a value a lost block may have changed. Only a RESET (a new subscription)
   * restores the full state; batch reports say `resyncNeeded` until then.
   */
  #desync(): void {
    this.#desynced = true;
    this.#syncEpoch += 1;
    this.#numeric.fill(Number.NaN);
    for (const index of this.#channelListeners.keys()) {
      if (this.#has[index] === 1) this.#markDirty(index);
    }
  }

  /** RESET: discard every cached value and the ring. */
  #clearState(): void {
    this.#desynced = false;
    for (let index = 0; index < CHANNEL_COUNT; index++) {
      if (this.#has[index] === 1) {
        this.#has[index] = 0;
        this.#quality[index] = Quality.Unavailable;
        this.#value[index] = null;
        this.#numeric[index] = Number.NaN;
        this.#stamp[index] = null;
        this.#version[index] = (this.#version[index] ?? 0) + 1;
        this.#markDirty(index);
      }
    }
    this.#classSamples.fill(0);
    this.#lastLiveAtMs = null;
    this.#ring.clear();
    this.#pendingGap = false;
    this.#ringChanged();
    this.#streamChanged();
  }

  /** Discards every value and the ring, as a RESET batch does (e.g. when the stream is disconnected). */
  reset(): void {
    this.#clearState();
    this.#counters.resets += 1;
    this.#lastSeq = null;
    const now = this.#now();
    this.#evaluateFreshness(now);
    this.#armFreshnessTimer(now);
    this.#statusChanged();
  }

  // ----- live ring ----------------------------------------------------------------------------------------------

  #appendRingRow(t: number): void {
    const ring = this.#ring;
    const newest = ring.newestTime();
    if (newest !== null && t === newest) {
      // Same session time as the newest row (e.g. a scoring update between physics frames, or a paused
      // session): keep one row per ET with the latest values instead of piling up rows that evict history.
      ring.replaceNewestRowGather(this.#numeric, this.#ringSource);
      this.#ringChanged();
      return;
    }
    if (this.#pendingGap && newest !== null && t > newest) {
      // A row of gaps between the last row before the drop and the first row after it, so graphs break the
      // line (spec §219) and point queries never bridge the drop.
      ring.pushGap(newest + (t - newest) / 2);
    }
    this.#pendingGap = false;
    this.#growRingIfShort(t);
    let error = ring.pushRowGather(t, this.#numeric, this.#ringSource);
    if (error?.kind === 'TimeRegression') {
      // Session time went backwards: a new session. The ring contract is that the caller clears it.
      ring.clear();
      this.#counters.ringClears += 1;
      error = ring.pushRowGather(t, this.#numeric, this.#ringSource);
    }
    if (error !== null) throw new Error(`live ring append failed: ${error.kind}`);
    this.#ringChanged();
  }

  /**
   * Grows the ring when it is full and appending would evict a row still inside the live window, i.e. the
   * measured row rate is above what the ring was sized for (bounded by MAX_ROW_RATE_HZ).
   */
  #growRingIfShort(t: number): void {
    const ring = this.#ring;
    if (ring.length < ring.capacity) return;
    const oldest = ring.oldestTime();
    if (oldest === null || t - oldest > this.#liveWindowS) return;
    const maxRows = rowsFor(this.#liveWindowS, MAX_ROW_RATE_HZ);
    if (ring.capacity >= maxRows) return;
    const span = t - oldest;
    const measuredRateHz = span > 0 ? ring.length / span : MAX_ROW_RATE_HZ;
    const wanted = Math.min(
      maxRows,
      Math.max(ring.capacity + 1, rowsFor(this.#liveWindowS, measuredRateHz * RING_GROWTH_HEADROOM)),
    );
    // On failure the ring keeps its size: it then covers less than the window, never more memory than allowed.
    ring.resize(wanted);
  }

  /** The live ring (read-only view) for graph renderers. */
  get ring(): LiveRingView {
    return this.#ring;
  }

  /** Live graph window in seconds. */
  get liveWindowS(): number {
    return this.#liveWindowS;
  }

  /** Changes the live window (30–120 s, spec §63), keeping the newest rows. Throws RangeError when out of range. */
  setLiveWindowS(windowS: number): void {
    const next = checkWindow(windowS);
    const ring = this.#ring;
    const rateHz = (ring.capacity - 1) / this.#liveWindowS;
    const capacity = Math.min(
      rowsFor(next, MAX_ROW_RATE_HZ),
      Math.max(rowsFor(next, NOMINAL_ROW_RATE_HZ), rowsFor(next, rateHz)),
    );
    const error = ring.resize(capacity);
    if (error !== null) throw new Error(`live ring resize failed: ${error.kind}`);
    this.#liveWindowS = next;
    this.#ringChanged();
  }

  // ----- staleness ----------------------------------------------------------------------------------------------

  /**
   * Source age (ms) the Rust core reports for class `c` (freshness.ts SOURCE_AGE_CHANNELS); null when the class
   * has no source-age channel or it has no VALID, confirmed, non-negative value.
   */
  #sourceAgeMs(c: number): number | null {
    const index = this.#sourceAgeIndex[c] ?? -1;
    if (index < 0 || this.#has[index] !== 1 || this.#quality[index] !== Quality.Valid) return null;
    if (!this.#isConfirmed(index)) return null;
    const seconds = this.#value[index];
    return typeof seconds === 'number' && Number.isFinite(seconds) && seconds >= 0 ? seconds * MS_PER_S : null;
  }

  /** Recomputes every class's source-age offset, basis and freshness at `now`, notifying what changed. */
  #evaluateFreshness(now: number): void {
    const linkAge = this.#lastLiveAtMs === null ? Number.POSITIVE_INFINITY : now - this.#lastLiveAtMs;
    for (let c = 0; c < CLASS_COUNT; c++) {
      const threshold = this.#thresholds[c];
      if (threshold === undefined) continue;
      const sourceAge = this.#sourceAgeMs(c);
      this.#classSourceAgeMs[c] = sourceAge ?? 0;
      const basis: FreshnessBasis = sourceAge === null && (this.#sourceAgeIndex[c] ?? -1) >= 0 ? 'linkOnly' : 'endToEnd';
      if (basis !== this.#classBasis[c]) {
        this.#classBasis[c] = basis;
        this.#streamChanged();
      }
      const next = freshnessForAge(linkAge + (sourceAge ?? 0), threshold);
      if (next !== this.#classFreshness[c]) {
        this.#classFreshness[c] = next;
        this.#classChanged(c);
      }
    }
  }

  /** Earliest time at which a class changes freshness without a new batch; null if none will. */
  #nextFreshnessChangeAt(): number | null {
    const lastLive = this.#lastLiveAtMs;
    if (lastLive === null) return null;
    let due = Number.POSITIVE_INFINITY;
    for (let c = 0; c < CLASS_COUNT; c++) {
      const threshold = this.#thresholds[c];
      if (threshold === undefined) continue;
      // The class age is (now − lastLive) + sourceAge, so it reaches a threshold at lastLive − sourceAge + limit.
      const base = lastLive - (this.#classSourceAgeMs[c] ?? 0);
      const current = this.#classFreshness[c];
      if (current === 'live' && Number.isFinite(threshold.degradedAfterMs)) {
        due = Math.min(due, base + threshold.degradedAfterMs);
      } else if (current !== 'stale' && Number.isFinite(threshold.staleAfterMs)) {
        due = Math.min(due, base + threshold.staleAfterMs + STALE_TIMER_SLACK_MS);
      }
    }
    return Number.isFinite(due) ? due : null;
  }

  /**
   * Keeps one timer pending for the next freshness transition; it re-arms itself when it fires. A pending
   * timer that fires no later than the next transition is kept (it re-evaluates and re-arms then); one that
   * would fire after it, e.g. a class's STALE timer still pending when a new batch made the class LIVE again,
   * is replaced so no transition is ever late.
   */
  #armFreshnessTimer(now: number): void {
    const due = this.#nextFreshnessChangeAt();
    const pendingAt = this.#freshnessDueAt;
    if (pendingAt !== null && due !== null && pendingAt <= due) return;
    this.#cancelFreshness();
    if (due === null) return;
    this.#freshnessDueAt = due;
    this.#cancelFreshnessTimer = this.#scheduleTimeout(
      () => {
        this.#cancelFreshnessTimer = null;
        this.#freshnessDueAt = null;
        const at = this.#now();
        this.#evaluateFreshness(at);
        this.#armFreshnessTimer(at);
      },
      Math.max(0, due - now),
    );
  }

  #cancelFreshness(): void {
    this.#cancelFreshnessTimer?.();
    this.#cancelFreshnessTimer = null;
    this.#freshnessDueAt = null;
  }

  /** Freshness of a frequency class ('noData' until one of its channels has a sample). */
  getStreamState(frequency: FrequencyClass): StreamState {
    const c = CLASS_INDEX.get(frequency) ?? 0;
    if (this.#classSamples[c] === 0) return 'noData';
    return this.#classFreshness[c] ?? 'stale';
  }

  /**
   * Whether a class's freshness includes the LMU source age ('endToEnd') or only the link to the Rust core
   * ('linkOnly', while the core does not report the source age; freshness.ts).
   */
  getStreamBasis(frequency: FrequencyClass): FreshnessBasis {
    return this.#classBasis[CLASS_INDEX.get(frequency) ?? 0] ?? 'linkOnly';
  }

  /**
   * Age in ms that a class's freshness is judged by: the age of the newest live batch plus the class's source
   * age when reported (null before the first live batch). Reads the clock: call it outside render.
   */
  streamAgeMs(frequency: FrequencyClass): number | null {
    const linkAge = this.liveBatchAgeMs();
    if (linkAge === null) return null;
    return linkAge + (this.#classSourceAgeMs[CLASS_INDEX.get(frequency) ?? 0] ?? 0);
  }

  /** Store-clock time of the newest live (non-BACKFILL) batch since the last reset. */
  getLastLiveBatchAtMs(): number | null {
    return this.#lastLiveAtMs;
  }

  /** Age of the newest live batch in ms (null before the first one). Reads the clock: call it outside render. */
  liveBatchAgeMs(): number | null {
    return this.#lastLiveAtMs === null ? null : this.#now() - this.#lastLiveAtMs;
  }

  // ----- snapshots ----------------------------------------------------------------------------------------------

  #stateOf(index: number): SampleState {
    if (this.#has[index] !== 1) return 'noData';
    const quality = this.#quality[index];
    if (quality === Quality.NotSupported) return 'notSupported';
    if (!this.#isConfirmed(index)) return 'stale';
    const freshness = this.#classFreshness[this.#classOf[index] ?? 0] ?? 'stale';
    if (freshness === 'stale') return 'stale';
    switch (quality) {
      case Quality.Valid:
        return freshness;
      case Quality.Stale:
        return 'stale';
      case Quality.Unavailable:
        return 'unavailable';
      default:
        return 'invalid';
    }
  }

  /** Snapshot of channel `id`; the same object until its sample or display state changes. */
  getSample(id: number): ChannelSample {
    const index = channelIndex(id);
    const def = CHANNELS[index];
    if (def === undefined) return this.#unknownSample(id);
    const state = this.#stateOf(index);
    const version = this.#version[index] ?? 0;
    const cached = this.#snapshots[index];
    if (cached?.state === state && this.#snapshotVersion[index] === version) return cached;
    const has = this.#has[index] === 1;
    const stamp = this.#stamp[index] ?? null;
    const quality = this.#quality[index];
    const sample: ChannelSample = {
      id,
      def,
      state,
      quality: has && quality !== undefined ? quality : null,
      value: state === 'live' || state === 'degraded' ? (this.#value[index] ?? null) : null,
      receivedAtMs: stamp?.receivedAtMs ?? null,
      captureMonoNs: stamp?.captureMonoNs ?? null,
      captureRelayNsEst: stamp?.captureRelayNsEst ?? null,
      sourceEtS: stamp?.sourceEtS ?? null,
    };
    this.#snapshots[index] = sample;
    this.#snapshotVersion[index] = version;
    return sample;
  }

  /** Ids this build does not know read as NOT SUPPORTED, as `TelemetryFrame::get` does. */
  #unknownSample(id: number): ChannelSample {
    let sample = this.#unknownSnapshots.get(id);
    if (sample === undefined) {
      sample = {
        id,
        def: null,
        state: 'notSupported',
        quality: Quality.NotSupported,
        value: null,
        receivedAtMs: null,
        captureMonoNs: null,
        captureRelayNsEst: null,
        sourceEtS: null,
      };
      this.#unknownSnapshots.set(id, sample);
    }
    return sample;
  }

  /** A selection over `ids` whose snapshot array is replaced only when one of its channels changes. */
  selectChannels(ids: readonly number[]): ChannelSelection {
    const frozen = [...ids];
    let last: readonly ChannelSample[] = frozen.map((id) => this.getSample(id));
    return {
      ids: frozen,
      subscribe: (listener) => this.subscribeChannels(frozen, listener),
      getSnapshot: () => {
        const changed = frozen.some((id, i) => this.getSample(id) !== last[i]);
        if (changed) last = frozen.map((id) => this.getSample(id));
        return last;
      },
    };
  }

  /** Current diagnostics status (same object until something changes). */
  getStatus(): TelemetryStoreStatus {
    if (this.#statusSnapshot === null) {
      this.#statusSnapshot = {
        state: this.#state,
        lastRejection: this.#lastRejection,
        lastBatchSeq: this.#lastSeq,
        resyncNeeded: this.#desynced,
        ...this.#counters,
      };
    }
    return this.#statusSnapshot;
  }

  // ----- subscriptions ------------------------------------------------------------------------------------------

  /** Notified (at most once per frame) when channel `id`'s snapshot may have changed. */
  subscribeChannel(id: number, listener: Listener): () => void {
    const index = channelIndex(id);
    if (index < 0) return () => undefined;
    let set = this.#channelListeners.get(index);
    if (set === undefined) {
      set = new Set();
      this.#channelListeners.set(index, set);
    }
    const listeners = set;
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0 && this.#channelListeners.get(index) === listeners) {
        this.#channelListeners.delete(index);
      }
    };
  }

  /** Notified at most once per frame when any of `ids` may have changed. */
  subscribeChannels(ids: readonly number[], listener: Listener): () => void {
    const unsubscribers = ids.map((id) => this.subscribeChannel(id, listener));
    return () => {
      for (const unsubscribe of unsubscribers) unsubscribe();
    };
  }

  /** Notified when a frequency class changes stream state (`getStreamState`). */
  subscribeStream(listener: Listener): () => void {
    this.#streamListeners.add(listener);
    return () => {
      this.#streamListeners.delete(listener);
    };
  }

  /** Notified when `getStatus()` changes. */
  subscribeStatus(listener: Listener): () => void {
    this.#statusListeners.add(listener);
    return () => {
      this.#statusListeners.delete(listener);
    };
  }

  /** Notified when ring rows were appended, replaced or cleared (graph renderers repaint then). */
  subscribeRing(listener: Listener): () => void {
    this.#ringListeners.add(listener);
    return () => {
      this.#ringListeners.delete(listener);
    };
  }

  /** Cancels pending callbacks and drops every listener. */
  dispose(): void {
    this.#cancelFrame?.();
    this.#cancelFrame = null;
    this.#cancelFreshness();
    this.#channelListeners.clear();
    this.#streamListeners.clear();
    this.#statusListeners.clear();
    this.#ringListeners.clear();
  }

  #markDirty(index: number): void {
    if (this.#dirty[index] === 1 || !this.#channelListeners.has(index)) return;
    this.#dirty[index] = 1;
    this.#dirtyList.push(index);
    this.#scheduleFlush();
  }

  #classChanged(c: number): void {
    for (const index of this.#channelListeners.keys()) {
      if (this.#classOf[index] === c && this.#has[index] === 1) this.#markDirty(index);
    }
    this.#streamChanged();
  }

  #streamChanged(): void {
    this.#streamDirty = true;
    this.#scheduleFlush();
  }

  #statusChanged(): void {
    this.#statusSnapshot = null;
    this.#statusDirty = true;
    this.#scheduleFlush();
  }

  #ringChanged(): void {
    this.#ringDirty = true;
    this.#scheduleFlush();
  }

  #scheduleFlush(): void {
    if (this.#cancelFrame !== null) return;
    this.#cancelFrame = this.#scheduleFrame(() => {
      this.#cancelFrame = null;
      this.#flush();
    });
  }

  /**
   * Calls every affected listener exactly once. Allocation-free per frame: the dirty list and the listener set
   * are reused (frame callbacks never run re-entrantly, so one set suffices).
   */
  #flush(): void {
    const listeners = this.#flushListeners;
    const dirtyList = this.#dirtyList;
    for (const index of dirtyList) {
      this.#dirty[index] = 0;
      const set = this.#channelListeners.get(index);
      if (set !== undefined) for (const listener of set) listeners.add(listener);
    }
    dirtyList.length = 0;
    if (this.#streamDirty) for (const listener of this.#streamListeners) listeners.add(listener);
    if (this.#statusDirty) for (const listener of this.#statusListeners) listeners.add(listener);
    if (this.#ringDirty) for (const listener of this.#ringListeners) listeners.add(listener);
    this.#streamDirty = false;
    this.#statusDirty = false;
    this.#ringDirty = false;
    let failure: Error | null = null;
    for (const listener of listeners) {
      try {
        listener();
      } catch (e: unknown) {
        failure ??= e instanceof Error ? e : new Error(String(e));
      }
    }
    listeners.clear();
    // Every listener ran; the first failure is reported afterwards.
    if (failure !== null) throw failure;
  }
}

function checkWindow(windowS: number): number {
  if (!(windowS >= MIN_LIVE_WINDOW_S && windowS <= MAX_LIVE_WINDOW_S)) {
    throw new RangeError(`live window must be ${MIN_LIVE_WINDOW_S}–${MAX_LIVE_WINDOW_S} s, got ${windowS}`);
  }
  return windowS;
}
