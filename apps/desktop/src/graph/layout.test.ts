import { describe, expect, it } from 'vitest';
import { Unit } from '../generated/channels';
import { unitAxisDisplay } from '../lib/units';
import {
  AXIS_TEXT_HEIGHT_CSS,
  EDGE_MARGIN_CSS,
  LABEL_EDGE_MARGIN_CSS,
  MIN_PANE_PLOT_CSS,
  TOP_MARGIN_CSS,
  Y_TITLE_GAP_CSS,
  Y_TITLE_MIN_PLOT_CSS,
  X_AXIS_HEIGHT_CSS,
  Y_AXIS_WIDTH_CSS,
  axisColumnLeft,
  axisKey,
  axisRange,
  buildYAxes,
  computeLayout,
  computePaneHeights,
  computeStackedPaneLayouts,
  allocateWithMinimum,
  drawableTickLabels,
  planStackedPanes,
  tickTarget,
  yLabelTop,
  type AxisMember,
  offScaleOf,
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

describe('off-scale detection (silent-failure M7)', () => {
  it('reports which side of a fixed range the visible data exceeds', () => {
    const r = { min: 0, max: 100 };
    expect(offScaleOf(r, { min: 10, max: 90 })).toBeNull();
    expect(offScaleOf(r, { min: 10, max: 140 })).toBe('above');
    expect(offScaleOf(r, { min: -5, max: 90 })).toBe('below');
    expect(offScaleOf(r, { min: -5, max: 140 })).toBe('both');
    expect(offScaleOf(r, null)).toBeNull();
  });

  it('flags only fixed axes', () => {
    const d = unitAxisDisplay(Unit.Pascal);
    const members = [{ display: d, label: 'Brake pressure', color: 'c', extent: { min: 0, max: 14e6 } }];
    const key = `${d.label}|${d.factor}|${d.tickStyle}`;
    expect(buildYAxes(members, {}, 200)[0]?.offScale).toBeNull();
    const fixed = buildYAxes(members, { [key]: { mode: 'fixed', min: 0, max: 14e6 * d.factor / 2 } }, 200)[0];
    expect(fixed?.offScale).toBe('above');
  });
});

describe('computePaneHeights (REDESIGN.md §4 W5.1)', () => {
  it('distributes total height according to Driver worksheet proportions (35, 15, 20, 15, 15)', () => {
    const proportions = [35, 15, 20, 15, 15];
    const heights500 = computePaneHeights(500, proportions);
    expect(heights500).toEqual([175, 75, 100, 75, 75]);
    expect(heights500.reduce((a, b) => a + b, 0)).toBe(500);

    const heights480 = computePaneHeights(480, proportions);
    expect(heights480).toEqual([168, 72, 96, 72, 72]);
    expect(heights480.reduce((a, b) => a + b, 0)).toBe(480);
  });

  it('guarantees integer pixel heights whose sum matches totalHeight exactly', () => {
    const proportions = [35, 15, 20, 15, 15];
    for (const h of [100, 101, 102, 103, 333, 768, 1080]) {
      const heights = computePaneHeights(h, proportions);
      expect(heights.reduce((a, b) => a + b, 0)).toBe(h);
      for (const val of heights) {
        expect(Number.isInteger(val)).toBe(true);
      }
    }
  });

  it('handles empty proportions and non-positive total height gracefully', () => {
    expect(computePaneHeights(500, [])).toEqual([]);
    expect(computePaneHeights(0, [50, 50])).toEqual([0, 0]);
    expect(computePaneHeights(-10, [50, 50])).toEqual([0, 0]);
  });
});

describe('computeStackedPaneLayouts (REDESIGN.md §4 W5.1)', () => {
  it('computes stacked layouts reserving x-axis margin on the bottom pane only', () => {
    const panes = [
      { id: 'speed', proportion: 60, leftAxes: 1, rightAxes: 0 },
      { id: 'inputs', proportion: 40, leftAxes: 1, rightAxes: 1 },
    ];
    const layouts = computeStackedPaneLayouts(800, 500, panes, true);
    expect(layouts).toHaveLength(2);

    expect(layouts[0]?.id).toBe('speed');
    expect(layouts[0]?.height).toBe(300);
    expect(layouts[0]?.plot.top).toBe(TOP_MARGIN_CSS);
    // Non-bottom pane does not reserve X_AXIS_HEIGHT_CSS
    expect(layouts[0]?.plot.height).toBe(300 - TOP_MARGIN_CSS);
    expect(layouts[0]?.plot.left).toBe(Y_AXIS_WIDTH_CSS);
    expect(layouts[0]?.plot.width).toBe(800 - Y_AXIS_WIDTH_CSS - EDGE_MARGIN_CSS);

    expect(layouts[1]?.id).toBe('inputs');
    expect(layouts[1]?.height).toBe(200);
    expect(layouts[1]?.top).toBe(300);
    expect(layouts[1]?.plot.top).toBe(300 + TOP_MARGIN_CSS);
    // Bottom pane reserves X_AXIS_HEIGHT_CSS
    expect(layouts[1]?.plot.height).toBe(200 - TOP_MARGIN_CSS - X_AXIS_HEIGHT_CSS);
    expect(layouts[1]?.plot.left).toBe(Y_AXIS_WIDTH_CSS);
    expect(layouts[1]?.plot.width).toBe(800 - 2 * Y_AXIS_WIDTH_CSS);
  });
});

describe('computeLayout x axis row and compact plots (V02)', () => {
  it('reserves the x axis row only when asked (one x axis per stacked graph)', () => {
    const withAxis = computeLayout(800, 200, 1, 0, true);
    const without = computeLayout(800, 200, 1, 0, false);
    expect(withAxis.xAxis).toBe(true);
    expect(without.xAxis).toBe(false);
    expect(withAxis.plot.top + withAxis.plot.height).toBe(200 - X_AXIS_HEIGHT_CSS);
    expect(without.plot.top + without.plot.height).toBe(200 - LABEL_EDGE_MARGIN_CSS);
  });

  it('drops the y unit titles and their margin on a low plot instead of overflowing', () => {
    const tall = computeLayout(800, TOP_MARGIN_CSS + X_AXIS_HEIGHT_CSS + Y_TITLE_MIN_PLOT_CSS, 1, 0);
    expect(tall.yTitles).toBe(true);
    expect(tall.plot.top).toBe(TOP_MARGIN_CSS);
    const low = computeLayout(800, TOP_MARGIN_CSS + X_AXIS_HEIGHT_CSS + Y_TITLE_MIN_PLOT_CSS - 1, 1, 0);
    expect(low.yTitles).toBe(false);
    expect(low.plot.top).toBe(LABEL_EDGE_MARGIN_CSS);
    // Everything stays inside the canvas.
    expect(low.plot.top + low.plot.height).toBeLessThanOrEqual(low.height - X_AXIS_HEIGHT_CSS);
  });

  it('keeps the unit title clear of the top tick label', () => {
    const l = computeLayout(800, 200, 1, 0);
    // The top tick label is centred on the plot edge; its box starts half a text line above it.
    const topLabelTop = l.plot.top - AXIS_TEXT_HEIGHT_CSS / 2;
    const titleBottom = l.plot.top - Y_TITLE_GAP_CSS;
    const titleTop = titleBottom - AXIS_TEXT_HEIGHT_CSS;
    expect(titleBottom).toBeLessThanOrEqual(topLabelTop);
    expect(titleTop).toBeGreaterThanOrEqual(0);
    expect(yLabelTop(l)).toBe(titleBottom);
    expect(yLabelTop(computeLayout(800, 60, 1, 0))).toBe(0);
  });
});

describe('drawableTickLabels', () => {
  it('keeps labels inside the bounds and one text height apart', () => {
    // 5 ticks 10 px apart: only every other one fits with 12 px text.
    expect(drawableTickLabels([100, 90, 80, 70, 60], 0, 200, 12)).toEqual([0, 2, 4]);
  });

  it('drops a label that would reach above the title line or below the canvas', () => {
    expect(drawableTickLabels([20, 50, 197], 16, 200, 12)).toEqual([1]);
    expect(drawableTickLabels([22, 50], 16, 200, 12)).toEqual([0, 1]);
    expect(drawableTickLabels([Number.NaN, 50], 0, 200, 12)).toEqual([1]);
  });
});

describe('allocateWithMinimum', () => {
  it('shares by weight and pins small shares at the minimum, summing to the total', () => {
    expect(allocateWithMinimum(400, [3, 1], 10)).toEqual([300, 100]);
    const pinned = allocateWithMinimum(300, [35, 15, 20, 15, 15], 48);
    expect(pinned.reduce((a, b) => a + b, 0)).toBe(300);
    expect(Math.min(...pinned)).toBeGreaterThanOrEqual(48);
    expect(pinned[0]).toBeGreaterThan(pinned[1] ?? 0);
  });

  it('splits evenly when the total cannot give every share its minimum', () => {
    expect(allocateWithMinimum(30, [1, 9], 20)).toEqual([15, 15]);
  });
});

describe('planStackedPanes (V02: panes share the height, never overflow)', () => {
  const chrome = { pane: 26, gap: 4 };
  const weights = [35, 15, 20, 15, 15];
  const sum = (xs: readonly number[]): number => xs.reduce((a, b) => a + b, 0);

  it('shows every pane with at least the minimum plot when they fit, filling the space exactly', () => {
    const plan = planStackedPanes(600, weights, chrome);
    expect(plan.visible).toBe(5);
    expect(sum(plan.heights) + chrome.gap * 4).toBe(600);
    // Plot height = pane height - chrome - canvas margins; the bottom pane carries the x axis row.
    plan.heights.forEach((h, i) => {
      const margins = LABEL_EDGE_MARGIN_CSS + (i === 4 ? X_AXIS_HEIGHT_CSS : LABEL_EDGE_MARGIN_CSS);
      expect(h - chrome.pane - margins).toBeGreaterThanOrEqual(MIN_PANE_PLOT_CSS);
    });
    // The heaviest pane gets the tallest plot.
    expect(plan.heights[0]).toBe(Math.max(...plan.heights));
  });

  it('leaves out trailing panes that do not fit rather than overflowing', () => {
    const plan = planStackedPanes(300, weights, chrome);
    expect(plan.visible).toBe(3);
    expect(sum(plan.heights) + chrome.gap * (plan.visible - 1)).toBe(300);
  });

  it('always shows the first pane, shrunk to the space when even it does not fit', () => {
    expect(planStackedPanes(40, weights, chrome)).toEqual({ visible: 1, heights: [40] });
    expect(planStackedPanes(0, weights, chrome)).toEqual({ visible: 1, heights: [0] });
    expect(planStackedPanes(500, [], chrome)).toEqual({ visible: 0, heights: [] });
  });
});
