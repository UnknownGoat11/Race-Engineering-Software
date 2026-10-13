/**
 * uPlot series renderer (D-011 default, provisional until the bench/ comparison is run in WebView2).
 *
 * uPlot draws only the series lines: its axes, legend, cursor and selection are disabled and every scale is set
 * explicitly from the engine's frame (the engine draws axes, grid, markers and cursor itself, so both renderers
 * produce the same chrome). Series that share a GraphWindow share an x column, so each distinct window gets one
 * uPlot instance (normally one for the live/recorded data and one per reference overlay).
 *
 * uPlot breaks a line only at `null` (a NaN would be connected across), so `fillAlignedData` converts NaN to
 * null and inserts an all-null row wherever consecutive rows are more than `maxGapT` apart in time or out of
 * time order (spec §219: never draw across a gap). uPlot needs ascending x: a row without x becomes a null row
 * at the previous x. When finite x does step back (the car reversed on a lap axis), uPlot's binary-searched x
 * window would drop parts of the line, so such frames are traced with the Canvas2D renderer's `tracePolyline`
 * into a Path2D that uPlot strokes (same colour, width and dash).
 *
 * The module is loaded with a dynamic `import('uplot')`: uPlot reads `matchMedia` at module load, and a lazy
 * chunk keeps it out of code paths that never draw a graph. Frames rendered before it loaded are drawn when it
 * arrives. Drawing happens synchronously inside `uPlot.batch` (no microtask), so frame timing covers it.
 */
import type uPlot from 'uplot';
import { tracePolyline } from './decimate';
import { REFERENCE_DASH_CSS, SERIES_LINE_WIDTH_DEVICE_PX, type RenderFrame, type RenderSeries, type SeriesRenderer } from './renderer';
import { visibleRows } from './sample';
import { linearAffine } from './scale';
import type { GraphWindow, XRange } from './types';

/** Reusable uPlot columns of one x group. Arrays only grow; `length` is set to the row count each frame. */
export interface AlignedBuffers {
  readonly xs: number[];
  readonly ys: (number | null)[][];
  /** False when finite x stepped back in the last fill (uPlot's x windowing would then be wrong). */
  sorted: boolean;
}

/** Empty buffers for `seriesCount` series. */
export function alignedBuffers(seriesCount: number): AlignedBuffers {
  return { xs: [], ys: Array.from({ length: seriesCount }, () => []), sorted: true };
}

/**
 * Fills `out` with rows `[start, end)` of `window` and the value columns `columns` (see the module comment) and
 * returns the number of rows written (each array's length is set to it).
 */
export function fillAlignedData(
  window: GraphWindow,
  columns: readonly Float64Array[],
  start: number,
  end: number,
  maxGapT: number,
  out: AlignedBuffers,
): number {
  const { xs, ts } = window;
  const outXs = out.xs;
  const outYs = out.ys;
  let n = 0;
  let prevT = Number.NaN;
  let lastX = Number.NaN;
  let sorted = true;
  const nullRow = (x: number): void => {
    outXs[n] = x;
    for (const col of outYs) col[n] = null;
    n += 1;
  };
  for (let i = start; i < end; i++) {
    const x = xs[i] ?? Number.NaN;
    const t = ts[i] ?? Number.NaN;
    if (!Number.isFinite(x)) {
      if (Number.isFinite(lastX)) nullRow(lastX);
      prevT = Number.NaN;
      continue;
    }
    if (Number.isFinite(prevT) && Number.isFinite(t) && (t - prevT > maxGapT || t < prevT)) {
      // An all-null row between the two rows ends every line at the gap.
      nullRow(Number.isFinite(lastX) ? (lastX + x) / 2 : x);
    }
    if (x < lastX) sorted = false;
    outXs[n] = x;
    const timed = Number.isFinite(t);
    for (let k = 0; k < outYs.length; k++) {
      const col = outYs[k];
      if (col === undefined) continue;
      const v = columns[k]?.[i] ?? Number.NaN;
      col[n] = timed && Number.isFinite(v) ? v : null;
    }
    n += 1;
    prevT = t;
    lastX = x;
  }
  outXs.length = n;
  for (const col of outYs) col.length = n;
  out.sorted = sorted;
  return n;
}

type UPlotConstructor = typeof uPlot;

