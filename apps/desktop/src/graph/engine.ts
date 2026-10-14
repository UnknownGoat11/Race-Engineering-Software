/**
 * GraphEngine: one telemetry graph outside React (spec §61–63, §81–82, §121–123, §219, §277).
 *
 * The engine owns three stacked layers in its container: an under-canvas (grid, gap bands), the series layer of
 * a `SeriesRenderer` (Canvas2D by default, D-011) and an over-canvas (axes, markers, current-car marker, cursor,
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
import { Availability, Interpolation, Unit, type ChannelDef } from '../generated/channels';
import { channelAxisDisplay, unitAxisDisplay, type AxisDisplay } from '../lib/units';
import { SPEC_DEGRADED_AFTER_MS } from '../telemetry/freshness';
import { channelDef } from '../telemetry/registry';
import type { ChannelSample, SampleState, ScheduleFrame } from '../telemetry/store';
import { browserContext, sizeCanvas, type Canvas2DLike, type ContextFactory } from './canvasContext';
import { Canvas2DRenderer } from './canvas2dRenderer';
import { findTimeGaps } from './decimate';
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
import { MARKER_LABEL_GAP_CSS, MARKER_ROWS, drawOverlay, drawUnderlay, type GraphColors, type OverlayScene } from './overlay';
import type { RenderSeries, SeriesRenderer } from './renderer';
import { nearestRowX, sampleAt, visibleRows } from './sample';
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
  /** Series renderer: a kind, or a factory (called on every attach). Default 'canvas2d' (D-011 bench: see renderer.ts). */
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
}

/** A y axis as the legend shows it. */
export interface AxisSummary {
  readonly key: string;
  readonly title: string;
  readonly setting: YScaleSetting;
  /** Current range in display units (null without data), so the UI can fix the scale where it is. */
  readonly displayRange: XRange | null;
}

/** What the React layer shows next to the canvas. Immutable; replaced only when something in it changed. */
export interface GraphSnapshot {
  /** LIVE (following the newest data / whole lap) or paused / zoomed. */
  readonly follow: boolean;
  readonly origin: DataOrigin;
  /** Readout per series key (`seriesKey` / `referenceKey`); null when there is nothing to read (no cursor). */
  readonly readouts: Readonly<Record<string, ChannelSample | null>>;
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

function sameSample(a: ChannelSample | null | undefined, b: ChannelSample | null): boolean {
  if (a === undefined) return false;
  if (a === null || b === null) return a === b;
  return a.id === b.id && a.state === b.state && a.value === b.value;
}

function sameRange(a: XRange | null, b: XRange | null): boolean {
  return a === b || (a !== null && b !== null && a.min === b.min && a.max === b.max);
}

function sameSetting(a: YScaleSetting, b: YScaleSetting): boolean {
  if (a.mode !== b.mode) return false;
  return a.mode === 'auto' || (b.mode === 'fixed' && a.min === b.min && a.max === b.max);
}

let nextEngineId = 0;

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

  #snapshot: GraphSnapshot;
  readonly #listeners = new Set<() => void>();

  constructor(options: GraphEngineOptions) {
    this.id = options.id ?? `graph-${nextEngineId++}`;
    this.#config = {
      source: options.source,
      xMode: options.xMode,
      series: options.series,
      references: options.references ?? [],
      markers: options.markers ?? [],
      yScales: options.yScales ?? {},
      maxGapS: options.maxGapS ?? DEFAULT_MAX_GAP_S,
      xDomain: options.xDomain ?? null,
      syncGroup: options.syncGroup ?? null,
    };
    this.#rendererOption = options.renderer ?? DEFAULT_RENDERER;
    this.#initialSpan = options.initialSpan;
    this.#scheduleFrame = options.scheduleFrame ?? sharedFrameLoop();
    this.#contextFactory = options.context ?? browserContext;
    this.#now = options.now ?? (() => performance.now());
    this.#view = this.#initialView();
    this.#snapshot = { follow: this.#view.follow, origin: this.#config.source.origin, readouts: {}, axes: [], cursorLabel: null };
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
    const next: GraphConfig = { ...prev, ...change };
    this.#config = next;
    if (next.source !== prev.source || next.xMode !== prev.xMode) {
      this.#view = this.#initialView();
      this.#cursorX = null;
    }
    const resubscribe =
      next.source !== prev.source || next.references !== prev.references || next.syncGroup !== prev.syncGroup;
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
    const group = this.#config.syncGroup;
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
    this.#resolveTheme(container);
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

