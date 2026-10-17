/**
 * Visual parity check of the series renderers (bench page with `?parity=1`). SYNTHETIC TEST DATA only.
 *
 * Draws the same fixed frames with every renderer into real browser canvases and compares, per series and per
 * device-pixel column, the vertical extent of the lit pixels (alpha above a threshold). Canvas2D (Skia hairlines)
 * is the reference; antialiasing differs between renderers, so extents may differ by `TOLERANCE_PX`. The cases
 * cover what the decimation must not lie about: single-row spikes in dense data (min/max kept), NaN rows and time
 * gaps (line broken, nothing drawn inside the gap), Discrete steps, a lone sample (dot), sparse diagonal lines,
 * a dashed reference and lines leaving the plot (clipped). Results go to `window.__parityResults`.
 */
import { Canvas2DRenderer } from '../src/graph/canvas2dRenderer';
import type { RenderFrame, SeriesRenderer } from '../src/graph/renderer';
import type { GraphWindow, RendererKind } from '../src/graph/types';
import { UplotRenderer } from '../src/graph/uplotRenderer';
import { WebGLRenderer } from '../src/graph/webglRenderer';

const WIDTH_CSS = 900;
const HEIGHT_CSS = 240;
const PLOT = { left: 40, top: 10, width: 820, height: 210 } as const;
/** Lit pixel: alpha above this (0…255). */
const ALPHA_LIT = 60;
/** Allowed difference of a column's lit extent between renderers (device px). */
const TOLERANCE_PX = 1;
const COLOR = '#2f9bff';
/** `?keep=1` leaves every rendering on the page (for screenshots). */
const KEEP = new URLSearchParams(window.location.search).get('keep') === '1';

interface Case {
  readonly name: string;
  readonly xs: Float64Array;
  readonly ts: Float64Array;
  readonly ys: Float64Array;
  readonly step?: boolean;
  readonly dashed?: boolean;
  /** Column ranges (device px, inclusive) that must stay empty (gaps). */
  readonly emptyX?: readonly (readonly [number, number])[];
}

function windowOf(c: Case): GraphWindow {
  return { xs: c.xs, ts: c.ts, count: c.xs.length, monotonicX: true, column: () => c.ys };
}

function dense(): Case {
  const n = 6000;
  const xs = new Float64Array(n);
  const ys = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    xs[i] = i / 100;
    ys[i] = Math.sin(i / 300) * 0.6 + Math.sin(i * 1.7) * 0.15;
  }
  // Single-row spikes must survive decimation (min/max per column).
  ys[1234] = 0.98;
  ys[4321] = -0.97;
  return { name: 'dense 6000 rows + 1-row spikes', xs, ts: xs, ys };
}

function gaps(): Case {
  const n = 3000;
  const xs = new Float64Array(n);
  const ts = new Float64Array(n);
  const ys = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    xs[i] = i / 50;
    ts[i] = i / 50;
    ys[i] = Math.cos(i / 120) * 0.7;
  }
  for (let i = 1000; i < 1200; i++) ys[i] = Number.NaN; // invalid rows
  // A 4 s outage: no rows between x = 40 and x = 44 (spec §219: never bridged).
  for (let i = 2000; i < n; i++) {
    xs[i] = (i + 200) / 50;
    ts[i] = xs[i] ?? 0;
  }
  const px = (x: number): number => PLOT.left + ((x - 0) / (63 - 0)) * PLOT.width;
  return {
    name: 'NaN rows + 4 s time gap',
    xs,
    ts,
    ys,
    emptyX: [
      [Math.ceil(px(1000 / 50)) + 2, Math.floor(px(1199 / 50)) - 2],
      [Math.ceil(px(1999 / 50)) + 2, Math.floor(px(2200 / 50)) - 2],
    ],
  };
}

function steps(): Case {
  const n = 6000;
  const xs = new Float64Array(n);
  const ys = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    xs[i] = i / 100;
    ys[i] = -0.8 + 0.3 * (Math.floor(i / 250) % 6);
  }
  return { name: 'Discrete steps', xs, ts: xs, ys, step: true };
}

