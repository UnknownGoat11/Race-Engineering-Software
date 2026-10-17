/**
 * The series renderer interface (D-011): the engine decides WHAT to draw (ranges, scales, colours, which rows) and
 * draws axes, grid, markers and cursor itself; a renderer only strokes the series lines into its own layer. Three
 * implementations are benchmarked against each other (bench/): WebGL line strips (`webglRenderer.ts`, the default:
 * `DEFAULT_RENDERER`), Canvas2D (`canvas2dRenderer.ts`, also WebGL's fallback) and uPlot (`uplotRenderer.ts`); the
 * WebGL and Canvas2D renderers share the per-pixel min/max decimation of `decimate.ts`.
 */
import type { GraphWindow, RendererKind, XRange } from './types';

/** A rectangle in CSS pixels, relative to the graph's drawing area. */
export interface PlotRect {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
}

/** One line to stroke. */
export interface RenderSeries {
  /** Stable key (unique within a frame), so renderers can keep per-series state between frames. */
  readonly key: string;
  /** Rows of the series; series of the same window share its x and time columns. */
  readonly window: GraphWindow;
  /** Values per row, internal units (NaN = no value). */
  readonly ys: Float64Array;
  /** Value range mapped to the plot's bottom (`min`) and top (`max`), internal units. */
  readonly yRange: XRange;
  /** Resolved CSS colour. */
  readonly color: string;
  /** Dashed line (reference overlays). */
  readonly dashed: boolean;
  /** Step drawing (Discrete channels, spec §125). */
  readonly step: boolean;
}

/** Everything a renderer needs for one frame. */
export interface RenderFrame {
  /** The plot area inside the renderer's layer; lines are clipped to it. */
  readonly plot: PlotRect;
  readonly xRange: XRange;
  /** Largest time step (s) a line may bridge (spec §219). */
  readonly maxGapT: number;
  readonly series: readonly RenderSeries[];
}

/**
 * What a renderer can currently do (silent-failure M1, M5): 'ok'; 'degraded' = it draws correctly but with a
 * fallback (e.g. WebGL lost, Canvas2D draws instead); 'failed' = it cannot draw the series at all.
 */
export interface RendererStatus {
  /** The renderer actually drawing ('canvas2d' after a WebGL or uPlot fallback). */
  readonly activeKind: RendererKind;
  readonly state: 'ok' | 'degraded' | 'failed';
  /** Why it is not 'ok' (shader log, context lost, load error …), or null. */
  readonly reason: string | null;
}

/** The status of a renderer that has no failure modes of its own. */
export function okStatus(kind: RendererKind): RendererStatus {
  return { activeKind: kind, state: 'ok', reason: null };
}

/** A series renderer (see the module comment). Every method must be cheap when nothing changed. */
export interface SeriesRenderer {
  readonly kind: RendererKind;
  /** Current status; absent = always 'ok'. */
  status?(): RendererStatus;
  /**
   * Calls `listener` whenever `status()` changes (fallback, failure). The engine then redraws from a fresh read
   * (a renderer never replays data it was given earlier) and shows the state. Returns the unsubscribe function.
   */
  onStatusChange?(listener: () => void): () => void;
  /** Attaches the renderer's layer to `host` (an absolutely positioned element covering the drawing area). */
  mount(host: HTMLElement): void;
  /** Size of the layer in CSS pixels and the device pixel ratio. */
  setSize(widthCss: number, heightCss: number, dpr: number): void;
  /** Draws `frame`, replacing the previous one. */
  render(frame: RenderFrame): void;
  /** Detaches the layer and releases everything the renderer holds. */
  destroy(): void;
}

/**
 * Line width of a series in DEVICE pixels. Exactly 1 keeps Chromium's (WebView2's) Skia on its hairline fast path:
 * in the bench/ harness (4 graphs × 6 series × 6 000 rows, GPU-accelerated Chromium) any width above 1 device px —
 * even 1.01 — cut the frame rate about tenfold (≈6 fps instead of ≈60 fps) for both renderers. On high-DPI screens
 * the trace is therefore thinner than 1 CSS px; it stays crisp.
 */
export const SERIES_LINE_WIDTH_DEVICE_PX = 1;
/** Dash pattern of reference overlays, CSS px (dash, gap). */
export const REFERENCE_DASH_CSS: readonly [number, number] = [6, 4];
