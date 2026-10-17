/**
 * GraphEngine: one telemetry graph outside React (spec §61–63, §81–82, §121–123, §219, §277).
 *
 * The engine owns three stacked layers in its container: an under-canvas (grid, gap bands), the series layer of
 * a `SeriesRenderer` (WebGL by default with a Canvas2D fallback, D-011 / DEFAULT_RENDERER) and an over-canvas (axes, markers, current-car marker, cursor,
 * zoom box) that also takes the pointer, wheel and keyboard input. It repaints only in a scheduled animation
 * frame and only when something changed: data (source notifications), the view (zoom / pan / LIVE, also from the
 * sync group), the cursor, the configuration or the size / device pixel ratio. A cursor-only change redraws
 * the over-canvas and the readouts, not the series.
 *
 * Every frame reads just the visible window from the data source (live: a copy of the ring rows in range, see
 * liveSource.ts). Readouts at the cursor (or, for live data without a cursor, at the current car position) are
 * published as an immutable snapshot for `useSyncExternalStore`; a new snapshot is made only when something it
 * shows changed. Values are never invented: a channel without a value at the cursor reads NO DATA, a channel
 * the source does not carry reads UNAVAILABLE, and without a cursor a stale live feed reads STALE (no value) and a
 * degraded one shows its newest value marked DEGRADED.
 */
import { Availability, Interpolation, Quality, Unit, type ChannelDef } from '../generated/channels';
import { channelAxisDisplay, unitAxisDisplay, type AxisDisplay } from '../lib/units';
import { SPEC_DEGRADED_AFTER_MS } from '../telemetry/freshness';
import { channelDef } from '../telemetry/registry';
import type { ChannelSample, SampleState, ScheduleFrame } from '../telemetry/store';
import { browserContext, sizeCanvas, type Canvas2DLike, type ContextFactory } from './canvasContext';
import { Canvas2DRenderer } from './canvas2dRenderer';
import { findTimeGaps, findValueBreaks } from './decimate';
import { sharedFrameLoop } from './frameLoop';
import { DurationRecorder, summarize, type DurationSummary } from './frameStats';
import { dragPanAction, keyAction, wheelAction, zoomBoxAction } from './interaction';
import {
  X_TICK_SPACING_CSS,
  axisKey,
  buildYAxes,
  computeLayout,
  tickTarget,
  type AxisMember,
  type GraphLayout,
  type YAxisModel,
} from './layout';
import { layoutMarkers, type PlacedMarker } from './markers';
import {
  MARKER_LABEL_GAP_CSS,
  MARKER_ROWS,
  drawOverlay,
  drawUnderlay,
  type GraphColors,
  type OverlayScene,
  type SeriesBreaks,
  type StaleSeriesTag,
} from './overlay';
import { okStatus, type RendererStatus, type RenderSeries, type SeriesRenderer } from './renderer';
import { bracket, nearestRowX, rowTimeAt, sampleAt, visibleRows } from './sample';
import { applyAffine, dataExtent, invertAffine, linearAffine } from './scale';
import type { GraphSyncGroup, SyncEvent } from './syncBus';
import {
  DEFAULT_RENDERER,
  type CurrentPosition,
  type DataOrigin,
  type GraphDataSource,
  type GraphMarker,
  type GraphSeries,
  type GraphWindow,
  type ReferenceSeries,
  type RendererKind,
  type XAxisMode,
  type XRange,
  type YScaleSetting,
} from './types';
import { UplotRenderer } from './uplotRenderer';
import { WebGLRenderer } from './webglRenderer';
import { liveView, reduceView, resolveRange, type ViewAction, type ViewBounds, type ViewState } from './view';
import { X_AXIS_SPECS, formatX, xAxisTitle, xTicks, type LabelledTicks } from './xAxis';

/** What a graph shows; every field can be changed with `update`. */
export interface GraphConfig {
  readonly source: GraphDataSource;
  readonly xMode: XAxisMode;
  readonly series: readonly GraphSeries[];
  /** Dashed overlays from other sources, e.g. a reference lap. */
  readonly references: readonly ReferenceSeries[];
  readonly markers: readonly GraphMarker[];
  /** Scale per y axis key (`layout.axisKey`); axes not listed autoscale. */
  readonly yScales: Readonly<Record<string, YScaleSetting>>;
  /** Largest time step (s) a line may bridge (spec §219). */
  readonly maxGapS: number;
  /** Domain of a lap axis (e.g. 0…track length on 'lapDistance'); default: the mode's domain or the data. */
  readonly xDomain: XRange | null;
  /** Graphs sharing cursor and x view (spec §81); null for a standalone graph. */
  readonly syncGroup: GraphSyncGroup | null;
}

/** Options of a new engine. */
export interface GraphEngineOptions extends Partial<GraphConfig> {
  readonly source: GraphDataSource;
  readonly xMode: XAxisMode;
  readonly series: readonly GraphSeries[];
  /** Series renderer: a kind, or a factory (called on every attach). Default DEFAULT_RENDERER ('webgl', D-011; see types.ts). */
  readonly renderer?: RendererKind | (() => SeriesRenderer);
  /** Initial visible span of a scrolling axis in x units; default 10 s live, the whole recording otherwise. */
  readonly initialSpan?: number;
  /** Frame scheduler (default: the shared requestAnimationFrame loop). */
  readonly scheduleFrame?: ScheduleFrame;
  /** 2D context factory for the chrome canvases (tests inject a recording context). */
  readonly context?: ContextFactory;
  /** Monotonic clock in ms for frame timing (default `performance.now`). */
  readonly now?: () => number;
  /** Id of this graph in its sync group (default: a generated id). */
  readonly id?: string;
  /** Validates a CSS colour token (default: `CSS.supports('color', …)`; tests inject one). */
  readonly isColor?: (css: string) => boolean;
}

/** A y axis as the legend shows it. */
export interface AxisSummary {
  readonly key: string;
  readonly title: string;
  readonly setting: YScaleSetting;
  /** Current range in display units (null without data), so the UI can fix the scale where it is. */
  readonly displayRange: XRange | null;
  /** A fixed scale clips visible data (silent-failure M7). */
  readonly offScale: boolean;
}

/**
 * Health of a graph: 'ok', or 'error' when the graph cannot show current data (a frame threw, a canvas has no
 * context, the series renderer failed, a bad configuration). An erroring graph never keeps showing its last
 * picture or readouts as if they were current (spec §249 "Telemetry flowing, graph crashed", §332).
 */
export type GraphHealth = 'ok' | 'error';

/** Freshness of a live graph's feed as a whole (its current-car marker's frequency class). */
export type FeedState = 'live' | 'degraded' | 'stale' | 'noData';

/** Feed state of `source` (null for recordings). */
function feedOf(source: GraphDataSource): FeedState | null {
  if (source.origin !== 'live') return null;
  const current = source.current();
  if (current === null) return 'noData';
  return current.stale ? 'stale' : current.degraded ? 'degraded' : 'live';
}

/**
 * What a readout refers to: 'current' = the newest data of a live source (its state is the channel's current
 * freshness); 'history' = an older live value at the cursor (shown as recorded at the cursor time, never as
 * current); 'recorded' = a value of a recording at the cursor.
 */
export type ReadoutKind = 'current' | 'history' | 'recorded';

/** One legend readout. */
export interface GraphReadout {
  readonly kind: ReadoutKind;
  readonly sample: ChannelSample;
}

