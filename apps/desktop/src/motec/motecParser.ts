/**
 * MoTeC i2 Pro Logged Data (.ld) Binary File Parser.
 *
 * Layout (little endian), as documented by the public ldparser project (github.com/gotzl/ldparser) and as
 * written by `motecWriter.ts`:
 * - file header (1762 bytes): u32 marker 0x40 @0, u32 channel-meta ptr @8, u32 channel-data ptr @12,
 *   u32 event ptr @36, u32 channel count @86, date @94 (16), time @126 (16), driver @158 (64),
 *   vehicle @222 (64), venue @350 (64), short comment @1572 (64);
 * - event block: name (64), session (64), comment (1024);
 * - channel headers (124 bytes, doubly linked list): u32 prev, u32 next, u32 data ptr, u32 sample count,
 *   u16 counter, u16 dtype_a, u16 dtype, u16 rate Hz, i16 shift, i16 mul, i16 scale, i16 dec places,
 *   name (32), short name (8), unit (12);
 * - sample type: dtype_a 0x07 -> float (dtype 2 = f16, 4 = f32); dtype_a 0x00/0x03/0x05 -> integer
 *   (dtype 2 = i16, 4 = i32); dtype_a 0x08 with dtype 8 = f64. Anything else is rejected.
 * - value = ((raw / scale) * 10^-dec + shift) * mul.
 *
 * Owner rule: nothing is invented. Missing header fields are null; samples the file does not contain are not
 * created; non-finite values are NaN; a channel whose unit cannot be converted to the registry unit is not
 * mapped. Problems that do not prevent reading are reported in `warnings`.
 */
import { ch } from '../generated/channels';
import type {
  MotecChannelData,
  MotecChannelMeta,
  MotecDataType,
  MotecHeader,
  MotecLap,
  MotecLapView,
  MotecSessionData,
  ParsedMotecFile,
} from './motecTypes';
import { matchChannelNameToAppId, motecUnitConverter, registryUnit, registryUnitSymbol } from './motecUnits';

export { matchChannelNameToAppId } from './motecUnits';

/** u32 at offset 0 of every .ld file. */
export const LD_MARKER = 0x40;
/** Size of the .ld file header. */
export const LD_HEADER_SIZE = 1762;
/** Size of one channel header. */
const LD_CHANNEL_HEADER_SIZE = 124;
/** Upper bound on channel headers walked (guards against corrupt pointer chains). */
const MAX_CHANNELS = 4096;
/** Shortest lap accepted as a valid lap (s). */
const MIN_VALID_LAP_S = 25;
/** Allowed deviation of a valid lap's distance from the median lap distance. */
const LAP_DISTANCE_TOLERANCE = 0.05;

/**
 * Decodes an IEEE 754 16-bit half-precision floating point number.
 */
function decodeFloat16(h: number): number {
  const sign = (h & 0x8000) >> 15;
  const exp = (h & 0x7c00) >> 10;
  const mant = h & 0x03ff;

  if (exp === 0) {
    return (sign ? -1 : 1) * Math.pow(2, -14) * (mant / 1024);
  }
  if (exp === 0x1f) {
    return mant ? Number.NaN : sign ? -Infinity : Infinity;
  }
  return (sign ? -1 : 1) * Math.pow(2, exp - 15) * (1 + mant / 1024);
}

/**
 * Reads an ASCII null-terminated string of fixed length from a DataView.
 */
function readAscii(view: DataView, offset: number, maxLen: number): string {
  if (offset < 0 || offset >= view.byteLength) return '';
  const len = Math.min(maxLen, view.byteLength - offset);
  let str = '';
  for (let i = 0; i < len; i++) {
    const code = view.getUint8(offset + i);
    if (code === 0) break;
    str += String.fromCharCode(code);
  }
  return str.trim();
}

function orNull(text: string): string | null {
  return text === '' ? null : text;
}

/** Parses MoTeC `dd/mm/yyyy` + `hh:mm[:ss]` (local time). Null when absent or not a real date. */
function parseMotecDateTime(date: string, time: string): number | null {
  const d = /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/.exec(date.trim());
  if (!d) return null;
  const t = /^(\d{1,2}):(\d{2})(?::(\d{2}))?$/.exec(time.trim());
  const day = Number(d[1]);
  const month = Number(d[2]);
  const year = Number(d[3]);
  const hh = t ? Number(t[1]) : 0;
  const mm = t ? Number(t[2]) : 0;
  const ss = t?.[3] ? Number(t[3]) : 0;
  const result = new Date(year, month - 1, day, hh, mm, ss);
  if (result.getFullYear() !== year || result.getMonth() !== month - 1 || result.getDate() !== day) return null;
  if (hh > 23 || mm > 59 || ss > 59) return null;
  return result.getTime();
}

