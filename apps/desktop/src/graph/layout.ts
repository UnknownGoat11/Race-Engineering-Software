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
/**
 * Space above the plot for the y axis titles, CSS px. The title's box ends `Y_TITLE_GAP_CSS` above the plot, so
 * the top tick label (centred on the plot's top edge, half a text line high above it) never touches it.
 */
export const TOP_MARGIN_CSS = 20;
/** Distance from the bottom of a y axis title to the plot's top edge, CSS px. */
export const Y_TITLE_GAP_CSS = 8;
/** Height of one line of axis text, CSS px (the chrome font is 11 px). */
export const AXIS_TEXT_HEIGHT_CSS = 12;
/**
 * Space at a plot edge without a y title (top) or without x tick labels (bottom), CSS px: half a text line plus
 * one, so a tick label centred on the edge is not cut by the canvas.
 */
export const LABEL_EDGE_MARGIN_CSS = 7;
/**
 * Least plot height (measured with the full title margin) at which y axes draw their unit titles; a lower plot is
 * compact: no unit title (the pane's readout carries the unit) and the smaller top margin.
 */
export const Y_TITLE_MIN_PLOT_CSS = 60;
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
  /** The x tick labels and title are drawn under the plot (false: a stacked pane above the bottom one). */
  readonly xAxis: boolean;
  /** The y axes draw their unit titles above the plot (false: compact, see `Y_TITLE_MIN_PLOT_CSS`). */
  readonly yTitles: boolean;
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

/**
 * Layout for a graph of `width × height` CSS px with `leftAxes` / `rightAxes` y axis columns. `xAxis` false leaves
 * out the x tick label row (only the bottom pane of a stacked graph draws it). A plot lower than
 * `Y_TITLE_MIN_PLOT_CSS` becomes compact (no y unit titles, smaller top margin) instead of overflowing.
 */
export function computeLayout(width: number, height: number, leftAxes: number, rightAxes: number, xAxis = true): GraphLayout {
  const left = leftAxes > 0 ? leftAxes * Y_AXIS_WIDTH_CSS : EDGE_MARGIN_CSS;
  const right = rightAxes > 0 ? rightAxes * Y_AXIS_WIDTH_CSS : EDGE_MARGIN_CSS;
  const plotWidth = Math.max(0, Math.floor(width - left - right));
  const bottom = xAxis ? X_AXIS_HEIGHT_CSS : LABEL_EDGE_MARGIN_CSS;
  const yTitles = height - TOP_MARGIN_CSS - bottom >= Y_TITLE_MIN_PLOT_CSS;
  const top = yTitles ? TOP_MARGIN_CSS : LABEL_EDGE_MARGIN_CSS;
  const plotHeight = Math.max(0, Math.floor(height - top - bottom));
  return { width, height, plot: { left, top, width: plotWidth, height: plotHeight }, xAxis, yTitles };
}

/**
 * Indices of the y ticks (centres `pys`, CSS px, in tick order) whose labels can be drawn without overlap: each
 * label box (`textHeight` high, centred on its tick) lies inside `minY…maxY` and is at least one text height from
 * the previous drawn label.
 */
export function drawableTickLabels(
  pys: readonly number[],
  minY: number,
  maxY: number,
  textHeight: number = AXIS_TEXT_HEIGHT_CSS,
): number[] {
  const out: number[] = [];
  let last: number | null = null;
  pys.forEach((py, i) => {
    if (!Number.isFinite(py) || py - textHeight / 2 < minY || py + textHeight / 2 > maxY) return;
    if (last !== null && Math.abs(py - last) < textHeight) return;
    out.push(i);
    last = py;
  });
  return out;
}

/** Topmost y (CSS px) a y tick label may reach in `layout`: below the unit titles when they are drawn. */
export function yLabelTop(layout: GraphLayout): number {
  return layout.yTitles ? layout.plot.top - Y_TITLE_GAP_CSS : 0;
}