/** What a group draws; also read by the traced path builder while uPlot redraws. */
interface GroupFrame {
  window: GraphWindow;
  series: readonly RenderSeries[];
  start: number;
  end: number;
  xRange: XRange;
  maxGapT: number;
}

/** One uPlot instance drawing the series of one window. */
interface Group {
  /** Element the uPlot root lives in. */
  readonly el: HTMLDivElement;
  readonly signature: string;
  readonly plot: uPlot;
  readonly buffers: AlignedBuffers;
  readonly frame: GroupFrame;
}

const X_SCALE = 'x';
const yScaleKey = (i: number): string => `y${i}`;
/** Scale ranges are exactly what the engine set (uPlot pads y ranges by default). */
const exactRange: uPlot.Scale.Range = (_u, min, max) => [min, max];

/** Identity of a uPlot instance: its series' looks and the pixel ratio its line width was derived from. */
function signatureOf(series: readonly RenderSeries[], dpr: number): string {
  return `${String(dpr)}#${series.map((s) => `${s.key}|${s.color}|${s.dashed ? 'd' : ''}|${s.step ? 's' : ''}`).join(';')}`;
}

/** uPlot implementation of `SeriesRenderer` (see the module comment). */
export class UplotRenderer implements SeriesRenderer {
  readonly kind = 'uplot' as const;
  readonly #layer: HTMLDivElement;
  #UPlot: UPlotConstructor | null = null;
  #groups: Group[] = [];
  #pending: RenderFrame | null = null;
  #destroyed = false;
  #dpr = 1;
  /** Settles once uPlot has loaded (or failed to load, which is logged). */
  readonly ready: Promise<void>;