  #resolveTheme(container: HTMLElement): void {
    const style = getComputedStyle(container);
    const token = (name: string): string => style.getPropertyValue(name).trim();
    this.#colors = {
      grid: token('--graph-grid'),
      axis: token('--graph-axis'),
      axisText: token('--graph-axis-text'),
      cursor: token('--graph-cursor'),
      cursorText: token('--graph-cursor-text'),
      zoomFill: token('--graph-zoom-fill'),
      zoomStroke: token('--graph-zoom-stroke'),
      markers: {
        lap: token('--graph-marker-lap'),
        sector: token('--graph-marker-sector'),
        event: token('--graph-marker-event'),
      },
      currentCar: token('--graph-current-car'),
      currentCarStale: token('--graph-current-car-stale'),
      gapFill: token('--graph-gap-fill'),
      gapText: token('--graph-gap-text'),
    };
    this.#seriesColors = Array.from({ length: SERIES_PALETTE_SIZE }, (_, i) => token(`--graph-series-${i + 1}`));
    this.#referenceColor = token('--graph-reference');
    const size = token('--font-size-xs');
    const family = token('--font-ui');
    this.#font = `${size === '' ? FALLBACK_FONT_SIZE : size} ${family === '' ? FALLBACK_FONT_FAMILY : family}`;
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
    if (group !== null) {
      offs.push(group.subscribe((event) => {
        this.#onSync(event);
      }));
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

  #setView(view: ViewState): void {
    if (view === this.#view) return;
    this.#view = view;
    this.#config.syncGroup?.setView(this.id, view);
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
    // The range can move before the source's change notification arrives; the series must follow it.
    if (!sameRange(drawnRange, this.#xRange)) drawSeries = true;
    if (drawSeries) this.#drawData();
    this.#drawChrome();
    this.#publish();
    this.frameTimes.record(this.#now() - start);
  }

  /** Resolves the view against the data and reads the visible windows. */
  #read(): void {
    const bounds = this.#bounds();
    let view = this.#view;
    if (!Number.isFinite(view.span) && bounds.extent !== null) view = reduceView(view, { type: 'setSpan', span: bounds.maxSpan }, bounds);
    view = reduceView(view, { type: 'clamp' }, bounds);
    if (view !== this.#view) {
      this.#view = view;
      this.#config.syncGroup?.setView(this.id, view);
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
    const add = (key: string, window: GraphWindow | null | undefined, id: number, color: string, dashed: boolean): void => {
      const def = channelDef(id);
      const display = displayOf(def);
      const ys = window?.column(id) ?? null;
      let extent: XRange | null = null;
      if (window != null && ys !== null && xRange !== null) {
        const rows = visibleRows(window, xRange);
        extent = dataExtent(ys, rows.start, rows.end, window.xs, xRange);
        pending.push({ key, window, ys, axis: axisKey(display), color, dashed, step: def?.interpolation === Interpolation.Discrete });
      }
      members.push({ display, label: def?.label ?? `Channel ${id}`, color, extent });
    };
    cfg.series.forEach((s, i) => {
      if (s.enabled === false) return;
      add(seriesKey(s.channelId), primary, s.channelId, this.#seriesColor(s, i), false);
    });
    for (const ref of cfg.references) {
      const color = ref.color === undefined ? this.#referenceColor : this.#seriesColor({ channelId: ref.channelId, color: ref.color }, 0);
      add(referenceKey(ref.key), this.#referenceWindows.get(ref.key), ref.channelId, color, true);
    }
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
    if (primary !== null && xRange !== null) {
      const rows = visibleRows(primary, xRange);
      findTimeGaps(primary.xs, primary.ts, rows.start, rows.end, maxGapT, this.#gaps);
    }
    this.#current = cfg.source.origin === 'live' ? cfg.source.current() : null;
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
      markers: this.#markers,
      current: this.#current,
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
    const live = cfg.source.origin === 'live';
    const current = live ? cfg.source.current() : null;
    // Read at the cursor; without one, live graphs read the newest sample (the current car position).
    const at = this.#cursorX ?? current?.x ?? null;
    // The newest sample carries the feed's freshness (spec §126); history at a cursor is what it was.
    let freshness: SampleState = 'live';
    if (this.#cursorX === null && current !== null) {
      if (current.stale) freshness = 'stale';
      else if (current.degraded) freshness = 'degraded';
    }
    const readouts: Record<string, ChannelSample | null> = {};
    let changed = prev.follow !== this.#view.follow || prev.origin !== cfg.source.origin;
    const read = (key: string, window: GraphWindow | null | undefined, id: number): void => {
      const sample = this.#readout(window ?? null, id, at, freshness);
      readouts[key] = sample;
      if (!sameSample(prev.readouts[key], sample)) changed = true;
    };
    for (const s of cfg.series) {
      if (s.enabled === false) readouts[seriesKey(s.channelId)] = null;
      else read(seriesKey(s.channelId), this.#primary, s.channelId);
    }
    for (const ref of cfg.references) read(referenceKey(ref.key), this.#referenceWindows.get(ref.key), ref.channelId);
    if (Object.keys(prev.readouts).length !== Object.keys(readouts).length) changed = true;
    const axes: AxisSummary[] = this.#yAxes.map((a) => ({
      key: a.key,
      title: a.title,
      setting: a.setting,
      displayRange: a.range === null ? null : { min: a.range.min * a.display.factor, max: a.range.max * a.display.factor },
    }));
    if (
      axes.length !== prev.axes.length ||
      axes.some((a, i) => {
        const p = prev.axes[i];
        return p?.key !== a.key || p.title !== a.title || !sameSetting(p.setting, a.setting) || !sameRange(p.displayRange, a.displayRange);
      })
    ) {
      changed = true;
    }
    const cursorLabel = this.#cursorX === null ? null : formatX(spec, this.#cursorX);
    if (cursorLabel !== prev.cursorLabel) changed = true;
    if (!changed) return;
    this.#snapshot = { follow: this.#view.follow, origin: cfg.source.origin, readouts, axes, cursorLabel };
    for (const listener of [...this.#listeners]) listener();
  }

  /** Readout of channel `id` at x `at`; `freshness` is 'live', or 'stale' / 'degraded' for the newest sample. */
  #readout(
    window: GraphWindow | null,
    id: number,
    at: number | null,
    freshness: SampleState,
  ): ChannelSample | null {
    const def = channelDef(id);
    if (at === null) return null;
    if (def?.availability === Availability.NotProvided) return readoutSample(id, def, 'notSupported', null);
    if (freshness === 'stale') return readoutSample(id, def, 'stale', null);
    const column = window?.column(id) ?? null;
    if (window === null || column === null) return readoutSample(id, def, 'unavailable', null);
    const value = sampleAt(window, column, at, def?.interpolation === Interpolation.Discrete, this.#config.maxGapS);
    return value === null ? readoutSample(id, def, 'noData', null) : readoutSample(id, def, freshness, value);
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
    const capture = (pointerId: number, on: boolean): void => {
      try {
        if (on) el.setPointerCapture(pointerId);
        else if (el.hasPointerCapture(pointerId)) el.releasePointerCapture(pointerId);
      } catch {
        // Capture is a convenience (drags that leave the canvas); environments without it still work.
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
