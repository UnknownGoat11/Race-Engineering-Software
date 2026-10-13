import { describe, expect, it } from 'vitest';
import { Unit } from '../generated/channels';
import { unitAxisDisplay } from '../lib/units';
import {
  EDGE_MARGIN_CSS,
  TOP_MARGIN_CSS,
  X_AXIS_HEIGHT_CSS,
  Y_AXIS_WIDTH_CSS,
  axisColumnLeft,
  axisKey,
  axisRange,
  buildYAxes,
  computeLayout,
  tickTarget,
  type AxisMember,
} from './layout';

const SPEED = unitAxisDisplay(Unit.MetrePerSecond);
const PRESSURE = unitAxisDisplay(Unit.Pascal);
const FRACTION = unitAxisDisplay(Unit.Fraction);
const UNITLESS = unitAxisDisplay(Unit.None);

function member(display: AxisMember['display'], extent: AxisMember['extent'], label = 'Channel', color = '#111'): AxisMember {
  return { display, label, color, extent };
}

describe('axisRange', () => {
  it('autoscales in display units to nice ticks and converts back to internal units', () => {
    // 10…50 m/s = 36…180 km/h → 0…200 km/h with 5 intervals of 40.
    const r = axisRange({ mode: 'auto' }, { min: 10, max: 50 }, SPEED, 5);
    expect(r?.min).toBeCloseTo(0, 9);
    expect((r?.max ?? 0) * SPEED.factor).toBeCloseTo(200, 9);
  });

  it('uses a fixed scale given in display units', () => {
    const r = axisRange({ mode: 'fixed', min: 0, max: 300 }, { min: 10, max: 50 }, SPEED, 5);
    expect(r?.min).toBe(0);
    expect(r?.max).toBeCloseTo(300 / 3.6, 9);
  });

  it('has no range without data or for an invalid fixed scale (never invents one)', () => {
    expect(axisRange({ mode: 'auto' }, null, SPEED, 5)).toBeNull();
    expect(axisRange({ mode: 'fixed', min: 5, max: 5 }, null, SPEED, 5)).toBeNull();
    expect(axisRange({ mode: 'fixed', min: Number.NaN, max: 5 }, null, SPEED, 5)).toBeNull();
  });
});

describe('buildYAxes', () => {
  it('groups series by display unit, in order of first appearance, alternating sides', () => {
    const axes = buildYAxes(
      [
        member(SPEED, { min: 10, max: 20 }, 'Speed', '#a'),
        member(PRESSURE, { min: 1e5, max: 2e5 }, 'Tyre pressure', '#b'),
        member(SPEED, { min: 5, max: 40 }, 'Other speed', '#c'),
        member(FRACTION, { min: 0, max: 1 }, 'Throttle', '#d'),
        member(UNITLESS, { min: 0, max: 3 }, 'Gear', '#e'),
      ],
      {},
      200,
    );
    expect(axes.map((a) => a.title)).toEqual([SPEED.label, PRESSURE.label, '%', 'Gear']);
    expect(axes.map((a) => [a.side, a.slot])).toEqual([
      ['left', 0],
      ['right', 0],
      ['left', 1],
      ['right', 1],
    ]);
    expect(axes[0]?.color).toBe('#a');
    // The speed axis covers both speed series (5…40 m/s).
    expect((axes[0]?.range?.min ?? Number.NaN) * SPEED.factor).toBeLessThanOrEqual(18);
    expect((axes[0]?.range?.max ?? Number.NaN) * SPEED.factor).toBeGreaterThanOrEqual(144);
    expect(axes[0]?.ticks.values.length).toBeGreaterThan(1);
  });

  it('applies per-axis settings by axis key and shows no ticks without a range', () => {
    const key = axisKey(SPEED);
    const axes = buildYAxes(
      [member(SPEED, { min: 10, max: 20 }), member(PRESSURE, null)],
      { [key]: { mode: 'fixed', min: 0, max: 100 } },
      200,
    );
    expect(axes[0]?.setting).toEqual({ mode: 'fixed', min: 0, max: 100 });
    expect(axes[0]?.ticks.values[0]).toBe(0);
    expect(axes[0]?.ticks.values.at(-1)).toBe(100);
    expect(axes[1]?.range).toBeNull();
    expect(axes[1]?.ticks.values).toEqual([]);
  });
});

describe('computeLayout', () => {
  it('reserves one column per y axis on each side and room for the x axis', () => {
    const l = computeLayout(800, 200, 2, 1);
    expect(l.plot).toEqual({
      left: 2 * Y_AXIS_WIDTH_CSS,
      top: TOP_MARGIN_CSS,
      width: 800 - 3 * Y_AXIS_WIDTH_CSS,
      height: 200 - TOP_MARGIN_CSS - X_AXIS_HEIGHT_CSS,
    });
    expect(axisColumnLeft({ side: 'left', slot: 0 }, l)).toBe(Y_AXIS_WIDTH_CSS);
    expect(axisColumnLeft({ side: 'left', slot: 1 }, l)).toBe(0);
    expect(axisColumnLeft({ side: 'right', slot: 0 }, l)).toBe(800 - Y_AXIS_WIDTH_CSS);
  });

  it('keeps a small margin on sides without axes and never goes negative', () => {
    expect(computeLayout(300, 100, 0, 0).plot.left).toBe(EDGE_MARGIN_CSS);
    expect(computeLayout(10, 10, 3, 3).plot).toMatchObject({ width: 0, height: 0 });
  });

  it('asks for about one tick per spacing, at least two', () => {
    expect(tickTarget(360, 36)).toBe(10);
    expect(tickTarget(10, 36)).toBe(2);
  });
});