/** What the React layer shows next to the canvas. Immutable; replaced only when something in it changed. */
export interface GraphSnapshot {
  /** LIVE (following the newest data / whole lap) or paused / zoomed. */
  readonly follow: boolean;
  readonly origin: DataOrigin;
  readonly health: GraphHealth;
  /** Why the graph is not healthy (shown to the user), or null when healthy. */
  readonly problem: string | null;
  /**
   * Freshness of a live feed (spec §126, §248: the live indicator follows the telemetry age, not the view):
   * 'noData' before data or after a RESET; null for recordings.
   */
  readonly feed: FeedState | null;
  /** Renderer actually drawing the series (after any fallback); null before the first frame. */
  readonly renderer: RendererKind | null;
  /** Shown while the series renderer runs on a fallback (e.g. "CANVAS2D fallback: WebGL context lost"). */
  readonly rendererNote: string | null;
  /** Readout per series key (`seriesKey` / `referenceKey`); null when there is nothing to read. */
  readonly readouts: Readonly<Record<string, GraphReadout | null>>;
  readonly axes: readonly AxisSummary[];
  /** Cursor x as text, e.g. "12:41.25", or null. */
  readonly cursorLabel: string | null;
}

/** Default live span of a scrolling axis (s). */
export const DEFAULT_LIVE_SPAN_S = 10;
/** Default largest bridgeable time step: the spec §126 DEGRADED threshold (a longer silence is a real gap). */
export const DEFAULT_MAX_GAP_S = SPEC_DEGRADED_AFTER_MS / 1_000;
/** Number of series colours (tokens --graph-series-1 … 8). */
export const SERIES_PALETTE_SIZE = 8;
/** Text shown in an empty plot. */
export const NO_DATA_TEXT = 'NO DATA';
/** Text drawn over the plot while frames fail (a failing graph must never look like a frozen live one). */
export const GRAPH_ERROR_TEXT = 'GRAPH ERROR · not updating';
/** A failure streak is logged at its first frame and then every this many frames (no 60 Hz console flood). */
const FAILURE_LOG_EVERY = 600;
/** Error text colour when the theme has not been resolved (high contrast on the dark and light surfaces). */
const ERROR_TEXT_FALLBACK = '#ff5f5f';
/** Frame durations kept for diagnostics. */
const FRAME_HISTORY = 600;
/** Long-frame threshold of `frameStats` (one 60 Hz frame). */
const DEFAULT_LONG_FRAME_MS = 1_000 / 60;
const FALLBACK_FONT_SIZE = '11px';
const FALLBACK_FONT_FAMILY = 'sans-serif';
const POINTER_BUTTON_PRIMARY = 0;
const POINTER_BUTTON_MIDDLE = 1;

/** Readout key of a primary series. */
export function seriesKey(channelId: number): string {
  return `s:${channelId}`;
}

/** Readout key of a reference series. */
export function referenceKey(key: string): string {
  return `r:${key}`;
}

/** Palette slot (1…8) of the series at `index`. */
export function seriesColorSlot(series: GraphSeries, index: number): number {
  const slot = series.color ?? (index % SERIES_PALETTE_SIZE) + 1;
  return ((Math.round(slot) - 1 + SERIES_PALETTE_SIZE) % SERIES_PALETTE_SIZE) + 1;
}

/** Axis display of a channel (unknown channels plot on a unitless axis). */
function displayOf(def: ChannelDef | undefined): AxisDisplay {
  return def === undefined ? unitAxisDisplay(Unit.None) : channelAxisDisplay(def);
}

/** A readout sample: the channel, a state and a value only when the state shows one. */
function readoutSample(id: number, def: ChannelDef | undefined, state: SampleState, value: number | null): ChannelSample {
  return {
    id,
    def: def ?? null,
    state,
    quality: null,
    value: state === 'live' || state === 'degraded' ? value : null,
    receivedAtMs: null,
    captureMonoNs: null,
    captureRelayNsEst: null,
    sourceEtS: null,
  };
}

function sameReadout(a: GraphReadout | null | undefined, b: GraphReadout | null): boolean {
  if (a === undefined) return false;
  if (a === null || b === null) return a === b;
  return a.kind === b.kind && a.sample.id === b.sample.id && a.sample.state === b.sample.state && a.sample.value === b.sample.value;
}

function sameRange(a: XRange | null, b: XRange | null): boolean {
  return a === b || (a !== null && b !== null && a.min === b.min && a.max === b.max);
}

function sameSetting(a: YScaleSetting, b: YScaleSetting): boolean {
  if (a.mode !== b.mode) return false;
  return a.mode === 'auto' || (b.mode === 'fixed' && a.min === b.min && a.max === b.max);
}

let nextEngineId = 0;

/**
 * A valid largest bridgeable gap: positive and finite. Anything else (NaN, ≤ 0, ∞) would make every readout NO
 * DATA, draw only dots and hide every gap band (silent-failure L1), so it is replaced by DEFAULT_MAX_GAP_S and
 * logged.
 */
function checkMaxGapS(value: number, id: string): number {
  if (Number.isFinite(value) && value > 0) return value;
  console.error('[graph] invalid maxGapS; using the default', { id, maxGapS: value, default: DEFAULT_MAX_GAP_S });
  return DEFAULT_MAX_GAP_S;
}

/** Colour tokens the engine reads. */
type ThemeToken =
  | '--graph-grid'
  | '--graph-axis'
  | '--graph-axis-text'
  | '--graph-cursor'
  | '--graph-cursor-text'
  | '--graph-zoom-fill'
  | '--graph-zoom-stroke'
  | '--graph-marker-lap'
  | '--graph-marker-sector'
  | '--graph-marker-event'
  | '--graph-current-car'
  | '--graph-current-car-stale'
  | '--graph-gap-fill'
  | '--graph-gap-text'
  | '--graph-error-text'
  | '--graph-reference'
  | `--graph-series-${1 | 2 | 3 | 4 | 5 | 6 | 7 | 8}`;

const SERIES_TOKENS = [
  '--graph-series-1',
  '--graph-series-2',
  '--graph-series-3',
  '--graph-series-4',
  '--graph-series-5',
  '--graph-series-6',
  '--graph-series-7',
  '--graph-series-8',
] as const satisfies readonly ThemeToken[];

/**
 * Documented fallback per colour token (the dark theme's values in tokens.css), used only when a token is empty or
 * invalid (logged): eight distinct, high-contrast series colours (never black, never two alike), and a STALE
 * colour distinct from the live current-car colour.
 */
export const THEME_FALLBACK: Readonly<Record<ThemeToken, string>> = {
  '--graph-grid': '#1d2229',
  '--graph-axis': '#3a434f',
  '--graph-axis-text': '#9aa4b1',
  '--graph-cursor': '#e6e9ee',
  '--graph-cursor-text': '#0b0d10',
  '--graph-zoom-fill': '#4c9dff26',
  '--graph-zoom-stroke': '#4c9dff',
  '--graph-marker-lap': '#9aa4b1',
  '--graph-marker-sector': '#6b7480',
  '--graph-marker-event': '#4c9dff',
  '--graph-current-car': '#7fb8ff',
  '--graph-current-car-stale': '#a78bfa',
  '--graph-gap-fill': '#ffffff0f',
  '--graph-gap-text': '#6b7480',
  '--graph-error-text': '#ff5f5f',
  '--graph-reference': '#9aa4b1',
  '--graph-series-1': '#5aa9ff',
  '--graph-series-2': '#ff8c42',
  '--graph-series-3': '#3dd6c6',
  '--graph-series-4': '#f06fb0',
  '--graph-series-5': '#d4d46a',
  '--graph-series-6': '#e0a96d',
  '--graph-series-7': '#b0e05c',
  '--graph-series-8': '#6fd3ff',
};

