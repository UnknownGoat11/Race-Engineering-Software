import { describe, expect, it } from 'vitest';
import { CHANNEL_BY_ID, ch } from '../generated/channels';
import { DISPLAY_UNITS, NO_VALUE, channelAxisDisplay, formatAxisTick } from '../lib/units';
import { X_AXIS_SPECS, formatX, xAxisTitle, xTicks } from './xAxis';

function def(id: number) {
  const d = CHANNEL_BY_ID.get(id);
  if (d === undefined) throw new Error(`unknown channel ${id}`);
  return d;
}

describe('x axis modes', () => {
  it('describes every mode with a title, unit and live behaviour', () => {
    expect(xAxisTitle(X_AXIS_SPECS.time)).toBe('Session time');
    expect(xAxisTitle(X_AXIS_SPECS.lapTime)).toBe('Lap time');
    expect(xAxisTitle(X_AXIS_SPECS.lapDistance)).toBe('Lap distance (m)');
    expect(xAxisTitle(X_AXIS_SPECS.lapFraction)).toBe('Lap distance (normalized) (%)');
    expect(X_AXIS_SPECS.time.viewKind).toBe('scroll');
    expect(X_AXIS_SPECS.lapDistance.viewKind).toBe('domain');
    expect(X_AXIS_SPECS.lapFraction.domain).toEqual({ min: 0, max: 1 });
  });

  it('labels time ticks as a clock on clock-friendly steps', () => {
    const t = xTicks(X_AXIS_SPECS.time, { min: 750, max: 810 }, 4);
    expect(t.values).toEqual([750, 765, 780, 795, 810]);
    expect(t.labels).toEqual(['12:30', '12:45', '13:00', '13:15', '13:30']);
  });

  it('labels sub-second time ticks with decimals', () => {
    const t = xTicks(X_AXIS_SPECS.time, { min: 3600, max: 3601 }, 4);
    expect(t.labels[0]).toBe('1:00:00.0');
    expect(t.labels[1]).toBe('1:00:00.5');
  });

  it('maps normalized lap distance to percent ticks while keeping x in fractions', () => {
    const t = xTicks(X_AXIS_SPECS.lapFraction, { min: 0, max: 1 }, 5);
    expect(t.values).toEqual([0, 0.2, 0.4, 0.6, 0.8, 1]);
    expect(t.labels).toEqual(['0', '20', '40', '60', '80', '100']);
  });

  it('labels lap distance in metres', () => {
    const t = xTicks(X_AXIS_SPECS.lapDistance, { min: 0, max: 13_626 }, 5);
    expect(t.values).toEqual([0, 5000, 10_000]);
    expect(t.labels).toEqual(['0', '5000', '10000']);
  });

  it('formats a cursor position with the axis unit', () => {
    expect(formatX(X_AXIS_SPECS.time, 752.256)).toBe('12:32.26');
    expect(formatX(X_AXIS_SPECS.lapDistance, 1250.4)).toBe('1250 m');
    expect(formatX(X_AXIS_SPECS.lapFraction, 0.4567)).toBe('45.7 %');
    expect(formatX(X_AXIS_SPECS.lapTime, Number.NaN)).toBe(NO_VALUE);
  });
});

describe('channel axis display (lib/units.ts)', () => {
  it('converts proportionally to display units', () => {
    const speed = channelAxisDisplay(def(ch.SPEED));
    expect(speed.factor).toBeCloseTo(3.6, 12);
    expect(speed.label).toBe('km/h');
    expect(channelAxisDisplay(def(ch.THROTTLE))).toMatchObject({ factor: 100, label: '%', tickStyle: 'number' });
    expect(channelAxisDisplay(def(ch.LAP_TIME))).toMatchObject({ factor: 1, label: '', tickStyle: 'clock' });
  });

  it('has no offset in any display conversion (axes rely on proportionality)', () => {
    for (const unit of Object.values(DISPLAY_UNITS)) {
      const f = unit.toDisplay(1);
      expect(unit.toDisplay(0)).toBe(0);
      expect(unit.toDisplay(-2.5)).toBeCloseTo(-2.5 * f, 9);
    }
  });

  it('writes ticks in every style', () => {
    expect(formatAxisTick(12.5, 1, 'number')).toBe('12.5');
    expect(formatAxisTick(-0.0001, 1, 'number')).toBe('0.0');
    expect(formatAxisTick(0.25, 2, 'signed')).toBe('+0.25');
    expect(formatAxisTick(0, 1, 'signed')).toBe('0.0');
    expect(formatAxisTick(-0.5, 1, 'signed')).toBe('-0.5');
    expect(formatAxisTick(83.456, 3, 'clock')).toBe('1:23.456');
    expect(formatAxisTick(-5, 0, 'clock')).toBe('-0:05');
    expect(formatAxisTick(45_296, 0, 'timeOfDay')).toBe('12:34:56');
    expect(formatAxisTick(Number.POSITIVE_INFINITY, 0, 'number')).toBe(NO_VALUE);
  });
});
