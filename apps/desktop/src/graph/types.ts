/**
 * Shared types of the telemetry graph engine (spec §61–63, §81–82, §219, §277, §317–318, D-011).
 *
 * Units: x values are in the x axis mode's internal unit (seconds or metres or lap fraction), y values in each
 * channel's internal (SI) unit; conversion to display units happens only through `lib/units.ts`.
 */

/** X axis modes (spec §62). The x value of every row comes from the caller's data source. */
export type XAxisMode = 'time' | 'lapTime' | 'lapDistance' | 'lapFraction';

/** Every x axis mode, in menu order. */
export const X_AXIS_MODES: readonly XAxisMode[] = ['time', 'lapTime', 'lapDistance', 'lapFraction'];

/** A closed x (or value) interval with `min < max`. */
export interface XRange {
  readonly min: number;
  readonly max: number;
}

/** Where a graph's data comes from (spec §317 "Source: …"). */
export type DataOrigin = 'live' | 'recording';

/**
 * The rows a source returns for one visible range: shared x and time columns plus one value column per
 * requested channel, valid in `[0, count)`. Arrays are owned and reused by the source: they are valid until its
 * next `read` and must not be kept.
 */
export interface GraphWindow {
  /** X value of each row in the axis mode's unit; NaN where the row has no x (breaks the line). */
  readonly xs: Float64Array;
  /** Session time of each row in seconds (the same array as `xs` on a time axis); used for gap detection. */
  readonly ts: Float64Array;
  /** Number of valid rows. */
  readonly count: number;
  /** True when `xs` never decreases, so lookups may binary-search. */
  readonly monotonicX: boolean;
  /** Values of `channelId` per row (NaN = no valid value at that row), or null if it was not requested/known. */
  column(channelId: number): Float64Array | null;
}

/** The current car position on the x axis (spec §82) and the freshness of the feed behind it (spec §126). */
export interface CurrentPosition {
  readonly x: number;
  /** The feed is STALE (or has no data): the position is not current. */
  readonly stale: boolean;
  /** The feed is DEGRADED: the position is late but still shown. */
  readonly degraded: boolean;
}

/** Supplies graph rows. Implementations: `LiveRingSource` (TelemetryStore ring) and `StaticSource` (arrays). */
export interface GraphDataSource {
  readonly origin: DataOrigin;
  /**
   * The rows are generated test data (development / demo mode, spec §108–§109): labelled "Synthetic Test Data",
   * never as LMU telemetry or a session recording. Absent = false.
   */
  readonly synthetic?: boolean;
  /** Calls `listener` when data or freshness changed; returns the unsubscribe function. */
  subscribe(listener: () => void): () => void;
  /** X extent of the available rows, or null when there are none. */
  extent(): XRange | null;
  /**
   * Rows needed to draw `range`: every row inside it plus the nearest row beyond each edge (so lines reach the
   * plot edges). Copies at most that window; the returned arrays are reused by the next call.
   */
  read(range: XRange, channelIds: readonly number[]): GraphWindow;
  /** Current car position for live sources; null for recordings or before data. */
  current(): CurrentPosition | null;
}

/** One plotted channel. */
export interface GraphSeries {
  readonly channelId: number;
  /** Palette slot 1…8 (tokens `--graph-series-N`); defaults to the series' position. */
  readonly color?: number;
  /** Shown initially (channel enable/disable, spec §62); default true. */
  readonly enabled?: boolean;
}

/** A dashed overlay of one channel from another source, e.g. a reference lap (spec §62 reference overlay). */
export interface ReferenceSeries {
  /** Stable key, unique within the graph. */
  readonly key: string;
  /** Legend label, e.g. "Reference lap 12". */
  readonly label: string;
  readonly channelId: number;
  readonly source: GraphDataSource;
  /** Palette slot 1…8; default the neutral `--graph-reference` colour. */
  readonly color?: number;
}

/** Vertical marker kinds (spec §62 sector / lap / event markers). */
export type MarkerKind = 'sector' | 'lap' | 'event';

/** A labelled vertical line at `x`. */
export interface GraphMarker {
  readonly x: number;
  readonly kind: MarkerKind;
  readonly label: string;
}

/**
 * Scale of one y axis (spec §62 autoscale / fixed scale). Fixed bounds are in the axis' DISPLAY units (what its
 * tick labels show, e.g. km/h), not in internal units.
 */
export type YScaleSetting =
  | { readonly mode: 'auto' }
  | { readonly mode: 'fixed'; readonly min: number; readonly max: number };

/** Which renderer draws the series layer (D-011: uPlot provisional, benchmarked against Canvas2D). */
export type RendererKind = 'uplot' | 'canvas2d';
