import { describe, expect, it } from 'vitest';
import { TrackGeometryBuilder } from './trackGeometry';

describe('TrackGeometryBuilder', () => {
  it('rejects non-finite coordinates and negative distances', () => {
    const builder = new TrackGeometryBuilder({ trackLengthM: 1000, binSizeM: 20 });
    expect(builder.ingest(NaN, 100, 200)).toBe(false);
    expect(builder.ingest(100, Infinity, 200)).toBe(false);
    expect(builder.ingest(100, 200, -10)).toBe(false);
    expect(builder.progress()).toBe(0);
  });

  it('accumulates coverage progress as points arrive', () => {
    const builder = new TrackGeometryBuilder({ trackLengthM: 1000, binSizeM: 50 });
    expect(builder.progress()).toBe(0);

    // Ingest first 250m (5 out of 20 bins = 25%)
    for (let d = 0; d < 250; d += 25) {
      builder.ingest(d, d * 0.5, d);
    }
    expect(builder.progress()).toBeCloseTo(0.25, 2);
    expect(builder.isLocked()).toBe(false);
    expect(builder.build()).toBeNull();
  });

  it('builds closed loop, interpolates small gaps, applies circular smoothing, and locks after full lap', () => {
    const builder = new TrackGeometryBuilder({ trackLengthM: 1000, binSizeM: 25, minBinsCoveredRatio: 0.9 });

    // Drive a square circuit: 0..250 (east), 250..500 (north), 500..750 (west), 750..1000 (south)
    for (let d = 0; d < 1000; d += 10) {
      let x: number;
      let z: number;
      if (d < 250) {
        x = d;
        z = 0;
      } else if (d < 500) {
        x = 250;
        z = d - 250;
      } else if (d < 750) {
        x = 250 - (d - 500);
        z = 250;
      } else {
        x = 0;
        z = 250 - (d - 750);
      }
      builder.ingest(x, z, d);
    }

    expect(builder.progress()).toBe(1);
    const map = builder.build();
    expect(map).not.toBeNull();
    expect(builder.isLocked()).toBe(true);

    if (!map) throw new Error('Map must be built');
    expect(map.points.length).toBeGreaterThanOrEqual(4);
    expect(map.lengthM).toBe(1000);
    expect(map.sectors.length).toBe(2);
    expect(map.startFinish).toBeDefined();

    // Re-querying after lock returns identical cached map
    expect(builder.build()).toBe(map);
  });

  it('infers observed track length upon lap wrap when not reported', () => {
    const builder = new TrackGeometryBuilder({ binSizeM: 25 });
    // Ingest up to 4000m then wrap back to 50m
    for (let d = 0; d <= 4000; d += 20) {
      builder.ingest(Math.cos(d / 500) * 500, Math.sin(d / 500) * 500, d);
    }
    expect(builder.getTrackLengthSource()).toBe('observed');

    // Simulate wrap
    builder.ingest(500, 0, 30);
    expect(builder.getTrackLengthSource()).toBe('observed');
    expect(builder.getTrackLength()).toBeGreaterThanOrEqual(4000);
  });

  it('never invents a track name: the default id is an em dash (v1.3 U18)', () => {
    const builder = new TrackGeometryBuilder({ trackLengthM: 1000, binSizeM: 25 });
    for (let d = 0; d < 1000; d += 10) {
      const a = (d / 1000) * 2 * Math.PI;
      builder.ingest(Math.cos(a) * 100, Math.sin(a) * 100, d);
    }
    const map = builder.build();
    expect(map).not.toBeNull();
    expect(map?.trackId).toBe('—');
    expect(map?.trackId).not.toBe('Live Circuit');
  });
});
