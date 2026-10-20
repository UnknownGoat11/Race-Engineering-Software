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
  'v car': ch.SPEED,
  'corr speed': ch.SPEED,
  'wheel speed': ch.SPEED,
  'vehicle speed': ch.SPEED,
  'speed kmh': ch.SPEED,
  'speed ms': ch.SPEED,
  'velocity': ch.SPEED,
  'wspeed': ch.SPEED,

  // Throttle
  'throttle pos': ch.THROTTLE,
  'throttle': ch.THROTTLE,
  'throt': ch.THROTTLE,
  'rthrottle': ch.THROTTLE,
  'athrottle': ch.THROTTLE,
  'pedal': ch.THROTTLE,
  'gas': ch.THROTTLE,
  'accel': ch.THROTTLE,
  'accel pos': ch.THROTTLE,
  'pedal pos': ch.THROTTLE,
  'throttle filtered': ch.THROTTLE_FILTERED,

  // Brake
  'brake pos': ch.BRAKE,
  'brake': ch.BRAKE,
  'brakes force': ch.BRAKE,
  'brake press': ch.BRAKE,
  'brake pressure': ch.BRAKE,
  'press brake': ch.BRAKE,
  'rbrake': ch.BRAKE,
  'abrake': ch.BRAKE,
  'brakepos': ch.BRAKE,
  'brake filtered': ch.BRAKE_FILTERED,

  // Clutch
  'clutch pos': ch.CLUTCH,
  'clutch': ch.CLUTCH,
  'rclutch': ch.CLUTCH,
  'aclutch': ch.CLUTCH,

  // Steering
  'steering pos': ch.STEERING,
  'steering': ch.STEERING,
  'steer': ch.STEERING,
  'steering angle': ch.STEERING,
  'steer angle': ch.STEERING,
  'steered angle': ch.STEERING,
  'str angle': ch.STEERING,
  'steering wheel angle': ch.STEERING,
  'asteer': ch.STEERING,

  // Engine
  'engine rpm': ch.RPM,
  'rpm': ch.RPM,
  'revs': ch.RPM,
  'nengine': ch.RPM,
  'engine speed': ch.RPM,
  'eng rpm': ch.RPM,
  'engine torque': ch.ENGINE_TORQUE,
  'torque': ch.ENGINE_TORQUE,
  'gear': ch.GEAR,
  'ngear': ch.GEAR,
  'current gear': ch.GEAR,

  // Accelerometer G
  'g force lat': ch.G_LAT,
  'g lat': ch.G_LAT,
  'lat g': ch.G_LAT,
  'lateral accel': ch.G_LAT,
  'lateral g': ch.G_LAT,
  'lat accel': ch.G_LAT,
  'alat': ch.G_LAT,
  'ay': ch.G_LAT,
  'g force long': ch.G_LONG,
  'g long': ch.G_LONG,
  'long g': ch.G_LONG,
  'longitudinal accel': ch.G_LONG,
  'longitudinal g': ch.G_LONG,
  'along': ch.G_LONG,
  'ax': ch.G_LONG,

  // Lap / Track Position
  'lap distance': ch.LAP_DISTANCE,
  'distance': ch.LAP_DISTANCE,
  'lapdist': ch.LAP_DISTANCE,
  'slap': ch.LAP_DISTANCE,
  'lap time': ch.LAP_TIME,
  'time': ch.LAP_TIME,
  'laptime': ch.LAP_TIME,

  // Tyre Pressures (kPa)
  'tyre pressure fl': ch.TYRE_PRESSURE_FL,
  'tyrespressure fl': ch.TYRE_PRESSURE_FL,
  'tyre press fl': ch.TYRE_PRESSURE_FL,
  'tire press fl': ch.TYRE_PRESSURE_FL,
  'fl press': ch.TYRE_PRESSURE_FL,
  'tyre pressure fr': ch.TYRE_PRESSURE_FR,
  'tyrespressure fr': ch.TYRE_PRESSURE_FR,
  'tyre press fr': ch.TYRE_PRESSURE_FR,
  'tire press fr': ch.TYRE_PRESSURE_FR,
  'fr press': ch.TYRE_PRESSURE_FR,
  'tyre pressure rl': ch.TYRE_PRESSURE_RL,
  'tyrespressure rl': ch.TYRE_PRESSURE_RL,
  'tyre press rl': ch.TYRE_PRESSURE_RL,
  'tire press rl': ch.TYRE_PRESSURE_RL,
  'rl press': ch.TYRE_PRESSURE_RL,
  'tyre pressure rr': ch.TYRE_PRESSURE_RR,
  'tyrespressure rr': ch.TYRE_PRESSURE_RR,
  'tyre press rr': ch.TYRE_PRESSURE_RR,
  'tire press rr': ch.TYRE_PRESSURE_RR,
  'rr press': ch.TYRE_PRESSURE_RR,

  // Brake Temperatures (°C)
  'brake temp fl': ch.BRAKE_TEMP_FL,
  'fl brake temp': ch.BRAKE_TEMP_FL,
  'disc temp fl': ch.BRAKE_TEMP_FL,
  'brake temp fr': ch.BRAKE_TEMP_FR,
  'fr brake temp': ch.BRAKE_TEMP_FR,
  'disc temp fr': ch.BRAKE_TEMP_FR,
  'brake temp rl': ch.BRAKE_TEMP_RL,
  'rl brake temp': ch.BRAKE_TEMP_RL,
  'disc temp rl': ch.BRAKE_TEMP_RL,
  'brake temp rr': ch.BRAKE_TEMP_RR,
  'rr brake temp': ch.BRAKE_TEMP_RR,
  'disc temp rr': ch.BRAKE_TEMP_RR,

  // Tyre Temperatures Inner / Core / Outer
  'tyre temp fl inner': ch.TYRE_TEMP_INNER_FL,
  'tyre temp fl core': ch.TYRE_TEMP_MIDDLE_FL,
  'tyre temp fl outer': ch.TYRE_TEMP_OUTER_FL,
  'tire temp fl inner': ch.TYRE_TEMP_INNER_FL,
  'tire temp fl core': ch.TYRE_TEMP_MIDDLE_FL,
  'tire temp fl outer': ch.TYRE_TEMP_OUTER_FL,
  'tyre temp fr inner': ch.TYRE_TEMP_INNER_FR,
  'tyre temp fr core': ch.TYRE_TEMP_MIDDLE_FR,
  'tyre temp fr outer': ch.TYRE_TEMP_OUTER_FR,
  'tire temp fr inner': ch.TYRE_TEMP_INNER_FR,
  'tire temp fr core': ch.TYRE_TEMP_MIDDLE_FR,
  'tire temp fr outer': ch.TYRE_TEMP_OUTER_FR,
  'tyre temp rl inner': ch.TYRE_TEMP_INNER_RL,
  'tyre temp rl core': ch.TYRE_TEMP_MIDDLE_RL,
  'tyre temp rl outer': ch.TYRE_TEMP_OUTER_RL,
  'tire temp rl inner': ch.TYRE_TEMP_INNER_RL,
  'tire temp rl core': ch.TYRE_TEMP_MIDDLE_RL,
  'tire temp rl outer': ch.TYRE_TEMP_OUTER_RL,
  'tyre temp rr inner': ch.TYRE_TEMP_INNER_RR,
  'tyre temp rr core': ch.TYRE_TEMP_MIDDLE_RR,
  'tyre temp rr outer': ch.TYRE_TEMP_OUTER_RR,
  'tire temp rr inner': ch.TYRE_TEMP_INNER_RR,
  'tire temp rr core': ch.TYRE_TEMP_MIDDLE_RR,
  'tire temp rr outer': ch.TYRE_TEMP_OUTER_RR,
};

