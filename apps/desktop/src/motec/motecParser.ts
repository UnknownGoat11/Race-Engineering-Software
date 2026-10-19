/**
 * MoTeC i2 Pro Logged Data (.ld) Binary File Parser.
 *
 * Fully decodes header metadata, channel linked-lists, and raw sample streams
 * adhering to the MoTeC binary specification.
 */
import { ch } from '../generated/channels';
import type {
  MotecChannelData,
  MotecChannelMeta,
  MotecDataType,
  MotecHeader,
  MotecLap,
  ParsedMotecFile,
} from './motecTypes';

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
    return mant ? Number.NaN : (sign ? -Infinity : Infinity);
  }
  return (sign ? -1 : 1) * Math.pow(2, exp - 15) * (1 + mant / 1024);
}

/**
 * Reads an ASCII null-terminated string of fixed length from a DataView.
 */
function readAscii(view: DataView, offset: number, maxLen: number): string {
  if (offset + maxLen > view.byteLength) {
    maxLen = Math.max(0, view.byteLength - offset);
  }
  let str = '';
  for (let i = 0; i < maxLen; i++) {
    const code = view.getUint8(offset + i);
    if (code === 0) break;
    str += String.fromCharCode(code);
  }
  return str.trim();
}

/**
 * Common MoTeC channel name mappings to our canonical channel IDs.
 */
const MOTEC_NAME_TO_CH_ID: Readonly<Record<string, number>> = {
  // Speed
  'ground speed': ch.SPEED,
  'speed': ch.SPEED,
  'car speed': ch.SPEED,
  'gps speed': ch.SPEED,
  'vcar': ch.SPEED,

  // Throttle
  'throttle pos': ch.THROTTLE,
  'throttle': ch.THROTTLE,
  'throt': ch.THROTTLE,
  'rthrottle': ch.THROTTLE,
  'throttle filtered': ch.THROTTLE_FILTERED,

  // Brake
  'brake pos': ch.BRAKE,
  'brake': ch.BRAKE,
  'brakes force': ch.BRAKE,
  'rbrake': ch.BRAKE,
  'brake filtered': ch.BRAKE_FILTERED,

  // Clutch
  'clutch pos': ch.CLUTCH,
  'clutch': ch.CLUTCH,
  'rclutch': ch.CLUTCH,

  // Steering
  'steering pos': ch.STEERING,
  'steering': ch.STEERING,
  'steer': ch.STEERING,
  'steering angle': ch.STEERING,
  'asteer': ch.STEERING,

  // Engine
  'engine rpm': ch.RPM,
  'rpm': ch.RPM,
  'revs': ch.RPM,
  'nengine': ch.RPM,
  'engine torque': ch.ENGINE_TORQUE,
  'torque': ch.ENGINE_TORQUE,
  'gear': ch.GEAR,
  'ngear': ch.GEAR,

  // Accelerometer G
  'g force lat': ch.G_LAT,
  'g lat': ch.G_LAT,
  'lateral accel': ch.G_LAT,
  'alat': ch.G_LAT,
  'g force long': ch.G_LONG,
  'g long': ch.G_LONG,
  'longitudinal accel': ch.G_LONG,
  'along': ch.G_LONG,

  // Lap / Track Position
  'lap distance': ch.LAP_DISTANCE,
  'distance': ch.LAP_DISTANCE,
  'lapdist': ch.LAP_DISTANCE,
  'slap': ch.LAP_DISTANCE,
  'lap time': ch.LAP_TIME,
  'time': ch.LAP_TIME,

  // Tyre Pressures (kPa)
  'tyre pressure fl': ch.TYRE_PRESSURE_FL,
  'tyrespressure fl': ch.TYRE_PRESSURE_FL,
  'tyre pressure fr': ch.TYRE_PRESSURE_FR,
  'tyrespressure fr': ch.TYRE_PRESSURE_FR,
  'tyre pressure rl': ch.TYRE_PRESSURE_RL,
  'tyrespressure rl': ch.TYRE_PRESSURE_RL,
  'tyre pressure rr': ch.TYRE_PRESSURE_RR,
  'tyrespressure rr': ch.TYRE_PRESSURE_RR,

  // Brake Temperatures (°C)
  'brake temp fl': ch.BRAKE_TEMP_FL,
  'brake temp fr': ch.BRAKE_TEMP_FR,
  'brake temp rl': ch.BRAKE_TEMP_RL,
  'brake temp rr': ch.BRAKE_TEMP_RR,

  // Tyre Temperatures Inner / Core / Outer
  'tyre temp fl inner': ch.TYRE_TEMP_INNER_FL,
  'tyre temp fl core': ch.TYRE_TEMP_MIDDLE_FL,
  'tyre temp fl outer': ch.TYRE_TEMP_OUTER_FL,
  'tyre temp fr inner': ch.TYRE_TEMP_INNER_FR,
  'tyre temp fr core': ch.TYRE_TEMP_MIDDLE_FR,
  'tyre temp fr outer': ch.TYRE_TEMP_OUTER_FR,
};