  /** `load` resolves the uPlot constructor (injectable for tests); default: dynamic import of 'uplot'. */
  constructor(load: () => Promise<UPlotConstructor> = async () => (await import('uplot')).default) {
    this.#layer = document.createElement('div');
    this.#layer.className = 'telemetry-graph__layer telemetry-graph__uplot';
    this.ready = load().then(
      (ctor) => {
        if (this.#destroyed) return;
        this.#UPlot = ctor;
        const frame = this.#pending;
        this.#pending = null;
        if (frame !== null) this.render(frame);
      },
      (error: unknown) => {
        // Without uPlot nothing can be drawn; the failure must be visible to developers, never silent.
        console.error('uPlot failed to load', error);
      },
    );
  }

  mount(host: HTMLElement): void {
    host.appendChild(this.#layer);
  }

  setSize(widthCss: number, heightCss: number, dpr: number): void {
    // uPlot sizes its canvases for the device pixel ratio itself; the ratio only sets the line width here.
    this.#dpr = dpr > 0 ? dpr : 1;
    this.#layer.style.width = `${widthCss}px`;
    this.#layer.style.height = `${heightCss}px`;
  }

  render(frame: RenderFrame): void {
    const ctor = this.#UPlot;
    if (ctor === null) {
      this.#pending = frame;
      return;
    }
    const grouped = new Map<GraphWindow, RenderSeries[]>();
    for (const s of frame.series) {
      if (!(s.yRange.max > s.yRange.min)) continue;
      const list = grouped.get(s.window);
      if (list === undefined) grouped.set(s.window, [s]);
      else list.push(s);
    }
    const next: Group[] = [];
    const reusable = [...this.#groups];
    for (const [window, series] of grouped) {
      const signature = signatureOf(series, this.#dpr);
      const index = reusable.findIndex((g) => g.signature === signature);
      const found = index >= 0 ? reusable.splice(index, 1)[0] : undefined;
      const group = found ?? this.#createGroup(ctor, window, series, signature, frame);
      group.frame.window = window;
      group.frame.series = series;
      next.push(group);
    }
    for (const g of reusable) this.#destroyGroup(g);
    this.#groups = next;
    for (const g of next) this.#draw(g, frame);
  }

  destroy(): void {
    this.#destroyed = true;
    for (const g of this.#groups) this.#destroyGroup(g);
    this.#groups = [];
    this.#pending = null;
    this.#layer.remove();
  }

  #createGroup(
    ctor: UPlotConstructor,
    window: GraphWindow,
    series: RenderSeries[],
    signature: string,
    frame: RenderFrame,
  ): Group {
    const el = document.createElement('div');
    el.className = 'telemetry-graph__uplot-group';
    this.#layer.appendChild(el);
    const scales: uPlot.Scales = { [X_SCALE]: { time: false, auto: false, range: exactRange } };
    series.forEach((_s, i) => {
      scales[yScaleKey(i)] = { auto: false, range: exactRange };
    });
    const linear = ctor.paths.linear?.();
    const stepped = ctor.paths.stepped?.({ align: 1 });
    const buffers = alignedBuffers(series.length);
    const state: GroupFrame = { window, series, start: 0, end: 0, xRange: frame.xRange, maxGapT: frame.maxGapT };
    const pathsFor = (step: boolean): uPlot.Series.PathBuilder => {
      const base = step ? stepped : linear;
      return (u, seriesIdx, idx0, idx1) => {
        // uPlot series 0 is x, so series k of the group is uPlot series k + 1.
        if (!buffers.sorted) return tracedPaths(u, state, seriesIdx - 1, step);
        return base === undefined ? null : base(u, seriesIdx, idx0, idx1);
      };
    };
    const options: uPlot.Options = {
      width: Math.max(1, frame.plot.width),
      height: Math.max(1, frame.plot.height),
      padding: [0, 0, 0, 0],
      legend: { show: false },
      cursor: { show: false },
      select: { show: false, left: 0, top: 0, width: 0, height: 0 },
      axes: [{ show: false }, { show: false }],
      scales,
      series: [
        {},
        ...series.map(
          (s, i): uPlot.Series => ({
            scale: yScaleKey(i),
            stroke: s.color,
            // uPlot multiplies widths by the pixel ratio, so this strokes exactly SERIES_LINE_WIDTH_DEVICE_PX.
            width: SERIES_LINE_WIDTH_DEVICE_PX / this.#dpr,
            spanGaps: false,
            points: { show: false },
            paths: pathsFor(s.step),
            ...(s.dashed ? { dash: [...REFERENCE_DASH_CSS] } : {}),
          }),
        ),
      ],
    };
    const plot = new ctor(options, [buffers.xs, ...buffers.ys], el);
    return { el, signature, plot, buffers, frame: state };
  }

  #draw(group: Group, frame: RenderFrame): void {
    const { plot, buffers } = group;
    const { window, series } = group.frame;
    const { start, end } = visibleRows(window, frame.xRange);
    group.frame.start = start;
    group.frame.end = end;
    group.frame.xRange = frame.xRange;
    group.frame.maxGapT = frame.maxGapT;
    fillAlignedData(
      window,
      series.map((s) => s.ys),
      start,
      end,
      frame.maxGapT,
      buffers,
    );
    // The instance covers exactly the plot area (no axes, no padding).
    group.el.style.left = `${frame.plot.left}px`;
    group.el.style.top = `${frame.plot.top}px`;
    const width = Math.max(1, Math.round(frame.plot.width));
    const height = Math.max(1, Math.round(frame.plot.height));
    plot.batch(() => {
      if (plot.width !== width || plot.height !== height) plot.setSize({ width, height });
      plot.setData([buffers.xs, ...buffers.ys], false);
      plot.setScale(X_SCALE, { min: frame.xRange.min, max: frame.xRange.max });
      series.forEach((s, i) => {
        plot.setScale(yScaleKey(i), { min: s.yRange.min, max: s.yRange.max });
      });
    });
  }

  #destroyGroup(group: Group): void {
    group.plot.destroy();
    group.el.remove();
  }
}

/**
 * Paths of series `index` of a group whose x is not monotonic: traced from the group's rows by `tracePolyline`
 * (the Canvas2D renderer's per-pixel min/max, gap and step rules) in uPlot's canvas pixels.
 */
function tracedPaths(u: uPlot, frame: GroupFrame, index: number, step: boolean): uPlot.Series.Paths | null {
  const s = frame.series[index];
  if (s === undefined) return null;
  const { left, top, width, height } = u.bbox;
  const path = new Path2D();
  tracePolyline(
    {
      xs: frame.window.xs,
      ts: frame.window.ts,
      ys: s.ys,
      start: frame.start,
      end: frame.end,
      x: linearAffine(frame.xRange.min, frame.xRange.max, left, left + width),
      y: linearAffine(s.yRange.min, s.yRange.max, top + height, top),
      maxGapT: frame.maxGapT,
      step,
    },
    path,
  );
  return { stroke: path, fill: null, clip: null, flags: 0 };
}
