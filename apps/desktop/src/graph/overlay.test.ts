import { describe, expect, it } from 'vitest';
import { Unit } from '../generated/channels';
import { unitAxisDisplay } from '../lib/units';
import { RecordingContext } from '../test/recordingContext';
import { buildYAxes, computeLayout } from './layout';
import {
  BREAK_TICK_HEIGHT_CSS,
  CURRENT_LABEL,
  CURRENT_STALE_LABEL,
  CURRENT_DEGRADED_LABEL,
  FIXED_SCALE_SUFFIX,
  GAP_LABEL,
  MIN_BREAK_WIDTH_CSS,
  OFF_SCALE_MARK_CSS,
  OFF_SCALE_SUFFIX,
  STALE_SERIES_PREFIX,
  drawOverlay,
  drawUnderlay,
  type GraphColors,
  type OverlayScene,
} from './overlay';

const COLORS: GraphColors = {
  grid: 'grid',
  axis: 'axis',
  axisText: 'axisText',
  cursor: 'cursor',
  cursorText: 'cursorText',
  zoomFill: 'zoomFill',
  zoomStroke: 'zoomStroke',
  markers: { lap: 'lap', sector: 'sector', event: 'event' },
  currentCar: 'car',
  currentCarStale: 'carStale',
  gapFill: 'gapFill',
  gapText: 'gapText',
  errorText: 'errorText',
};

const SPEED = unitAxisDisplay(Unit.MetrePerSecond);

/** 1 axis left, plot 100…800 px wide x 0…100 s → 7 px per second. */
function scene(extra: Partial<OverlayScene> = {}): OverlayScene {
  const layout = computeLayout(852, 200, 1, 0);
  return {
    layout,
    xRange: { min: 0, max: 100 },
    xTicks: { values: [0, 50, 100], labels: ['0:00', '0:50', '1:40'] },
    xTitle: 'Session time',
    yAxes: buildYAxes([{ display: SPEED, label: 'Speed', color: 'speedColor', extent: { min: 0, max: 50 } }], {}, layout.plot.height),
    gaps: [],
    valueBreaks: [],
    markers: [],
    current: null,
    staleSeries: [],
    cursor: null,
    zoomBox: null,
    emptyText: null,
    colors: COLORS,
    font: '11px test',
    ...extra,
  };
}

describe('drawUnderlay', () => {
  it('fills significant gaps as bands under the series, clipped to the plot', () => {
    const ctx = new RecordingContext();
    const s = scene({ gaps: [{ min: 10, max: 20 }, { min: 95, max: 200 }] });
    drawUnderlay(ctx, s);
    const fills = ctx.ops('fillRect').filter((c) => c.style === 'gapFill');
    const { plot } = s.layout;
    const pxPerS = plot.width / 100;
    expect(fills).toHaveLength(2);
    expect(fills[0]?.args[0]).toBeCloseTo(plot.left + 10 * pxPerS, 6);
    expect(fills[0]?.args[2]).toBeCloseTo(10 * pxPerS, 6);
    // The second gap is cut at the plot's right edge.
    expect((fills[1]?.args[0] as number) + (fills[1]?.args[2] as number)).toBeCloseTo(plot.left + plot.width, 6);
  });

  it('draws vertical grid lines at the x ticks', () => {
    const ctx = new RecordingContext();
    drawUnderlay(ctx, scene());
    const moves = ctx.ops('moveTo').filter((c) => c.style === 'grid');
    expect(moves.length).toBeGreaterThanOrEqual(3);
  });
});