function sparse(dashed: boolean): Case {
  const n = 40;
  const xs = new Float64Array(n);
  const ys = new Float64Array(n);
  const ts = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    xs[i] = (i * 63) / (n - 1);
    ts[i] = i * 0.1; // close in time: the lines are not broken by the gap rule
    ys[i] = i % 2 === 0 ? 0.9 : -0.9; // zig-zag of long diagonals; the extremes leave nothing to guess
  }
  // A lone sample between NaN neighbours (drawn as a dot).
  ys[20] = Number.NaN;
  ys[22] = Number.NaN;
  ys[21] = 0;
  return { name: dashed ? 'sparse dashed reference' : 'sparse diagonals + lone dot', xs, ts, ys, dashed };
}

function clipped(): Case {
  const n = 600;
  const xs = new Float64Array(n);
  const ys = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    xs[i] = -5 + (i * 75) / n;
    ys[i] = Math.sin(i / 40) * 1.6; // leaves the y range at both ends
  }
  return { name: 'clipped (outside plot)', xs, ts: xs, ys };
}

function frameOf(c: Case): RenderFrame {
  return {
    plot: PLOT,
    xRange: { min: 0, max: 63 },
    maxGapT: 0.25,
    series: [
      { key: 'p', window: windowOf(c), ys: c.ys, yRange: { min: -1, max: 1 }, color: COLOR, dashed: c.dashed === true, step: c.step === true },
    ],
  };
}

/** Lit extent per column: [top, bottom] or null. */
type Extents = ([number, number] | null)[];

function extents(data: Uint8ClampedArray, w: number, h: number): Extents {
  const out: Extents = [];
  for (let x = 0; x < w; x++) {
    let top = -1;
    let bottom = -1;
    for (let y = 0; y < h; y++) {
      if ((data[(y * w + x) * 4 + 3] ?? 0) > ALPHA_LIT) {
        if (top < 0) top = y;
        bottom = y;
      }
    }
    out.push(top < 0 ? null : [top, bottom]);
  }
  return out;
}

function makeRenderer(kind: RendererKind): SeriesRenderer {
  if (kind === 'webgl') return new WebGLRenderer();
  if (kind === 'uplot') return new UplotRenderer();
  return new Canvas2DRenderer();
}

/** Renders `c` with `kind` and returns the pixels (device px). */
async function pixels(kind: RendererKind, c: Case, host: HTMLElement): Promise<{ data: Uint8ClampedArray; w: number; h: number }> {
  const el = document.createElement('div');
  // Inside `.telemetry-graph` so the graph stylesheet (uPlot's layer rules) applies, as in the app.
  el.className = 'telemetry-graph';
  el.style.cssText = `position:relative;display:block;width:${WIDTH_CSS}px;height:${HEIGHT_CSS}px;padding:0;border:0;`;
  host.appendChild(el);
  const r = makeRenderer(kind);
  r.mount(el);
  const dpr = window.devicePixelRatio;
  r.setSize(WIDTH_CSS, HEIGHT_CSS, dpr);
  if (r instanceof UplotRenderer) await r.ready;
  r.render(frameOf(c));
  const w = Math.round(WIDTH_CSS * dpr);
  const h = Math.round(HEIGHT_CSS * dpr);
  const out = document.createElement('canvas');
  out.width = w;
  out.height = h;
  const ctx = out.getContext('2d', { willReadFrequently: true });
  if (ctx === null) throw new Error('no 2D context');
  // Read back in the same task as the draw (a WebGL drawing buffer is not preserved past it).
  for (const canvas of el.querySelectorAll('canvas')) {
    const rect = canvas.getBoundingClientRect();
    const base = el.getBoundingClientRect();
    ctx.drawImage(canvas, Math.round((rect.left - base.left) * dpr), Math.round((rect.top - base.top) * dpr));
  }
  const data = ctx.getImageData(0, 0, w, h).data;
  if (KEEP) {
    el.title = `${kind}: ${c.name}`;
  } else {
    r.destroy();
    el.remove();
  }
  return { data, w, h };
}

