/**
 * The graph chrome the engine draws around the series layer, in CSS pixels (callers set the device-pixel
 * transform). Under the series: grid and gap bands (spec §219 gap indicator). Over the series: axes with unit
 * titles (§62 units), sector/lap/event markers, the current-car marker (§82: frozen and marked STALE when the
 * feed is stale), the synchronized cursor with its x label (§81) and the zoom box.
 */
import { formatAxisTick } from '../lib/units';
import type { Canvas2DLike } from './canvasContext';
import {
  Y_AXIS_WIDTH_CSS,
  Y_TITLE_GAP_CSS,
  axisColumnLeft,
  drawableTickLabels,
  yLabelTop,
  type GraphLayout,
  type YAxisModel,
} from './layout';
import type { PlacedMarker } from './markers';
import { applyAffine, linearAffine, type Affine } from './scale';
import type { CurrentPosition, MarkerKind, XRange } from './types';
import type { LabelledTicks } from './xAxis';

/** Resolved colours of the chrome (from the --graph-* tokens). */
export interface GraphColors {
  readonly grid: string;
  readonly axis: string;
  readonly axisText: string;
  readonly cursor: string;
  readonly cursorText: string;
  readonly zoomFill: string;
  readonly zoomStroke: string;
  readonly markers: Readonly<Record<MarkerKind, string>>;
  readonly currentCar: string;
  readonly currentCarStale: string;
  readonly gapFill: string;
  readonly gapText: string;
  /** Text of the error state (a graph that cannot show current data). */
  readonly errorText: string;
}

/** A series whose channel is STALE while the feed as a whole is not (named on the plot). */
export interface StaleSeriesTag {
  readonly label: string;
  readonly color: string;
}

/** Value breaks of one series (see `findValueBreaks`). */
export interface SeriesBreaks {
  readonly color: string;
  readonly ranges: readonly XRange[];
}

/** Cursor position and its x label. */
export interface CursorMark {
  readonly x: number;
  readonly label: string;
}

/** Everything the chrome shows in one frame. */
export interface OverlayScene {
  readonly layout: GraphLayout;
  /** Visible x range, or null when there is nothing to show yet. */
  readonly xRange: XRange | null;
  readonly xTicks: LabelledTicks;
  /** X axis title with unit, e.g. "Lap distance (m)". */
  readonly xTitle: string;
  readonly yAxes: readonly YAxisModel[];
  /** X intervals of significant time gaps. */
  readonly gaps: readonly XRange[];
  /** Per series: x intervals where it has no value between valid rows (drawn as ticks in its colour). */
  readonly valueBreaks: readonly SeriesBreaks[];
  readonly markers: readonly PlacedMarker[];
  /** Current car position (live sources only). */
  readonly current: CurrentPosition | null;
  /** Series drawn dashed because their channel is STALE (spec §126; the feed itself may be live). */
  readonly staleSeries: readonly StaleSeriesTag[];
  readonly cursor: CursorMark | null;
  /** Zoom box being dragged, CSS px within the drawing area. */
  readonly zoomBox: { readonly x0: number; readonly x1: number } | null;
  /** Shown in the middle of an empty plot, e.g. "NO DATA". */
  readonly emptyText: string | null;
  readonly colors: GraphColors;
  /** CSS font of the chrome text. */
  readonly font: string;
}

/** Height of a marker label row, CSS px. */
export const MARKER_ROW_HEIGHT_CSS = 13;
/** Number of marker label rows at the top of the plot. */
export const MARKER_ROWS = 2;
/** Space between a marker line and its label, and between two labels, CSS px. */
export const MARKER_LABEL_GAP_CSS = 3;
/** Label of the current-car marker (spec §82) and its stale / degraded variants (spec §126). */
export const CURRENT_LABEL = 'CURRENT';
export const CURRENT_STALE_LABEL = 'CURRENT · STALE';
export const CURRENT_DEGRADED_LABEL = 'CURRENT · DEGRADED';
/** Prefix of the tag naming a STALE series. */
export const STALE_SERIES_PREFIX = 'STALE · ';
/** Label of a significant data gap (spec §219). */
export const GAP_LABEL = 'GAP';
/** Suffix of the title of an axis with a fixed scale. */
export const FIXED_SCALE_SUFFIX = ' · FIXED';
/** Added to the title of a fixed axis whose visible data is clipped (silent-failure M7). */
export const OFF_SCALE_SUFFIX = ' · OFF-SCALE';
/** Size of the off-scale marker drawn at the clipped end of an axis, CSS px. */
export const OFF_SCALE_MARK_CSS = { width: 8, height: 4 } as const;

