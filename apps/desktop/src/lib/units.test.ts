import { describe, expect, it } from 'vitest';
import { ALL_UNITS, CHANNEL_BY_NAME, type ChannelDef, type Unit } from '../generated/channels';
import {
  CHANNEL_DISPLAY_KINDS,
  DISPLAY_UNITS,
  NO_VALUE,
  channelUnitLabel,
  formatAge,
  formatBytes,
  formatChannelValue,
  formatClock,
  formatDelta,
  formatFuel,
  formatFuelPerLap,
  formatGap,
  formatLapGap,
  formatLapTime,
  formatPressure,
  formatSpeed,
  formatTemperature,
  formatTimeOfDay,
  mpsToKmh,
  paToBar,
  type DisplayKind,
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

  it('accepts 0 for a running lap timer only when asked (review F6)', () => {
    expect(formatLapTime(0, { allowZero: true })).toBe('0:00.000');
    expect(formatLapTime(-0.001, { allowZero: true })).toBe(NO_VALUE);
    expect(formatLapTime(Number.NaN, { allowZero: true })).toBe(NO_VALUE);
  });
});

describe('formatLapGap (spec §341, lapped cars)', () => {
  it('formats whole laps with a sign and LAP / LAPS', () => {
    expect(formatLapGap(1)).toBe('+1 LAP');
    expect(formatLapGap(3)).toBe('+3 LAPS');
    expect(formatLapGap(-2)).toBe('-2 LAPS');
  });

  it('rejects fractional, zero and missing laps', () => {
    expect(formatLapGap(1.5)).toBe(NO_VALUE);
    expect(formatLapGap(0)).toBe(NO_VALUE);
    expect(formatLapGap(null)).toBe(NO_VALUE);
  });
});

describe('formatClock / formatTimeOfDay', () => {
  it('formats durations as m:ss or h:mm:ss, truncated, signed when negative', () => {
    expect(formatClock(59.9)).toBe('0:59');
    expect(formatClock(125)).toBe('2:05');
    expect(formatClock(3_725.8)).toBe('1:02:05');
    expect(formatClock(-5.5)).toBe('-0:05');
    expect(formatClock(-0.4)).toBe('0:00');
    expect(formatClock(Number.NaN)).toBe(NO_VALUE);
  });

  it('formats seconds since midnight as hh:mm:ss within one day', () => {
    expect(formatTimeOfDay(0)).toBe('00:00:00');
    expect(formatTimeOfDay(45_296.9)).toBe('12:34:56');
    expect(formatTimeOfDay(86_399.99)).toBe('23:59:59');
    // The registry range [0, 86400] is inclusive: the end of the day is a valid time of day (review F6).
    expect(formatTimeOfDay(86_400)).toBe('24:00:00');
    expect(formatTimeOfDay(86_400.5)).toBe(NO_VALUE);
    expect(formatTimeOfDay(-1)).toBe(NO_VALUE);
  });
});

describe('formatBytes', () => {
  it('scales by magnitude and keeps small exact counts', () => {
    expect(formatBytes(512)).toEqual({ text: '512', unitLabel: 'B', spoken: '512 bytes' });
    expect(formatBytes(1_536)).toEqual({ text: '1.5', unitLabel: 'KiB', spoken: '1.5 kibibytes' });
    expect(formatBytes(5n * 1024n ** 3n)).toEqual({ text: '5.0', unitLabel: 'GiB', spoken: '5.0 gibibytes' });
    expect(formatBytes(null).text).toBe(NO_VALUE);
  });
});

describe('formatAge', () => {
  it('uses ms below a second, seconds below a minute, then a clock', () => {
    expect(formatAge(820)).toEqual({ text: '820', unitLabel: 'ms', spoken: '820 milliseconds' });
    expect(formatAge(3_420)).toEqual({ text: '3.4', unitLabel: 's', spoken: '3.4 seconds' });
    expect(formatAge(3_725_000)).toEqual({ text: '1:02:05', unitLabel: '', spoken: '1 hour 2 minutes 5 seconds' });
    expect(formatAge(-1).text).toBe(NO_VALUE);
  });

  it('truncates like a clock, so no band shows its upper limit', () => {
    expect(formatAge(999.9).text).toBe('999');
    expect(formatAge(1_000).text).toBe('1.0');
    expect(formatAge(3_499).text).toBe('3.4');
    expect(formatAge(59_999).text).toBe('59.9');
    expect(formatAge(60_000)).toEqual({ text: '1:00', unitLabel: '', spoken: '1 minute 0 seconds' });
    expect(formatAge(0).text).toBe('0');
  });
});

/** The registry channel called `name`. */
function channel(name: string): ChannelDef {
  const found = CHANNEL_BY_NAME.get(name);
  if (found === undefined) throw new Error(`no channel ${name}`);
  return found;
}

