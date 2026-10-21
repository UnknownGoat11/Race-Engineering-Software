/**
 * MoTeC i2 Pro Logged Data (.ld) Binary Generator & Serializer.
 *
 * Encodes telemetry channels into the MoTeC binary layout (see motecParser.ts). Used by tests to build valid
 * and deliberately malformed files. The generators below produce SYNTHETIC data (sine-wave shapes): they are
 * not recordings of any real car, driver or lap and must never be presented as such.
 */

export interface MotecChannelInput {
  readonly name: string;
  readonly shortName?: string;
  readonly unit?: string;
  readonly frequencyHz?: number;
  readonly samples: ArrayLike<number>;
  readonly isFloat?: boolean;
  readonly scale?: number;
  readonly shift?: number;
  readonly mul?: number;
  readonly decPlaces?: number;
}

export interface MotecFileOptions {
  readonly driver?: string;
  readonly vehicleId?: string;
  readonly venue?: string;
  readonly session?: string;
  readonly comment?: string;
  readonly dateStr?: string;
  readonly timeStr?: string;
}

/** Byte layout used by `createMotecLdBuffer` (exported so tests can patch fields to build malformed files). */
export const WRITER_LAYOUT = {
  headerSize: 2048,
  eventSize: 1152,
  chanHeaderSize: 128,
  /** Offset of channel header `i`. */
  channelHeaderOffset(i: number): number {
    return this.headerSize + this.eventSize + i * this.chanHeaderSize;
  },
} as const;

function writeAscii(view: DataView, offset: number, str: string, maxLen: number): void {
  const enc = new TextEncoder();
  const bytes = enc.encode(str);
  const len = Math.min(bytes.length, maxLen - 1);
  for (let i = 0; i < len; i++) {
    view.setUint8(offset + i, bytes[i] ?? 0);
  }
  // null terminator
  view.setUint8(offset + len, 0);
}

/**
 * Creates a valid MoTeC .ld binary from a list of channels and metadata. Header fields not given stay empty.
 */
export function createMotecLdBuffer(
  channels: readonly MotecChannelInput[],
  options: MotecFileOptions = {},
): Uint8Array {
  const numChanns = channels.length;
  const { headerSize, eventSize, chanHeaderSize } = WRITER_LAYOUT;

  const eventPtr = headerSize;
  const metaPtr = headerSize + eventSize;
  const dataPtr = metaPtr + numChanns * chanHeaderSize;

  // Calculate total payload size
  let totalDataBytes = 0;
  for (const c of channels) {
    const bytesPerSample = (c.isFloat ?? true) ? 4 : 2;
    totalDataBytes += c.samples.length * bytesPerSample;
  }

  const totalFileSize = dataPtr + totalDataBytes;
  const buffer = new Uint8Array(totalFileSize);
  const view = new DataView(buffer.buffer);

  // 1. Write ldHead
  view.setUint32(0, 0x40, true); // ldmarker
  view.setUint32(8, metaPtr, true);
  view.setUint32(12, dataPtr, true);
  view.setUint32(36, eventPtr, true);
  view.setUint32(86, numChanns, true);

  writeAscii(view, 94, options.dateStr ?? '', 16);
  writeAscii(view, 126, options.timeStr ?? '', 16);
  writeAscii(view, 158, options.driver ?? '', 64);
  writeAscii(view, 222, options.vehicleId ?? '', 64);
  writeAscii(view, 350, options.venue ?? '', 64);
  writeAscii(view, 1572, options.comment ?? '', 64);

  // 2. Write ldEvent
  writeAscii(view, eventPtr, options.venue ?? '', 64);
  writeAscii(view, eventPtr + 64, options.session ?? '', 64);
  writeAscii(view, eventPtr + 128, options.comment ?? '', 1024);

  // 3. Write Channels & Raw Data
  let currChanDataPtr = dataPtr;
  for (const [i, c] of channels.entries()) {
    const isFloat = c.isFloat ?? true;
    const bytesPerSample = isFloat ? 4 : 2;
    const sampleCount = c.samples.length;
    const chanDataByteLen = sampleCount * bytesPerSample;

    const currMeta = WRITER_LAYOUT.channelHeaderOffset(i);
    const prevMeta = i > 0 ? WRITER_LAYOUT.channelHeaderOffset(i - 1) : 0;
    const nextMeta = i < numChanns - 1 ? WRITER_LAYOUT.channelHeaderOffset(i + 1) : 0;

    view.setUint32(currMeta, prevMeta, true);
    view.setUint32(currMeta + 4, nextMeta, true);
    view.setUint32(currMeta + 8, currChanDataPtr, true);
    view.setUint32(currMeta + 12, sampleCount, true);

    if (isFloat) {
      view.setUint16(currMeta + 18, 0x07, true); // dtype_a = float
      view.setUint16(currMeta + 20, 4, true); // dtype = 4 bytes (f32)
    } else {
      view.setUint16(currMeta + 18, 0x03, true); // dtype_a = int
      view.setUint16(currMeta + 20, 2, true); // dtype = 2 bytes (i16)
    }

    const scale = c.scale ?? 1;
    const shift = c.shift ?? 0;
    const mul = c.mul ?? 1;
    const decPlaces = c.decPlaces ?? 0;

    view.setUint16(currMeta + 22, c.frequencyHz ?? 100, true);
    view.setInt16(currMeta + 24, shift, true);
    view.setInt16(currMeta + 26, mul, true);
    view.setInt16(currMeta + 28, scale, true);
    view.setInt16(currMeta + 30, decPlaces, true);

    writeAscii(view, currMeta + 32, c.name, 32);
    writeAscii(view, currMeta + 64, c.shortName ?? c.name.slice(0, 6), 8);
    writeAscii(view, currMeta + 72, c.unit ?? '', 12);

    // Write samples
    let sampleOffset = currChanDataPtr;
    for (let s = 0; s < sampleCount; s++) {
      const val = c.samples[s] ?? 0;
      if (isFloat) {
        view.setFloat32(sampleOffset, val, true);
      } else {
        // Reverse calibration formula: raw = ((val / mul) - shift) * scale / 10^-dec
        const decFactor = Math.pow(10, -decPlaces);
        const raw = Math.round((((val / mul) - shift) * scale) / decFactor);
        view.setInt16(sampleOffset, raw, true);
      }
      sampleOffset += bytesPerSample;
    }

    currChanDataPtr += chanDataByteLen;
  }

  return buffer;
}