/**
 * Least drawn width of a gap band or a value-break tick, CSS px: a break inside one pixel column would otherwise be
 * invisible and the trace would look continuous (silent-failure M2).
 */
export const MIN_BREAK_WIDTH_CSS = 2;
/** Height of a value-break tick at the bottom of the plot, CSS px. */
export const BREAK_TICK_HEIGHT_CSS = 5;

/** Tick mark length, CSS px. */
const TICK_CSS = 4;
/** Padding inside text boxes, CSS px. */
const TEXT_PAD_CSS = 3;
/** Text height used for boxes, CSS px. */
const TEXT_HEIGHT_CSS = 12;
/** Least space between an x tick label and the axis title, CSS px. */
const TITLE_CLEARANCE_CSS = 6;
/**
 * Space between the plot's bottom edge and the x tick labels, CSS px: below the bottom y tick label (half a text
 * line under the edge), still inside the x axis row.
 */
export const X_LABEL_OFFSET_CSS = 7;
/** Dash of stale current-car markers and event markers, CSS px. */
const SHORT_DASH: number[] = [3, 3];
const NO_DASH: number[] = [];
/** Width of the current-car marker line, CSS px. */
const CURRENT_LINE_WIDTH_CSS = 2;
/** Offset to a pixel centre, so 1-px lines are crisp. */
const HALF_PX = 0.5;

/** x → CSS px of the scene's plot (null without a range). */
export function xToPx(scene: Pick<OverlayScene, 'layout' | 'xRange'>): Affine | null {
  const { plot } = scene.layout;
  const r = scene.xRange;
  if (r === null || !(r.max > r.min) || !(plot.width > 0)) return null;
  return linearAffine(r.min, r.max, plot.left, plot.left + plot.width);
}

function crisp(v: number): number {
  return Math.round(v) + HALF_PX;
}

function vLine(ctx: Canvas2DLike, x: number, top: number, bottom: number): void {
  ctx.moveTo(x, top);
  ctx.lineTo(x, bottom);
}

/** Plot-clipped pixel interval of an x interval, or null when it is not visible. */
function visibleSpan(scene: OverlayScene, x: Affine, gap: XRange): readonly [number, number] | null {
  const { plot } = scene.layout;
  const x0 = Math.max(plot.left, applyAffine(x, gap.min));
  const x1 = Math.min(plot.left + plot.width, applyAffine(x, gap.max));
  return x1 > x0 ? [x0, x1] : null;
}

/** Like `visibleSpan`, widened around its centre to at least `MIN_BREAK_WIDTH_CSS` (a zero-width break too). */
function breakSpan(scene: OverlayScene, x: Affine, gap: XRange): readonly [number, number] | null {
  const { plot } = scene.layout;
  const a = applyAffine(x, gap.min);
  const b = applyAffine(x, gap.max);
  if (!Number.isFinite(a) || !Number.isFinite(b) || b < plot.left || a > plot.left + plot.width) return null;
  const centre = (Math.max(plot.left, a) + Math.min(plot.left + plot.width, b)) / 2;
  const half = Math.max(MIN_BREAK_WIDTH_CSS, Math.min(b, plot.left + plot.width) - Math.max(a, plot.left)) / 2;
  return [centre - half, centre + half];
}

/** Grid lines and gap bands, drawn under the series. */
export function drawUnderlay(ctx: Canvas2DLike, scene: OverlayScene): void {
  const { plot } = scene.layout;
  const x = xToPx(scene);
  const bottom = plot.top + plot.height;
  ctx.save();
  ctx.beginPath();
  ctx.rect(plot.left, plot.top, plot.width, plot.height);
  ctx.clip();
  if (x !== null) {
    ctx.fillStyle = scene.colors.gapFill;
    for (const gap of scene.gaps) {
      const span = breakSpan(scene, x, gap);
      if (span !== null) ctx.fillRect(span[0], plot.top, span[1] - span[0], plot.height);
    }
  }
  ctx.lineWidth = 1;
  ctx.strokeStyle = scene.colors.grid;
  ctx.setLineDash(NO_DASH);
  ctx.beginPath();
  if (x !== null) {
    for (const v of scene.xTicks.values) vLine(ctx, crisp(applyAffine(x, v)), plot.top, bottom);
  }
  const primary = scene.yAxes[0];
  if (primary?.range != null) {
    const f = primary.display.factor;
    const y = linearAffine(primary.range.min * f, primary.range.max * f, bottom, plot.top);
    for (const v of primary.ticks.values) {
      const py = crisp(applyAffine(y, v));
      ctx.moveTo(plot.left, py);
      ctx.lineTo(plot.left + plot.width, py);
    }
  }
  ctx.stroke();
  ctx.restore();
}