/** Left edge (CSS px) of the column of `axis` in `layout`. */
export function axisColumnLeft(axis: Pick<YAxisModel, 'side' | 'slot'>, layout: GraphLayout): number {
  const { plot } = layout;
  return axis.side === 'left'
    ? plot.left - (axis.slot + 1) * Y_AXIS_WIDTH_CSS
    : plot.left + plot.width + axis.slot * Y_AXIS_WIDTH_CSS;
}

/**
 * Distributes `totalHeight` CSS pixels among stacked panes proportionally to `proportions`.
 * Guarantees that the sum of returned heights equals `totalHeight` (allocating remainders to largest fractional parts).
 */
export function computePaneHeights(totalHeight: number, proportions: readonly number[]): number[] {
  const n = proportions.length;
  if (n === 0) return [];
  if (totalHeight <= 0) return Array.from({ length: n }, () => 0);
  const totalProp = proportions.reduce((sum, p) => sum + p, 0);
  if (totalProp <= 0) {
    const base = Math.floor(totalHeight / n);
    const rem = totalHeight - base * n;
    return Array.from({ length: n }, (_, i) => base + (i < rem ? 1 : 0));
  }
  const floats = proportions.map((p) => (p / totalProp) * totalHeight);
  const heights = floats.map((f) => Math.floor(f));
  const remaining = totalHeight - heights.reduce((sum, h) => sum + h, 0);
  const remainders = floats.map((f, i) => {
    const h = heights[i] ?? 0;
    return { index: i, rem: f - h };
  });
  remainders.sort((a, b) => b.rem - a.rem);
  for (let i = 0; i < remaining && i < remainders.length; i++) {
    const entry = remainders[i];
    if (entry !== undefined) {
      const idx = entry.index;
      heights[idx] = (heights[idx] ?? 0) + 1;
    }
  }
  return heights;
}

/** Layout specification of one pane in a stacked multi-pane graph. */
export interface StackedPaneLayout {
  readonly id: string;
  readonly proportion: number;
  readonly top: number;
  readonly height: number;
  readonly plot: PlotRect;
}

/**
 * Computes stacked pane layouts within `width x height` CSS pixels.
 * Height is distributed according to pane proportions, with margins reserved appropriately.
 */
export function computeStackedPaneLayouts(
  width: number,
  height: number,
  panes: readonly { readonly id: string; readonly proportion: number; readonly leftAxes: number; readonly rightAxes: number }[],
  isLastPaneWithXAxis = true,
): StackedPaneLayout[] {
  if (panes.length === 0) return [];
  const heights = computePaneHeights(height, panes.map((p) => p.proportion));
  let currentTop = 0;
  return panes.map((pane, i) => {
    const paneHeight = heights[i] ?? 0;
    const isBottom = i === panes.length - 1;
    const left = pane.leftAxes > 0 ? pane.leftAxes * Y_AXIS_WIDTH_CSS : EDGE_MARGIN_CSS;
    const right = pane.rightAxes > 0 ? pane.rightAxes * Y_AXIS_WIDTH_CSS : EDGE_MARGIN_CSS;
    const plotWidth = Math.max(0, Math.floor(width - left - right));
    const xAxisHeight = isBottom && isLastPaneWithXAxis ? X_AXIS_HEIGHT_CSS : 0;
    const plotHeight = Math.max(0, Math.floor(paneHeight - TOP_MARGIN_CSS - xAxisHeight));
    const layout: StackedPaneLayout = {
      id: pane.id,
      proportion: pane.proportion,
      top: currentTop,
      height: paneHeight,
      plot: {
        left,
        top: currentTop + TOP_MARGIN_CSS,
        width: plotWidth,
        height: plotHeight,
      },
    };
    currentTop += paneHeight;
    return layout;
  });
}