export function matchChannelNameToAppId(name: string): number | undefined {
  const norm = name.trim().toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
  if (MOTEC_NAME_TO_CH_ID[norm] !== undefined) {
    return MOTEC_NAME_TO_CH_ID[norm];
  }
  const noSpaces = norm.replace(/\s+/g, '');
  if (MOTEC_NAME_TO_CH_ID[noSpaces] !== undefined) {
    return MOTEC_NAME_TO_CH_ID[noSpaces];
  }
  return undefined;
}

/**
 * Parses MoTeC .ldx XML companion content to extract lap beacon markers.
 */
export function parseLdxMarkers(xmlContent: string): { timeS: number; distM: number; name: string }[] {
  const markers: { timeS: number; distM: number; name: string }[] = [];
  if (!xmlContent || !xmlContent.includes('<Marker')) return markers;

  // Match all <Marker ... /> or <Marker ...>...</Marker>
  const markerRegex = /<Marker\b([^>]*)/gi;
  let match: RegExpExecArray | null;

  while ((match = markerRegex.exec(xmlContent)) !== null) {
    const attrs = match[1] || '';
    const timeMatch = /Time="([^"]+)"/i.exec(attrs);
    const distMatch = /Distance="([^"]+)"/i.exec(attrs);
    const nameMatch = /(?:Name|String)="([^"]+)"/i.exec(attrs);

    if (timeMatch && timeMatch[1]) {
      let timeVal = parseFloat(timeMatch[1]);
      // If time is expressed in microseconds (> 100,000), convert to seconds
      if (timeVal > 100000) {
        timeVal = timeVal / 1000000;
      }
      const distVal = distMatch && distMatch[1] ? parseFloat(distMatch[1]) : 0;
      const name = nameMatch && nameMatch[1] ? nameMatch[1] : `Marker_${markers.length}`;

      if (Number.isFinite(timeVal)) {
        markers.push({ timeS: timeVal, distM: distVal, name });
      }
    }
  }

  // Sort markers by time
  markers.sort((a, b) => a.timeS - b.timeS);
  return markers;
}