/** Token problems already logged (one line per distinct set, not one per graph or frame). */
const reportedThemeProblems = new Set<string>();

/** True for a non-empty CSS colour the browser accepts (`CSS.supports`; any non-empty value where it is absent). */
export function cssColorIsValid(css: string): boolean {
  if (css.trim() === '') return false;
  if (typeof CSS !== 'undefined' && typeof CSS.supports === 'function') return CSS.supports('color', css);
  return true;
}

interface DragState {
  readonly pointerId: number;
  readonly mode: 'zoom' | 'pan';
  readonly startPx: number;
  lastPx: number;
}

/** One telemetry graph (see the module comment). */
export class GraphEngine {
  readonly id: string;
  /** Durations of recent frames (ms of JS work per repaint). */
  readonly frameTimes = new DurationRecorder(FRAME_HISTORY);

  #config: GraphConfig;
  readonly #rendererOption: RendererKind | (() => SeriesRenderer);
  readonly #initialSpan: number | undefined;
  readonly #scheduleFrame: ScheduleFrame;
  readonly #contextFactory: ContextFactory;
  readonly #now: () => number;

  #view: ViewState;
  #cursorX: number | null = null;
  #zoomBox: { x0: number; x1: number } | null = null;
  #drag: DragState | null = null;

  // Attached state.
  #container: HTMLElement | null = null;
  #under: HTMLCanvasElement | null = null;
  #host: HTMLDivElement | null = null;
  #over: HTMLCanvasElement | null = null;
  #underCtx: Canvas2DLike | null = null;
  #overCtx: Canvas2DLike | null = null;
  #renderer: SeriesRenderer | null = null;
  #resizeObserver: ResizeObserver | null = null;
  #detachInput: (() => void) | null = null;
  #subscriptions: (() => void)[] = [];
  #cancelFrame: (() => void) | null = null;
  #width = 0;
  #height = 0;
  #dpr = 1;
  #colors: GraphColors | null = null;
  #seriesColors: string[] = [];
  #referenceColor = '';
  #font = `${FALLBACK_FONT_SIZE} ${FALLBACK_FONT_FAMILY}`;

  // Frame state.
  #dirtyData = true;
  #layout: GraphLayout = computeLayout(0, 0, 0, 0);
  #xRange: XRange | null = null;
  #primary: GraphWindow | null = null;
  #referenceWindows = new Map<string, GraphWindow>();
  #yAxes: YAxisModel[] = [];
  #xTicks: LabelledTicks = { values: [], labels: [] };
  #gaps: XRange[] = [];
  #markers: PlacedMarker[] = [];
  #current: CurrentPosition | null = null;
  #staleSeries: StaleSeriesTag[] = [];
  #valueBreaks: SeriesBreaks[] = [];

  #snapshot: GraphSnapshot;
  readonly #listeners = new Set<() => void>();
  readonly #isColor: (css: string) => boolean;
  #themeRetry = false;
  #offRenderer: (() => void) | null = null;
  /** Set when a chrome canvas has no 2D context (silent-failure M4). */
  #contextProblem: string | null = null;
  /** Set when the sync group refused this graph (silent-failure M6). */
  #syncProblem: string | null = null;

  // Health.
  /** Error of the last frame (null after a frame that completed). */
  #frameError: string | null = null;
  /** Consecutive failed frames. */
  #frameFailures = 0;
  /** Failed frames since construction (diagnostics). */
  #totalFrameFailures = 0;