/** Least plot height of a stacked pane before trailing panes are hidden, CSS px. */
export const MIN_PANE_PLOT_CSS = 48;
/** Height of a stacked pane's header row (title and legend; each legend row is a >= 24 px checkbox target, A08). */
export const PANE_HEADER_HEIGHT_CSS = 24;

/** Fixed (non-plot) heights of a stacked graph's panes. */
export interface PaneChrome {
  /** Header row, borders and padding of one pane, CSS px. */
  readonly pane: number;
  /** Space between two panes, CSS px. */
  readonly gap: number;
}

/** Heights of the visible panes of a stacked graph. */
export interface StackedPanePlan {
  /** Number of leading panes shown (the rest do not fit at `MIN_PANE_PLOT_CSS`). */
  readonly visible: number;
  /** Outer height of each visible pane, CSS px; with the gaps they never exceed the available height. */
  readonly heights: readonly number[];
}

/** Canvas margins (top + bottom) of a pane around its plot, with the compact top margin. */
function paneCanvasMargins(bottom: boolean): number {
  return LABEL_EDGE_MARGIN_CSS + (bottom ? X_AXIS_HEIGHT_CSS : LABEL_EDGE_MARGIN_CSS);
}

/**
 * Shares `available` CSS px among stacked panes: as many leading panes as fit with a plot of at least
 * `MIN_PANE_PLOT_CSS` each (always at least one; the last visible pane carries the x axis row), with the plot
 * heights in proportion to `weights` as far as every plot keeps its minimum. The heights plus gaps never exceed
 * `available`, so the graph does not clip its own content.
 */
export function planStackedPanes(available: number, weights: readonly number[], chrome: PaneChrome): StackedPanePlan {
  const n = weights.length;
  if (n === 0) return { visible: 0, heights: [] };
  const space = Math.max(0, Math.floor(available));
  const fixed = (i: number, count: number): number => chrome.pane + paneCanvasMargins(i === count - 1);
  let visible = n;
  for (; visible > 1; visible--) {
    let need = chrome.gap * (visible - 1);
    for (let i = 0; i < visible; i++) need += fixed(i, visible) + MIN_PANE_PLOT_CSS;
    if (need <= space) break;
  }
  let plotSpace = space - chrome.gap * (visible - 1);
  for (let i = 0; i < visible; i++) plotSpace -= fixed(i, visible);
  // Only a single pane can be short of its minimum (the loop above keeps more panes only when they fit).
  if (plotSpace < 0) return { visible: 1, heights: [space] };
  const shown = weights.slice(0, visible).map((w) => (Number.isFinite(w) && w > 0 ? w : 0));
  const plots = allocateWithMinimum(plotSpace, shown, MIN_PANE_PLOT_CSS);
  return { visible, heights: plots.map((p, i) => p + fixed(i, visible)) };
}

/**
 * Integer shares of `total` in proportion to `weights`, each at least `min` while the total allows it (a share
 * that would fall below `min` is pinned there and the rest is shared among the others). Sums to `total`.
 */
export function allocateWithMinimum(total: number, weights: readonly number[], min: number): number[] {
  const n = weights.length;
  if (n === 0) return [];
  if (total < min * n) return computePaneHeights(total, weights.map(() => 1));
  const pinned = new Set<number>();
  for (;;) {
    const free = total - pinned.size * min;
    const open = weights.map((_, i) => i).filter((i) => !pinned.has(i));
    const weightSum = open.reduce((sum, i) => sum + (weights[i] ?? 0), 0);
    const share = (i: number): number => (weightSum > 0 ? ((weights[i] ?? 0) / weightSum) * free : free / open.length);
    const below = open.filter((i) => share(i) < min);
    if (below.length === 0) {
      const openHeights = computePaneHeights(free, open.map((i) => (weightSum > 0 ? (weights[i] ?? 0) : 1)));
      const out = weights.map(() => min);
      open.forEach((i, k) => {
        out[i] = openHeights[k] ?? 0;
      });
      return out;
    }
    for (const i of below) pinned.add(i);
  }
}