/** Header used for every synthetic file: says plainly that the data is generated. */
const SYNTHETIC_HEADER: MotecFileOptions = {
  driver: 'Synthetic data',
  vehicleId: 'Synthetic test car',
  venue: 'Synthetic test track',
  session: 'Synthetic demo lap',
  comment: 'Generated sine-wave data - not a real lap',
};

/**
 * Generates a SYNTHETIC single-run file (136.42 s at 100 Hz, sine-wave speed profile). Test data only.
 */
export function generateSyntheticDemoMotecBuffer(): Uint8Array {
  const sampleCount = 13642;
  const freq = 100;
  const dt = 1 / freq;

  const speedKmh = new Float32Array(sampleCount);
  const throttlePct = new Float32Array(sampleCount);
  const brakePct = new Float32Array(sampleCount);
  const rpm = new Float32Array(sampleCount);
  const gear = new Float32Array(sampleCount);
  const steerAngleDeg = new Float32Array(sampleCount);
  const distanceM = new Float32Array(sampleCount);

  let currentDist = 0;
  for (let i = 0; i < sampleCount; i++) {
    const t = i * dt;
    const phase = (i / sampleCount) * Math.PI * 10;
    const cornerFactor = Math.abs(Math.sin(phase));
    let spd = (280 - cornerFactor * 160 + Math.sin(t * 0.5) * 15) * 1.03;
    spd = Math.max(70, Math.min(335, spd));

    const throt = Math.max(0, Math.min(1.0, spd > 170 ? 1.0 : (spd - 70) / 100));
    const brk = Math.max(0, Math.min(1.0, spd < 140 ? (140 - spd) / 100 : 0.0));

    let g: number;
    if (spd < 90) g = 2;
    else if (spd < 135) g = 3;
    else if (spd < 190) g = 4;
    else if (spd < 250) g = 5;
    else if (spd < 300) g = 6;
    else g = 7;

    currentDist += (spd / 3.6) * dt;

    speedKmh[i] = spd;
    throttlePct[i] = throt * 100;
    brakePct[i] = brk * 100;
    rpm[i] = 5000 + (spd / 324) * 3800;
    gear[i] = g;
    steerAngleDeg[i] = Math.sin(phase * 1.5) * 28;
    distanceM[i] = currentDist;
  }

  return createMotecLdBuffer(
    [
      { name: 'Ground Speed', shortName: 'Speed', unit: 'km/h', frequencyHz: freq, samples: speedKmh },
      { name: 'Throttle Pos', shortName: 'Throt', unit: '%', frequencyHz: freq, samples: throttlePct },
      { name: 'Brake Pos', shortName: 'Brake', unit: '%', frequencyHz: freq, samples: brakePct },
      { name: 'Engine RPM', shortName: 'RPM', unit: 'rpm', frequencyHz: freq, samples: rpm },
      { name: 'Gear', shortName: 'Gear', unit: '', frequencyHz: freq, samples: gear },
      { name: 'Steering Angle', shortName: 'Steer', unit: 'deg', frequencyHz: freq, samples: steerAngleDeg },
      { name: 'Lap Distance', shortName: 'Distance', unit: 'm', frequencyHz: freq, samples: distanceM },
    ],
    SYNTHETIC_HEADER,
  );
}

