/**
 * The graph chrome the engine draws around the series layer, in CSS pixels (callers set the device-pixel
 * transform). Under the series: grid and gap bands (spec §219 gap indicator). Over the series: axes with unit
 * titles (§62 units), sector/lap/event markers, the current-car marker (§82: frozen and marked STALE when the
 * feed is stale), the synchronized cursor with its x label (§81) and the zoom box.
 */
import { formatAxisTick } from '../lib/units';
import type { Canvas2DLike } from './canvasContext';
import { Y_AXIS_WIDTH_CSS, axisColumnLeft, type GraphLayout, type YAxisModel } from './layout';
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
  readonly markers: readonly PlacedMarker[];
  /** Current car position (live sources only). */
  readonly current: CurrentPosition | null;
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
/** Label of a significant data gap (spec §219). */
export const GAP_LABEL = 'GAP';
/** Suffix of the title of an axis with a fixed scale. */
export const FIXED_SCALE_SUFFIX = ' · FIXED';

/** Tick mark length, CSS px. */
const TICK_CSS = 4;
/** Padding inside text boxes, CSS px. */
const TEXT_PAD_CSS = 3;
/** Text height used for boxes, CSS px. */
const TEXT_HEIGHT_CSS = 12;
/** Least space between an x tick label and the axis title, CSS px. */
const TITLE_CLEARANCE_CSS = 6;
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
      const span = visibleSpan(scene, x, gap);
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

  drawXAxis(ctx, scene, x);
  for (const axis of scene.yAxes) drawYAxis(ctx, scene, axis);

  ctx.save();
  ctx.beginPath();
  ctx.rect(plot.left, plot.top, plot.width, plot.height);
  ctx.clip();
  if (x !== null) {
    drawGapLabels(ctx, scene, x);
    drawMarkers(ctx, scene);
    drawCurrent(ctx, scene, x);
  }
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
  const labelTop = bottom + TICK_CSS + 1;
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
  ctx.fillText(axis.setting.mode === 'fixed' ? `${axis.title}${FIXED_SCALE_SUFFIX}` : axis.title, textX, plot.top - 2);
  if (axis.range !== null) {
    const f = axis.display.factor;
    const y = linearAffine(axis.range.min * f, axis.range.max * f, bottom, plot.top);
    ctx.textBaseline = 'middle';
    ctx.fillStyle = scene.colors.axisText;
    for (const v of axis.ticks.values) {
      const py = crisp(applyAffine(y, v));
      ctx.moveTo(lineX, py);
      ctx.lineTo(left ? lineX - TICK_CSS : lineX + TICK_CSS, py);
      ctx.fillText(formatAxisTick(v, axis.ticks.decimals, axis.display.tickStyle), textX, py);
    }
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

function drawCursor(ctx: Canvas2DLike, scene: OverlayScene, x: Affine, cursor: CursorMark): void {
  const { plot } = scene.layout;
  const px = applyAffine(x, cursor.x);
  if (!Number.isFinite(px) || px < plot.left || px > plot.left + plot.width) return;
  const bottom = plot.top + plot.height;
  ctx.strokeStyle = scene.colors.cursor;
  ctx.beginPath();
  vLine(ctx, crisp(px), plot.top, bottom);
  ctx.stroke();
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
