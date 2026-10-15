/**
 * Telemetry graph engine (spec §61–63, §81–82, §121–125, §219, §277, §317–318; D-011, D-023).
 *
 * Typical use: one `LiveRingSource` per graph over the app's TelemetryStore, one `GraphSyncGroup` per group of
 * graphs, and a `TelemetryGraph` per graph. Recordings and reference laps use `StaticSource`.
 */
export { TelemetryGraph, type TelemetryGraphProps } from './TelemetryGraph';
export {
  DEFAULT_LIVE_SPAN_S,
  DEFAULT_MAX_GAP_S,
  GRAPH_ERROR_TEXT,
  GraphEngine,
  NO_DATA_TEXT,
  SERIES_PALETTE_SIZE,
  THEME_FALLBACK,
  cssColorIsValid,
  referenceKey,
  seriesColorSlot,
  seriesKey,
  type AxisSummary,
  type FeedState,
  type GraphConfig,
  type GraphHealth,
  type GraphReadout,
  type ReadoutKind,
  type GraphEngineOptions,
  type GraphSnapshot,
} from './engine';
export {
  LiveRingSource,
  advanceLapScan,
  newLapScan,
  type LapScanState,
  type LiveRingSourceOptions,
} from './liveSource';
export { StaticSource, type StaticSourceData } from './staticSource';
export { GraphSyncGroup, SyncGroupMismatchError, type SyncEvent, type SyncListener } from './syncBus';
export { createFrameLoop, sharedFrameLoop, type FrameLoopHost } from './frameLoop';
export { DurationRecorder, percentile, summarize, type DurationSummary } from './frameStats';
export { Canvas2DRenderer, INVALID_COLOR_STROKE, drawSeriesFrame, type Canvas2DRendererOptions } from './canvas2dRenderer';
export {
  WebGLRenderer,
  StripBuilder,
  browserColorParser,
  canvasAcceptsColor,
  type ColorParser,
  type Rgba,
  type WebGLRendererOptions,
} from './webglRenderer';
export { UplotRenderer, alignedBuffers, fillAlignedData, type AlignedBuffers } from './uplotRenderer';
export {
  REFERENCE_DASH_CSS,
  SERIES_LINE_WIDTH_DEVICE_PX,
  okStatus,
  type PlotRect,
  type RendererStatus,
  type RenderFrame,
  type RenderSeries,
  type SeriesRenderer,
} from './renderer';
export { axisKey, offScaleOf, type OffScale, type YAxisModel } from './layout';
export { GRAPH_SOURCE_LABELS, checkDataKind, graphSourceText, isLmuData, seriesSourceLabel, type GraphSourceLabel } from './provenance';
export { X_AXIS_SPECS, formatX, xAxisTitle, type XAxisSpec } from './xAxis';
export { liveView, reduceView, resolveRange, type ViewAction, type ViewBounds, type ViewState } from './view';
export {
  DEFAULT_RENDERER,
  X_AXIS_MODES,
  type CurrentPosition,
  type DataKind,
  type DataOrigin,
  type GraphDataSource,
  type GraphMarker,
  type GraphSeries,
  type GraphWindow,
  type MarkerKind,
  type ReferenceSeries,
  type RendererKind,
  type XAxisMode,
  type XRange,
  type YScaleSetting,
} from './types';
