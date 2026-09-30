import { describe, expect, it } from 'vitest';
import {
  NO_VALUE,
  formatDelta,
  formatFuel,
  formatFuelPerLap,
  formatGap,
  formatLapTime,
  formatPressure,
  formatSpeed,
  formatTemperature,
  mpsToKmh,
  paToBar,
} from './units';

const INVALID: readonly (number | null | undefined)[] = [
  null,
  undefined,
  Number.NaN,
  Number.POSITIVE_INFINITY,
  Number.NEGATIVE_INFINITY,
];

const ALL_FORMATTERS = [
  formatSpeed,
  formatPressure,
  formatTemperature,
  formatFuel,
  formatFuelPerLap,
  formatDelta,
  formatGap,
  formatLapTime,
] as const;

describe('missing / invalid values', () => {
  it.each(ALL_FORMATTERS.map((f) => [f.name, f] as const))('%s renders an em dash', (_name, format) => {
    for (const value of INVALID) {
      expect(format(value)).toBe(NO_VALUE);
    }
  });

  it('uses U+2014 and never renders a missing value as zero', () => {
    expect(NO_VALUE).toBe('—');
    expect(formatFuel(null)).not.toBe('0.0');
  });
});

describe('conversions', () => {
  it('converts m/s to km/h', () => {
    expect(mpsToKmh(10)).toBeCloseTo(36, 12);
  });

  it('converts Pa to bar', () => {
    expect(paToBar(100_000)).toBe(1);
  });
});

describe('formatSpeed (km/h, integer)', () => {
  it('converts and rounds', () => {
    expect(formatSpeed(0)).toBe('0');
    expect(formatSpeed(83.5)).toBe('301'); // 300.6 km/h
    expect(formatSpeed(80)).toBe('288');
    expect(formatSpeed(1.25)).toBe('5'); // 4.5 → half away from zero
  });

  it('appends the unit on request', () => {
    expect(formatSpeed(80, { withUnit: true })).toBe('288 km/h');
  });

  it('never shows negative zero', () => {
    expect(formatSpeed(-0.05)).toBe('0');
  });
});

describe('formatPressure (bar, 2 decimals, input Pa)', () => {
  it('converts and rounds', () => {
    expect(formatPressure(182_300)).toBe('1.82');
    expect(formatPressure(180_000)).toBe('1.80');
    expect(formatPressure(100_500)).toBe('1.01'); // 1.005 must not become 1.00
    expect(formatPressure(0)).toBe('0.00');
  });

  it('appends the unit on request', () => {
    expect(formatPressure(182_300, { withUnit: true })).toBe('1.82 bar');
  });
});

describe('formatTemperature (°C, integer)', () => {
  it('rounds', () => {
    expect(formatTemperature(87.5)).toBe('88');
    expect(formatTemperature(87.49)).toBe('87');
    expect(formatTemperature(-3.6)).toBe('-4');
    expect(formatTemperature(-0.4)).toBe('0');
  });

  it('appends the unit on request', () => {
    expect(formatTemperature(95, { withUnit: true })).toBe('95 °C');
  });
});

describe('formatFuel (L, 1 decimal)', () => {
  it('rounds', () => {
    expect(formatFuel(45.67)).toBe('45.7');
    expect(formatFuel(45.65)).toBe('45.7');
    expect(formatFuel(0.04)).toBe('0.0');
    expect(formatFuel(110)).toBe('110.0');
  });

  it('appends the unit on request', () => {
    expect(formatFuel(12.3, { withUnit: true })).toBe('12.3 L');
  });
});

describe('formatFuelPerLap (L/lap, 2 decimals)', () => {
  it('rounds', () => {
    expect(formatFuelPerLap(3.456)).toBe('3.46');
    expect(formatFuelPerLap(3.4)).toBe('3.40');
    expect(formatFuelPerLap(2.675)).toBe('2.68');
  });

  it('appends the unit on request', () => {
    expect(formatFuelPerLap(3.456, { withUnit: true })).toBe('3.46 L/lap');
  });
});

describe('formatDelta (signed, 3 decimals)', () => {
  it('always carries a sign', () => {
    expect(formatDelta(-0.183)).toBe('-0.183');
    expect(formatDelta(0.421)).toBe('+0.421');
    expect(formatDelta(0)).toBe('+0.000');
    expect(formatDelta(12.3456)).toBe('+12.346');
  });

  it('does not show a minus sign for values that round to zero', () => {
    expect(formatDelta(-0.0004)).toBe('+0.000');
    expect(formatDelta(-0)).toBe('+0.000');
  });

  it('appends the unit on request', () => {
    expect(formatDelta(-0.183, { withUnit: true })).toBe('-0.183 s');
  });
});

describe('formatGap (signed, 3 decimals)', () => {
  it('formats', () => {
    expect(formatGap(1.234)).toBe('+1.234');
    expect(formatGap(-1.234)).toBe('-1.234');
    expect(formatGap(65.1)).toBe('+65.100');
  });
});

describe('formatLapTime (m:ss.mmm)', () => {
  it('formats', () => {
    expect(formatLapTime(83.456)).toBe('1:23.456');
    expect(formatLapTime(59.5)).toBe('0:59.500');
    expect(formatLapTime(209.049)).toBe('3:29.049');
    expect(formatLapTime(3723.5)).toBe('62:03.500');
    expect(formatLapTime(0.001)).toBe('0:00.001');
  });

  it('carries rounding into seconds and minutes', () => {
    expect(formatLapTime(59.9996)).toBe('1:00.000');
    expect(formatLapTime(119.9999)).toBe('2:00.000');
  });

  it('treats non-positive lap times as missing', () => {
    expect(formatLapTime(0)).toBe(NO_VALUE);
    expect(formatLapTime(-1)).toBe(NO_VALUE);
  });
});
