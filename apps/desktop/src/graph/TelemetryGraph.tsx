import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type JSX } from 'react';
import { ProvenanceBadge } from '../components/telemetry/ProvenanceBadge';
import { SampleReadout } from '../components/telemetry/ValueReadout';
import { channelDef } from '../telemetry/registry';
import type { ScheduleFrame } from '../telemetry/store';
import { GraphEngine, referenceKey, seriesColorSlot, seriesKey, type AxisSummary, type GraphReadout } from './engine';
import { GRAPH_SOURCE_LABELS, graphSourceText, seriesSourceLabel, type GraphSourceLabel } from './provenance';
import type { SeriesRenderer } from './renderer';
import type { GraphSyncGroup } from './syncBus';
import type {
  GraphDataSource,
  GraphMarker,
  GraphSeries,
  ReferenceSeries,
  RendererKind,
  XAxisMode,
  XRange,
  YScaleSetting,
} from './types';
import { X_AXIS_SPECS } from './xAxis';
import './graph.css';

/** Props of `TelemetryGraph`. Data-shaping props may change at any time; renderer and scheduler are fixed. */
export interface TelemetryGraphProps {
  /** Rows to plot: `LiveRingSource` (live ring) or `StaticSource` (recording). Use one source per graph. */
  readonly source: GraphDataSource;
  readonly xMode: XAxisMode;
  /** Channels to plot; `enabled: false` starts a channel hidden (the legend toggles it). */
  readonly series: readonly GraphSeries[];
  /** Dashed overlays, e.g. a reference lap. */
  readonly references?: readonly ReferenceSeries[];
  /** Sector / lap / event markers. */
  readonly markers?: readonly GraphMarker[];
  /** Graphs sharing cursor and x view (spec §81). */
  readonly syncGroup?: GraphSyncGroup;
  /** Initial scale per y axis key (`axisKey`); the legend's axis buttons switch AUTO / FIXED afterwards. */
  readonly yScales?: Readonly<Record<string, YScaleSetting>>;
  /** Largest time step (s) a line may bridge; default 0.25 s (spec §126 DEGRADED threshold). */
  readonly maxGapS?: number;
  /** Domain of a lap axis, e.g. `{ min: 0, max: trackLengthM }`. */
  readonly xDomain?: XRange;
  /** Series renderer, fixed for the component's lifetime (remount with a `key` to change it). Default 'webgl' (DEFAULT_RENDERER). */
  readonly renderer?: RendererKind | (() => SeriesRenderer);
  /** Initial span of a scrolling axis (x units). */
  readonly initialSpan?: number;
  /** Frame scheduler, fixed for the component's lifetime (default: the shared animation-frame loop). */
  readonly scheduleFrame?: ScheduleFrame;
  /** Heading shown above the legend. */
  readonly title?: string;
  readonly className?: string;
  /** Receives the engine once mounted and null on unmount (diagnostics, benchmark). */
  readonly onEngine?: (engine: GraphEngine | null) => void;
  /** Optional callback when a channel series visibility is toggled on/off (MoTeC i2 style). */
  readonly onToggleSeries?: (channelId: number, enabled: boolean) => void;
}

const NO_REFERENCES: readonly ReferenceSeries[] = [];
const NO_MARKERS: readonly GraphMarker[] = [];
const NO_SCALES: Readonly<Record<string, YScaleSetting>> = {};
const AUTO: YScaleSetting = { mode: 'auto' };

/** Label of the view state for a live graph (spec §63) and for zoomed recordings. */
const MODE_TEXT = {
  live: 'LIVE',
  paused: 'PAUSED · scrolled back',
  zoomed: 'ZOOMED',
  error: 'GRAPH ERROR',
} as const;

/** Header text of a live feed that is not current (silent-failure H1: LIVE only while telemetry is fresh). */
const FEED_TEXT = {
  degraded: 'DEGRADED',
  stale: 'STALE',
  noData: 'NO DATA',
} as const;

/** Tag of a readout that is not the newest data (silent-failure H3: history never reads as current). */
const READOUT_KIND_TEXT = { history: 'HISTORY', recorded: 'RECORDED' } as const;