describe('drawOverlay', () => {
  it('labels the axes with their units and tick values', () => {
    const ctx = new RecordingContext();
    drawOverlay(ctx, scene());
    const texts = ctx.texts();
    expect(texts).toContain('Session time');
    expect(texts).toContain(SPEED.label);
    expect(texts).toContain('0:50');
    // Tick labels of the speed axis are km/h values (0…180 km/h for 0…50 m/s).
    expect(texts).toContain('0');
    expect(texts.some((t) => t === '180' || t === '200')).toBe(true);
  });

  it('marks a fixed axis in its title', () => {
    const ctx = new RecordingContext();
    const layout = computeLayout(852, 200, 1, 0);
    const yAxes = buildYAxes(
      [{ display: SPEED, label: 'Speed', color: 'c', extent: null }],
      { [`${SPEED.label}|${SPEED.factor}|number`]: { mode: 'fixed', min: 0, max: 300 } },
      layout.plot.height,
    );
    drawOverlay(ctx, scene({ yAxes }));
    expect(ctx.texts()).toContain(`${SPEED.label}${FIXED_SCALE_SUFFIX}`);
  });

  it('draws markers as full-height vertical lines with their labels, events dashed', () => {
    const ctx = new RecordingContext();
    const s = scene({
      markers: [
        { marker: { x: 50, kind: 'lap', label: 'Lap 3' }, px: 450, row: 0, labelX: 453, labelVisible: true },
        { marker: { x: 60, kind: 'event', label: 'Yellow' }, px: 520, row: 1, labelX: 523, labelVisible: false },
      ],
    });
    drawOverlay(ctx, s);
    const { plot } = s.layout;
    const lapLine = ctx.ops('moveTo').find((c) => c.style === 'lap');
    expect(lapLine?.args).toEqual([450.5, plot.top]);
    const lapEnd = ctx.ops('lineTo').find((c) => c.style === 'lap');
    expect(lapEnd?.args).toEqual([450.5, plot.top + plot.height]);
    expect(ctx.ops('stroke').find((c) => c.style === 'event')?.dash).toEqual([3, 3]);
    expect(ctx.ops('stroke').find((c) => c.style === 'lap')?.dash).toEqual([]);
    expect(ctx.texts()).toContain('Lap 3');
    // A label that found no row is hidden; its line is still drawn.
    expect(ctx.texts()).not.toContain('Yellow');
    expect(ctx.ops('moveTo').some((c) => c.style === 'event')).toBe(true);
  });

  it('draws the current-car marker, dashed and labelled STALE when the feed is stale (spec §82)', () => {
    const live = new RecordingContext();
    drawOverlay(live, scene({ current: { x: 80, t: 0, stale: false, degraded: false } }));
    expect(live.texts()).toContain(CURRENT_LABEL);
    expect(live.ops('stroke').find((c) => c.style === 'car')?.dash).toEqual([]);

    const stale = new RecordingContext();
    drawOverlay(stale, scene({ current: { x: 80, t: 0, stale: true, degraded: false } }));
    expect(stale.texts()).toContain(CURRENT_STALE_LABEL);
    expect(stale.ops('stroke').find((c) => c.style === 'carStale')?.dash).toEqual([3, 3]);
    expect(stale.ops('stroke').some((c) => c.style === 'car')).toBe(false);

    const degraded = new RecordingContext();
    drawOverlay(degraded, scene({ current: { x: 80, t: 0, stale: false, degraded: true } }));
    expect(degraded.texts()).toContain(CURRENT_DEGRADED_LABEL);
  });

  it('does not draw a current-car marker outside the visible range', () => {
    const ctx = new RecordingContext();
    drawOverlay(ctx, scene({ current: { x: 150, t: 0, stale: false, degraded: false } }));
    expect(ctx.texts()).not.toContain(CURRENT_LABEL);
  });

  it('draws the cursor line with its x label and the gap indicator text', () => {
    const ctx = new RecordingContext();
    drawOverlay(ctx, scene({ cursor: { x: 25, label: '0:25.00' }, gaps: [{ min: 30, max: 60 }] }));
    expect(ctx.texts()).toContain('0:25.00');
    expect(ctx.ops('fillText').find((c) => c.args[0] === '0:25.00')?.style).toBe('cursorText');
    expect(ctx.texts()).toContain(GAP_LABEL);
  });

  it('shows the empty text and no x ticks without a range', () => {
    const ctx = new RecordingContext();
    drawOverlay(ctx, scene({ xRange: null, emptyText: 'NO DATA', current: { x: 1, t: 0, stale: false, degraded: false } }));
    expect(ctx.texts()).toContain('NO DATA');
    expect(ctx.texts()).not.toContain('0:50');
    expect(ctx.texts()).not.toContain(CURRENT_LABEL);
  });

  it('draws the zoom box being dragged', () => {
    const ctx = new RecordingContext();
    drawOverlay(ctx, scene({ zoomBox: { x0: 400, x1: 300 } }));
    const box = ctx.ops('fillRect').find((c) => c.style === 'zoomFill');
    expect(box?.args[0]).toBe(300);
    expect(box?.args[2]).toBe(100);
  });

  it('names each stale series at the plot right edge in the stale colour', () => {
    const ctx = new RecordingContext();
    drawOverlay(ctx, scene({ staleSeries: [{ label: 'Speed', color: 'speedColor' }] }));
    expect(ctx.texts()).toContain(`${STALE_SERIES_PREFIX}Speed`);
    const fill = ctx.ops('fillText').find((c) => c.args[0] === `${STALE_SERIES_PREFIX}Speed`);
    expect(fill?.style).toBe('carStale');
  });
});

describe('break visibility (silent-failure M2)', () => {
  it('draws a gap narrower than a pixel at least MIN_BREAK_WIDTH_CSS wide', () => {
    const ctx = new RecordingContext();
    // 0.01 s at 7 px/s = 0.07 px.
    drawUnderlay(ctx, scene({ gaps: [{ min: 50, max: 50.01 }] }));
    const band = ctx.ops('fillRect').find((c) => c.style === 'gapFill');
    expect(band?.args[2]).toBeCloseTo(MIN_BREAK_WIDTH_CSS, 9);
  });

  it('marks each value break of a series with a tick in its colour', () => {
    const ctx = new RecordingContext();
    drawOverlay(ctx, scene({ valueBreaks: [{ color: 'speedColor', ranges: [{ min: 20, max: 20 }] }] }));
    const tick = ctx.ops('fillRect').find((c) => c.style === 'speedColor');
    expect(tick).toBeDefined();
    expect(tick?.args[2]).toBeCloseTo(MIN_BREAK_WIDTH_CSS, 9);
    expect(tick?.args[3]).toBe(BREAK_TICK_HEIGHT_CSS);
  });
});

describe('off-scale marking (silent-failure M7)', () => {
  it('marks a fixed axis that clips visible data in its title and at the clipped end', () => {
    const ctx = new RecordingContext();
    const layout = computeLayout(852, 200, 1, 0);
    const yAxes = buildYAxes(
      [{ display: SPEED, label: 'Speed', color: 'axisColor', extent: { min: 0, max: 100 } }],
      { [`${SPEED.label}|${SPEED.factor}|number`]: { mode: 'fixed', min: 0, max: 100 } },
      layout.plot.height,
    );
    expect(yAxes[0]?.offScale).toBe('above');
    drawOverlay(ctx, scene({ yAxes }));
    expect(ctx.texts()).toContain(`${SPEED.label}${FIXED_SCALE_SUFFIX}${OFF_SCALE_SUFFIX}`);
    const bars = ctx.ops('fillRect').filter((c) => c.style === 'axisColor');
    expect(bars).toHaveLength(1);
    expect(bars[0]?.args[1]).toBe(layout.plot.top);
    expect(bars[0]?.args.slice(2)).toEqual([OFF_SCALE_MARK_CSS.width, OFF_SCALE_MARK_CSS.height]);
  });
});