/** Axes, markers, current-car marker, cursor and zoom box, drawn over the series. */
export function drawOverlay(ctx: Canvas2DLike, scene: OverlayScene): void {
  const { plot } = scene.layout;
  const x = xToPx(scene);
  ctx.save();
  ctx.font = scene.font;
  ctx.lineWidth = 1;
  ctx.setLineDash(NO_DASH);

  ctx.strokeStyle = scene.colors.axis;
  ctx.beginPath();
  ctx.rect(crisp(plot.left), crisp(plot.top), Math.max(0, Math.round(plot.width) - 1), Math.max(0, Math.round(plot.height) - 1));
  ctx.stroke();

  if (scene.layout.xAxis) drawXAxis(ctx, scene, x);
  for (const axis of scene.yAxes) drawYAxis(ctx, scene, axis);

  ctx.save();
  ctx.beginPath();
  ctx.rect(plot.left, plot.top, plot.width, plot.height);
  ctx.clip();
  if (x !== null) {
    drawGapLabels(ctx, scene, x);
    drawValueBreaks(ctx, scene, x);
    drawMarkers(ctx, scene);
    drawCurrent(ctx, scene, x);
  }
  drawStaleSeries(ctx, scene);
  ctx.restore();

  if (x !== null && scene.cursor !== null) drawCursor(ctx, scene, x, scene.cursor);
  if (scene.zoomBox !== null) drawZoomBox(ctx, scene, scene.zoomBox);
  if (scene.emptyText !== null) {
    ctx.fillStyle = scene.colors.axisText;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(scene.emptyText, plot.left + plot.width / 2, plot.top + plot.height / 2);
  }
  ctx.restore();
}

function drawXAxis(ctx: Canvas2DLike, scene: OverlayScene, x: Affine | null): void {
  const { plot } = scene.layout;
  const bottom = plot.top + plot.height;
  const labelTop = bottom + X_LABEL_OFFSET_CSS;
  // The title is right-aligned under the plot's right end; tick labels that would touch it are left out.
  const titleWidth = ctx.measureText(scene.xTitle).width;
  const titleLeft = plot.left + plot.width - titleWidth;
  ctx.fillStyle = scene.colors.axisText;
  ctx.strokeStyle = scene.colors.axis;
  ctx.textBaseline = 'top';
  if (x !== null) {
    ctx.textAlign = 'center';
    ctx.beginPath();
    scene.xTicks.values.forEach((v, i) => {
      const px = crisp(applyAffine(x, v));
      vLine(ctx, px, bottom, bottom + TICK_CSS);
      const label = scene.xTicks.labels[i];
      if (label === undefined) return;
      const half = ctx.measureText(label).width / 2;
      if (px + half + TITLE_CLEARANCE_CSS <= titleLeft && px - half >= 0) ctx.fillText(label, px, labelTop);
    });
    ctx.stroke();
  }
  ctx.textAlign = 'right';
  ctx.fillText(scene.xTitle, plot.left + plot.width, labelTop);
}