  constructor(options: GraphEngineOptions) {
    this.id = options.id ?? `graph-${nextEngineId++}`;
    this.#config = {
      source: options.source,
      xMode: options.xMode,
      series: options.series,
      references: options.references ?? [],
      markers: options.markers ?? [],
      yScales: options.yScales ?? {},
      maxGapS: checkMaxGapS(options.maxGapS ?? DEFAULT_MAX_GAP_S, this.id),
      xDomain: options.xDomain ?? null,
      syncGroup: options.syncGroup ?? null,
    };
    this.#rendererOption = options.renderer ?? DEFAULT_RENDERER;
    this.#initialSpan = options.initialSpan;
    this.#scheduleFrame = options.scheduleFrame ?? sharedFrameLoop();
    this.#contextFactory = options.context ?? browserContext;
    this.#now = options.now ?? (() => performance.now());
    this.#isColor = options.isColor ?? cssColorIsValid;
    this.#view = this.#initialView();
    this.#snapshot = {
      follow: this.#view.follow,
      origin: this.#config.source.origin,
      health: 'ok',
      feed: feedOf(this.#config.source),
      problem: null,
      renderer: null,
      rendererNote: null,
      readouts: {},
      axes: [],
      cursorLabel: null,
    };
  }

  /** Current configuration. */
  get config(): GraphConfig {
    return this.#config;
  }

  /** Current view state. */
  get view(): ViewState {
    return this.#view;
  }

  /** Current cursor x, or null. */
  get cursorX(): number | null {
    return this.#cursorX;
  }

  /** Number of frames that failed since the engine was created (diagnostics). */
  get frameFailures(): number {
    return this.#totalFrameFailures;
  }

  /** Repaints everything from a fresh read (the "Retry" of an erroring graph, spec §332). */
  retry(): void {
    this.#invalidate(true);
  }

  /** Summary of recent frame durations; frames above `longMs` count as long. */
  frameStats(longMs: number = DEFAULT_LONG_FRAME_MS): DurationSummary {
    return summarize(this.frameTimes.values(), longMs);
  }

  // ---------------------------------------------------------------------------------------------------------
  // External store (React)
  // ---------------------------------------------------------------------------------------------------------

  /** Subscribes to snapshot changes; returns the unsubscribe function. */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** The current snapshot (same object until something shown changed). */
  readonly getSnapshot = (): GraphSnapshot => this.#snapshot;

  // ---------------------------------------------------------------------------------------------------------
  // Configuration and view
  // ---------------------------------------------------------------------------------------------------------

  /** Changes the configuration. A new source or x mode resets the view to LIVE. */
  update(change: Partial<GraphConfig>): void {
    const prev = this.#config;
    const next: GraphConfig = {
      ...prev,
      ...change,
      maxGapS: change.maxGapS === undefined ? prev.maxGapS : checkMaxGapS(change.maxGapS, this.id),
    };
    this.#config = next;
    if (next.source !== prev.source || next.xMode !== prev.xMode) {
      this.#view = this.#initialView();
      this.#cursorX = null;
    }
    const resubscribe =
      next.source !== prev.source || next.xMode !== prev.xMode || next.references !== prev.references || next.syncGroup !== prev.syncGroup;
    if (resubscribe && this.#container !== null) this.#subscribe();
    this.#invalidate(true);
  }

  /** Applies a view action (zoom, pan, LIVE …) and shares the result with the sync group. */
  dispatch(action: ViewAction): void {
    this.#setView(reduceView(this.#view, action, this.#bounds()));
  }

  /** Returns to LIVE (newest data on a scrolling axis, the whole lap on a lap axis). */
  goLive(): void {
    this.dispatch({ type: 'live' });
  }

  /** Sets the cursor x (snapped to the nearest row) and shares it with the sync group; null hides it. */
  setCursor(x: number | null): void {
    const snapped = x === null || this.#primary === null ? null : nearestRowX(this.#primary, x);
    const group = this.#group();
    if (group !== null) group.setCursor(this.id, snapped);
    else this.#applyCursor(snapped);
  }

  /** Sets the drawing size in CSS px (normally done by the ResizeObserver). */
  resize(width: number, height: number): void {
    if (width === this.#width && height === this.#height) return;
    this.#width = Math.max(0, width);
    this.#height = Math.max(0, height);
    this.#applySize();
    this.#invalidate(true);
  }

  /** Re-reads the colour and font tokens (call after a theme change). */
  refreshTheme(): void {
    if (this.#container === null) return;
    this.#resolveTheme(this.#container);
    this.#invalidate(true);
  }

  /** Runs a pending frame now (tests and the benchmark); does nothing when nothing is pending. */
  flush(): void {
    const cancel = this.#cancelFrame;
    if (cancel === null) return;
    cancel();
    this.#frame();
  }

  // ---------------------------------------------------------------------------------------------------------
  // Attach / detach
  // ---------------------------------------------------------------------------------------------------------

  /**
   * Creates the layers inside `container`, starts observing its size and the data, and returns the function
   * that removes everything again. An engine can be attached again after detaching (React StrictMode).
   */
  attach(container: HTMLElement): () => void {
    if (this.#container !== null) this.#detach();
    this.#container = container;
    const under = document.createElement('canvas');
    under.className = 'telemetry-graph__layer';
    under.setAttribute('aria-hidden', 'true');
    const host = document.createElement('div');
    host.className = 'telemetry-graph__layer telemetry-graph__series';
    host.setAttribute('aria-hidden', 'true');
    const over = document.createElement('canvas');
    over.className = 'telemetry-graph__layer telemetry-graph__input';
    over.tabIndex = 0;
    over.setAttribute('role', 'img');
    container.append(under, host, over);
    this.#under = under;
    this.#host = host;
    this.#over = over;
    this.#underCtx = this.#contextFactory(under);
    this.#overCtx = this.#contextFactory(over);
    const renderer = this.#createRenderer();
    renderer.mount(host);
    this.#renderer = renderer;
    this.#offRenderer =
      renderer.onStatusChange?.(() => {
        this.#onRendererStatus();
      }) ?? null;
    if ((renderer.status?.().state ?? 'ok') !== 'ok') this.#onRendererStatus();
    // The stylesheet may not be applied yet when attaching: an incomplete theme is read again on the first frame.
    this.#themeRetry = !this.#resolveTheme(container);
    this.#contextProblem =
      this.#underCtx === null || this.#overCtx === null
        ? 'no 2D canvas context: axes, cursor, gap and STALE markers cannot be drawn'
        : null;
    if (this.#contextProblem !== null) console.error('[graph] chrome canvas unavailable', this.#diagnosticContext());
    this.#detachInput = this.#bindInput(over);
    if (typeof ResizeObserver !== 'undefined') {
      const observer = new ResizeObserver((entries) => {
        const entry = entries[entries.length - 1];
        if (entry !== undefined) this.resize(entry.contentRect.width, entry.contentRect.height);
      });
      observer.observe(container);
      this.#resizeObserver = observer;
    }
    this.#width = container.clientWidth;
    this.#height = container.clientHeight;
    this.#applySize();
    this.#subscribe();
    this.#invalidate(true);
    return () => {
      if (this.#container === container) this.#detach();
    };
  }

  #detach(): void {
    this.#cancelFrame?.();
    this.#cancelFrame = null;
    for (const off of this.#subscriptions) off();
    this.#subscriptions = [];
    this.#resizeObserver?.disconnect();
    this.#resizeObserver = null;
    this.#detachInput?.();
    this.#detachInput = null;
    this.#offRenderer?.();
    this.#offRenderer = null;
    this.#renderer?.destroy();
    this.#renderer = null;
    this.#under?.remove();
    this.#host?.remove();
    this.#over?.remove();
    this.#under = null;
    this.#host = null;
    this.#over = null;
    this.#underCtx = null;
    this.#overCtx = null;
    this.#container = null;
    this.#drag = null;
    this.#zoomBox = null;
  }

  #createRenderer(): SeriesRenderer {
    const option = this.#rendererOption;
    if (typeof option === 'function') return option();
    if (option === 'uplot') return new UplotRenderer();
    const canvas2d = (): SeriesRenderer => new Canvas2DRenderer({ context: this.#contextFactory });
    return option === 'webgl' ? new WebGLRenderer({ fallback: canvas2d }) : canvas2d();
  }

  /**
   * Reads the colour and font tokens. Every colour is validated (silent-failure H5): an empty or invalid token
   * would be ignored by the canvas (a series stroked in the previous series' colour, a STALE marker looking live)
   * or parsed as black by WebGL, so it is replaced by its documented fallback (`THEME_FALLBACK`) and logged once.
   * Returns false when a token had to be replaced.
   */
  #resolveTheme(container: HTMLElement): boolean {
    const style = getComputedStyle(container);
    const token = (name: string): string => style.getPropertyValue(name).trim();
    const bad: string[] = [];
    const color = (name: ThemeToken): string => {
      const value = token(name);
      if (this.#isColor(value)) return value;
      bad.push(value === '' ? name : `${name}: ${value}`);
      return THEME_FALLBACK[name];
    };
    this.#colors = {
      grid: color('--graph-grid'),
      axis: color('--graph-axis'),
      axisText: color('--graph-axis-text'),
      cursor: color('--graph-cursor'),
      cursorText: color('--graph-cursor-text'),
      zoomFill: color('--graph-zoom-fill'),
      zoomStroke: color('--graph-zoom-stroke'),
      markers: {
        lap: color('--graph-marker-lap'),
        sector: color('--graph-marker-sector'),
        event: color('--graph-marker-event'),
      },
      currentCar: color('--graph-current-car'),
      currentCarStale: color('--graph-current-car-stale'),
      gapFill: color('--graph-gap-fill'),
      gapText: color('--graph-gap-text'),
      errorText: color('--graph-error-text'),
    };
    this.#seriesColors = SERIES_TOKENS.map((name) => color(name));
    this.#referenceColor = color('--graph-reference');
    if (bad.length > 0) {
      const key = bad.join(', ');
      if (!reportedThemeProblems.has(key)) {
        reportedThemeProblems.add(key);
        console.error('[graph] invalid or missing colour tokens; using the documented fallbacks', { id: this.id, tokens: bad });
      }
    }
    const size = token('--font-size-xs');
    const family = token('--font-ui');
    this.#font = `${size === '' ? FALLBACK_FONT_SIZE : size} ${family === '' ? FALLBACK_FONT_FAMILY : family}`;
    return bad.length === 0;
  }

  #subscribe(): void {
    for (const off of this.#subscriptions) off();
    const offs: (() => void)[] = [];
    const onData = (): void => {
      this.#invalidate(true);
    };
    offs.push(this.#config.source.subscribe(onData));
    for (const ref of this.#config.references) offs.push(ref.source.subscribe(onData));
    const group = this.#config.syncGroup;
    this.#syncProblem = null;
    let joined = false;
    if (group !== null) {
      const axis = `${this.#config.xMode}|${this.#config.source.origin}`;
      try {
        offs.push(
          group.subscribe((event) => {
            this.#onSync(event);
          }, axis),
        );
        joined = true;
      } catch (error: unknown) {
        // Refused (different x semantics): run standalone and say why, never share a cursor across units.
        this.#syncProblem = `not synchronized: ${error instanceof Error ? error.message : String(error)}`;
        console.error('[graph] sync group refused this graph', this.#diagnosticContext(), error);
      }
    }
    if (group !== null && joined) {
      const shared = group.view;
      if (shared !== null) this.#view = shared;
      else group.setView(this.id, this.#view);
      this.#cursorX = group.cursor;
    }
    this.#subscriptions = offs;
  }

  #onSync(event: SyncEvent): void {
    if (event.kind === 'cursor') {
      this.#applyCursor(event.x);
      return;
    }
    if (event.source === this.id) return;
    this.#view = event.view;
    this.#invalidate(true);
  }

  #applyCursor(x: number | null): void {
    if (x === this.#cursorX) return;
    this.#cursorX = x;
    this.#invalidate(false);
  }

  /** The sync group this graph actually joined (null when standalone or refused). */
  #group(): GraphSyncGroup | null {
    return this.#syncProblem === null ? this.#config.syncGroup : null;
  }

  #setView(view: ViewState): void {
    if (view === this.#view) return;
    this.#view = view;
    this.#group()?.setView(this.id, view);
    this.#invalidate(true);
  }

  #initialView(): ViewState {
    const live = this.#config.source.origin === 'live';
    return liveView(this.#initialSpan ?? (live ? DEFAULT_LIVE_SPAN_S : Number.POSITIVE_INFINITY));
  }

  /** View bounds from the axis mode, the caller's domain and the data. */
  #bounds(): ViewBounds {
    const spec = X_AXIS_SPECS[this.#config.xMode];
    const extent = this.#config.source.extent();
    if (spec.viewKind === 'domain') {
      const domain = this.#config.xDomain ?? spec.domain ?? extent;
      const width = domain === null ? spec.minSpan : domain.max - domain.min;
      return { kind: 'domain', extent: domain, minSpan: spec.minSpan, maxSpan: Math.max(spec.minSpan, width) };
    }
    const width = extent === null ? 0 : extent.max - extent.min;
    const initial = this.#initialSpan ?? DEFAULT_LIVE_SPAN_S;
    return {
      kind: 'scroll',
      extent,
      minSpan: spec.minSpan,
      maxSpan: Math.max(spec.minSpan, width, this.#config.source.origin === 'live' ? initial : 0),
    };
  }

  #applySize(): void {
    const dpr = this.#dpr;
    if (this.#under !== null) sizeCanvas(this.#under, this.#width, this.#height, dpr);
    if (this.#over !== null) sizeCanvas(this.#over, this.#width, this.#height, dpr);
    this.#renderer?.setSize(this.#width, this.#height, dpr);
  }

  #invalidate(data: boolean): void {
    if (data) this.#dirtyData = true;
    if (this.#container === null || this.#cancelFrame !== null) return;
    this.#cancelFrame = this.#scheduleFrame(() => {
      this.#cancelFrame = null;
      this.#frame();
    });
  }

  // ---------------------------------------------------------------------------------------------------------
  // Frame
  // ---------------------------------------------------------------------------------------------------------

  #frame(): void {
    this.#cancelFrame = null;
    if (this.#container === null) return;
    const start = this.#now();
    try {
      this.#frameBody();
    } catch (error: unknown) {
      // The series must be redrawn from a fresh read once frames work again.
      this.#dirtyData = true;
      this.#failFrame(error);
    }
    this.frameTimes.record(this.#now() - start);
  }

  #frameBody(): void {
    if (this.#themeRetry && this.#container !== null) {
      this.#themeRetry = false;
      this.#resolveTheme(this.#container);
      this.#dirtyData = true;
    }
    const dpr = typeof devicePixelRatio === 'number' && devicePixelRatio > 0 ? devicePixelRatio : 1;
    if (dpr !== this.#dpr) {
      this.#dpr = dpr;
      this.#applySize();
      this.#dirtyData = true;
    }
    const drawnRange = this.#xRange;
    let drawSeries = this.#dirtyData;
    this.#dirtyData = false;
    this.#read();
    this.#dropScrolledCursor();
    // The range can move before the source's change notification arrives; the series must follow it.
    if (!sameRange(drawnRange, this.#xRange)) drawSeries = true;
    if (drawSeries) this.#drawData();
    this.#drawChrome();
    if (this.#frameError !== null) {
      console.warn('[graph] frame recovered', { ...this.#diagnosticContext(), failedFrames: this.#frameFailures });
    }
    this.#frameError = null;
    this.#frameFailures = 0;
    this.#publish();
  }

  /**
   * A cursor left behind by a following live view (the pointer stopped, the trace scrolled on) is dropped once
   * its x leaves the visible range, so readouts return to the current data instead of reading a point that is
   * no longer on screen (silent-failure H3).
   */
  #dropScrolledCursor(): void {
    const x = this.#cursorX;
    const range = this.#xRange;
    if (x === null || range === null || !this.#view.follow || this.#config.source.origin !== 'live') return;
    if (x >= range.min && x <= range.max) return;
    this.#cursorX = null;
    this.#group()?.setCursor(this.id, null);
  }

  /** Status of the series renderer ('ok' for renderers without one). */
  #rendererStatus(): RendererStatus {
    const renderer = this.#renderer;
    if (renderer === null) return okStatus(this.#rendererKindOption());
    return renderer.status?.() ?? okStatus(renderer.kind);
  }

  #rendererKindOption(): RendererKind {
    const option = this.#rendererOption;
    return typeof option === 'function' ? DEFAULT_RENDERER : option;
  }

  /** Renderer actually drawing the series (after any fallback). */
  get activeRenderer(): RendererKind {
    return this.#rendererStatus().activeKind;
  }

  /** A problem that keeps the graph from showing current data until it is fixed (not a single failed frame). */
  #standingProblem(): string | null {
    const status = this.#rendererStatus();
    if (status.state === 'failed') return `series renderer failed: ${status.reason ?? 'unknown reason'}`;
    return this.#contextProblem ?? this.#syncProblem;
  }

  /** The renderer fell back or failed (silent-failure M1, M5): log it with context and redraw from a fresh read. */
  #onRendererStatus(): void {
    const status = this.#rendererStatus();
    if (status.state === 'failed') console.error('[graph] series renderer failed', this.#diagnosticContext(), status);
    else if (status.state === 'degraded') console.warn('[graph] series renderer degraded', this.#diagnosticContext(), status);
    this.#invalidate(true);
  }

  /** Context of every graph log line: which graph, renderer, axis and data. */
  #diagnosticContext(): Record<string, unknown> {
    const source = this.#config.source;
    return {
      id: this.id,
      renderer: this.#renderer === null ? null : this.#rendererStatus().activeKind,
      xMode: this.#config.xMode,
      origin: source.origin,
      dataKind: source.dataKind,
    };
  }

  /**
   * A frame threw: log it with context, publish an error snapshot (no readout keeps a value) and replace the
   * picture with the error text, so the graph never looks like a frozen live plot. Later frames retry.
   */
  #failFrame(error: unknown): void {
    this.#frameFailures += 1;
    this.#totalFrameFailures += 1;
    const message = error instanceof Error ? error.message : String(error);
    if (this.#frameError !== message || this.#frameFailures % FAILURE_LOG_EVERY === 1) {
      console.error('[graph] frame failed', { ...this.#diagnosticContext(), failedFrames: this.#frameFailures }, error);
    }
    this.#frameError = message;
    try {
      this.#publishError(`${GRAPH_ERROR_TEXT}: ${message}`);
    } catch (listenerError: unknown) {
      console.error('[graph] error snapshot listener failed', this.#diagnosticContext(), listenerError);
    }
    this.#drawErrorPicture();
  }

  /** Clears the series and draws the error text over the plot; failures here are logged, never thrown. */
  #drawErrorPicture(): void {
    try {
      const plot = this.#layout.plot;
      this.#renderer?.render({ plot, xRange: { min: 0, max: 1 }, maxGapT: DEFAULT_MAX_GAP_S, series: [] });
    } catch (error: unknown) {
      console.error('[graph] clearing the series after a failed frame failed', this.#diagnosticContext(), error);
    }
    try {
      const ctx = this.#overCtx;
      const over = this.#over;
      if (over !== null) over.setAttribute('aria-label', `${GRAPH_ERROR_TEXT}. ${this.#ariaLabel()}`);
      if (ctx === null || over === null) return;
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, over.width, over.height);
      ctx.setTransform(this.#dpr, 0, 0, this.#dpr, 0, 0);
      ctx.font = this.#font;
      ctx.fillStyle = this.#colors?.errorText ?? ERROR_TEXT_FALLBACK;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText(GRAPH_ERROR_TEXT, this.#width / 2, this.#height / 2);
    } catch (error: unknown) {
      console.error('[graph] drawing the error state failed', this.#diagnosticContext(), error);
    }
  }

  /** Publishes an error snapshot: every readout UNAVAILABLE (no value), health 'error' with `problem`. */
  #publishError(problem: string): void {
    const prev = this.#snapshot;
    if (prev.health === 'error' && prev.problem === problem) return;
    const readouts: Record<string, GraphReadout | null> = {};
    const unavailable = (id: number): GraphReadout => ({ kind: 'current', sample: readoutSample(id, channelDef(id), 'unavailable', null) });
    for (const s of this.#config.series) {
      readouts[seriesKey(s.channelId)] = s.enabled === false ? null : unavailable(s.channelId);
    }
    for (const ref of this.#config.references) readouts[referenceKey(ref.key)] = unavailable(ref.channelId);
    this.#snapshot = { ...prev, health: 'error', problem, readouts, cursorLabel: null, feed: feedOf(this.#config.source) };
    for (const listener of [...this.#listeners]) listener();
  }

  /** Resolves the view against the data and reads the visible windows. */
  #read(): void {
    const bounds = this.#bounds();
    let view = this.#view;
    if (!Number.isFinite(view.span) && bounds.extent !== null) view = reduceView(view, { type: 'setSpan', span: bounds.maxSpan }, bounds);
    view = reduceView(view, { type: 'clamp' }, bounds);
    if (view !== this.#view) {
      this.#view = view;
      this.#group()?.setView(this.id, view);
    }
    const range = resolveRange(view, bounds);
    this.#xRange = range !== null && range.max > range.min ? range : null;
    this.#primary = null;
    this.#referenceWindows.clear();
    const xRange = this.#xRange;
    if (xRange === null) return;
    const ids = this.#config.series.filter((s) => s.enabled !== false).map((s) => s.channelId);
    this.#primary = this.#config.source.read(xRange, ids);
    for (const ref of this.#config.references) {
      this.#referenceWindows.set(ref.key, ref.source.read(xRange, [ref.channelId]));
    }
  }

  /** Axes, layout, series, grid and gaps (only when data, view, size or configuration changed). */
  #drawData(): void {
    const cfg = this.#config;
    const xRange = this.#xRange;
    const primary = this.#primary;
    const members: AxisMember[] = [];
    const pending: { key: string; window: GraphWindow; ys: Float64Array; axis: string; color: string; dashed: boolean; step: boolean }[] = [];
    const current = cfg.source.origin === 'live' ? cfg.source.current() : null;
    const staleSeries: StaleSeriesTag[] = [];
    const add = (
      key: string,
      source: GraphDataSource,
      window: GraphWindow | null | undefined,
      id: number,
      color: string,
      dashed: boolean,
    ): void => {
      const def = channelDef(id);
      const display = displayOf(def);
      const label = def?.label ?? `Channel ${id}`;
      const ys = window?.column(id) ?? null;
      // A STALE channel (its class or the channel itself, per the store) keeps its last value in every new ring row,
      // so its trace would run on as a flat live-looking line: draw it dashed and name it (silent-failure C2).
      const stale = source.origin === 'live' && source.latest(id)?.state === 'stale';
      if (stale && current?.stale !== true) staleSeries.push({ label, color });
      let extent: XRange | null = null;
      if (window != null && ys !== null && xRange !== null) {
        const rows = visibleRows(window, xRange);
        extent = dataExtent(ys, rows.start, rows.end, window.xs, xRange);
        pending.push({
          key,
          window,
          ys,
          axis: axisKey(display),
          color,
          dashed: dashed || stale,
          step: def?.interpolation === Interpolation.Discrete,
        });
      }
      members.push({ display, label, color, extent });
    };
    cfg.series.forEach((s, i) => {
      if (s.enabled === false) return;
      add(seriesKey(s.channelId), cfg.source, primary, s.channelId, this.#seriesColor(s, i), false);
    });
    for (const ref of cfg.references) {
      const color = ref.color === undefined ? this.#referenceColor : this.#seriesColor({ channelId: ref.channelId, color: ref.color }, 0);
      add(referenceKey(ref.key), ref.source, this.#referenceWindows.get(ref.key), ref.channelId, color, true);
    }
    this.#staleSeries = staleSeries;
    const keys = [...new Set(members.map((m) => axisKey(m.display)))];
    const left = Math.ceil(keys.length / 2);
    const layout = computeLayout(this.#width, this.#height, left, keys.length - left);
    this.#layout = layout;
    this.#yAxes = buildYAxes(members, cfg.yScales, layout.plot.height);
    const axisByKey = new Map(this.#yAxes.map((a) => [a.key, a] as const));
    const spec = X_AXIS_SPECS[cfg.xMode];
    const maxGapT = cfg.maxGapS;

    const series: RenderSeries[] = [];
    for (const p of pending) {
      const range = axisByKey.get(p.axis)?.range ?? null;
      if (range === null) continue;
      series.push({ key: p.key, window: p.window, ys: p.ys, yRange: range, color: p.color, dashed: p.dashed, step: p.step });
    }
    this.#renderer?.render({ plot: layout.plot, xRange: xRange ?? { min: 0, max: 1 }, maxGapT, series: xRange === null ? [] : series });

    this.#xTicks = xRange === null ? { values: [], labels: [] } : xTicks(spec, xRange, tickTarget(layout.plot.width, X_TICK_SPACING_CSS));
    this.#gaps = [];
    this.#valueBreaks = [];
    if (primary !== null && xRange !== null) {
      const rows = visibleRows(primary, xRange);
      // A row where no plotted channel has a value (e.g. the store's gap row inside a dropped stretch) is no data.
      const columns = pending.filter((p) => p.window === primary).map((p) => p.ys);
      const empty = (i: number): boolean => columns.length > 0 && columns.every((c) => !Number.isFinite(c[i] ?? Number.NaN));
      findTimeGaps(primary.xs, primary.ts, rows.start, rows.end, maxGapT, this.#gaps, empty);
    }
    for (const p of pending) {
      if (xRange === null) break;
      const rows = visibleRows(p.window, xRange);
      const ranges = findValueBreaks(p.window.xs, p.ys, rows.start, rows.end);
      if (ranges.length > 0) this.#valueBreaks.push({ color: p.color, ranges });
    }
    this.#current = current;
    this.#markers = this.#placeMarkers(layout, xRange);

    const ctx = this.#underCtx;
    const scene = this.#scene();
    if (ctx !== null && this.#under !== null && scene !== null) {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, this.#under.width, this.#under.height);
      ctx.setTransform(this.#dpr, 0, 0, this.#dpr, 0, 0);
      drawUnderlay(ctx, scene);
    }
  }

  #placeMarkers(layout: GraphLayout, xRange: XRange | null): PlacedMarker[] {
    const ctx = this.#overCtx;
    if (xRange === null || layout.plot.width <= 0 || this.#config.markers.length === 0) return [];
    const x = linearAffine(xRange.min, xRange.max, layout.plot.left, layout.plot.left + layout.plot.width);
    if (ctx !== null) ctx.font = this.#font;
    return layoutMarkers(this.#config.markers, x, {
      plotLeft: layout.plot.left,
      plotRight: layout.plot.left + layout.plot.width,
      rows: MARKER_ROWS,
      gap: MARKER_LABEL_GAP_CSS,
      measure: (text) => (ctx === null ? 0 : ctx.measureText(text).width),
    });
  }

  #seriesColor(series: GraphSeries, index: number): string {
    return this.#seriesColors[seriesColorSlot(series, index) - 1] ?? '';
  }

  #scene(): OverlayScene | null {
    const colors = this.#colors;
    if (colors === null) return null;
    const spec = X_AXIS_SPECS[this.#config.xMode];
    const cursorX = this.#cursorX;
    return {
      layout: this.#layout,
      xRange: this.#xRange,
      xTicks: this.#xTicks,
      xTitle: xAxisTitle(spec),
      yAxes: this.#yAxes,
      gaps: this.#gaps,
      valueBreaks: this.#valueBreaks,
      markers: this.#markers,
      current: this.#current,
      staleSeries: this.#staleSeries,
      cursor: cursorX === null ? null : { x: cursorX, label: formatX(spec, cursorX) },
      zoomBox: this.#zoomBox,
      emptyText: this.#xRange === null ? NO_DATA_TEXT : null,
      colors,
      font: this.#font,
    };
  }

  #drawChrome(): void {
    const ctx = this.#overCtx;
    const over = this.#over;
    const scene = this.#scene();
    if (over !== null) over.setAttribute('aria-label', this.#ariaLabel());
    if (ctx === null || over === null || scene === null) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, over.width, over.height);
    ctx.setTransform(this.#dpr, 0, 0, this.#dpr, 0, 0);
    drawOverlay(ctx, scene);
  }

  #ariaLabel(): string {
    const names = this.#config.series
      .filter((s) => s.enabled !== false)
      .map((s) => channelDef(s.channelId)?.label ?? `Channel ${s.channelId}`);
    const mode = xAxisTitle(X_AXIS_SPECS[this.#config.xMode]);
    return `Telemetry graph of ${names.join(', ')} over ${mode}. Arrow keys pan, plus and minus zoom, End returns to live.`;
  }

  // ---------------------------------------------------------------------------------------------------------
  // Readouts and snapshot
  // ---------------------------------------------------------------------------------------------------------

  #publish(): void {
    const prev = this.#snapshot;
    const cfg = this.#config;
    const spec = X_AXIS_SPECS[cfg.xMode];
    const readouts: Record<string, GraphReadout | null> = {};
    const feed = feedOf(cfg.source);
    const problem = this.#standingProblem();
    const status = this.#rendererStatus();
    const rendererNote = status.state === 'degraded' ? `${status.activeKind.toUpperCase()} fallback: ${status.reason ?? ''}` : null;
    let changed =
      prev.follow !== this.#view.follow ||
      prev.origin !== cfg.source.origin ||
      prev.health !== (problem === null ? 'ok' : 'error') ||
      prev.problem !== problem ||
      prev.feed !== feed ||
      prev.renderer !== status.activeKind ||
      prev.rendererNote !== rendererNote;
    const read = (key: string, source: GraphDataSource, window: GraphWindow | null | undefined, id: number): void => {
      const readout = this.#readout(source, window ?? null, id, this.#cursorX);
      readouts[key] = readout;
      if (!sameReadout(prev.readouts[key], readout)) changed = true;
    };
    for (const s of cfg.series) {
      if (s.enabled === false) readouts[seriesKey(s.channelId)] = null;
      else read(seriesKey(s.channelId), cfg.source, this.#primary, s.channelId);
    }
    for (const ref of cfg.references) read(referenceKey(ref.key), ref.source, this.#referenceWindows.get(ref.key), ref.channelId);
    if (Object.keys(prev.readouts).length !== Object.keys(readouts).length) changed = true;
    const axes: AxisSummary[] = this.#yAxes.map((a) => ({
      key: a.key,
      title: a.title,
      setting: a.setting,
      displayRange: a.range === null ? null : { min: a.range.min * a.display.factor, max: a.range.max * a.display.factor },
      offScale: a.offScale !== null,
    }));
    if (
      axes.length !== prev.axes.length ||
      axes.some((a, i) => {
        const p = prev.axes[i];
        return (
          p?.key !== a.key ||
          p.title !== a.title ||
          p.offScale !== a.offScale ||
          !sameSetting(p.setting, a.setting) ||
          !sameRange(p.displayRange, a.displayRange)
        );
      })
    ) {
      changed = true;
    }
    const cursorLabel = this.#cursorX === null ? null : formatX(spec, this.#cursorX);
    if (cursorLabel !== prev.cursorLabel) changed = true;
    if (!changed) return;
    this.#snapshot = {
      follow: this.#view.follow,
      origin: cfg.source.origin,
      health: problem === null ? 'ok' : 'error',
      feed,
      problem,
      renderer: status.activeKind,
      rendererNote,
      readouts,
      axes,
      cursorLabel,
    };
    for (const listener of [...this.#listeners]) listener();
  }

  /**
   * Readout of channel `id` of `source` (see `GraphReadout`):
   * - no cursor: a live source's newest sample exactly as the store reports it (per-channel freshness, in-band
   *   quality: the same rules as `ValueReadout`); a recording reads nothing.
   * - cursor on the newest data of a live source (its row is within `maxGapS` of the newest row): the value at the
   *   cursor with the channel's current state (STALE shows no value, DEGRADED is marked).
   * - cursor elsewhere: the recorded value, marked as history (or as a recording).
   */
  #readout(source: GraphDataSource, window: GraphWindow | null, id: number, at: number | null): GraphReadout | null {
    const def = channelDef(id);
    const live = source.origin === 'live';
    if (at === null) {
      if (!live) return null;
      return { kind: 'current', sample: source.latest(id) ?? readoutSample(id, def, 'noData', null) };
    }
    let kind: ReadoutKind = live ? 'history' : 'recorded';
    let latest: ChannelSample | null = null;
    if (live && window !== null) {
      const current = source.current();
      const rowT = rowTimeAt(window, at);
      if (current !== null && rowT !== null && current.t - rowT <= this.#config.maxGapS) {
        kind = 'current';
        latest = source.latest(id);
      }
    }
    if (def?.availability === Availability.NotProvided) return { kind, sample: readoutSample(id, def, 'notSupported', null) };
    if (latest !== null && latest.state !== 'live' && latest.state !== 'degraded') {
      // The newest data is not valid now (STALE, INVALID, UNAVAILABLE, …): say why, never show a value.
      return { kind, sample: readoutSample(id, def, latest.state, null) };
    }
    const column = window?.column(id) ?? null;
    if (window === null || column === null) return { kind, sample: readoutSample(id, def, 'unavailable', null) };
    const value = sampleAt(window, column, at, def?.interpolation === Interpolation.Discrete, this.#config.maxGapS);
    if (value === null) {
      const i = bracket(window, at);
      if (i >= 0) {
        const q = window.quality?.(id)?.[i];
        if (q === Quality.Invalid) return { kind, sample: readoutSample(id, def, 'invalid', null) };
        if (q === Quality.Unavailable) return { kind, sample: readoutSample(id, def, 'unavailable', null) };
        if (q === Quality.Stale) return { kind, sample: readoutSample(id, def, 'stale', null) };
        if (q === Quality.NotSupported) return { kind, sample: readoutSample(id, def, 'notSupported', null) };
      }
      return { kind, sample: readoutSample(id, def, 'noData', null) };
    }
    return { kind, sample: readoutSample(id, def, latest?.state ?? 'live', value) };
  }

  // ---------------------------------------------------------------------------------------------------------
  // Input
  // ---------------------------------------------------------------------------------------------------------

  /** Pixel x (relative to the drawing area) → x value, or null without a visible range. */
  #pxToX(px: number): number | null {
    const range = this.#xRange;
    const { plot } = this.#layout;
    if (range === null || !(plot.width > 0)) return null;
    return applyAffine(invertAffine(linearAffine(range.min, range.max, plot.left, plot.left + plot.width)), px);
  }

  #bindInput(el: HTMLCanvasElement): () => void {
    const localX = (e: { clientX: number }): number => e.clientX - el.getBoundingClientRect().left;
    // Environments without pointer capture (jsdom) simply skip it.
    const canCapture = typeof el.setPointerCapture === 'function' && typeof el.hasPointerCapture === 'function';
    const capture = (pointerId: number, on: boolean): void => {
      if (!canCapture) return;
      try {
        if (on) el.setPointerCapture(pointerId);
        else if (el.hasPointerCapture(pointerId)) el.releasePointerCapture(pointerId);
      } catch (error: unknown) {
        // Capture is a convenience (drags that leave the canvas): only its documented DOM failures (pointer no
        // longer active, element detached) are ignored; anything else is a real error (silent-failure L6).
        if (!(error instanceof DOMException)) throw error;
      }
    };
    const inPlot = (px: number): boolean => {
      const { plot } = this.#layout;
      return px >= plot.left && px <= plot.left + plot.width;
    };
    const onPointerDown = (e: PointerEvent): void => {
      const px = localX(e);
      if (!inPlot(px) || this.#xRange === null) return;
      const pan = e.button === POINTER_BUTTON_MIDDLE || (e.button === POINTER_BUTTON_PRIMARY && e.shiftKey);
      if (!pan && e.button !== POINTER_BUTTON_PRIMARY) return;
      e.preventDefault();
      el.focus();
      capture(e.pointerId, true);
      this.#drag = { pointerId: e.pointerId, mode: pan ? 'pan' : 'zoom', startPx: px, lastPx: px };
      if (!pan) this.#zoomBox = { x0: px, x1: px };
      this.#invalidate(false);
    };
    const onPointerMove = (e: PointerEvent): void => {
      const px = localX(e);
      const drag = this.#drag;
      if (drag?.pointerId === e.pointerId) {
        if (drag.mode === 'pan') {
          const range = this.#xRange;
          const action = range === null ? null : dragPanAction(px - drag.lastPx, range, this.#layout.plot.width);
          drag.lastPx = px;
          if (action !== null) this.dispatch(action);
        } else {
          drag.lastPx = px;
          this.#zoomBox = { x0: drag.startPx, x1: px };
          this.#invalidate(false);
        }
        return;
      }
      this.setCursor(inPlot(px) ? this.#pxToX(px) : null);
    };
    const endDrag = (e: PointerEvent, apply: boolean): void => {
      const drag = this.#drag;
      if (drag?.pointerId !== e.pointerId) return;
      this.#drag = null;
      this.#zoomBox = null;
      capture(e.pointerId, false);
      if (apply && drag.mode === 'zoom') {
        const x0 = this.#pxToX(drag.startPx);
        const x1 = this.#pxToX(localX(e));
        const action = x0 === null || x1 === null ? null : zoomBoxAction(x0, x1, localX(e) - drag.startPx);
        if (action !== null) this.dispatch(action);
      }
      this.#invalidate(false);
    };
    const onPointerUp = (e: PointerEvent): void => {
      endDrag(e, true);
    };
    const onPointerCancel = (e: PointerEvent): void => {
      endDrag(e, false);
    };
    const onPointerLeave = (): void => {
      if (this.#drag === null) this.setCursor(null);
    };
    const onWheel = (e: WheelEvent): void => {
      const px = localX(e);
      const range = this.#xRange;
      const anchor = this.#pxToX(px);
      if (range === null || anchor === null || !inPlot(px)) return;
      const action = wheelAction(e, anchor, range, this.#layout.plot.width);
      if (action === null) return;
      e.preventDefault();
      this.dispatch(action);
    };
    const onDoubleClick = (): void => {
      this.goLive();
    };
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && this.#drag !== null) {
        this.#drag = null;
        this.#zoomBox = null;
        this.#invalidate(false);
        return;
      }
      const range = this.#xRange;
      const action = range === null ? (e.key === 'End' ? ({ type: 'live' } as const) : null) : keyAction(e.key, range);
      if (action === null) return;
      e.preventDefault();
      this.dispatch(action);
    };
    el.addEventListener('pointerdown', onPointerDown);
    el.addEventListener('pointermove', onPointerMove);
    el.addEventListener('pointerup', onPointerUp);
    el.addEventListener('pointercancel', onPointerCancel);
    el.addEventListener('pointerleave', onPointerLeave);
    el.addEventListener('wheel', onWheel, { passive: false });
    el.addEventListener('dblclick', onDoubleClick);
    el.addEventListener('keydown', onKeyDown);
    return () => {
      el.removeEventListener('pointerdown', onPointerDown);
      el.removeEventListener('pointermove', onPointerMove);
      el.removeEventListener('pointerup', onPointerUp);
      el.removeEventListener('pointercancel', onPointerCancel);
      el.removeEventListener('pointerleave', onPointerLeave);
      el.removeEventListener('wheel', onWheel);
      el.removeEventListener('dblclick', onDoubleClick);
      el.removeEventListener('keydown', onKeyDown);
    };
  }
}