/**
 * Detects all laps within a MoTeC session using:
 * 1. .ldx companion markers (if provided)
 * 2. Lap Number channels (1 -> 2 -> 3)
 * 3. Lap Distance reset drops (> 800m drop)
 * 4. Beacon pulse channels (0 -> 1)
 */
function detectSessionLaps(
  channels: Map<string, MotecChannelData>,
  channelsByAppId: Map<number, MotecChannelData>,
  distanceTimelineM: Float64Array,
  timeTimelineS: Float64Array,
  primaryFreqHz: number,
  ldxContent?: string,
): MotecLap[] {
  const totalSamples = timeTimelineS.length;
  const totalDurationS = timeTimelineS[totalSamples - 1] ?? 0;
  const totalDistanceM = distanceTimelineM[totalSamples - 1] ?? 0;

  if (totalSamples < 10) return [];

  // 1. Try parsing .ldx companion markers if provided
  if (ldxContent) {
    const markers = parseLdxMarkers(ldxContent);
    if (markers.length >= 2) {
      const laps: MotecLap[] = [];
      for (let i = 0; i < markers.length - 1; i++) {
        const mStart = markers[i]!;
        const mEnd = markers[i + 1]!;
        const startTimeS = mStart.timeS;
        const endTimeS = mEnd.timeS;
        const lapTimeS = endTimeS - startTimeS;
        if (lapTimeS > 10) {
          const startIdx = Math.min(totalSamples - 1, Math.max(0, Math.round(startTimeS * primaryFreqHz)));
          const endIdx = Math.min(totalSamples - 1, Math.max(startIdx, Math.round(endTimeS * primaryFreqHz) - 1));
          const startDistM = distanceTimelineM[startIdx] ?? mStart.distM;
          const endDistM = distanceTimelineM[endIdx] ?? mEnd.distM;

          laps.push({
            lapNumber: laps.length + 1,
            lapTimeS,
            startTimeS,
            endTimeS,
            startDistM,
            endDistM,
            startSampleIdx: startIdx,
            endSampleIdx: endIdx,
            isValid: lapTimeS >= 25,
          });
        }
      }
      if (laps.length > 0) return laps;
    }
  }

  // 2. Try detecting from Lap Number channel
  const lapChan =
    channelsByAppId.get(ch.LAP_NUMBER) ||
    channels.get('lap') ||
    channels.get('lap number') ||
    channels.get('lap count') ||
    channels.get('current lap') ||
    channels.get('lap_number') ||
    channels.get('lap_index') ||
    channels.get('lapnum') ||
    channels.get('mlap') ||
    channels.get('lap_count') ||
    channels.get('car_lap');

  if (lapChan && lapChan.samples.length === totalSamples) {
    const boundaries: { lapNum: number; startIdx: number }[] = [];
    let lastLap = -999;

    for (let i = 0; i < totalSamples; i++) {
      const cur = Math.round(lapChan.samples[i] ?? 0);
      if (cur !== lastLap && cur >= 1) {
        boundaries.push({ lapNum: cur, startIdx: i });
        lastLap = cur;
      }
    }

    if (boundaries.length > 1) {
      const laps: MotecLap[] = [];
      for (let k = 0; k < boundaries.length; k++) {
        const startIdx = boundaries[k]!.startIdx;
        const endIdx = k + 1 < boundaries.length ? boundaries[k + 1]!.startIdx - 1 : totalSamples - 1;
        const startTimeS = timeTimelineS[startIdx] ?? 0;
        const endTimeS = timeTimelineS[endIdx] ?? 0;
        const lapTimeS = endTimeS - startTimeS;
        const startDistM = distanceTimelineM[startIdx] ?? 0;
        const endDistM = distanceTimelineM[endIdx] ?? 0;

        if (lapTimeS > 15) {
          laps.push({
            lapNumber: boundaries[k]!.lapNum,
            lapTimeS,
            startTimeS,
            endTimeS,
            startDistM,
            endDistM,
            startSampleIdx: startIdx,
            endSampleIdx: endIdx,
            isValid: lapTimeS >= 25,
          });
        }
      }
      if (laps.length > 0) return laps;
    }
  }

  // 3. Try detecting from Lap Distance Reset
  const distChan =
    channelsByAppId.get(ch.LAP_DISTANCE) ||
    channels.get('lap distance') ||
    channels.get('lapdist') ||
    channels.get('lap dist') ||
    channels.get('slap') ||
    channels.get('lap_distance') ||
    channels.get('distance in lap');

  if (distChan && distChan.samples.length === totalSamples) {
    const cutIndices: number[] = [0];
    for (let i = 1; i < totalSamples; i++) {
      const prev = distChan.samples[i - 1] ?? 0;
      const cur = distChan.samples[i] ?? 0;
      // Sharp drop indicating finish line crossing (e.g. from >1000m to <200m)
      if (prev - cur > 800 && prev > 1000) {
        const lastCut = cutIndices[cutIndices.length - 1] ?? 0;
        if ((i - lastCut) / primaryFreqHz > 25) {
          cutIndices.push(i);
        }
      }
    }

    if (cutIndices.length > 1) {
      const laps: MotecLap[] = [];
      for (let k = 0; k < cutIndices.length; k++) {
        const startIdx = cutIndices[k]!;
        const endIdx = k + 1 < cutIndices.length ? cutIndices[k + 1]! - 1 : totalSamples - 1;
        const startTimeS = timeTimelineS[startIdx] ?? 0;
        const endTimeS = timeTimelineS[endIdx] ?? 0;
        const lapTimeS = endTimeS - startTimeS;
        const startDistM = distanceTimelineM[startIdx] ?? 0;
        const endDistM = distanceTimelineM[endIdx] ?? 0;

        if (lapTimeS > 15) {
          laps.push({
            lapNumber: k + 1,
            lapTimeS,
            startTimeS,
            endTimeS,
            startDistM,
            endDistM,
            startSampleIdx: startIdx,
            endSampleIdx: endIdx,
            isValid: lapTimeS >= 25,
          });
        }
      }
      if (laps.length > 0) return laps;
    }
  }

  // 4. Try detecting from Beacon Marker Pulses
  const beaconChan =
    channels.get('beacon') ||
    channels.get('lap marker') ||
    channels.get('beacon markers') ||
    channels.get('beacon pulse') ||
    channels.get('split beacon');

  if (beaconChan && beaconChan.samples.length === totalSamples) {
    const beaconIndices: number[] = [0];
    for (let i = 1; i < totalSamples; i++) {
      if ((beaconChan.samples[i - 1] ?? 0) <= 0.5 && (beaconChan.samples[i] ?? 0) > 0.5) {
        const lastIdx = beaconIndices[beaconIndices.length - 1] ?? 0;
        if ((i - lastIdx) / primaryFreqHz > 25) {
          beaconIndices.push(i);
        }
      }
    }

    if (beaconIndices.length > 1) {
      const laps: MotecLap[] = [];
      for (let k = 0; k < beaconIndices.length; k++) {
        const startIdx = beaconIndices[k]!;
        const endIdx = k + 1 < beaconIndices.length ? beaconIndices[k + 1]! - 1 : totalSamples - 1;
        const startTimeS = timeTimelineS[startIdx] ?? 0;
        const endTimeS = timeTimelineS[endIdx] ?? 0;
        const lapTimeS = endTimeS - startTimeS;
        const startDistM = distanceTimelineM[startIdx] ?? 0;
        const endDistM = distanceTimelineM[endIdx] ?? 0;

        if (lapTimeS > 15) {
          laps.push({
            lapNumber: k + 1,
            lapTimeS,
            startTimeS,
            endTimeS,
            startDistM,
            endDistM,
            startSampleIdx: startIdx,
            endSampleIdx: endIdx,
            isValid: lapTimeS >= 25,
          });
        }
      }
      if (laps.length > 0) return laps;
    }
  }

  // 5. Fallback: single lap covering the entire file
  return [
    {
      lapNumber: 1,
      lapTimeS: totalDurationS,
      startTimeS: 0,
      endTimeS: totalDurationS,
      startDistM: 0,
      endDistM: totalDistanceM,
      startSampleIdx: 0,
      endSampleIdx: totalSamples - 1,
      isValid: true,
    },
  ];
}