/** A legend readout: the sample, plus "HISTORY @ time" / "RECORDED @ time" when it is not the newest data. */
function LegendReadout({ readout, label, at }: { readout: GraphReadout; label: string; at: string | null }): JSX.Element {
  if (readout.kind === 'current') return <SampleReadout sample={readout.sample} label={label} />;
  const tag = `${READOUT_KIND_TEXT[readout.kind]}${at === null ? '' : ` @ ${at}`}`;
  return (
    <span className="telemetry-graph__readout" data-kind={readout.kind}>
      <SampleReadout sample={readout.sample} label={label} />
      <span className="telemetry-graph__readout-at">{tag}</span>
    </span>
  );
}

/**
 * Badge of a series that is not LMU data, shown INSTEAD of its MEASURED / DERIVED badge, whose meaning ("Directly
 * from LMU") would be false (silent-failure H4, spec §108).
 */
const NON_LMU_BADGES: Readonly<Partial<Record<GraphSourceLabel, { readonly label: string; readonly description: string }>>> = {
  [GRAPH_SOURCE_LABELS.synthetic]: { label: 'SYNTHETIC', description: 'Generated test data, not from LMU' },
  [GRAPH_SOURCE_LABELS.unverified]: { label: 'UNVERIFIED', description: 'Source not confirmed as LMU data' },
};

/** The provenance badge of a series: MEASURED / DERIVED / ESTIMATED for LMU data, SYNTHETIC / UNVERIFIED otherwise. */
function SeriesBadge({ id, sourceLabel }: { id: number; sourceLabel: GraphSourceLabel }): JSX.Element | null {
  const nonLmu = NON_LMU_BADGES[sourceLabel];
  if (nonLmu !== undefined) {
    return (
      <span className="badge telemetry-graph__data-badge" title={nonLmu.description}>
        {nonLmu.label}
        <span className="telemetry-visually-hidden">: {nonLmu.description}</span>
      </span>
    );
  }
  const def = channelDef(id);
  return def === undefined ? null : <ProvenanceBadge provenance={def.provenance} />;
}

function channelLabel(id: number): string {
  return channelDef(id)?.label ?? `Channel ${id}`;
}

/**
 * A telemetry graph (spec §61–63, §81–82, §317): canvas plot driven by a `GraphEngine` (no per-point DOM, repaint
 * on animation frames only when something changed) with a legend of channel toggles, provenance badges and
 * readouts at the cursor, the data source line, the LIVE / PAUSED indicator and per-axis AUTO / FIXED scale.
 * High-rate data never passes through React state: the legend re-renders only when its snapshot changes.
 */