/** Track length used by `generateMultiLapMotecBuffer` (m). */
export const SYNTHETIC_TRACK_LENGTH_M = 7000;

/**
 * Generates a SYNTHETIC multi-lap file at 10 Hz with 'Lap Number' and 'Lap Distance' channels:
 * - lap 1: out-lap from the start of the file, 160 s (partial: no start boundary)
 * - lap 2: 140 s (complete)
 * - lap 3: 134.5 s (complete, quickest)
 * - lap 4: in-lap of 40 s cut off by the end of the file (partial: no end boundary)
 * Complete laps cover `SYNTHETIC_TRACK_LENGTH_M`.
 */
export function generateMultiLapMotecBuffer(): Uint8Array {
  const freq = 10;
  const dt = 1 / freq;

  const lapConfigs = [
    { lapNum: 1, durationS: 160, distanceM: 6200 },
    { lapNum: 2, durationS: 140, distanceM: SYNTHETIC_TRACK_LENGTH_M },
    { lapNum: 3, durationS: 134.5, distanceM: SYNTHETIC_TRACK_LENGTH_M },
    { lapNum: 4, durationS: 40, distanceM: 1500 },
  ];

  const totalSamples = lapConfigs.reduce((acc, l) => acc + Math.round(l.durationS * freq), 0);

  const speedKmh = new Float32Array(totalSamples);
  const throttlePct = new Float32Array(totalSamples);
  const brakePct = new Float32Array(totalSamples);
  const lapNumber = new Float32Array(totalSamples);
  const lapDistanceM = new Float32Array(totalSamples);

  let globalIdx = 0;
  for (const config of lapConfigs) {
    const lapSampleCount = Math.round(config.durationS * freq);
    const shape = new Float64Array(lapSampleCount);
    let shapeDist = 0;
    for (let i = 0; i < lapSampleCount; i++) {
      const corner = Math.abs(Math.sin((i / lapSampleCount) * Math.PI * 10));
      shape[i] = Math.max(65, 280 - corner * 140);
      shapeDist += ((shape[i] ?? 0) / 3.6) * dt;
    }
    const k = config.distanceM / shapeDist;

    let currentLapDist = 0;
    for (let i = 0; i < lapSampleCount; i++) {
      const spd = (shape[i] ?? 0) * k;
      speedKmh[globalIdx] = spd;
      throttlePct[globalIdx] = spd > 160 ? 100 : 30;
      brakePct[globalIdx] = spd < 120 ? 80 : 0;
      lapNumber[globalIdx] = config.lapNum;
      lapDistanceM[globalIdx] = currentLapDist;
      currentLapDist += (spd / 3.6) * dt;
      globalIdx++;
    }
  }

  return createMotecLdBuffer(
    [
      { name: 'Ground Speed', shortName: 'Speed', unit: 'km/h', frequencyHz: freq, samples: speedKmh },
      { name: 'Throttle Pos', shortName: 'Throt', unit: '%', frequencyHz: freq, samples: throttlePct },
      { name: 'Brake Pos', shortName: 'Brake', unit: '%', frequencyHz: freq, samples: brakePct },
      { name: 'Lap Number', shortName: 'Lap', unit: '', frequencyHz: freq, samples: lapNumber },
      { name: 'Lap Distance', shortName: 'LapDist', unit: 'm', frequencyHz: freq, samples: lapDistanceM },
    ],
    { ...SYNTHETIC_HEADER, session: 'Synthetic multi-lap session' },
  );
}