function drawYAxis(ctx: Canvas2DLike, scene: OverlayScene, axis: YAxisModel): void {
  const { plot } = scene.layout;
  const bottom = plot.top + plot.height;
  const left = axis.side === 'left';
  const columnLeft = axisColumnLeft(axis, scene.layout);
  // The axis line is the column edge nearest the plot.
  const lineX = crisp(left ? columnLeft + Y_AXIS_WIDTH_CSS : columnLeft);
  const textX = left ? lineX - TICK_CSS - 1 : lineX + TICK_CSS + 1;
  ctx.strokeStyle = axis.color;
  ctx.fillStyle = axis.color;
  ctx.beginPath();
  vLine(ctx, lineX, plot.top, bottom);
  ctx.textAlign = left ? 'right' : 'left';
  ctx.textBaseline = 'bottom';
  // A compact plot has no room for the unit title; the pane's readout carries the unit.
  if (scene.layout.yTitles) {
    const title =
      axis.setting.mode === 'fixed' ? `${axis.title}${FIXED_SCALE_SUFFIX}${axis.offScale === null ? '' : OFF_SCALE_SUFFIX}` : axis.title;
    ctx.fillText(title, textX, plot.top - Y_TITLE_GAP_CSS);
  }
  if (axis.offScale !== null) {
    // A solid bar on the clipped end(s) of the axis line, in the axis colour.
    const { width, height } = OFF_SCALE_MARK_CSS;
    const barLeft = left ? lineX - width : lineX;
    if (axis.offScale !== 'below') ctx.fillRect(barLeft, plot.top, width, height);
    if (axis.offScale !== 'above') ctx.fillRect(barLeft, bottom - height, width, height);
  }
  if (axis.range !== null) {
    const f = axis.display.factor;
    const y = linearAffine(axis.range.min * f, axis.range.max * f, bottom, plot.top);
    ctx.textBaseline = 'middle';
    ctx.fillStyle = scene.colors.axisText;
    const pys = axis.ticks.values.map((v) => crisp(applyAffine(y, v)));
    // Labels that would touch the unit title, another label or the canvas edge are left out (the tick stays).
    const labelled = new Set(drawableTickLabels(pys, yLabelTop(scene.layout), scene.layout.height));
    axis.ticks.values.forEach((v, i) => {
      const py = pys[i] ?? 0;
      ctx.moveTo(lineX, py);
      ctx.lineTo(left ? lineX - TICK_CSS : lineX + TICK_CSS, py);
      if (labelled.has(i)) ctx.fillText(formatAxisTick(v, axis.ticks.decimals, axis.display.tickStyle), textX, py);
    });
  }
  ctx.stroke();
}

function drawGapLabels(ctx: Canvas2DLike, scene: OverlayScene, x: Affine): void {
  const { plot } = scene.layout;
  const labelWidth = ctx.measureText(GAP_LABEL).width + 2 * TEXT_PAD_CSS;
  ctx.fillStyle = scene.colors.gapText;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'bottom';
  for (const gap of scene.gaps) {
    const span = visibleSpan(scene, x, gap);
    if (span === null || span[1] - span[0] < labelWidth) continue;
    ctx.fillText(GAP_LABEL, (span[0] + span[1]) / 2, plot.top + plot.height - TEXT_PAD_CSS);
  }
}

/** A tick in the series' colour at the bottom of the plot for each value break (at least 2 px wide). */
function drawValueBreaks(ctx: Canvas2DLike, scene: OverlayScene, x: Affine): void {
  const { plot } = scene.layout;
  const top = plot.top + plot.height - BREAK_TICK_HEIGHT_CSS;
  for (const series of scene.valueBreaks) {
    ctx.fillStyle = series.color;
    for (const range of series.ranges) {
      const span = breakSpan(scene, x, range);
      if (span !== null) ctx.fillRect(span[0], top, span[1] - span[0], BREAK_TICK_HEIGHT_CSS);
    }
  }
}

function drawMarkers(ctx: Canvas2DLike, scene: OverlayScene): void {
  const { plot } = scene.layout;
  const bottom = plot.top + plot.height;
  ctx.textAlign = 'left';
  ctx.textBaseline = 'top';
  for (const m of scene.markers) {
    const color = scene.colors.markers[m.marker.kind];
    ctx.strokeStyle = color;
    ctx.setLineDash(m.marker.kind === 'event' ? SHORT_DASH : NO_DASH);
    ctx.beginPath();
    vLine(ctx, crisp(m.px), plot.top, bottom);
    ctx.stroke();
    if (m.labelVisible) {
      ctx.fillStyle = color;
      ctx.fillText(m.marker.label, m.labelX, plot.top + m.row * MARKER_ROW_HEIGHT_CSS + 1);
    }
  }
  ctx.setLineDash(NO_DASH);
}