export function TelemetryGraph(props: TelemetryGraphProps): JSX.Element {
  const {
    source,
    xMode,
    series,
    references = NO_REFERENCES,
    markers = NO_MARKERS,
    syncGroup,
    maxGapS,
    xDomain,
    title,
    className,
    onEngine,
  } = props;
  const plotRef = useRef<HTMLDivElement>(null);
  const [engine] = useState(
    () =>
      new GraphEngine({
        source,
        xMode,
        series,
        references,
        markers,
        ...(props.renderer === undefined ? {} : { renderer: props.renderer }),
        ...(props.initialSpan === undefined ? {} : { initialSpan: props.initialSpan }),
        ...(props.scheduleFrame === undefined ? {} : { scheduleFrame: props.scheduleFrame }),
      }),
  );
  const [disabled, setDisabled] = useState<ReadonlySet<number>>(
    () => new Set(series.filter((s) => s.enabled === false).map((s) => s.channelId)),
  );
  const seriesIdsKey = useMemo(() => series.map((s) => `${s.channelId}:${s.enabled ?? true}`).join(','), [series]);
  const prevSeriesIdsKey = useRef(seriesIdsKey);
  useEffect(() => {
    if (prevSeriesIdsKey.current !== seriesIdsKey) {
      prevSeriesIdsKey.current = seriesIdsKey;
      setDisabled(new Set(series.filter((s) => s.enabled === false).map((s) => s.channelId)));
    }
  }, [series, seriesIdsKey]);
  const [yScales, setYScales] = useState<Readonly<Record<string, YScaleSetting>>>(props.yScales ?? NO_SCALES);
  const effectiveSeries = useMemo(
    () => series.map((s) => ({ ...s, enabled: !disabled.has(s.channelId) })),
    [series, disabled],
  );

  useEffect(() => {
    const el = plotRef.current;
    if (el === null) return undefined;
    return engine.attach(el);
  }, [engine]);

  useEffect(() => {
    engine.update({
      source,
      xMode,
      series: effectiveSeries,
      references,
      markers,
      yScales,
      syncGroup: syncGroup ?? null,
      xDomain: xDomain ?? null,
      ...(maxGapS === undefined ? {} : { maxGapS }),
    });
  }, [engine, source, xMode, effectiveSeries, references, markers, yScales, syncGroup, xDomain, maxGapS]);

  useEffect(() => {
    if (onEngine === undefined) return undefined;
    onEngine(engine);
    return () => {
      onEngine(null);
    };
  }, [engine, onEngine]);

  const snapshot = useSyncExternalStore(engine.subscribe, engine.getSnapshot);
  const scrolling = X_AXIS_SPECS[xMode].viewKind === 'scroll';
  const live = snapshot.origin === 'live';
  const labelOf = (from: GraphDataSource, id: number): GraphSourceLabel =>
    seriesSourceLabel(from.origin, channelDef(id), from.dataKind);
  const failed = snapshot.health === 'error';
  const feed = snapshot.feed;
  const feedText = feed === null || feed === 'live' ? null : FEED_TEXT[feed];
  let modeText: string | null;
  let modeKind: string;
  if (failed) {
    modeText = MODE_TEXT.error;
    modeKind = 'error';
  } else if (live && feedText !== null && feed !== null) {
    // The feed is not current: never LIVE, whatever the view does.
    modeText = snapshot.follow ? feedText : `${scrolling ? MODE_TEXT.paused : MODE_TEXT.zoomed} · ${feedText}`;
    modeKind = feed;
  } else if (snapshot.follow) {
    const nonLmu = NON_LMU_BADGES[labelOf(source, series[0]?.channelId ?? -1)];
    modeText = live ? (nonLmu === undefined ? MODE_TEXT.live : `${MODE_TEXT.live} · ${nonLmu.label}`) : null;
    modeKind = 'live';
  } else {
    modeText = live && scrolling ? MODE_TEXT.paused : MODE_TEXT.zoomed;
    modeKind = live && scrolling ? 'paused' : 'zoomed';
  }

  const sourceLabels: GraphSourceLabel[] = [
    ...series.map((s) => labelOf(source, s.channelId)),
    ...references.map((r) => labelOf(r.source, r.channelId)),
  ];

  // DS04: Render one provenance pill per graph; per-series chips only when a series differs.
  const firstSourceLabel = sourceLabels[0];
  const isUniformSource = firstSourceLabel !== undefined && sourceLabels.every((l) => l === firstSourceLabel);
  const commonSourceLabel = isUniformSource ? firstSourceLabel : null;
  const commonNonLmu = commonSourceLabel !== null ? NON_LMU_BADGES[commonSourceLabel] : undefined;

  const toggle = (id: number): void => {
    setDisabled((prev) => {
      const next = new Set(prev);
      const willBeEnabled = next.has(id);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      props.onToggleSeries?.(id, willBeEnabled);
      return next;
    });
  };

  const toggleScale = (axis: AxisSummary): void => {
    const range = axis.displayRange;
    setYScales((prev) => ({
      ...prev,
      [axis.key]: axis.setting.mode === 'fixed' || range === null ? AUTO : { mode: 'fixed', min: range.min, max: range.max },
    }));
  };

  return (
    <section className={className === undefined ? 'telemetry-graph' : `telemetry-graph ${className}`} aria-label={title ?? 'Telemetry graph'}>
      <header className="telemetry-graph__header">
        {title !== undefined && <h3 className="telemetry-graph__title">{title}</h3>}
        <span className="telemetry-graph__source">{graphSourceText(sourceLabels)}</span>
        {snapshot.cursorLabel !== null && (
          <span className="telemetry-graph__cursor" aria-live="off">
            Cursor {snapshot.cursorLabel}
          </span>
        )}
        {snapshot.rendererNote !== null && (
          <span className="telemetry-graph__renderer-note" title={snapshot.rendererNote}>
            {snapshot.rendererNote}
          </span>
        )}
        {modeText !== null && (
          <span className="telemetry-graph__mode" data-mode={modeKind} role="status">
            {modeText}
          </span>
        )}
        {!snapshot.follow && (
          <button
            type="button"
            className="telemetry-graph__button"
            onClick={() => {
              engine.goLive();
            }}
          >
            {live ? 'Back to live' : 'Reset zoom'}
          </button>
        )}
      </header>
      {failed && (
        <div className="telemetry-graph__problem" role="alert">
          <span>Graph unavailable: {snapshot.problem}</span>
          <button
            type="button"
            className="telemetry-graph__button"
            onClick={() => {
              engine.retry();
            }}
          >
            Retry
          </button>
        </div>
      )}
      <ul className="telemetry-graph__legend" aria-label="Channels">
        {commonNonLmu !== undefined && (
          <li className="telemetry-graph__legend-common" key="common-provenance">
            <span className="badge telemetry-graph__data-badge" title={commonNonLmu.description}>
              {commonNonLmu.label}
              <span className="telemetry-visually-hidden">: {commonNonLmu.description}</span>
            </span>
          </li>
        )}
        {series.map((s, i) => {
          const enabled = !disabled.has(s.channelId);
          const label = channelLabel(s.channelId);
          const sample = snapshot.readouts[seriesKey(s.channelId)] ?? null;
          const sLabel = labelOf(source, s.channelId);
          const showBadge = commonNonLmu === undefined;
          return (
            <li
              key={s.channelId}
              className="telemetry-graph__legend-item"
              data-enabled={enabled}
              title={`Source: ${sLabel} · Click to ${enabled ? 'hide' : 'show'}`}
            >
              <label className="telemetry-graph__toggle" title={enabled ? `Hide ${label}` : `Show ${label}`}>
                <input
                  type="checkbox"
                  checked={enabled}
                  onChange={() => {
                    toggle(s.channelId);
                  }}
                  aria-label={`Show ${label}`}
                />
                <span className="telemetry-graph__swatch" data-slot={seriesColorSlot(s, i)} aria-hidden="true" />
              </label>
              {enabled && sample !== null ? (
                <span
                  className="telemetry-graph__readout-click"
                  onClick={() => {
                    toggle(s.channelId);
                  }}
                  title={`Click to hide ${label}`}
                >
                  <LegendReadout readout={sample} label={label} at={snapshot.cursorLabel} />
                </span>
              ) : (
                <span
                  className="telemetry-graph__series-name"
                  onClick={() => {
                    toggle(s.channelId);
                  }}
                  title={`Click to show ${label}`}
                >
                  {label}
                </span>
              )}
              {showBadge && <SeriesBadge id={s.channelId} sourceLabel={sLabel} />}
            </li>
          );
        })}
        {references.map((r) => {
          const sample = snapshot.readouts[referenceKey(r.key)] ?? null;
          const rLabel = labelOf(r.source, r.channelId);
          const showRefBadge = commonNonLmu === undefined || rLabel !== commonSourceLabel;
          return (
            <li
              key={`ref-${r.key}`}
              className="telemetry-graph__legend-item"
              data-reference="true"
              title={`Source: ${rLabel}`}
            >
              <span
                className="telemetry-graph__swatch telemetry-graph__swatch--reference"
                {...(r.color === undefined ? {} : { 'data-slot': r.color })}
                aria-hidden="true"
              />
              {sample !== null ? (
                <LegendReadout readout={sample} label={r.label} at={snapshot.cursorLabel} />
              ) : (
                <span className="telemetry-graph__series-name">{r.label}</span>
              )}
              {showRefBadge && <SeriesBadge id={r.channelId} sourceLabel={rLabel} />}
            </li>
          );
        })}
      </ul>
      {snapshot.axes.length > 0 && (
        <div className="telemetry-graph__axes" role="group" aria-label="Y axis scales">
          {snapshot.axes.map((axis) => {
            const fixed = axis.setting.mode === 'fixed';
            return (
              <button
                key={axis.key}
                type="button"
                className="telemetry-graph__button telemetry-graph__axis"
                aria-pressed={fixed}
                data-off-scale={axis.offScale}
                disabled={!fixed && axis.displayRange === null}
                title={fixed ? 'Fixed scale: click to autoscale' : 'Autoscale: click to fix the current scale'}
                onClick={() => {
                  toggleScale(axis);
                }}
              >
                {axis.title} {fixed ? (axis.offScale ? 'FIXED · OFF-SCALE' : 'FIXED') : 'AUTO'}
              </button>
            );
          })}
        </div>
      )}
      <div ref={plotRef} className="telemetry-graph__plot" />
    </section>
  );
}