function decodeSampleType(dtypeA: number, dtype: number): { type: MotecDataType; bytes: number } | null {
  if (dtypeA === 0x07) {
    if (dtype === 2) return { type: 'f16', bytes: 2 };
    if (dtype === 4) return { type: 'f32', bytes: 4 };
    return null;
  }
  if (dtypeA === 0x08 && dtype === 8) return { type: 'f64', bytes: 8 };
  if (dtypeA === 0x00 || dtypeA === 0x03 || dtypeA === 0x05) {
    if (dtype === 2) return { type: 'i16', bytes: 2 };
    if (dtype === 4) return { type: 'i32', bytes: 4 };
  }
  return null;
}

/**
 * Parses MoTeC .ldx XML companion content to extract lap beacon markers.
 *
 * The time unit is decided once per file: MoTeC writes marker times in microseconds; a file whose largest time
 * exceeds 100 000 (longer than a 24 h race in seconds) is read as microseconds, otherwise as seconds.
 */
export function parseLdxMarkers(xmlContent: string): { timeS: number; distM: number | null; name: string }[] {
  if (!xmlContent.includes('<Marker')) return [];

  const raw: { time: number; distM: number | null; name: string }[] = [];
  const markerRegex = /<Marker\b([^>]*)/gi;
  let match: RegExpExecArray | null;

  while ((match = markerRegex.exec(xmlContent)) !== null) {
    const attrs = match[1] ?? '';
    const timeMatch = /\bTime="([^"]+)"/i.exec(attrs);
    if (!timeMatch?.[1]) continue;
    const time = Number.parseFloat(timeMatch[1]);
    if (!Number.isFinite(time)) continue;
    const distMatch = /\bDistance="([^"]+)"/i.exec(attrs);
    const dist = distMatch?.[1] !== undefined ? Number.parseFloat(distMatch[1]) : Number.NaN;
    const nameMatch = /\b(?:Name|String)="([^"]+)"/i.exec(attrs);
    raw.push({
      time,
      distM: Number.isFinite(dist) ? dist : null,
      name: nameMatch?.[1] ?? `Marker_${raw.length}`,
    });
  }

  const maxTime = raw.reduce((m, r) => Math.max(m, r.time), Number.NEGATIVE_INFINITY);
  const divisor = maxTime > 100_000 ? 1_000_000 : 1;
  const markers = raw.map((r) => ({ timeS: r.time / divisor, distM: r.distM, name: r.name }));
  markers.sort((a, b) => a.timeS - b.timeS);
  return markers;
}

type ResampleMode = 'linear' | 'hold' | 'distance';

function resampleModeFor(appChannelId: number): ResampleMode {
  if (appChannelId === ch.LAP_DISTANCE) return 'distance';
  const unit = registryUnit(appChannelId);
  return unit === 'Enum' || unit === 'Count' ? 'hold' : 'linear';
}

/**
 * Resamples `raw` (sampled at `freqHz` from t = 0) onto `count` points at `t0 + k / targetHz`. Points outside
 * the channel's samples are NaN. 'hold' keeps the previous sample (discrete channels); 'distance' interpolates
 * but holds across a decrease (lap distance reset), so no fake intermediate distance is created.
 */
export function resampleByTime(
  raw: Float64Array,
  freqHz: number,
  t0: number,
  targetHz: number,
  count: number,
  mode: ResampleMode,
): Float64Array {
  const out = new Float64Array(count);
  const n = raw.length;
  for (let k = 0; k < count; k++) {
    let pos = (t0 + k / targetHz) * freqHz;
    const rounded = Math.round(pos);
    if (Math.abs(pos - rounded) < 1e-6) pos = rounded;
    const i = Math.floor(pos);
    if (i < 0 || i > n - 1) {
      out[k] = Number.NaN;
      continue;
    }
    const frac = pos - i;
    const a = raw[i] ?? Number.NaN;
    if (frac === 0 || mode === 'hold') {
      out[k] = a;
      continue;
    }
    if (i + 1 > n - 1) {
      out[k] = Number.NaN;
      continue;
    }
    const b = raw[i + 1] ?? Number.NaN;
    out[k] = mode === 'distance' && b < a ? a : a + (b - a) * frac;
  }
  return out;
}