describe('formatChannelValue (selected by the generated unit metadata)', () => {
  it('has a display for every canonical unit', () => {
    for (const unit of ALL_UNITS) {
      expect(DISPLAY_UNITS[unit], unit).toBeDefined();
    }
  });

  it.each([
    ['vehicle.speed_mps', 83.5, '301', 'km/h', '301 kilometres per hour'],
    ['powertrain.engine_rpm', 8_123.6, '8124', 'rpm', '8124 revolutions per minute'],
    ['inputs.throttle_raw_frac', 0.876, '88', '%', '88 percent'],
    ['inputs.brake_raw_frac', 0, '0', '%', '0 percent'],
    ['fuel.level_l', 45.67, '45.7', 'L', '45.7 litres'],
    ['fuel.used_per_lap_l', 3.456, '3.46', 'L/lap', '3.46 litres per lap'],
    ['tyres.fl.pressure_pa', 182_300, '1.82', 'bar', '1.82 bar'],
    ['weather.ambient_temp_c', 21.5, '22', '°C', '22 degrees Celsius'],
    ['vehicle.heading_rad', Math.PI / 2, '90.0', '°', '90.0 degrees'],
    ['dynamics.g_lat', -1.234, '-1.23', 'g', '-1.23 g'],
    ['track.lap_dist_m', 5_012.34, '5012.3', 'm', '5012.3 metres'],
    ['chassis.front_ride_height_m', 0.0523, '52.3', 'mm', '52.3 millimetres'],
    ['net.rtt_s', 0.0456, '45.6', 'ms', '45.6 milliseconds'],
    ['timing.delta_best_s', -0.183, '-0.183', 's', '-0.183 seconds'],
    ['timing.gap_car_ahead_s', 1.234, '+1.234', 's', '+1.234 seconds'],
    ['session.time_remaining_s', 3_725.8, '1:02:05', '', '1 hour 2 minutes 5 seconds'],
    ['fuel.margin_laps', 0.42, '+0.4', 'laps', '+0.4 laps'],
    ['powertrain.gear', 4, '4', '', '4'],
  ] as const)('%s %s', (name, value, text, unitLabel, spoken) => {
    expect(formatChannelValue(channel(name), value)).toEqual({ text, unitLabel, spoken });
  });

  it('formats lap times as m:ss.mmm and reads them in minutes and seconds', () => {
    expect(formatChannelValue(channel('timing.best_lap_s'), 209.049)).toEqual({
      text: '3:29.049',
      unitLabel: '',
      spoken: '3 minutes 29.049 seconds',
    });
    expect(formatChannelValue(channel('timing.best_lap_s'), 0).text).toBe(NO_VALUE);
  });

  it('reads 0 on a running lap timer as 0:00.000, never as a completed lap time (review F6)', () => {
    expect(formatChannelValue(channel('lap.current_time_s'), 0)).toEqual({
      text: '0:00.000',
      unitLabel: '',
      spoken: '0 minutes 00.000 seconds',
    });
    for (const name of ['timing.best_lap_s', 'timing.last_lap_s', 'timing.estimated_lap_s']) {
      expect(formatChannelValue(channel(name), 0).text, name).toBe(NO_VALUE);
    }
    expect(formatChannelValue(channel('lap.current_time_s'), -1).text).toBe(NO_VALUE);
  });

  it('reads clocks and times of day with unit words (review F7)', () => {
    expect(formatChannelValue(channel('session.time_remaining_s'), 125.4)).toEqual({
      text: '2:05',
      unitLabel: '',
      spoken: '2 minutes 5 seconds',
    });
    expect(formatChannelValue(channel('fuel.time_remaining_s'), 3_600)).toEqual({
      text: '1:00:00',
      unitLabel: '',
      spoken: '1 hour 0 minutes 0 seconds',
    });
    expect(formatChannelValue(channel('session.time_remaining_s'), -5.5)).toEqual({
      text: '-0:05',
      unitLabel: '',
      spoken: 'minus 0 minutes 5 seconds',
    });
    expect(formatChannelValue(channel('session.time_remaining_s'), -0.4).spoken).toBe('0 minutes 0 seconds');
    expect(formatChannelValue(channel('session.start_time_of_day_s'), 50_525)).toEqual({
      text: '14:02:05',
      unitLabel: '',
      spoken: '14 hours 2 minutes 5 seconds',
    });
    expect(formatChannelValue(channel('session.start_time_of_day_s'), 125).spoken).toBe('0 hours 2 minutes 5 seconds');
    expect(formatChannelValue(channel('session.start_time_of_day_s'), 86_400).text).toBe('24:00:00');
  });

  it('keeps U64 values above 2^53 exact and reads BOOL as YES / NO', () => {
    expect(formatChannelValue(channel('rec.frames_dropped'), 2n ** 53n + 1n).text).toBe('9007199254740993');
    expect(formatChannelValue(channel('rec.bytes_written'), 3n * 1024n ** 2n)).toEqual({
      text: '3.0',
      unitLabel: 'MiB',
      spoken: '3.0 mebibytes',
    });
    expect(formatChannelValue(channel('aids.tc_active'), true).text).toBe('YES');
    expect(formatChannelValue(channel('aids.tc_active'), false).text).toBe('NO');
  });

  it('never renders a missing value as zero', () => {
    for (const value of [null, undefined, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(formatChannelValue(channel('fuel.level_l'), value)).toEqual({ text: NO_VALUE, unitLabel: '', spoken: '' });
    }
  });

  it('accepts a precision override (spec §339 "make format configurable")', () => {
    expect(formatChannelValue(channel('tyres.fl.pressure_pa'), 182_345, { decimals: 3 }).text).toBe('1.823');
    expect(formatChannelValue(channel('inputs.throttle_raw_frac'), 0.8765, { decimals: 1 }).text).toBe('87.7');
  });

  it('every display-kind entry names a registry channel of a compatible unit', () => {
    const units: Record<DisplayKind, readonly Unit[]> = {
      lapTime: ['Second'],
      delta: ['Second'],
      gap: ['Second'],
      clock: ['Second'],
      timeOfDay: ['Second'],
      milliseconds: ['Second'],
      millimetres: ['Metre'],
      signed: ALL_UNITS,
    };
    for (const [name, kind] of CHANNEL_DISPLAY_KINDS) {
      expect(units[kind], name).toContain(channel(name).unit);
    }
    expect(channelUnitLabel(channel('chassis.front_ride_height_m'))).toEqual({ label: 'mm', name: 'millimetres' });
    expect(channelUnitLabel(channel('vehicle.speed_mps'))).toEqual({ label: 'km/h', name: 'kilometres per hour' });
  });
});
