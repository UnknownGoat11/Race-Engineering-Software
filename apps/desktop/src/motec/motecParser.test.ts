import { describe, expect, it } from 'vitest';
import { ch } from '../generated/channels';
import { matchChannelNameToAppId, parseMotecLd } from './motecParser';
import { createMotecGraphSource, createMotecReferenceSeries, sampleMotecAtDistance, computeMotecDelta } from './motecSource';
import { createMotecLdBuffer, generateSpaBenchmarkMotecBuffer } from './motecWriter';

describe('MoTeC .ld Binary Parser (Stand-Alone Verification)', () => {
  describe('Header & Metadata Decoding', () => {
    it('decodes header strings, date/time, and channel count correctly', () => {
      const buffer = createMotecLdBuffer(
        [
          { name: 'Ground Speed', unit: 'km/h', samples: [100.5, 120.2, 135.0] },
          { name: 'Engine RPM', unit: 'rpm', samples: [6500, 7200, 8100] },
        ],
        {
          driver: 'Antonio Fuoco',
          vehicleId: 'Ferrari 499P #51',
          venue: 'Circuit de Spa-Francorchamps',
          session: 'Qualifying Hyperpole',
          comment: 'Reference Lap',
          dateStr: '19/10/2026',
          timeStr: '15:30:00',
        },
      );

      const parsed = parseMotecLd(buffer, 'test_spa.ld');

      expect(parsed.filename).toBe('test_spa.ld');
      expect(parsed.header.driver).toBe('Antonio Fuoco');
      expect(parsed.header.vehicleId).toBe('Ferrari 499P #51');
      expect(parsed.header.venue).toBe('Circuit de Spa-Francorchamps');
      expect(parsed.header.session).toBe('Qualifying Hyperpole');
      expect(parsed.header.shortComment).toBe('Reference Lap');
      expect(parsed.header.dateTimeStr).toContain('19/10/2026 15:30:00');
      expect(parsed.channels.size).toBe(2);
    });
  });

  describe('Channel Linked-List & Raw Sample Decoding', () => {
    it('decodes float32 channels with high floating point accuracy', () => {
      const originalSpeed = [72.45, 150.12, 280.95, 312.44];
      const buffer = createMotecLdBuffer([
        {
          name: 'Ground Speed',
          shortName: 'Speed',
          unit: 'km/h',
          frequencyHz: 100,
          samples: originalSpeed,
          isFloat: true,
        },
      ]);

      const parsed = parseMotecLd(buffer);
      const chan = parsed.channels.get('ground speed');

      expect(chan).toBeDefined();
      expect(chan?.meta.name).toBe('Ground Speed');
      expect(chan?.meta.shortName).toBe('Speed');
      expect(chan?.meta.unit).toBe('km/h');
      expect(chan?.meta.frequencyHz).toBe(100);
      expect(chan?.meta.sampleCount).toBe(4);
      expect(chan?.meta.dataType).toBe('f32');

      expect(chan?.samples[0]).toBeCloseTo(72.45, 2);
      expect(chan?.samples[1]).toBeCloseTo(150.12, 2);
      expect(chan?.samples[2]).toBeCloseTo(280.95, 2);
      expect(chan?.samples[3]).toBeCloseTo(312.44, 2);
    });

    it('decodes 16-bit integer channels accurately', () => {
      const originalRpm = [4500, 6200, 7800, 8500];
      const buffer = createMotecLdBuffer([
        {
          name: 'Engine RPM',
          shortName: 'RPM',
          unit: 'rpm',
          frequencyHz: 50,
          samples: originalRpm,
          isFloat: false,
        },
      ]);

      const parsed = parseMotecLd(buffer);
      const chan = parsed.channels.get('engine rpm');

      expect(chan).toBeDefined();
      expect(chan?.meta.dataType).toBe('i16');
      expect(chan?.meta.frequencyHz).toBe(50);
      expect(chan?.samples[0]).toBe(4500);
      expect(chan?.samples[1]).toBe(6200);
      expect(chan?.samples[2]).toBe(7800);
      expect(chan?.samples[3]).toBe(8500);
    });

    it('decodes calibrated channels with scale, shift, and decPlaces formula', () => {
      const buffer = createMotecLdBuffer([
        {
          name: 'Brake Temp FL',
          unit: '°C',
          samples: [320.5, 450.2, 510.0],
          isFloat: false,
          scale: 1,
          decPlaces: 1,
          shift: 0,
          mul: 1,
        },
      ]);

      const parsed = parseMotecLd(buffer);
      const chan = parsed.channels.get('brake temp fl');

      expect(chan).toBeDefined();
      expect(chan?.samples[0]).toBeCloseTo(320.5, 1);
      expect(chan?.samples[1]).toBeCloseTo(450.2, 1);
      expect(chan?.samples[2]).toBeCloseTo(510.0, 1);
    });
  });

  describe('MoTeC Channel Name to Canonical App ID Mapping', () => {
    it('correctly maps standard MoTeC / DAMPlugin channel names', () => {
      expect(matchChannelNameToAppId('Ground Speed')).toBe(ch.SPEED);
      expect(matchChannelNameToAppId('speed')).toBe(ch.SPEED);
      expect(matchChannelNameToAppId('Throttle Pos')).toBe(ch.THROTTLE);
      expect(matchChannelNameToAppId('Brake Pos')).toBe(ch.BRAKE);
      expect(matchChannelNameToAppId('Engine RPM')).toBe(ch.RPM);
      expect(matchChannelNameToAppId('Steering Pos')).toBe(ch.STEERING);
      expect(matchChannelNameToAppId('Gear')).toBe(ch.GEAR);
      expect(matchChannelNameToAppId('G Force Lat')).toBe(ch.G_LAT);
      expect(matchChannelNameToAppId('Lap Distance')).toBe(ch.LAP_DISTANCE);
      expect(matchChannelNameToAppId('Tyre Pressure FL')).toBe(ch.TYRE_PRESSURE_FL);
    });

    it('indexes matched channels into channelsByAppId', () => {
      const buffer = createMotecLdBuffer([
        { name: 'Ground Speed', unit: 'km/h', samples: [240] },
        { name: 'Throttle Pos', unit: '%', samples: [98] },
        { name: 'Brake Pos', unit: '%', samples: [0] },
      ]);

      const parsed = parseMotecLd(buffer);

      expect(parsed.channelsByAppId.has(ch.SPEED)).toBe(true);
      expect(parsed.channelsByAppId.has(ch.THROTTLE)).toBe(true);
      expect(parsed.channelsByAppId.has(ch.BRAKE)).toBe(true);
      expect(parsed.channelsByAppId.get(ch.SPEED)?.samples[0]).toBe(240);
    });
  });

  describe('Benchmark Telemetry File (Spa-Francorchamps 2:16.421)', () => {
    it('successfully parses high-frequency 100Hz benchmark lap', () => {
      const buffer = generateSpaBenchmarkMotecBuffer();
      const parsed = parseMotecLd(buffer, 'spa_benchmark_pole.ld');

      expect(parsed.filename).toBe('spa_benchmark_pole.ld');
      expect(parsed.header.venue).toBe('Circuit de Spa-Francorchamps');
      expect(parsed.header.driver).toContain('Antonio Fuoco');
      expect(parsed.header.vehicleId).toContain('Ferrari 499P');

      // Duration: 13,642 samples @ 100 Hz = 136.42s (~2m 16.42s)
      expect(parsed.durationS).toBeCloseTo(136.42, 1);
      expect(parsed.timeTimelineS.length).toBe(13642);

      // Track distance ~7004m
      expect(parsed.maxDistanceM).toBeGreaterThan(6900);
      expect(parsed.maxDistanceM).toBeLessThan(7150);

      // Verify channel presence
      expect(parsed.channels.has('ground speed')).toBe(true);
      expect(parsed.channels.has('throttle pos')).toBe(true);
      expect(parsed.channels.has('brake pos')).toBe(true);
      expect(parsed.channels.has('engine rpm')).toBe(true);
      expect(parsed.channels.has('gear')).toBe(true);
      expect(parsed.channels.has('steering pos')).toBe(true);
      expect(parsed.channels.has('lap distance')).toBe(true);

      // Verify telemetry physics plausibility
      const speedChan = parsed.channelsByAppId.get(ch.SPEED);
      expect(speedChan).toBeDefined();
      if (speedChan) {
        const speeds = Array.from(speedChan.samples);
        const maxSpeed = Math.max(...speeds);
        const minSpeed = Math.min(...speeds);
        expect(maxSpeed).toBeGreaterThan(300); // Kemmel Straight > 300 km/h
        expect(minSpeed).toBeGreaterThan(60);  // Hairpin apex > 60 km/h
      }

      // Check lap object
      expect(parsed.laps.length).toBe(1);
      expect(parsed.fastestLap?.lapTimeS).toBeCloseTo(136.42, 1);
    });
  });

  describe('Error Handling & Resilience', () => {
    it('throws descriptive error on truncated buffer (< 128 bytes)', () => {
      const emptyBuffer = new Uint8Array(64);
      expect(() => parseMotecLd(emptyBuffer)).toThrow(/too small/);
    });

    it('gracefully handles file with truncated sample array without crashing', () => {
      const buffer = createMotecLdBuffer([
        { name: 'Ground Speed', unit: 'km/h', samples: [100, 200, 300, 400, 500] },
      ]);
      // Artificially cut the buffer by 10 bytes
      const truncated = buffer.subarray(0, buffer.byteLength - 10);

      const parsed = parseMotecLd(truncated);
      const chan = parsed.channels.get('ground speed');
      expect(chan).toBeDefined();
      // Should read valid samples up to the truncation point
      expect(chan?.samples.length).toBe(5);
    });
  });

  describe('Graph Engine & Live Comparison Overlay Integration', () => {
    it('creates a StaticSource and ReferenceSeries with orange/amber styling for lap comparison', () => {
      const buffer = generateSpaBenchmarkMotecBuffer();
      const parsed = parseMotecLd(buffer, 'spa_ref.ld');

      const source = createMotecGraphSource(parsed, 'distance');
      expect(source.origin).toBe('recording');
      expect(source.dataKind).toBe('lmu');

      // Request window covering Turn 1 (0 to 500m)
      const window = source.read({ min: 0, max: 500 }, [ch.SPEED]);
      expect(window.count).toBeGreaterThan(0);
      expect(window.monotonicX).toBe(true);

      const speedCol = window.column(ch.SPEED);
      expect(speedCol).not.toBeNull();
      expect(speedCol![0]).toBeGreaterThan(60);

      // Create reference series with palette slot 2 (orange)
      const ref = createMotecReferenceSeries(parsed, ch.SPEED, source, 2);
      expect(ref.key).toBe(`motec-ref-${ch.SPEED}`);
      expect(ref.color).toBe(2);
      expect(ref.label).toContain('[MoTeC]');
      expect(ref.label).toContain('Antonio');
    });

    it('samples MoTeC channels at any distance and calculates live delta accurately', () => {
      const buffer = generateSpaBenchmarkMotecBuffer();
      const parsed = parseMotecLd(buffer);

      // Sample at 1000m (approaching Eau Rouge)
      const speedAt1000 = sampleMotecAtDistance(parsed, ch.SPEED, 1000);
      expect(speedAt1000).not.toBeNull();
      expect(speedAt1000!).toBeGreaterThan(100);

      // Compare live car traveling at 285 km/h vs MoTeC
      const delta = computeMotecDelta(parsed, 1000, 285, 100, 0);
      expect(delta.motecSpeedKmh).toBeGreaterThan(100);
      expect(delta.deltaSpeedKmh).toBeCloseTo(285 - delta.motecSpeedKmh, 2);
      expect(delta.deltaThrottlePct).toBeDefined();
    });
  });
});