interface Boundary {
  readonly timeS: number;
  /** Lap number of the segment that starts here (lap-number method only). */
  readonly lapNumber?: number;
}

interface BoundaryResult {
  readonly source: NonNullable<ParsedMotecFile['lapSource']>;
  readonly boundaries: readonly Boundary[];
  /** Lap number of the segment before the first boundary (lap-number method only). */
  readonly firstLapNumber?: number;
}

function findBoundaries(
  channels: ReadonlyMap<string, MotecChannelData>,
  appChannels: ReadonlyMap<number, MotecChannelData>,
  durationS: number,
  ldxContent: string | undefined,
): BoundaryResult | null {
  // 1. .ldx companion markers.
  if (ldxContent) {
    const markers = parseLdxMarkers(ldxContent).filter((m) => m.timeS >= 0 && m.timeS <= durationS);
    if (markers.length > 0) {
      return { source: 'ldx', boundaries: markers.map((m) => ({ timeS: m.timeS })) };
    }
  }

  // 2. Lap number channel: a boundary wherever the value changes.
  const lapChan = appChannels.get(ch.LAP_NUMBER);
  if (lapChan && lapChan.meta.frequencyHz > 0) {
    const f = lapChan.meta.frequencyHz;
    const boundaries: Boundary[] = [];
    let last: number | null = null;
    let first: number | null = null;
    for (let i = 0; i < lapChan.samples.length; i++) {
      const v = lapChan.samples[i] ?? Number.NaN;
      if (!Number.isFinite(v)) continue;
      const cur = Math.round(v);
      if (last === null) {
        first = cur;
      } else if (cur !== last) {
        boundaries.push({ timeS: i / f, lapNumber: cur });
      }
      last = cur;
    }
    if (boundaries.length > 0) {
      return first === null
        ? { source: 'lap-number', boundaries }
        : { source: 'lap-number', boundaries, firstLapNumber: first };
    }
  }

  // 3. Lap distance reset (a sharp drop at the finish line).
  const distChan = appChannels.get(ch.LAP_DISTANCE);
  if (distChan && distChan.meta.frequencyHz > 0) {
    const f = distChan.meta.frequencyHz;
    const boundaries: Boundary[] = [];
    let lastT = Number.NEGATIVE_INFINITY;
    for (let i = 1; i < distChan.samples.length; i++) {
      const prev = distChan.samples[i - 1] ?? Number.NaN;
      const cur = distChan.samples[i] ?? Number.NaN;
      if (prev - cur > 800 && prev > 1000) {
        const t = i / f;
        if (t - lastT > MIN_VALID_LAP_S) {
          boundaries.push({ timeS: t });
          lastT = t;
        }
      }
    }
    if (boundaries.length > 0) return { source: 'lap-distance', boundaries };
  }

  // 4. Beacon pulses (rising edges).
  const beaconChan =
    channels.get('beacon') ??
    channels.get('lap marker') ??
    channels.get('beacon markers') ??
    channels.get('beacon pulse') ??
    channels.get('split beacon');
  if (beaconChan && beaconChan.meta.frequencyHz > 0) {
    const f = beaconChan.meta.frequencyHz;
    const boundaries: Boundary[] = [];
    let lastT = Number.NEGATIVE_INFINITY;
    for (let i = 1; i < beaconChan.samples.length; i++) {
      if ((beaconChan.samples[i - 1] ?? Number.NaN) <= 0.5 && (beaconChan.samples[i] ?? Number.NaN) > 0.5) {
        const t = i / f;
        if (t - lastT > MIN_VALID_LAP_S) {
          boundaries.push({ timeS: t });
          lastT = t;
        }
      }
    }
    if (boundaries.length > 0) return { source: 'beacon', boundaries };
  }

  return null;
}

function finiteOrNull(v: number | undefined): number | null {
  return v !== undefined && Number.isFinite(v) ? v : null;
}

function median(values: readonly number[]): number {
  const s = [...values].sort((a, b) => a - b);
  const mid = s.length >> 1;
  return s.length % 2 === 1 ? (s[mid] ?? Number.NaN) : ((s[mid - 1] ?? Number.NaN) + (s[mid] ?? Number.NaN)) / 2;
}

