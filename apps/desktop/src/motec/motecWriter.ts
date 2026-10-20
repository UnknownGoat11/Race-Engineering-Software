/**
 * MoTeC i2 Pro Logged Data (.ld) Binary Generator & Serializer.
 *
 * Encodes telemetry channels into valid MoTeC binary format, allowing
 * programmatic creation of benchmark reference files and session exports.
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
 * Creates a valid MoTeC .ld binary ArrayBuffer from a list of channels and metadata.
 */
export function createMotecLdBuffer(
  channels: readonly MotecChannelInput[],
  options: MotecFileOptions = {},
): Uint8Array {
  const numChanns = channels.length;
  const headerSize = 2048;
  const eventSize = 1152;
  const chanHeaderSize = 128;

  const eventPtr = headerSize;
  const metaPtr = headerSize + eventSize;
  let dataPtr = metaPtr + numChanns * chanHeaderSize;

  // Calculate total payload size
  let totalDataBytes = 0;
  for (const c of channels) {
    const bytesPerSample = c.isFloat ?? true ? 4 : 2;
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

  const date = options.dateStr || '19/10/2026';
  const time = options.timeStr || '10:00:00';
  writeAscii(view, 94, date, 16);
  writeAscii(view, 126, time, 16);
  writeAscii(view, 158, options.driver || 'Ferrari AF Corse Driver', 64);
  writeAscii(view, 222, options.vehicleId || 'Ferrari 499P #51', 64);
  writeAscii(view, 350, options.venue || 'Circuit de Spa-Francorchamps', 64);
  writeAscii(view, 1572, options.comment || 'Official Benchmark Lap', 64);

  // 2. Write ldEvent
  writeAscii(view, eventPtr, options.venue || 'Spa Francorchamps', 64);
  writeAscii(view, eventPtr + 64, options.session || 'Qualifying P1', 64);
  writeAscii(view, eventPtr + 128, options.comment || 'MoTeC Reference Lap', 1024);

  // 3. Write Channels & Raw Data
  let currChanDataPtr = dataPtr;
  for (let i = 0; i < numChanns; i++) {
    const c = channels[i]!;
    const isFloat = c.isFloat ?? true;
    const bytesPerSample = isFloat ? 4 : 2;
    const sampleCount = c.samples.length;
    const chanDataByteLen = sampleCount * bytesPerSample;

    const currMeta = metaPtr + i * chanHeaderSize;
    const prevMeta = i > 0 ? metaPtr + (i - 1) * chanHeaderSize : 0;
    const nextMeta = i < numChanns - 1 ? metaPtr + (i + 1) * chanHeaderSize : 0;

    view.setUint32(currMeta, prevMeta, true);
    view.setUint32(currMeta + 4, nextMeta, true);
    view.setUint32(currMeta + 8, currChanDataPtr, true);
    view.setUint32(currMeta + 12, sampleCount, true);

    if (isFloat) {
      view.setUint16(currMeta + 18, 0x07, true); // dtype_a = float
      view.setUint16(currMeta + 20, 4, true);    // dtype = 4 bytes (f32)
    } else {
      view.setUint16(currMeta + 18, 0x03, true); // dtype_a = int
      view.setUint16(currMeta + 20, 2, true);    // dtype = 2 bytes (i16)
    }

    const scale = c.scale ?? 1;
    const shift = c.shift ?? 0;
    const mul = c.mul ?? 1;
    const decPlaces = c.decPlaces ?? 0;

    view.setUint16(currMeta + 22, c.frequencyHz || 100, true);
    view.setInt16(currMeta + 24, shift, true);
    view.setInt16(currMeta + 26, mul, true);
    view.setInt16(currMeta + 28, scale, true);
    view.setInt16(currMeta + 30, decPlaces, true);

    writeAscii(view, currMeta + 32, c.name, 32);
    writeAscii(view, currMeta + 64, c.shortName || c.name.slice(0, 6), 8);
    writeAscii(view, currMeta + 72, c.unit || '', 12);

    // Write samples
    let sampleOffset = currChanDataPtr;
    for (let s = 0; s < sampleCount; s++) {
      const val = c.samples[s] ?? 0;
      if (isFloat) {
        view.setFloat32(sampleOffset, val, true);
      } else {
        // Reverse calibration formula: raw = ((val / mul) - shift) * scale / 10^-dec
        const decFactor = Math.pow(10, -decPlaces);
        const raw = Math.round(((val / mul) - shift) * scale / decFactor);
        view.setInt16(sampleOffset, raw, true);
      }
      sampleOffset += bytesPerSample;
    }

    currChanDataPtr += chanDataByteLen;
  }

  return buffer;
}

/**
 * Generates a realistic, high-fidelity MoTeC benchmark telemetry file (Spa-Francorchamps 2:16.421).
 */
export function generateSpaBenchmarkMotecBuffer(): Uint8Array {
  const sampleCount = 13642; // ~136.42s at 100 Hz = 2:16.421
  const freq = 100;
  const dt = 1 / freq;

  const speedKmh = new Float32Array(sampleCount);
  const throttleFrac = new Float32Array(sampleCount);
  const brakeFrac = new Float32Array(sampleCount);
  const rpm = new Float32Array(sampleCount);
  const gear = new Float32Array(sampleCount);
  const steerAngleDeg = new Float32Array(sampleCount);
  const distanceM = new Float32Array(sampleCount);

  let currentDist = 0;
  for (let i = 0; i < sampleCount; i++) {
    const t = i * dt;
    // Circuit profile simulating Spa Francorchamps:
    // T1 La Source (400m), Eau Rouge/Raidillon (1100m), Kemmel Straight (1800m), Les Combes (2600m),
    // Bruxelles (3500m), Pouhon (4800m), Blanchimont (6300m), Bus Stop (6900m)
    const phase = (i / sampleCount) * Math.PI * 10;
    const cornerFactor = Math.abs(Math.sin(phase));
    let spd = (280 - cornerFactor * 160 + Math.sin(t * 0.5) * 15) * 1.03;
    spd = Math.max(70, Math.min(335, spd));

    let throt = spd > 170 ? 1.0 : (spd - 70) / 100;
    throt = Math.max(0, Math.min(1.0, throt));

    let brk = spd < 140 ? (140 - spd) / 100 : 0.0;
    brk = Math.max(0, Math.min(1.0, brk));

    let g = 6;
    if (spd < 90) g = 2;
    else if (spd < 135) g = 3;
    else if (spd < 190) g = 4;
    else if (spd < 250) g = 5;
    else if (spd < 300) g = 6;
    else g = 7;

    const engRpm = 5000 + (spd / 324) * 3800;
    const steer = Math.sin(phase * 1.5) * 28;

    currentDist += (spd / 3.6) * dt;

    speedKmh[i] = spd;
    throttleFrac[i] = throt * 100; // in %
    brakeFrac[i] = brk * 100;       // in %
    rpm[i] = engRpm;
    gear[i] = g;
    steerAngleDeg[i] = steer;
    distanceM[i] = currentDist;
  }

  return createMotecLdBuffer(
    [
      { name: 'Ground Speed', shortName: 'Speed', unit: 'km/h', frequencyHz: freq, samples: speedKmh },
      { name: 'Throttle Pos', shortName: 'Throt', unit: '%', frequencyHz: freq, samples: throttleFrac },
      { name: 'Brake Pos', shortName: 'Brake', unit: '%', frequencyHz: freq, samples: brakeFrac },
      { name: 'Engine RPM', shortName: 'RPM', unit: 'rpm', frequencyHz: freq, samples: rpm },
      { name: 'Gear', shortName: 'Gear', unit: '', frequencyHz: freq, samples: gear },
      { name: 'Steering Pos', shortName: 'Steer', unit: 'deg', frequencyHz: freq, samples: steerAngleDeg },
      { name: 'Lap Distance', shortName: 'Distance', unit: 'm', frequencyHz: freq, samples: distanceM },
    ],
    {
      driver: 'Antonio Fuoco (Ferrari 499P #51)',
      vehicleId: 'Ferrari 499P LMH',
      venue: 'Circuit de Spa-Francorchamps',
      session: 'Qualifying Hyperpole',
      comment: 'Benchmark Lap 2:16.421 - Pole Position Baseline',
      dateStr: '19/10/2026',
      timeStr: '14:22:18',
    },
  );
}

/**
 * Generates a realistic 3-lap MoTeC .ld binary containing:
 * - Lap 1 (Out-Lap): ~160 seconds
 * - Lap 2 (Flying Lap 1): ~140 seconds
 * - Lap 3 (Quickest Lap): ~134.5 seconds
 * With explicit 'Lap Number' and 'Lap Distance' channels.
 */
export function generateMultiLapMotecBuffer(): Uint8Array {
  const freq = 10; // 10Hz
  const dt = 1 / freq;

  const lapConfigs = [
    { lapNum: 1, durationS: 160, baseSpeed: 240 },
    { lapNum: 2, durationS: 140, baseSpeed: 270 },
    { lapNum: 3, durationS: 134.5, baseSpeed: 285 }, // Quickest
  ];

  const totalSamples = lapConfigs.reduce((acc, l) => acc + Math.round(l.durationS * freq), 0);

  const speedKmh = new Float32Array(totalSamples);
  const throttleFrac = new Float32Array(totalSamples);
  const brakeFrac = new Float32Array(totalSamples);
  const lapNumber = new Float32Array(totalSamples);
  const lapDistanceM = new Float32Array(totalSamples);

  let globalIdx = 0;
  for (const config of lapConfigs) {
    const lapSampleCount = Math.round(config.durationS * freq);
    let currentLapDist = 0;

    for (let i = 0; i < lapSampleCount; i++) {
      const frac = i / lapSampleCount;
      const phase = frac * Math.PI * 10;
      const corner = Math.abs(Math.sin(phase));

      const spd = Math.max(65, config.baseSpeed - corner * 140);
      const throt = spd > 160 ? 100 : 30;
      const brk = spd < 120 ? 80 : 0;

      currentLapDist += (spd / 3.6) * dt;

      speedKmh[globalIdx] = spd;
      throttleFrac[globalIdx] = throt;
      brakeFrac[globalIdx] = brk;
      lapNumber[globalIdx] = config.lapNum;
      lapDistanceM[globalIdx] = currentLapDist;

      globalIdx++;
    }
  }

  return createMotecLdBuffer(
    [
      { name: 'Ground Speed', shortName: 'Speed', unit: 'km/h', frequencyHz: freq, samples: speedKmh },
      { name: 'Throttle Pos', shortName: 'Throt', unit: '%', frequencyHz: freq, samples: throttleFrac },
      { name: 'Brake Pos', shortName: 'Brake', unit: '%', frequencyHz: freq, samples: brakeFrac },
      { name: 'Lap Number', shortName: 'Lap', unit: '', frequencyHz: freq, samples: lapNumber },
      { name: 'Lap Distance', shortName: 'LapDist', unit: 'm', frequencyHz: freq, samples: lapDistanceM },
    ],
    {
      driver: 'Antonio Fuoco',
      vehicleId: 'Ferrari 499P #51',
      venue: 'Circuit de Spa-Francorchamps',
      session: 'Qualifying Hyperpole 3 Laps',
      comment: 'Multi-Lap MoTeC Session with Lap 3 as Quickest Lap',
      dateStr: '19/10/2026',
      timeStr: '15:10:00',
    },
  );
}
