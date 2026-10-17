/**
 * Graph geometry and y axes (spec §62 "multiple y axes", "units", "autoscale / fixed scale"). Pure functions.
 *
 * Series are grouped onto one y axis per display unit (axes are keyed by unit label, conversion factor and tick
 * style, so km/h speed channels share an axis and bar pressures another). Axes alternate sides: the first on
 * the left next to the plot, the second on the right, the third left of the first, and so on. An axis range is
 * either autoscaled to the visible data (rounded out to nice ticks in display units) or fixed by the user in
 * display units; an autoscaled axis without visible data has no range and draws no scale (nothing invented).
 */
import type { AxisDisplay } from '../lib/units';
import type { PlotRect } from './renderer';
import { autoRange, niceTicks, unionRange, type Ticks } from './scale';
import type { XRange, YScaleSetting } from './types';

/** Width of one y axis column (tick labels and title), CSS px. */
export const Y_AXIS_WIDTH_CSS = 52;
/** Height under the plot for x tick labels, CSS px. */
export const X_AXIS_HEIGHT_CSS = 22;
/** Space above the plot for the y axis titles, CSS px. */
export const TOP_MARGIN_CSS = 16;
/** Space at a plot edge without an axis, CSS px. */
export const EDGE_MARGIN_CSS = 8;
/** About one y tick per this many CSS px of plot height. */
export const Y_TICK_SPACING_CSS = 36;
/** About one x tick per this many CSS px of plot width. */
export const X_TICK_SPACING_CSS = 90;
/** Fewest tick intervals an axis asks for. */
const MIN_TICK_INTERVALS = 2;

/** Side of a y axis. */
export type AxisSide = 'left' | 'right';

/** What a series contributes to its y axis. */
export interface AxisMember {
  /** Display of the series' channel (lib/units.ts `channelAxisDisplay`). */
  readonly display: AxisDisplay;
  /** Channel label, used as the axis title when the unit has no label. */
  readonly label: string;
  /** Resolved colour of the series (the axis title takes its first member's colour). */
  readonly color: string;
  /** Visible data extent in internal units, or null. */
  readonly extent: XRange | null;
}

/** One y axis of a frame. */
export interface YAxisModel {
  readonly key: string;
  readonly display: AxisDisplay;
  /** Unit label, or the first member's channel label for unitless axes. */
  readonly title: string;
  readonly side: AxisSide;
  /** Column on its side, 0 = next to the plot. */
  readonly slot: number;
  readonly setting: YScaleSetting;
  /** Range in internal units, or null when autoscaled without data. */
  readonly range: XRange | null;
  /** Ticks in display units for `range` (empty without range). */
  readonly ticks: Ticks;
  /**
   * Where visible data of a FIXED axis lies outside its range and is clipped (silent-failure M7): 'above',
   * 'below', 'both', or null (autoscaled, or everything inside).
   */
  readonly offScale: OffScale | null;
  readonly color: string;
}

/** Plot area and margins of a graph of `width × height` CSS px. */
export interface GraphLayout {
  readonly width: number;
  readonly height: number;
  readonly plot: PlotRect;
}

/** Side(s) of a fixed axis range that visible data exceeds. */
export type OffScale = 'above' | 'below' | 'both';

/** Where `extent` exceeds `range` (both internal units), or null. */
export function offScaleOf(range: XRange | null, extent: XRange | null): OffScale | null {
  if (range === null || extent === null) return null;
  const above = extent.max > range.max;
  const below = extent.min < range.min;
  return above && below ? 'both' : above ? 'above' : below ? 'below' : null;
}

/** Key of the axis a display belongs to. */
export function axisKey(display: AxisDisplay): string {
  return `${display.label}|${display.factor}|${display.tickStyle}`;
}

/** Number of tick intervals for `lengthCss` of axis at about one tick per `spacingCss`. */
export function tickTarget(lengthCss: number, spacingCss: number): number {
  return Math.max(MIN_TICK_INTERVALS, Math.floor(lengthCss / spacingCss));
}

/** Range of an axis in internal units for `setting` (see the module comment). */
export function axisRange(setting: YScaleSetting, extent: XRange | null, display: AxisDisplay, targetTicks: number): XRange | null {
  const f = display.factor;
  if (setting.mode === 'fixed') {
    if (!Number.isFinite(setting.min) || !Number.isFinite(setting.max) || !(setting.max > setting.min)) return null;
    return { min: setting.min / f, max: setting.max / f };
  }
  const shown = extent === null ? null : { min: extent.min * f, max: extent.max * f };
  const range = autoRange(shown, targetTicks);
  return range === null ? null : { min: range.min / f, max: range.max / f };
}

const AUTO: YScaleSetting = { mode: 'auto' };
const NO_TICKS: Ticks = { values: [], step: 0, decimals: 0 };

/**
 * Groups `members` onto y axes in order of first appearance and computes each axis' range and ticks for a plot
 * `plotHeight` CSS px high. `settings` maps axis keys to their scale (default auto).
 */
export function buildYAxes(
  members: readonly AxisMember[],
  settings: Readonly<Record<string, YScaleSetting>>,
  plotHeight: number,
): YAxisModel[] {
  const groups = new Map<string, { first: AxisMember; extent: XRange | null }>();
  for (const m of members) {
    const key = axisKey(m.display);
    const group = groups.get(key);
    if (group === undefined) groups.set(key, { first: m, extent: m.extent });
    else group.extent = unionRange(group.extent, m.extent);
  }
  const target = tickTarget(plotHeight, Y_TICK_SPACING_CSS);
  const axes: YAxisModel[] = [];
  let index = 0;
  for (const [key, { first, extent }] of groups) {
    const setting = settings[key] ?? AUTO;
    const range = axisRange(setting, extent, first.display, target);
    const f = first.display.factor;
    axes.push({
      key,
      display: first.display,
      title: first.display.label === '' ? first.label : first.display.label,
      side: index % 2 === 0 ? 'left' : 'right',
      slot: Math.floor(index / 2),
      setting,
      range,
      ticks: range === null ? NO_TICKS : niceTicks(range.min * f, range.max * f, target),
      offScale: setting.mode === 'fixed' ? offScaleOf(range, extent) : null,
      color: first.color,
    });
    index += 1;
  }
  return axes;
}

/** Layout for a graph of `width × height` CSS px with `leftAxes` / `rightAxes` y axis columns. */
export function computeLayout(width: number, height: number, leftAxes: number, rightAxes: number): GraphLayout {
  const left = leftAxes > 0 ? leftAxes * Y_AXIS_WIDTH_CSS : EDGE_MARGIN_CSS;
  const right = rightAxes > 0 ? rightAxes * Y_AXIS_WIDTH_CSS : EDGE_MARGIN_CSS;
  const plotWidth = Math.max(0, Math.floor(width - left - right));
  const plotHeight = Math.max(0, Math.floor(height - TOP_MARGIN_CSS - X_AXIS_HEIGHT_CSS));
  return { width, height, plot: { left, top: TOP_MARGIN_CSS, width: plotWidth, height: plotHeight } };
}

/** Left edge (CSS px) of the column of `axis` in `layout`. */
export function axisColumnLeft(axis: Pick<YAxisModel, 'side' | 'slot'>, layout: GraphLayout): number {
  const { plot } = layout;
  return axis.side === 'left'
    ? plot.left - (axis.slot + 1) * Y_AXIS_WIDTH_CSS
    : plot.left + plot.width + axis.slot * Y_AXIS_WIDTH_CSS;
}