function buildLaps(
  result: BoundaryResult,
  durationS: number,
  primaryHz: number,
  sampleCount: number,
  distance: Float64Array | null,
): MotecLap[] {
  const points: { timeS: number; real: boolean; lapNumber?: number | undefined }[] = [];
  const bs = result.boundaries;
  if ((bs[0]?.timeS ?? 0) > 0) points.push({ timeS: 0, real: false, lapNumber: result.firstLapNumber });
  for (const b of bs) points.push({ timeS: b.timeS, real: true, lapNumber: b.lapNumber });
  if ((bs[bs.length - 1]?.timeS ?? durationS) < durationS) points.push({ timeS: durationS, real: false });

  const minSegmentS = 1 / primaryHz;
  const draft: (Omit<MotecLap, 'isValid' | 'invalidReason'> & { reason: string | null; lapDistM: number | null })[] =
    [];
  for (let j = 0; j + 1 < points.length; j++) {
    const start = points[j];
    const end = points[j + 1];
    if (!start || !end) continue;
    const lapTimeS = end.timeS - start.timeS;
    if (lapTimeS < minSegmentS) continue;
    const startSampleIdx = Math.min(sampleCount - 1, Math.max(0, Math.round(start.timeS * primaryHz)));
    const endSampleIdx = Math.min(sampleCount - 1, Math.max(startSampleIdx, Math.round(end.timeS * primaryHz) - 1));
    const startDistM = distance ? finiteOrNull(distance[startSampleIdx]) : null;
    const endDistM = distance ? finiteOrNull(distance[endSampleIdx]) : null;
    const isComplete = start.real && end.real;
    let reason: string | null = null;
    if (!start.real) reason = 'starts at the beginning of the file (out-lap or partial lap)';
    else if (!end.real) reason = 'ends at the end of the file (partial lap)';
    else if (lapTimeS < MIN_VALID_LAP_S) reason = `shorter than ${MIN_VALID_LAP_S} s`;
    draft.push({
      lapNumber: result.source === 'lap-number' && start.lapNumber !== undefined ? start.lapNumber : draft.length + 1,
      lapTimeS,
      startTimeS: start.timeS,
      endTimeS: end.timeS,
      startDistM,
      endDistM,
      startSampleIdx,
      endSampleIdx,
      isComplete,
      reason,
      lapDistM: startDistM !== null && endDistM !== null ? endDistM - startDistM : null,
    });
  }

  // Distance plausibility: a valid lap covers about the median distance of the complete laps.
  const candidateDistances = draft
    .flatMap((l) => (l.reason === null && l.lapDistM !== null ? [l.lapDistM] : []));
  const medianDistM = candidateDistances.length > 0 ? median(candidateDistances) : null;

  return draft.map(({ reason, lapDistM, ...lap }) => {
    let invalidReason = reason;
    if (invalidReason === null && medianDistM !== null && medianDistM > 0 && lapDistM !== null) {
      if (Math.abs(lapDistM - medianDistM) > medianDistM * LAP_DISTANCE_TOLERANCE) {
        invalidReason = `distance ${lapDistM.toFixed(0)} m differs from the median lap distance ${medianDistM.toFixed(0)} m`;
      }
    }
    return { ...lap, isValid: invalidReason === null, invalidReason };
  });
}

function buildView(session: MotecSessionData, lap: MotecLap | null): MotecLapView {
  const hz = session.primaryFrequencyHz;
  const startIdx = lap ? lap.startSampleIdx : 0;
  const endIdx = lap ? lap.endSampleIdx : session.sampleCount - 1;
  const count = Math.max(1, endIdx - startIdx + 1);
  const t0 = startIdx / hz;

  const timeTimelineS = new Float64Array(count);
  for (let k = 0; k < count; k++) timeTimelineS[k] = k / hz;

  const channelsByAppId = new Map<number, MotecChannelData>();
  for (const [appId, chan] of session.appChannels) {
    channelsByAppId.set(appId, {
      meta: chan.meta,
      samples: resampleByTime(chan.samples, chan.meta.frequencyHz, t0, hz, count, resampleModeFor(appId)),
      appChannelId: appId,
    });
  }

  let distanceTimelineM: Float64Array | null = null;
  let maxDistanceM: number | null = null;
  const dist = session.distanceTimelineM;
  if (dist) {
    let offset = Number.NaN;
    for (let k = 0; k < count && !Number.isFinite(offset); k++) offset = dist[startIdx + k] ?? Number.NaN;
    if (Number.isFinite(offset)) {
      distanceTimelineM = new Float64Array(count);
      for (let k = 0; k < count; k++) {
        const v = (dist[startIdx + k] ?? Number.NaN) - offset;
        distanceTimelineM[k] = v;
        if (Number.isFinite(v)) maxDistanceM = v;
      }
    }
  }

  return {
    lap,
    channelsByAppId,
    durationS: lap ? lap.lapTimeS : count / hz,
    maxDistanceM,
    distanceTimelineM,
    timeTimelineS,
  };
}

