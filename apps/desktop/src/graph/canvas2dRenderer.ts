/**
 * Canvas2D series renderer (D-011; WebGL's fallback, see DEFAULT_RENDERER): one canvas, one path per series,
 * decimated per device-pixel column by `tracePolyline` (spec §277), lines broken at NaN and time gaps (§219),
 * Discrete channels drawn as steps (§125), no smoothing (§318).
 */
import { browserContext, sizeCanvas, type Canvas2DLike, type ContextFactory } from './canvasContext';
import { tracePolyline } from './decimate';
import { REFERENCE_DASH_CSS, SERIES_LINE_WIDTH_DEVICE_PX, type RenderFrame, type SeriesRenderer } from './renderer';
import { visibleRows } from './sample';
import { linearAffine } from './scale';

/** Options of the Canvas2D renderer. */
export interface Canvas2DRendererOptions {
  /** 2D context factory (tests inject a recording context). */
  readonly context?: ContextFactory;
}

const NO_DASH: number[] = [];

/** Strokes `frame` into `ctx`, whose backing store is `dpr` device pixels per CSS pixel. Returns path commands. */
export function drawSeriesFrame(ctx: Canvas2DLike, frame: RenderFrame, dpr: number): number {
  const { plot, xRange } = frame;
  const left = plot.left * dpr;
  const top = plot.top * dpr;
  const width = plot.width * dpr;
  const height = plot.height * dpr;
  if (!(width > 0) || !(height > 0) || !(xRange.max > xRange.min)) return 0;
  const x = linearAffine(xRange.min, xRange.max, left, left + width);
  const dash = REFERENCE_DASH_CSS.map((v) => v * dpr);
  let commands = 0;
  ctx.save();
  ctx.beginPath();
  ctx.rect(left, top, width, height);
  ctx.clip();
  ctx.lineWidth = SERIES_LINE_WIDTH_DEVICE_PX;
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';
  for (const s of frame.series) {
    if (!(s.yRange.max > s.yRange.min)) continue;
    const { start, end } = visibleRows(s.window, xRange);
    if (end <= start) continue;
    ctx.strokeStyle = s.color;
    ctx.setLineDash(s.dashed ? dash : NO_DASH);
    ctx.beginPath();
    commands += tracePolyline(
      {
        xs: s.window.xs,
        ts: s.window.ts,
        ys: s.ys,
        start,
        end,
        x,
        // Larger values are higher on screen: min maps to the plot's bottom edge.
        y: linearAffine(s.yRange.min, s.yRange.max, top + height, top),
        maxGapT: frame.maxGapT,
        step: s.step,
      },
      ctx,
    );
    ctx.stroke();
  }
  ctx.restore();
  return commands;
}

/** Canvas2D implementation of `SeriesRenderer`. */
export class Canvas2DRenderer implements SeriesRenderer {
  readonly kind = 'canvas2d' as const;
  readonly #canvas: HTMLCanvasElement;
  readonly #context: ContextFactory;
  #ctx: Canvas2DLike | null = null;
  #dpr = 1;

  constructor(options: Canvas2DRendererOptions = {}) {
    this.#canvas = document.createElement('canvas');
    this.#canvas.className = 'telemetry-graph__layer';
    this.#context = options.context ?? browserContext;
  }

  mount(host: HTMLElement): void {
    host.appendChild(this.#canvas);
    this.#ctx = this.#context(this.#canvas);
  }

  setSize(widthCss: number, heightCss: number, dpr: number): void {
    this.#dpr = dpr;
    sizeCanvas(this.#canvas, widthCss, heightCss, dpr);
  }

  render(frame: RenderFrame): void {
    const ctx = this.#ctx;
    if (ctx === null) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, this.#canvas.width, this.#canvas.height);
    drawSeriesFrame(ctx, frame, this.#dpr);
  }

  destroy(): void {
    this.#canvas.remove();
    this.#ctx = null;
  }
}
