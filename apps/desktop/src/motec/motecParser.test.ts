import { describe, expect, it } from 'vitest';
import { ch } from '../generated/channels';
import { extractMotecLap, matchChannelNameToAppId, parseLdxMarkers, parseMotecLd } from './motecParser';
import { computeMotecDelta, createMotecGraphSource, createMotecReferenceSeries, sampleMotecAtDistance } from './motecSource';
import { motecUnitConverter, normaliseUnit } from './motecUnits';
import {
  createMotecLdBuffer,
  generateMultiLapMotecBuffer,
  generateSyntheticDemoMotecBuffer,
  SYNTHETIC_TRACK_LENGTH_M,
  WRITER_LAYOUT,
  type MotecChannelInput,
} from './motecWriter';

function patch(buffer: Uint8Array, fn: (view: DataView) => void): Uint8Array {
  const copy = buffer.slice();
  fn(new DataView(copy.buffer));
  return copy;
}

/** Lap-number session at 10 Hz: each lap covers `distanceM` at constant speed over `durationS`. */
function lapSession(laps: readonly { durationS: number; distanceM: number }[]): Uint8Array {
  const freq = 10;
  const speed: number[] = [];
  const lapNo: number[] = [];
  const dist: number[] = [];
  laps.forEach((lap, k) => {
    const n = Math.round(lap.durationS * freq);
    const v = lap.distanceM / lap.durationS; // m/s
    for (let i = 0; i < n; i++) {
      speed.push(v * 3.6);
      lapNo.push(k + 1);
      dist.push((v * i) / freq);
    }
  });
  const chans: MotecChannelInput[] = [
    { name: 'Ground Speed', unit: 'km/h', frequencyHz: freq, samples: speed },
    { name: 'Lap Number', unit: '', frequencyHz: freq, samples: lapNo },
    { name: 'Lap Distance', unit: 'm', frequencyHz: freq, samples: dist },
  ];
  return createMotecLdBuffer(chans);
}