/**
 * The data of one lap of a parsed file, resampled onto a common timeline starting at 0 s / 0 m. Use it to compare
 * two laps from the same file.
 */
export function extractMotecLap(file: ParsedMotecFile, lap: MotecLap): MotecLapView {
  return buildView(file.session, lap);
}

/**
 * Parses an ArrayBuffer containing a binary MoTeC .ld file and optional .ldx companion.
 *
 * Detects laps and scopes the comparison data to the quickest valid lap. When no valid lap exists the whole file
 * is used (`scope: 'whole-file'`) and a warning says so. Throws a descriptive Error for files that are not
 * readable .ld files.
 */
export function parseMotecLd(
  buffer: ArrayBuffer | Uint8Array,
  filename = 'motec_log.ld',
  ldxContent?: string,
): ParsedMotecFile {
  const bytes = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
  if (bytes.byteLength === 0) {
    throw new Error('Invalid MoTeC .ld file: the file is empty (0 bytes).');
  }
  if (bytes.byteLength < LD_HEADER_SIZE) {
    throw new Error(
      `Invalid MoTeC .ld file: file length (${bytes.byteLength} bytes) is too small for the ${LD_HEADER_SIZE}-byte header.`,
    );
  }

  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const warnings: string[] = [];

  // --- 1. File header ---
  const marker = view.getUint32(0, true);
  if (marker !== LD_MARKER) {
    throw new Error(
      `Not a MoTeC .ld file: header marker is 0x${marker.toString(16)} at offset 0 (expected 0x${LD_MARKER.toString(16)}).`,
    );
  }
  const metaPtr = view.getUint32(8, true);
  const eventPtr = view.getUint32(36, true);
  const numChanns = view.getUint32(86, true);
  if (numChanns === 0) {
    throw new Error('Invalid MoTeC .ld file: the header declares 0 channels.');
  }
  if (metaPtr < LD_HEADER_SIZE || metaPtr + LD_CHANNEL_HEADER_SIZE > view.byteLength) {
    throw new Error(
      `Invalid MoTeC .ld file: channel table pointer ${metaPtr} is outside the file (${view.byteLength} bytes).`,
    );
  }

  const dateStr = readAscii(view, 94, 16);
  const timeStr = readAscii(view, 126, 16);
  let session: string | null = null;
  if (eventPtr > 0 && eventPtr + 128 <= view.byteLength) {
    session = orNull(readAscii(view, eventPtr + 64, 64));
  }
  const dateTimeStr = orNull(`${dateStr} ${timeStr}`.trim());

  const header: MotecHeader = {
    driver: orNull(readAscii(view, 158, 64)),
    vehicleId: orNull(readAscii(view, 222, 64)),
    venue: orNull(readAscii(view, 350, 64)),
    session,
    shortComment: orNull(readAscii(view, 1572, 64)),
    dateTimeStr,
    timestampMs: parseMotecDateTime(dateStr, timeStr),
    numChannels: numChanns,
  };

  // --- 2. Channel headers and samples ---
  const channels = new Map<string, MotecChannelData>();
  const visited = new Set<number>();
  let currMetaPtr = metaPtr;
  let chanIndex = 0;

  while (currMetaPtr > 0 && !visited.has(currMetaPtr)) {
    if (currMetaPtr + LD_CHANNEL_HEADER_SIZE > view.byteLength) {
      warnings.push(`Channel header at offset ${currMetaPtr} lies outside the file; remaining channels skipped.`);
      break;
    }
    if (chanIndex >= MAX_CHANNELS) {
      warnings.push(`More than ${MAX_CHANNELS} channel headers; the rest were skipped.`);
      break;
    }
    visited.add(currMetaPtr);
    chanIndex++;

    const nextMetaPtr = view.getUint32(currMetaPtr + 4, true);
    const dataPtr = view.getUint32(currMetaPtr + 8, true);
    const declaredCount = view.getUint32(currMetaPtr + 12, true);
    const dtypeA = view.getUint16(currMetaPtr + 18, true);
    const dtype = view.getUint16(currMetaPtr + 20, true);
    const freq = view.getUint16(currMetaPtr + 22, true);
    const shift = view.getInt16(currMetaPtr + 24, true);
    const mul = view.getInt16(currMetaPtr + 26, true);
    const scale = view.getInt16(currMetaPtr + 28, true);
    const decPlaces = view.getInt16(currMetaPtr + 30, true);
    const rawName = readAscii(view, currMetaPtr + 32, 32);
    const name = rawName || `Unnamed channel ${chanIndex}`;
    const shortName = readAscii(view, currMetaPtr + 64, 8);
    const unit = readAscii(view, currMetaPtr + 72, 12);

    currMetaPtr = nextMetaPtr;

    const sampleType = decodeSampleType(dtypeA, dtype);
    if (!sampleType) {
      throw new Error(
        `Unsupported MoTeC sample type in channel "${name}" (dtype_a=0x${dtypeA.toString(16)}, dtype=${dtype}).`,
      );
    }
    if (freq === 0) {
      warnings.push(`Channel "${name}" has a sample rate of 0 Hz; skipped.`);
      continue;
    }
    if (scale === 0 || mul === 0) {
      warnings.push(`Channel "${name}" has an invalid calibration (scale ${scale}, mul ${mul}); skipped.`);
      continue;
    }

    // Bounds check BEFORE allocating: never trust the declared count.
    const available =
      dataPtr >= view.byteLength ? 0 : Math.floor((view.byteLength - dataPtr) / sampleType.bytes);
    const readCount = Math.min(declaredCount, available);
    if (readCount < declaredCount) {
      warnings.push(
        `Channel "${name}": file is truncated or the sample count is corrupt; read ${readCount} of ${declaredCount} declared samples.`,
      );
    }
    if (readCount === 0) {
      if (declaredCount === 0) warnings.push(`Channel "${name}" has no samples; skipped.`);
      continue;
    }

    const samples = new Float64Array(readCount);
    const decFactor = Math.pow(10, -decPlaces);
    let offset = dataPtr;
    for (let i = 0; i < readCount; i++) {
      let rawVal: number;
      switch (sampleType.type) {
        case 'f32':
          rawVal = view.getFloat32(offset, true);
          break;
        case 'f16':
          rawVal = decodeFloat16(view.getUint16(offset, true));
          break;
        case 'f64':
          rawVal = view.getFloat64(offset, true);
          break;
        case 'i32':
          rawVal = view.getInt32(offset, true);
          break;
        case 'i16':
          rawVal = view.getInt16(offset, true);
          break;
      }
      const calibrated = ((rawVal / scale) * decFactor + shift) * mul;
      samples[i] = Number.isFinite(calibrated) ? calibrated : Number.NaN;
      offset += sampleType.bytes;
    }

    const meta: MotecChannelMeta = {
      id: chanIndex,
      name,
      shortName,
      unit,
      frequencyHz: freq,
      sampleCount: readCount,
      declaredSampleCount: declaredCount,
      scale,
      shift,
      mul,
      decPlaces,
      dataType: sampleType.type,
      dataPtr,
    };
    const key = name.toLowerCase();
    if (channels.has(key)) {
      warnings.push(`Duplicate channel name "${name}"; only the first is used.`);
      continue;
    }
    channels.set(key, { meta, samples });
  }

  if (chanIndex !== numChanns) {
    warnings.push(`The header declares ${numChanns} channels but ${chanIndex} channel headers were found.`);
  }
  if (channels.size === 0) {
    throw new Error('Invalid MoTeC .ld file: no channel contains any samples.');
  }

  // --- 3. Common timeline ---
  let primaryFrequencyHz = 0;
  let rawSessionDurationS = 0;
  for (const c of channels.values()) {
    primaryFrequencyHz = Math.max(primaryFrequencyHz, c.meta.frequencyHz);
    rawSessionDurationS = Math.max(rawSessionDurationS, c.meta.sampleCount / c.meta.frequencyHz);
  }
  const sampleCount = Math.max(1, Math.round(rawSessionDurationS * primaryFrequencyHz));
  for (const c of channels.values()) {
    const covered = c.meta.sampleCount / c.meta.frequencyHz;
    if (rawSessionDurationS - covered > Math.max(2 / c.meta.frequencyHz, 0.1)) {
      warnings.push(
        `Channel "${c.meta.name}" covers ${covered.toFixed(2)} s of the ${rawSessionDurationS.toFixed(2)} s file; the rest has no data.`,
      );
    }
  }

  // --- 4. Registry mapping with unit conversion ---
  const appChannels = new Map<number, MotecChannelData>();
  for (const c of channels.values()) {
    const appId = matchChannelNameToAppId(c.meta.name);
    if (appId === undefined || appChannels.has(appId)) continue;
    const convert = motecUnitConverter(appId, c.meta.unit);
    if (!convert) {
      const symbol = registryUnitSymbol(appId);
      const target = symbol !== '' ? symbol : (registryUnit(appId) ?? 'registry unit');
      warnings.push(
        `Channel "${c.meta.name}" (unit "${c.meta.unit}") not used: its unit cannot be converted to ${target}.`,
      );
      continue;
    }
    const si = new Float64Array(c.samples.length);
    for (let i = 0; i < si.length; i++) {
      const v = convert(c.samples[i] ?? Number.NaN);
      si[i] = Number.isFinite(v) ? v : Number.NaN;
    }
    appChannels.set(appId, { meta: c.meta, samples: si, appChannelId: appId });
  }

  // --- 5. Whole-file distance ---
  let distanceTimelineM: Float64Array | null = null;
  let distanceSource: MotecSessionData['distanceSource'] = null;
  const lapDist = appChannels.get(ch.LAP_DISTANCE);
  const speed = appChannels.get(ch.SPEED);
  if (lapDist) {
    distanceTimelineM = resampleByTime(lapDist.samples, lapDist.meta.frequencyHz, 0, primaryFrequencyHz, sampleCount, 'distance');
    distanceSource = 'lap-distance';
  } else if (speed) {
    const mps = resampleByTime(speed.samples, speed.meta.frequencyHz, 0, primaryFrequencyHz, sampleCount, 'linear');
    distanceTimelineM = new Float64Array(sampleCount);
    const dt = 1 / primaryFrequencyHz;
    let accum = 0;
    let missing = 0;
    for (let i = 0; i < sampleCount; i++) {
      const v = mps[i] ?? Number.NaN;
      if (Number.isFinite(v)) accum += Math.max(0, v) * dt;
      else missing++;
      distanceTimelineM[i] = accum;
    }
    distanceSource = 'integrated-speed';
    if (missing > 0) {
      warnings.push(`Distance is integrated from speed; ${missing} samples without speed add no distance.`);
    }
  } else {
    warnings.push('No lap distance or speed channel with a known unit: distance is unavailable.');
  }

  const sessionData: MotecSessionData = {
    primaryFrequencyHz,
    sampleCount,
    appChannels,
    distanceTimelineM,
    distanceSource,
  };

  let rawSessionDistanceM: number | null = null;
  if (distanceTimelineM && distanceSource === 'integrated-speed') {
    rawSessionDistanceM = distanceTimelineM[sampleCount - 1] ?? null;
  }

  // --- 6. Laps ---
  const boundaryResult = findBoundaries(channels, appChannels, rawSessionDurationS, ldxContent);
  const laps = boundaryResult
    ? buildLaps(boundaryResult, rawSessionDurationS, primaryFrequencyHz, sampleCount, distanceTimelineM)
    : [];
  const validLaps = laps.filter((l) => l.isValid);
  const fastestLap = validLaps.reduce<MotecLap | undefined>(
    (best, cur) => (best === undefined || cur.lapTimeS < best.lapTimeS ? cur : best),
    undefined,
  );

  if (!boundaryResult) {
    warnings.push(
      'No lap boundaries found (no .ldx markers, lap number, lap distance reset or beacon channel); showing the whole file.',
    );
  } else if (!fastestLap) {
    warnings.push('No complete, valid lap found; showing the whole file.');
  }

  const lapView = buildView(sessionData, fastestLap ?? null);

  return {
    ...lapView,
    filename,
    header,
    channels,
    scope: fastestLap ? 'fastest-lap' : 'whole-file',
    laps,
    fastestLap,
    lapSource: boundaryResult?.source ?? null,
    primaryFrequencyHz,
    rawSessionDurationS,
    rawSessionDistanceM,
    warnings,
    session: sessionData,
  };
}