export function matchChannelNameToAppId(name: string): number | undefined {
  const norm = name.trim().toLowerCase().replace(/[_]/g, ' ');
  return MOTEC_NAME_TO_CH_ID[norm];
}

/**
 * Parses an ArrayBuffer containing a binary MoTeC .ld file.
 */
export function parseMotecLd(buffer: ArrayBuffer | Uint8Array, filename = 'motec_log.ld'): ParsedMotecFile {
  const bytes = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
  if (bytes.byteLength < 128) {
    throw new Error(`Invalid MoTeC .ld file: file length (${bytes.byteLength} bytes) is too small.`);
  }

  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);

  // --- 1. File Header (ldHead) ---
  const metaPtr = view.getUint32(8, true);
  const eventPtr = view.getUint32(36, true);
  const numChanns = view.getUint32(86, true);

  const dateStr = readAscii(view, 94, 16);
  const timeStr = readAscii(view, 126, 16);
  const driver = readAscii(view, 158, 64) || 'MoTeC Driver';
  const vehicleId = readAscii(view, 222, 64) || 'MoTeC Car';
  const venue = readAscii(view, 350, 64) || 'Race Circuit';
  const shortComment = view.byteLength >= 1636 ? readAscii(view, 1572, 64) : '';

  let sessionName = 'Practice / Test';
  if (eventPtr > 0 && eventPtr + 128 <= view.byteLength) {
    const rawSession = readAscii(view, eventPtr + 64, 64);
    if (rawSession) sessionName = rawSession;
  }

  const dateTimeStr = `${dateStr} ${timeStr}`.trim();
  const timestampMs = Date.parse(dateTimeStr) || Date.now();

  const header: MotecHeader = {
    driver,
    vehicleId,
    venue,
    session: sessionName,
    shortComment,
    dateTimeStr,
    timestampMs,
    numChannels: numChanns,
  };

  // --- 2. Channel Headers Linked List (ldChan) ---
  const channels = new Map<string, MotecChannelData>();
  const channelsByAppId = new Map<number, MotecChannelData>();

  let currMetaPtr = metaPtr;
  let safetyLoopCounter = 0;
  const maxChannelsAllowed = 2048;

  let primarySampleCount = 0;
  let primaryFreqHz = 100;

  while (currMetaPtr > 0 && currMetaPtr + 84 <= view.byteLength && safetyLoopCounter < maxChannelsAllowed) {
    safetyLoopCounter++;

    const nextMetaPtr = view.getUint32(currMetaPtr + 4, true);
    const chanDataPtr = view.getUint32(currMetaPtr + 8, true);
    const chanDataLen = view.getUint32(currMetaPtr + 12, true);

    const dtypeA = view.getUint16(currMetaPtr + 18, true);
    const dtype = view.getUint16(currMetaPtr + 20, true);
    const freq = view.getUint16(currMetaPtr + 22, true) || 100;
    const shift = view.getInt16(currMetaPtr + 24, true);
    const mul = view.getInt16(currMetaPtr + 26, true) || 1;
    const scale = view.getInt16(currMetaPtr + 28, true) || 1;
    const decPlaces = view.getInt16(currMetaPtr + 30, true);

    const name = readAscii(view, currMetaPtr + 32, 32);
    const shortName = readAscii(view, currMetaPtr + 64, 8);
    const unit = readAscii(view, currMetaPtr + 72, 12);

    let dataType: MotecDataType = 'i16';
    let bytesPerSample = 2;

    if (dtypeA === 0x07 && (dtype === 4 || dtype === 0)) {
      dataType = 'f32';
      bytesPerSample = 4;
    } else if (dtypeA === 0x07 && dtype === 2) {
      dataType = 'f16';
      bytesPerSample = 2;
    } else if (dtypeA === 0x08 && dtype === 8) {
      dataType = 'f64';
      bytesPerSample = 8;
    } else if ((dtypeA === 0 || dtypeA === 3 || dtypeA === 5) && dtype === 4) {
      dataType = 'i32';
      bytesPerSample = 4;
    } else {
      dataType = 'i16';
      bytesPerSample = 2;
    }

    const chanMeta: MotecChannelMeta = {
      id: safetyLoopCounter,
      name: name || `Channel_${safetyLoopCounter}`,
      shortName: shortName || `Ch${safetyLoopCounter}`,
      unit: unit || '',
      frequencyHz: freq,
      sampleCount: chanDataLen,
      scale,
      shift,
      mul,
      decPlaces,
      dataType,
      dataPtr: chanDataPtr,
    };

    // --- 3. Read Raw Data Samples ---
    const samples = new Float64Array(chanDataLen);
    const decFactor = Math.pow(10, -decPlaces);
    const scaleDiv = scale !== 0 ? scale : 1;

    let readOffset = chanDataPtr;
    for (let i = 0; i < chanDataLen; i++) {
      if (readOffset + bytesPerSample > view.byteLength) {
        break; // file truncated safely
      }

      let rawVal = 0;
      if (dataType === 'f32') {
        rawVal = view.getFloat32(readOffset, true);
      } else if (dataType === 'f16') {
        rawVal = decodeFloat16(view.getUint16(readOffset, true));
      } else if (dataType === 'f64') {
        rawVal = view.getFloat64(readOffset, true);
      } else if (dataType === 'i32') {
        rawVal = view.getInt32(readOffset, true);
      } else {
        rawVal = view.getInt16(readOffset, true);
      }

      // MoTeC Calibration Formula: ((raw / scale) * (10^-dec) + shift) * mul
      const calibrated = ((rawVal / scaleDiv) * decFactor + shift) * mul;
      samples[i] = Number.isFinite(calibrated) ? calibrated : 0;

      readOffset += bytesPerSample;
    }

    const appChannelId = matchChannelNameToAppId(chanMeta.name);
    const channelData: MotecChannelData = {
      meta: chanMeta,
      samples,
      appChannelId,
    };

    channels.set(chanMeta.name.toLowerCase(), channelData);
    if (appChannelId !== undefined && !channelsByAppId.has(appChannelId)) {
      channelsByAppId.set(appChannelId, channelData);
    }

    if (chanDataLen > primarySampleCount) {
      primarySampleCount = chanDataLen;
      primaryFreqHz = freq;
    }

    if (nextMetaPtr <= currMetaPtr || nextMetaPtr === 0) {
      break;
    }
    currMetaPtr = nextMetaPtr;
  }

  // --- 4. Synthesize Timelines & Distance ---
  const durationS = primaryFreqHz > 0 ? primarySampleCount / primaryFreqHz : 0;
  const timeTimelineS = new Float64Array(primarySampleCount);
  for (let i = 0; i < primarySampleCount; i++) {
    timeTimelineS[i] = i / primaryFreqHz;
  }

  // Distance timeline: from Lap Distance channel or integrated from Speed
  let distanceTimelineM = new Float64Array(primarySampleCount);
  const distChan = channels.get('lap distance') || channels.get('distance') || channels.get('slap');
  const speedChan = channelsByAppId.get(ch.SPEED) || channels.get('ground speed') || channels.get('speed');

  if (distChan && distChan.samples.length === primarySampleCount) {
    distanceTimelineM = new Float64Array(distChan.samples);
  } else if (speedChan) {
    // Integrate speed (km/h or m/s) over time
    const dt = 1 / primaryFreqHz;
    let accumM = 0;
    const isKmh = speedChan.meta.unit.toLowerCase().includes('km') || (speedChan.samples[10] ?? 0) > 100;
    const speedRatio = isKmh ? 1 / 3.6 : 1.0;

    for (let i = 0; i < primarySampleCount; i++) {
      const spd = (speedChan.samples[i] ?? 0) * speedRatio;
      accumM += Math.max(0, spd) * dt;
      distanceTimelineM[i] = accumM;
    }
  } else {
    // Fallback linear distance
    for (let i = 0; i < primarySampleCount; i++) {
      distanceTimelineM[i] = i * 0.7; // ~70 m/s
    }
  }

  const maxDistanceM = distanceTimelineM[primarySampleCount - 1] ?? 0;

  // --- 5. Laps Breakdown ---
  const laps: MotecLap[] = [];
  if (durationS > 0) {
    // Single lap or detected laps
    laps.push({
      lapNumber: 1,
      lapTimeS: durationS,
      startTimeS: 0,
      endTimeS: durationS,
      startDistM: 0,
      endDistM: maxDistanceM,
      isValid: true,
    });
  }

  return {
    filename,
    header,
    channels,
    channelsByAppId,
    durationS,
    maxDistanceM,
    distanceTimelineM,
    timeTimelineS,
    laps,
    fastestLap: laps[0],
  };
}