function drawCurrent(ctx: Canvas2DLike, scene: OverlayScene, x: Affine): void {
  const current = scene.current;
  if (current === null) return;
  const { plot } = scene.layout;
  const right = plot.left + plot.width;
  const px = applyAffine(x, current.x);
  if (!Number.isFinite(px) || px < plot.left || px > right) return;
  const color = current.stale ? scene.colors.currentCarStale : scene.colors.currentCar;
  const label = current.stale ? CURRENT_STALE_LABEL : current.degraded ? CURRENT_DEGRADED_LABEL : CURRENT_LABEL;
  ctx.strokeStyle = color;
  ctx.fillStyle = color;
  ctx.lineWidth = CURRENT_LINE_WIDTH_CSS;
  ctx.setLineDash(current.stale ? SHORT_DASH : NO_DASH);
  ctx.beginPath();
  vLine(ctx, Math.round(px), plot.top, plot.top + plot.height);
  ctx.stroke();
  ctx.setLineDash(NO_DASH);
  ctx.lineWidth = 1;
  const width = ctx.measureText(label).width;
  const labelX = px + MARKER_LABEL_GAP_CSS + width <= right ? px + MARKER_LABEL_GAP_CSS : px - MARKER_LABEL_GAP_CSS - width;
  ctx.textAlign = 'left';
  ctx.textBaseline = 'top';
  // Below the marker label rows, so it never covers a lap or sector label.
  ctx.fillText(label, labelX, plot.top + MARKER_ROWS * MARKER_ROW_HEIGHT_CSS + 1);
}

/** Names each STALE series at the plot's right edge, below the current-car label row, in the stale colour. */
function drawStaleSeries(ctx: Canvas2DLike, scene: OverlayScene): void {
  if (scene.staleSeries.length === 0) return;
  const { plot } = scene.layout;
  ctx.textAlign = 'right';
  ctx.textBaseline = 'top';
  ctx.fillStyle = scene.colors.currentCarStale;
  scene.staleSeries.forEach((s, i) => {
    const row = MARKER_ROWS + 1 + i;
    ctx.fillText(`${STALE_SERIES_PREFIX}${s.label}`, plot.left + plot.width - TEXT_PAD_CSS, plot.top + row * MARKER_ROW_HEIGHT_CSS + 1);
  });
}

function drawCursor(ctx: Canvas2DLike, scene: OverlayScene, x: Affine, cursor: CursorMark): void {
  const { plot } = scene.layout;
  const px = applyAffine(x, cursor.x);
  if (!Number.isFinite(px) || px < plot.left || px > plot.left + plot.width) return;
  const bottom = plot.top + plot.height;
  ctx.strokeStyle = scene.colors.cursor;
  ctx.beginPath();
  vLine(ctx, crisp(px), plot.top, bottom);
  ctx.stroke();
  // Only the pane with the x axis row has room for the label (the stacked graph's header names the cursor too).
  if (!scene.layout.xAxis) return;
  // Label box on the x axis, kept inside the drawing area.
  const width = ctx.measureText(cursor.label).width + 2 * TEXT_PAD_CSS;
  const boxLeft = Math.max(0, Math.min(px - width / 2, scene.layout.width - width));
  ctx.fillStyle = scene.colors.cursor;
  ctx.fillRect(boxLeft, bottom + 1, width, TEXT_HEIGHT_CSS + TEXT_PAD_CSS);
  ctx.fillStyle = scene.colors.cursorText;
  ctx.textAlign = 'left';
  ctx.textBaseline = 'top';
  ctx.fillText(cursor.label, boxLeft + TEXT_PAD_CSS, bottom + 2);
}

function drawZoomBox(ctx: Canvas2DLike, scene: OverlayScene, box: { readonly x0: number; readonly x1: number }): void {
  const { plot } = scene.layout;
  const x0 = Math.max(plot.left, Math.min(box.x0, box.x1));
  const x1 = Math.min(plot.left + plot.width, Math.max(box.x0, box.x1));
  if (!(x1 > x0)) return;
  ctx.fillStyle = scene.colors.zoomFill;
  ctx.fillRect(x0, plot.top, x1 - x0, plot.height);
  ctx.strokeStyle = scene.colors.zoomStroke;
  ctx.beginPath();
  vLine(ctx, crisp(x0), plot.top, plot.top + plot.height);
  vLine(ctx, crisp(x1), plot.top, plot.top + plot.height);
  ctx.stroke();
}