/** Comparison of one renderer against the Canvas2D reference for one case. */
export interface ParityCase {
  readonly renderer: RendererKind;
  readonly name: string;
  /** Columns lit in one rendering with nothing lit within one column in the other. */
  readonly columnMismatch: number;
  /** Columns whose lit extent reaches more than TOLERANCE_PX beyond the other's (see `excess`). */
  readonly extentMismatch: number;
  readonly maxExtentDiffPx: number;
  /** Lit pixels inside a gap that must stay empty (this renderer). */
  readonly litInGaps: number;
  readonly litColumns: number;
}

export interface ParityResults {
  readonly label: 'SYNTHETIC TEST DATA';
  status: 'running' | 'done' | 'error';
  readonly devicePixelRatio: number;
  readonly tolerancePx: number;
  cases: ParityCase[];
  error?: string;
}

declare global {
  interface Window {
    __parityResults?: ParityResults;
  }
}

/**
 * How far (px) the lit extent of column `x` in `a` reaches beyond the lit extents of columns x-1…x+1 in `b`
 * (0 when `b` covers it; +Infinity when `b` has nothing lit nearby). A one-column neighbourhood absorbs the
 * different rasterisation of steep line segments by Skia hairlines and multisampled WebGL lines.
 */
function excess(a: Extents, b: Extents, x: number): number {
  const e = a[x] ?? null;
  if (e === null) return 0;
  let top = Number.POSITIVE_INFINITY;
  let bottom = Number.NEGATIVE_INFINITY;
  for (let k = x - 1; k <= x + 1; k++) {
    const n = b[k] ?? null;
    if (n === null) continue;
    top = Math.min(top, n[0]);
    bottom = Math.max(bottom, n[1]);
  }
  if (top > bottom) return Number.POSITIVE_INFINITY;
  return Math.max(0, top - e[0], e[1] - bottom);
}

function compare(kind: RendererKind, c: Case, ref: Extents, got: Extents, dpr: number): ParityCase {
  let columnMismatch = 0;
  let extentMismatch = 0;
  let maxDiff = 0;
  let litColumns = 0;
  for (let x = 0; x < got.length; x++) {
    if ((got[x] ?? null) !== null) litColumns += 1;
    // Symmetric: nothing drawn by one renderer is missing from the other, and nothing extra is drawn.
    const d = Math.max(excess(ref, got, x), excess(got, ref, x));
    if (d === Number.POSITIVE_INFINITY) {
      columnMismatch += 1;
      continue;
    }
    if (d > maxDiff) maxDiff = d;
    if (d > TOLERANCE_PX) extentMismatch += 1;
  }
  let litInGaps = 0;
  for (const [x0, x1] of c.emptyX ?? []) {
    for (let x = Math.round(x0 * dpr); x <= Math.round(x1 * dpr); x++) if ((got[x] ?? null) !== null) litInGaps += 1;
  }
  return { renderer: kind, name: c.name, columnMismatch, extentMismatch, maxExtentDiffPx: maxDiff, litInGaps, litColumns };
}

/** Runs the parity check and publishes `window.__parityResults`. */
export async function runParity(host: HTMLElement): Promise<ParityResults> {
  const dpr = window.devicePixelRatio;
  const results: ParityResults = { label: 'SYNTHETIC TEST DATA', status: 'running', devicePixelRatio: dpr, tolerancePx: TOLERANCE_PX, cases: [] };
  window.__parityResults = results;
  try {
    const cases = [dense(), gaps(), steps(), sparse(false), sparse(true), clipped()];
    for (const c of cases) {
      const ref = await pixels('canvas2d', c, host);
      const refExt = extents(ref.data, ref.w, ref.h);
      results.cases.push(compare('canvas2d', c, refExt, refExt, dpr));
      for (const kind of ['webgl', 'uplot'] as const) {
        const got = await pixels(kind, c, host);
        results.cases.push(compare(kind, c, refExt, extents(got.data, got.w, got.h), dpr));
      }
    }
    results.status = 'done';
  } catch (error: unknown) {
    results.status = 'error';
    results.error = String(error);
  }
  return results;
}