/**
 * Parses an ArrayBuffer containing a binary MoTeC .ld file and optional .ldx companion.
 *
 * Automatically detects laps and slices the dataset to ONLY the quickest lap
 * for accurate 1:1 telemetry comparison against live and target runs.
 */
export function parseMotecLd(
  buffer: ArrayBuffer | Uint8Array,
  filename = 'motec_log.ld',
  ldxContent?: string,
): ParsedMotecFile {
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
  const distChan =
    channelsByAppId.get(ch.LAP_DISTANCE) ||
    channels.get('lap distance') ||
    channels.get('distance') ||
    channels.get('slap');
  const speedChan =
    channelsByAppId.get(ch.SPEED) ||
    channels.get('ground speed') ||
    channels.get('speed') ||
    channels.get('vcar') ||
    channels.get('gps speed');

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

  // --- 5. Laps Breakdown & Quickest Lap Extraction ---
  const detectedLaps = detectSessionLaps(
    channels,
    channelsByAppId,
    distanceTimelineM,
    timeTimelineS,
    primaryFreqHz,
    ldxContent,
  );

  // Identify the quickest (fastest) lap in the session
  const validLaps = detectedLaps.filter((l) => l.isValid);
  const quickestLap = (validLaps.length > 0 ? validLaps : detectedLaps).reduce(
    (best, cur) => (cur.lapTimeS < best.lapTimeS ? cur : best),
    detectedLaps[0] || {
      lapNumber: 1,
      lapTimeS: durationS,
      startTimeS: 0,
      endTimeS: durationS,
      startDistM: 0,
      endDistM: maxDistanceM,
      isValid: true,
    },
  );

  const rawSessionDurationS = durationS;
  const rawSessionDistanceM = maxDistanceM;

  // Slicing channels to ONLY the quickest lap for comparison:
  let finalChannels = channels;
  let finalChannelsByAppId = channelsByAppId;
  let finalDistanceTimelineM = distanceTimelineM;
  let finalTimeTimelineS = timeTimelineS;
  let finalDurationS = durationS;
  let finalMaxDistanceM = maxDistanceM;

  if (quickestLap && detectedLaps.length > 1) {
    const qStart = quickestLap.startSampleIdx ?? 0;
    const qEnd = quickestLap.endSampleIdx ?? (primarySampleCount - 1);
    const qCount = Math.max(1, qEnd - qStart + 1);

    // Re-base distance timeline so it starts at 0m and ends at lap distance
    const slicedDist = new Float64Array(qCount);
    const distOffset = distanceTimelineM[qStart] ?? 0;
    for (let i = 0; i < qCount; i++) {
      slicedDist[i] = Math.max(0, (distanceTimelineM[qStart + i] ?? 0) - distOffset);
    }

    // Re-base time timeline so it starts at 0s and ends at lapTimeS
    const slicedTime = new Float64Array(qCount);
    for (let i = 0; i < qCount; i++) {
      slicedTime[i] = i / primaryFreqHz;
    }

    // Slice all channels proportionately according to their sample rate
    const slicedChans = new Map<string, MotecChannelData>();
    const slicedByAppId = new Map<number, MotecChannelData>();

    for (const [name, chan] of channels) {
      const rateRatio = primaryFreqHz > 0 ? chan.meta.frequencyHz / primaryFreqHz : 1;
      const chanStart = Math.min(chan.samples.length - 1, Math.max(0, Math.round(qStart * rateRatio)));
      const chanEnd = Math.min(chan.samples.length - 1, Math.max(chanStart, Math.round(qEnd * rateRatio)));
      const slicedSamples = chan.samples.slice(chanStart, chanEnd + 1);

      const slicedData: MotecChannelData = {
        meta: { ...chan.meta, sampleCount: slicedSamples.length },
        samples: slicedSamples,
        appChannelId: chan.appChannelId,
      };

      slicedChans.set(name, slicedData);
      if (chan.appChannelId !== undefined) {
        slicedByAppId.set(chan.appChannelId, slicedData);
      }
    }

    finalChannels = slicedChans;
    finalChannelsByAppId = slicedByAppId;
    finalDistanceTimelineM = slicedDist;
    finalTimeTimelineS = slicedTime;
    finalDurationS = quickestLap.lapTimeS;
    finalMaxDistanceM = slicedDist[qCount - 1] ?? (quickestLap.endDistM - quickestLap.startDistM);
  }

  return {
    filename,
    header,
    channels: finalChannels,
    channelsByAppId: finalChannelsByAppId,
    durationS: finalDurationS,
    maxDistanceM: finalMaxDistanceM,
    distanceTimelineM: finalDistanceTimelineM,
    timeTimelineS: finalTimeTimelineS,
    laps: detectedLaps,
    fastestLap: quickestLap,
    rawSessionDurationS,
    rawSessionDistanceM,
  };
}