describe('MoTeC .ld Binary Parser', () => {
  describe('Header & Metadata Decoding', () => {
    it('decodes header strings, date/time, and channel count correctly', () => {
      const buffer = createMotecLdBuffer(
        [
          { name: 'Ground Speed', unit: 'km/h', samples: [100.5, 120.2, 135.0] },
          { name: 'Engine RPM', unit: 'rpm', samples: [6500, 7200, 8100] },
        ],
        {
          driver: 'Test Driver',
          vehicleId: 'Test Car #1',
          venue: 'Test Circuit',
          session: 'Practice 1',
          comment: 'Reference Lap',
          dateStr: '19/10/2026',
          timeStr: '15:30:00',
        },
      );

      const parsed = parseMotecLd(buffer, 'test.ld');

      expect(parsed.filename).toBe('test.ld');
      expect(parsed.header.driver).toBe('Test Driver');
      expect(parsed.header.vehicleId).toBe('Test Car #1');
      expect(parsed.header.venue).toBe('Test Circuit');
      expect(parsed.header.session).toBe('Practice 1');
      expect(parsed.header.shortComment).toBe('Reference Lap');
      expect(parsed.header.dateTimeStr).toBe('19/10/2026 15:30:00');
      expect(parsed.header.timestampMs).toBe(new Date(2026, 9, 19, 15, 30, 0).getTime());
      expect(parsed.channels.size).toBe(2);
    });

    it('reports missing header fields as null instead of inventing values', () => {
      const parsed = parseMotecLd(createMotecLdBuffer([{ name: 'Engine RPM', unit: 'rpm', samples: [1, 2, 3] }]));
      expect(parsed.header.driver).toBeNull();
      expect(parsed.header.vehicleId).toBeNull();
      expect(parsed.header.venue).toBeNull();
      expect(parsed.header.session).toBeNull();
      expect(parsed.header.shortComment).toBeNull();
      expect(parsed.header.dateTimeStr).toBeNull();
      expect(parsed.header.timestampMs).toBeNull();
    });

    it('reports the real sample rate instead of a hardcoded one', () => {
      const parsed = parseMotecLd(
        createMotecLdBuffer([{ name: 'Engine RPM', unit: 'rpm', frequencyHz: 50, samples: [1, 2, 3, 4] }]),
      );
      expect(parsed.primaryFrequencyHz).toBe(50);
    });
  });

  describe('Channel Linked-List & Raw Sample Decoding', () => {
    it('decodes float32 channels with high floating point accuracy', () => {
      const buffer = createMotecLdBuffer([
        { name: 'Ground Speed', shortName: 'Speed', unit: 'km/h', frequencyHz: 100, samples: [72.45, 150.12, 280.95, 312.44] },
      ]);

      const chan = parseMotecLd(buffer).channels.get('ground speed');
      expect(chan?.meta.shortName).toBe('Speed');
      expect(chan?.meta.unit).toBe('km/h');
      expect(chan?.meta.frequencyHz).toBe(100);
      expect(chan?.meta.sampleCount).toBe(4);
      expect(chan?.meta.dataType).toBe('f32');
      // Raw channels keep the file's engineering unit.
      expect(chan?.samples[0]).toBeCloseTo(72.45, 2);
      expect(chan?.samples[3]).toBeCloseTo(312.44, 2);
    });

    it('decodes 16-bit integer channels accurately', () => {
      const buffer = createMotecLdBuffer([
        { name: 'Engine RPM', unit: 'rpm', frequencyHz: 50, samples: [4500, 6200, 7800, 8500], isFloat: false },
      ]);
      const chan = parseMotecLd(buffer).channels.get('engine rpm');
      expect(chan?.meta.dataType).toBe('i16');
      expect(Array.from(chan?.samples ?? [])).toEqual([4500, 6200, 7800, 8500]);
    });

    it('decodes calibrated channels with scale, shift, and decPlaces formula', () => {
      const buffer = createMotecLdBuffer([
        { name: 'Brake Temp FL', unit: '°C', samples: [320.5, 450.2, 510.0], isFloat: false, scale: 1, decPlaces: 1 },
      ]);
      const chan = parseMotecLd(buffer).channels.get('brake temp fl');
      expect(chan?.samples[0]).toBeCloseTo(320.5, 1);
      expect(chan?.samples[2]).toBeCloseTo(510.0, 1);
    });

    it('stores non-finite samples as NaN, never 0', () => {
      const parsed = parseMotecLd(createMotecLdBuffer([{ name: 'Engine RPM', unit: 'rpm', samples: [5000, Number.NaN, 6000] }]));
      const samples = parsed.channels.get('engine rpm')?.samples;
      expect(samples?.[1]).toBeNaN();
      expect(parsed.channelsByAppId.get(ch.RPM)?.samples[1]).toBeNaN();
    });
  });

  describe('Format validation (malformed files)', () => {
    const valid = createMotecLdBuffer([{ name: 'Ground Speed', unit: 'km/h', samples: [100, 200, 300, 400, 500] }]);

    it('rejects an empty file', () => {
      expect(() => parseMotecLd(new Uint8Array(0))).toThrow(/empty/);
    });

    it('rejects a file smaller than the header', () => {
      expect(() => parseMotecLd(new Uint8Array(64))).toThrow(/too small/);
    });

    it('rejects a file without the 0x40 header marker', () => {
      const bad = patch(valid, (v) => { v.setUint32(0, 0x1234, true); });
      expect(() => parseMotecLd(bad)).toThrow(/Not a MoTeC \.ld file/);
    });

    it('rejects an all-zero file', () => {
      expect(() => parseMotecLd(new Uint8Array(4096))).toThrow(/Not a MoTeC \.ld file/);
    });

    it('rejects random junk', () => {
      const junk = new Uint8Array(4096);
      for (let i = 0; i < junk.length; i++) junk[i] = (i * 7919 + 13) & 0xff;
      expect(() => parseMotecLd(junk)).toThrow(/Not a MoTeC \.ld file/);
    });

    it('rejects a header declaring zero channels', () => {
      const bad = patch(valid, (v) => { v.setUint32(86, 0, true); });
      expect(() => parseMotecLd(bad)).toThrow(/0 channels/);
    });

    it('rejects a channel table pointer outside the file', () => {
      const bad = patch(valid, (v) => { v.setUint32(8, 0x7fffffff, true); });
      expect(() => parseMotecLd(bad)).toThrow(/outside the file/);
    });

    it('rejects an unknown sample type, naming the channel', () => {
      const bad = patch(valid, (v) => { v.setUint16(WRITER_LAYOUT.channelHeaderOffset(0) + 20, 3, true); });
      expect(() => parseMotecLd(bad)).toThrow(/Unsupported MoTeC sample type in channel "Ground Speed"/);
    });

    it('does not allocate from a huge declared sample count', () => {
      const bad = patch(valid, (v) => { v.setUint32(WRITER_LAYOUT.channelHeaderOffset(0) + 12, 0xffffffff, true); });
      const parsed = parseMotecLd(bad);
      const chan = parsed.channels.get('ground speed');
      expect(chan?.meta.declaredSampleCount).toBe(0xffffffff);
      expect(chan?.meta.sampleCount).toBe(5);
      expect(chan?.samples.length).toBe(5);
      expect(parsed.warnings.some((w) => w.includes('Ground Speed') && w.includes('read 5 of'))).toBe(true);
    });

    it('reads only the samples present in a truncated file (no zero-fill)', () => {
      const truncated = valid.subarray(0, valid.byteLength - 10); // 5 f32 samples -> 2.5 left
      const parsed = parseMotecLd(truncated);
      const chan = parsed.channels.get('ground speed');
      expect(chan?.samples.length).toBe(2);
      expect(chan?.meta.sampleCount).toBe(2);
      expect(Array.from(chan?.samples ?? [])).toEqual([100, 200]);
      expect(parsed.warnings.some((w) => w.includes('"Ground Speed"') && w.includes('truncated'))).toBe(true);
    });

    it('skips a 0 Hz channel instead of inventing a rate', () => {
      const buffer = createMotecLdBuffer([
        { name: 'Engine RPM', unit: 'rpm', samples: [1, 2, 3] },
        { name: 'Ground Speed', unit: 'km/h', samples: [100, 200, 300] },
      ]);
      const bad = patch(buffer, (v) => { v.setUint16(WRITER_LAYOUT.channelHeaderOffset(1) + 22, 0, true); });
      const parsed = parseMotecLd(bad);
      expect(parsed.channels.has('ground speed')).toBe(false);
      expect(parsed.warnings.some((w) => w.includes('"Ground Speed"') && w.includes('0 Hz'))).toBe(true);
    });

    it('throws when no channel has samples', () => {
      expect(() => parseMotecLd(createMotecLdBuffer([{ name: 'Engine RPM', unit: 'rpm', samples: [] }]))).toThrow(
        /no channel contains any samples/,
      );
    });
  });

  describe('Channel mapping & unit conversion', () => {
    it('maps standard MoTeC channel names', () => {
      expect(matchChannelNameToAppId('Ground Speed')).toBe(ch.SPEED);
      expect(matchChannelNameToAppId('Throttle Pos')).toBe(ch.THROTTLE);
      expect(matchChannelNameToAppId('Brake Pos')).toBe(ch.BRAKE);
      expect(matchChannelNameToAppId('Engine RPM')).toBe(ch.RPM);
      expect(matchChannelNameToAppId('Steering Angle')).toBe(ch.STEERING_ANGLE);
      expect(matchChannelNameToAppId('Gear')).toBe(ch.GEAR);
      expect(matchChannelNameToAppId('Lap Distance')).toBe(ch.LAP_DISTANCE);
      expect(matchChannelNameToAppId('Lap Number')).toBe(ch.LAP_NUMBER);
      expect(matchChannelNameToAppId('Tyre Pressure FL')).toBe(ch.TYRE_PRESSURE_FL);
    });

    it('does not map ambiguous or wrong aliases', () => {
      expect(matchChannelNameToAppId('time')).toBeUndefined();
      expect(matchChannelNameToAppId('distance')).toBeUndefined();
      expect(matchChannelNameToAppId('Wheel Speed')).toBeUndefined();
      expect(matchChannelNameToAppId('Brake Press')).toBeUndefined();
      expect(matchChannelNameToAppId('Brakes Force')).toBeUndefined();
      expect(matchChannelNameToAppId('G Force Lat')).toBeUndefined();
    });

    it('normalises unit strings including UTF-8 degree signs', () => {
      expect(normaliseUnit('km/h')).toBe('km/h');
      expect(normaliseUnit('Â°C')).toBe('degc');
      expect(normaliseUnit('°')).toBe('deg');
      expect(normaliseUnit(' kPa ')).toBe('kpa');
      expect(motecUnitConverter(ch.SPEED, '')).toBeNull();
    });

    it('converts MoTeC engineering units into registry SI units', () => {
      const parsed = parseMotecLd(
        createMotecLdBuffer([
          { name: 'Ground Speed', unit: 'km/h', samples: [300, 360] },
          { name: 'Throttle Pos', unit: '%', samples: [100, 50] },
          { name: 'Brake Pos', unit: '%', samples: [0, 25] },
          { name: 'Steering Angle', unit: 'deg', samples: [90, -180] },
          { name: 'Tyre Pressure FL', unit: 'kPa', samples: [180, 190] },
          { name: 'Brake Temp FL', unit: '°C', samples: [400, 500] },
        ]),
      );
      const s = (id: number) => parsed.channelsByAppId.get(id)?.samples;
      expect(s(ch.SPEED)?.[0]).toBeCloseTo(83.333, 2); // a 300 km/h trace is 83.3 m/s, not 1080
      expect(s(ch.SPEED)?.[1]).toBeCloseTo(100, 3);
      expect(s(ch.THROTTLE)?.[0]).toBeCloseTo(1, 6);
      expect(s(ch.THROTTLE)?.[1]).toBeCloseTo(0.5, 6);
      expect(s(ch.BRAKE)?.[1]).toBeCloseTo(0.25, 6);
      expect(s(ch.STEERING_ANGLE)?.[0]).toBeCloseTo(Math.PI / 2, 6);
      expect(s(ch.STEERING_ANGLE)?.[1]).toBeCloseTo(-Math.PI, 6);
      expect(s(ch.TYRE_PRESSURE_FL)?.[0]).toBeCloseTo(180_000, 0);
      expect(s(ch.BRAKE_TEMP_FL)?.[0]).toBeCloseTo(400, 3);
      // The by-name map keeps the file's units.
      expect(parsed.channels.get('ground speed')?.samples[0]).toBeCloseTo(300, 3);
    });

    it('drops mappings whose unit is unknown or incompatible, with a warning', () => {
      const parsed = parseMotecLd(
        createMotecLdBuffer([
          { name: 'Ground Speed', unit: '', samples: [100, 200] },
          { name: 'Throttle Pos', unit: 'deg', samples: [1, 2] },
          { name: 'Steering Pos', unit: '%', samples: [10, 20] },
        ]),
      );
      expect(parsed.channelsByAppId.has(ch.SPEED)).toBe(false);
      expect(parsed.channelsByAppId.has(ch.THROTTLE)).toBe(false);
      expect(parsed.channelsByAppId.has(ch.STEERING_ANGLE)).toBe(false);
      expect(parsed.channels.has('ground speed')).toBe(true);
      expect(parsed.warnings.filter((w) => w.includes('cannot be converted')).length).toBe(3);
    });
  });

  describe('Multi-rate channels and distance', () => {
    it('resamples each channel by time onto the primary timeline', () => {
      const speed = Array.from({ length: 100 }, () => 180); // 100 Hz, 1 s
      const throttle = Array.from({ length: 10 }, (_, i) => i * 10); // 10 Hz, 1 s: 0..90 %
      const parsed = parseMotecLd(
        createMotecLdBuffer([
          { name: 'Ground Speed', unit: 'km/h', frequencyHz: 100, samples: speed },
          { name: 'Throttle Pos', unit: '%', frequencyHz: 10, samples: throttle },
        ]),
      );
      expect(parsed.primaryFrequencyHz).toBe(100);
      const thr = parsed.channelsByAppId.get(ch.THROTTLE)?.samples ?? new Float64Array(0);
      expect(thr.length).toBe(parsed.timeTimelineS.length);
      expect(thr[50]).toBeCloseTo(0.5, 6); // t = 0.5 s -> raw sample 5 = 50 %
      expect(thr[55]).toBeCloseTo(0.55, 6); // interpolated between samples 5 and 6
      expect(thr[95]).toBeNaN(); // past the last 10 Hz sample: no data, not a clamp
      expect(parsed.channels.get('throttle pos')?.meta.sampleCount).toBe(10);
    });

    it('integrates distance from converted speed', () => {
      const parsed = parseMotecLd(
        createMotecLdBuffer([{ name: 'Ground Speed', unit: 'km/h', frequencyHz: 10, samples: Array.from({ length: 101 }, () => 36) }]),
      );
      // 36 km/h = 10 m/s for 10 s
      expect(parsed.maxDistanceM).toBeCloseTo(100, 6); // 101 samples span 10 s
    });

    it('marks distance unavailable when there is no distance or speed channel', () => {
      const parsed = parseMotecLd(createMotecLdBuffer([{ name: 'Engine RPM', unit: 'rpm', samples: [1, 2, 3, 4] }]));
      expect(parsed.distanceTimelineM).toBeNull();
      expect(parsed.maxDistanceM).toBeNull();
      expect(parsed.warnings.some((w) => w.includes('distance is unavailable'))).toBe(true);
      expect(createMotecGraphSource(parsed, 'distance')).toBeNull();
      expect(createMotecGraphSource(parsed, 'time')).not.toBeNull();
    });
  });

  describe('Lap detection & quickest lap', () => {
    it('detects laps, flags partial out/in laps, and slices to the quickest complete lap', () => {
      const parsed = parseMotecLd(generateMultiLapMotecBuffer(), 'session.ld');

      expect(parsed.lapSource).toBe('lap-number');
      expect(parsed.laps.map((l) => l.lapNumber)).toEqual([1, 2, 3, 4]);
      expect(parsed.laps.map((l) => l.isComplete)).toEqual([false, true, true, false]);
      expect(parsed.laps[0]?.isValid).toBe(false);
      expect(parsed.laps[3]?.isValid).toBe(false);
      expect(parsed.laps[1]?.lapTimeS).toBeCloseTo(140, 3);
      expect(parsed.laps[2]?.lapTimeS).toBeCloseTo(134.5, 3);

      expect(parsed.scope).toBe('fastest-lap');
      expect(parsed.fastestLap?.lapNumber).toBe(3);
      expect(parsed.durationS).toBeCloseTo(134.5, 3);
      expect(parsed.rawSessionDurationS).toBeCloseTo(474.5, 3);

      expect(parsed.distanceTimelineM?.[0]).toBe(0);
      expect(parsed.timeTimelineS[0]).toBe(0);
      expect(parsed.maxDistanceM).toBeGreaterThan(SYNTHETIC_TRACK_LENGTH_M * 0.98);
      expect(parsed.maxDistanceM).toBeLessThan(SYNTHETIC_TRACK_LENGTH_M * 1.01);

      const quickestSampleCount = Math.round(134.5 * 10);
      expect(parsed.channelsByAppId.get(ch.SPEED)?.samples.length).toBe(quickestSampleCount);
      expect(parsed.distanceTimelineM?.length).toBe(quickestSampleCount);
      expect(parsed.timeTimelineS.length).toBe(quickestSampleCount);
    });

    it('never picks a faster partial first or last segment as the quickest lap', () => {
      const parsed = parseMotecLd(
        lapSession([
          { durationS: 30, distanceM: 1500 }, // out-lap fragment from file start
          { durationS: 100, distanceM: 5000 },
          { durationS: 98, distanceM: 5000 },
          { durationS: 26, distanceM: 1300 }, // cut off by end of file
        ]),
      );
      expect(parsed.fastestLap?.lapNumber).toBe(3);
      expect(parsed.fastestLap?.lapTimeS).toBeCloseTo(98, 3);
      expect(parsed.laps[0]?.invalidReason).toMatch(/beginning of the file/);
      expect(parsed.laps[3]?.invalidReason).toMatch(/end of the file/);
    });

    it('rejects a complete lap whose distance is far from the median (e.g. a cut or pit lap)', () => {
      const parsed = parseMotecLd(
        lapSession([
          { durationS: 20, distanceM: 1000 },
          { durationS: 100, distanceM: 5000 },
          { durationS: 101, distanceM: 5000 },
          { durationS: 80, distanceM: 3500 }, // short-cut lap
          { durationS: 102, distanceM: 5000 },
          { durationS: 20, distanceM: 1000 },
        ]),
      );
      expect(parsed.laps[3]?.isComplete).toBe(true);
      expect(parsed.laps[3]?.isValid).toBe(false);
      expect(parsed.laps[3]?.invalidReason).toMatch(/median lap distance/);
      expect(parsed.fastestLap?.lapNumber).toBe(2);
    });

    it('uses the whole file with a warning when no lap boundary exists', () => {
      const parsed = parseMotecLd(generateSyntheticDemoMotecBuffer(), 'demo.ld');
      expect(parsed.laps).toEqual([]);
      expect(parsed.fastestLap).toBeUndefined();
      expect(parsed.scope).toBe('whole-file');
      expect(parsed.lapSource).toBeNull();
      expect(parsed.durationS).toBeCloseTo(136.42, 2);
      expect(parsed.warnings.some((w) => w.includes('No lap boundaries'))).toBe(true);
    });

    it('extracts any other lap of the same file for comparison', () => {
      const parsed = parseMotecLd(generateMultiLapMotecBuffer());
      const lap2 = parsed.laps[1];
      if (!lap2) throw new Error('lap 2 missing');
      const view = extractMotecLap(parsed, lap2);
      expect(view.lap).toBe(lap2);
      expect(view.durationS).toBeCloseTo(140, 3);
      expect(view.timeTimelineS.length).toBe(1400);
      expect(view.distanceTimelineM?.[0]).toBe(0);
      expect(view.channelsByAppId.get(ch.SPEED)?.samples.length).toBe(1400);
    });
  });

  describe('.ldx markers', () => {
    it('parses second-based markers', () => {
      const markers = parseLdxMarkers(`<LDXFile>
      <Marker Index="0" Time="0.000" Distance="0.00" Name="Out Lap" />
      <Marker Index="1" Time="150.250" Distance="7004.50" Name="Lap 1" />
      <Marker Index="2" Time="284.813" Name="Lap 2" />
</LDXFile>`);
      expect(markers.map((m) => m.timeS)).toEqual([0, 150.25, 284.813]);
      expect(markers[2]?.distM).toBeNull();
    });

    it('decides the microsecond unit once per file, not per marker', () => {
      const markers = parseLdxMarkers(`<Marker Time="50000.0" /><Marker Time="150000000.0" /><Marker Time="285000000.0" />`);
      expect(markers[0]?.timeS).toBeCloseTo(0.05, 6); // would be read as 50000 s with a per-marker rule
      expect(markers[1]?.timeS).toBeCloseTo(150, 6);
      expect(markers[2]?.timeS).toBeCloseTo(285, 6);
    });

    it('uses .ldx markers as lap boundaries', () => {
      const ldx = `<Marker Time="30000000" /><Marker Time="130000000" /><Marker Time="228000000" />`;
      const parsed = parseMotecLd(
        lapSession([{ durationS: 30, distanceM: 1500 }, { durationS: 100, distanceM: 5000 }, { durationS: 98, distanceM: 5000 }, { durationS: 26, distanceM: 1300 }]),
        'x.ld',
        ldx,
      );
      expect(parsed.lapSource).toBe('ldx');
      expect(parsed.fastestLap?.lapTimeS).toBeCloseTo(98, 3);
    });
  });

  describe('Graph source & sampling', () => {
    it('creates a distance-axis StaticSource in SI units and a reference series', () => {
      const parsed = parseMotecLd(generateMultiLapMotecBuffer());
      const source = createMotecGraphSource(parsed, 'distance');
      if (!source) throw new Error('no distance source');
      expect(source.origin).toBe('recording');
      expect(source.dataKind).toBe('lmu');

      const win = source.read({ min: 0, max: 500 }, [ch.SPEED]);
      expect(win.count).toBeGreaterThan(0);
      expect(win.monotonicX).toBe(true);
      const speed = win.column(ch.SPEED);
      expect(speed?.[0]).toBeLessThan(120); // m/s, not km/h

      const ref = createMotecReferenceSeries(parsed, ch.SPEED, source, 2);
      expect(ref.key).toBe(`motec-ref-${ch.SPEED}`);
      expect(ref.color).toBe(2);
      expect(ref.label).toContain('[MoTeC]');
      expect(ref.label).toContain('lap 3');
    });

    it('labels generated data as synthetic', () => {
      const parsed = parseMotecLd(generateSyntheticDemoMotecBuffer());
      expect(createMotecGraphSource(parsed, 'distance', 'synthetic')?.dataKind).toBe('synthetic');
      expect(parsed.header.driver).toMatch(/Synthetic/);
    });

    it('samples at a distance and returns null past the lap end', () => {
      const parsed = parseMotecLd(generateMultiLapMotecBuffer());
      const at1000 = sampleMotecAtDistance(parsed, ch.SPEED, 1000);
      expect(at1000).not.toBeNull();
      expect(at1000).toBeGreaterThan(10);
      expect(sampleMotecAtDistance(parsed, ch.SPEED, (parsed.maxDistanceM ?? 0) + 50)).toBeNull();
      expect(sampleMotecAtDistance(parsed, ch.SPEED, -1)).toBeNull();
      expect(sampleMotecAtDistance(parsed, ch.RPM, 1000)).toBeNull(); // channel not in file
    });

    it('computes live deltas in SI units and null when a side is missing', () => {
      const parsed = parseMotecLd(generateMultiLapMotecBuffer());
      const delta = computeMotecDelta(parsed, 1000, { speedMps: 80, throttleFrac: 1, brakeFrac: null });
      expect(delta.motecSpeedMps).not.toBeNull();
      expect(delta.deltaSpeedMps).toBeCloseTo(80 - (delta.motecSpeedMps ?? Number.NaN), 6);
      expect(delta.deltaThrottleFrac).not.toBeNull();
      expect(delta.deltaBrakeFrac).toBeNull();
      const past = computeMotecDelta(parsed, 1e9, { speedMps: 80, throttleFrac: 1, brakeFrac: 0 });
      expect(past.motecSpeedMps).toBeNull();
      expect(past.deltaSpeedMps).toBeNull();
    });
  });
});
