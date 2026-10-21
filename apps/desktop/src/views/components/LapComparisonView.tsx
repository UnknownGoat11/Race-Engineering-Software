import { useMemo, useState, useRef, type JSX, type MouseEvent } from 'react';
import { ch } from '../../generated/channels';
import { MotecImportBar } from '../../motec/MotecImportBar';
import { extractMotecLap } from '../../motec/motecParser';
import { sampleMotecAtDistance } from '../../motec/motecSource';
import type { MotecLap, MotecLapView } from '../../motec/motecTypes';
import { useMotecSession } from '../../motec/useMotecStore';
import { formatDelta, formatLapTime, isFiniteNumber, mpsToKmh, NO_VALUE } from '../../lib/units';
import '../../styles/common.css';

/**
 * Lap comparison of real data only: the reference is the quickest valid lap of the imported MoTeC file; the
 * comparison is another valid lap of the same file chosen by the user. Without a second lap, comparison values
 * and the time delta show "no data". Only channels that exist in the file are drawn.
 */

type ViewMode = 'graphs-only' | 'split' | 'table-only';

/** Points sampled along the lap distance for the SVG traces. */
const POINTS = 400;

interface ZoomFraction {
  readonly min: number;
  readonly max: number;
}

const FULL: ZoomFraction = { min: 0, max: 1 };
const THIRDS: readonly ZoomFraction[] = [
  { min: 0, max: 1 / 3 },
  { min: 1 / 3, max: 2 / 3 },
  { min: 2 / 3, max: 1 },
];

/** Elapsed lap time (s) at a lap distance, interpolated; null outside the lap or without distance. */
function timeAtDistance(view: MotecLapView, distM: number): number | null {
  const xs = view.distanceTimelineM;
  if (!xs || xs.length === 0 || view.maxDistanceM === null || distM < 0 || distM > view.maxDistanceM) return null;
  let lo = 0;
  let hi = xs.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if ((xs[mid] ?? Number.NaN) < distM) lo = mid + 1;
    else hi = mid - 1;
  }
  if (lo > xs.length - 1) return null;
  const x1 = xs[lo] ?? Number.NaN;
  const t1 = view.timeTimelineS[lo] ?? Number.NaN;
  if (lo === 0 || x1 === distM) return Number.isFinite(t1) ? t1 : null;
  const x0 = xs[lo - 1] ?? Number.NaN;
  const t0 = view.timeTimelineS[lo - 1] ?? Number.NaN;
  const t = x1 > x0 ? t0 + ((distM - x0) / (x1 - x0)) * (t1 - t0) : t1;
  return Number.isFinite(t) ? t : null;
}

interface TracePoint {
  readonly dist: number;
  readonly refSpeedKmh: number | null;
  readonly cmpSpeedKmh: number | null;
  readonly deltaS: number | null;
  readonly refThrottlePct: number | null;
  readonly refBrakePct: number | null;
  readonly cmpThrottlePct: number | null;
  readonly cmpBrakePct: number | null;
}

function pct(v: number | null): number | null {
  return v === null ? null : v * 100;
}

function kmh(v: number | null): number | null {
  return v === null ? null : mpsToKmh(v);
}

/** SVG polyline point strings, split wherever a value is missing (no line is drawn through gaps). */
function polylines(
  pts: readonly TracePoint[],
  value: (p: TracePoint) => number | null,
  zoomM: { min: number; max: number },
  toY: (v: number) => number,
): string[] {
  const span = zoomM.max - zoomM.min;
  if (span <= 0) return [];
  const lines: string[] = [];
  let current: string[] = [];
  for (const p of pts) {
    const v = value(p);
    if (p.dist < zoomM.min - span * 0.02 || p.dist > zoomM.max + span * 0.02 || v === null) {
      if (current.length > 1) lines.push(current.join(' '));
      current = [];
      continue;
    }
    const x = ((p.dist - zoomM.min) / span) * 1000;
    current.push(`${x.toFixed(1)},${toY(v).toFixed(1)}`);
  }
  if (current.length > 1) lines.push(current.join(' '));
  return lines;
}

function fmt(v: number | null, decimals: number, unit: string): string {
  return v === null ? NO_VALUE : `${v.toFixed(decimals)}${unit}`;
}

function lapLabel(lap: MotecLap): string {
  return `Lap ${lap.lapNumber} (${formatLapTime(lap.lapTimeS)})`;
}

export function LapComparisonView(): JSX.Element {
  const { parsedFile, isOverlayEnabled } = useMotecSession();
  const [viewMode, setViewMode] = useState<ViewMode>('split');
  const [zoom, setZoom] = useState<ZoomFraction>(FULL);
  const [cursorDistM, setCursorDistM] = useState<number | null>(null);
  /** Comparison lap, identified by its start sample (unique within a file). */
  const [cmpLapKey, setCmpLapKey] = useState<number | null>(null);

  const graphContainerRef = useRef<HTMLDivElement>(null);

  const refLap = parsedFile?.fastestLap ?? null;
  const cmpCandidates = useMemo(
    () => (parsedFile && refLap ? parsedFile.laps.filter((l) => l.isValid && l !== refLap) : []),
    [parsedFile, refLap],
  );
  const cmpLap = cmpCandidates.find((l) => l.startSampleIdx === cmpLapKey) ?? null;
  const cmpView = useMemo(
    () => (parsedFile && cmpLap ? extractMotecLap(parsedFile, cmpLap) : null),
    [parsedFile, cmpLap],
  );

  const refLapTimeS = refLap?.lapTimeS ?? null;
  const overallDeltaS = cmpLap && refLapTimeS !== null ? cmpLap.lapTimeS - refLapTimeS : null;

  const trackLenM = parsedFile?.maxDistanceM ?? null;
  const distanceKnown = (parsedFile?.distanceTimelineM ?? null) !== null;
  const hasDistance = distanceKnown && trackLenM !== null && trackLenM > 0;
  const zoomM = trackLenM !== null && hasDistance ? { min: zoom.min * trackLenM, max: zoom.max * trackLenM } : { min: 0, max: 0 };

  const hasRefSpeed = parsedFile?.channelsByAppId.has(ch.SPEED) ?? false;
  const hasRefThrottle = parsedFile?.channelsByAppId.has(ch.THROTTLE) ?? false;
  const hasRefBrake = parsedFile?.channelsByAppId.has(ch.BRAKE) ?? false;
  const hasCmpSpeed = cmpView?.channelsByAppId.has(ch.SPEED) ?? false;
  const hasCmpPedals =
    (cmpView?.channelsByAppId.has(ch.THROTTLE) ?? false) || (cmpView?.channelsByAppId.has(ch.BRAKE) ?? false);

  const points = useMemo((): TracePoint[] => {
    if (!parsedFile || trackLenM === null || !hasDistance) return [];
    const pts: TracePoint[] = [];
    for (let i = 0; i <= POINTS; i++) {
      const dist = (i / POINTS) * trackLenM;
      const tRef = timeAtDistance(parsedFile, dist);
      const tCmp = cmpView ? timeAtDistance(cmpView, dist) : null;
      pts.push({
        dist,
        refSpeedKmh: kmh(sampleMotecAtDistance(parsedFile, ch.SPEED, dist)),
        cmpSpeedKmh: cmpView ? kmh(sampleMotecAtDistance(cmpView, ch.SPEED, dist)) : null,
        deltaS: tRef !== null && tCmp !== null ? tCmp - tRef : null,
        refThrottlePct: pct(sampleMotecAtDistance(parsedFile, ch.THROTTLE, dist)),
        refBrakePct: pct(sampleMotecAtDistance(parsedFile, ch.BRAKE, dist)),
        cmpThrottlePct: cmpView ? pct(sampleMotecAtDistance(cmpView, ch.THROTTLE, dist)) : null,
        cmpBrakePct: cmpView ? pct(sampleMotecAtDistance(cmpView, ch.BRAKE, dist)) : null,
      });
    }
    return pts;
  }, [parsedFile, cmpView, hasDistance, trackLenM]);

  // Axis scales from the data actually plotted.
  const speedAxisMaxKmh = useMemo(() => {
    let max = 0;
    for (const p of points) {
      if (p.refSpeedKmh !== null) max = Math.max(max, p.refSpeedKmh);
      if (p.cmpSpeedKmh !== null) max = Math.max(max, p.cmpSpeedKmh);
    }
    return Math.max(50, Math.ceil(max / 50) * 50);
  }, [points]);
  const deltaAxisS = useMemo(() => {
    let max = 0;
    for (const p of points) if (p.deltaS !== null) max = Math.max(max, Math.abs(p.deltaS));
    return Math.max(0.1, Math.ceil(max * 10) / 10);
  }, [points]);

  const handleMouseMove = (e: MouseEvent<HTMLDivElement>) => {
    if (!graphContainerRef.current || !hasDistance) return;
    const rect = graphContainerRef.current.getBoundingClientRect();
    if (rect.width <= 0) return;
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    setCursorDistM(zoomM.min + frac * (zoomM.max - zoomM.min));
  };

  const handleMouseLeave = () => {
    setCursorDistM(null);
  };

  const activeSample = useMemo(() => {
    if (cursorDistM === null || points.length === 0) return null;
    let closest: TracePoint | null = null;
    let minDiff = Infinity;
    for (const pt of points) {
      const d = Math.abs(pt.dist - cursorDistM);
      if (d < minDiff) {
        minDiff = d;
        closest = pt;
      }
    }
    return closest;
  }, [cursorDistM, points]);

  const speedY = (v: number) => 100 - (v / speedAxisMaxKmh) * 90;
  const deltaY = (v: number) => 50 + (v / deltaAxisS) * 40;
  const pedalY = (v: number) => 80 - (v / 100) * 70;

  const curves = {
    speedRef: polylines(points, (p) => p.refSpeedKmh, zoomM, speedY),
    speedCmp: polylines(points, (p) => p.cmpSpeedKmh, zoomM, speedY),
    delta: polylines(points, (p) => p.deltaS, zoomM, deltaY),
    throtRef: polylines(points, (p) => p.refThrottlePct, zoomM, pedalY),
    brakeRef: polylines(points, (p) => p.refBrakePct, zoomM, pedalY),
    throtCmp: polylines(points, (p) => p.cmpThrottlePct, zoomM, pedalY),
    brakeCmp: polylines(points, (p) => p.cmpBrakePct, zoomM, pedalY),
  };

  const cursorSvgX =
    cursorDistM !== null && zoomM.max > zoomM.min
      ? ((cursorDistM - zoomM.min) / (zoomM.max - zoomM.min)) * 1000
      : null;

  const isZoom = (z: ZoomFraction) => zoom.min === z.min && zoom.max === z.max;

  const refRoleTag = parsedFile
    ? refLap
      ? `MoTeC QUICKEST LAP (Lap ${refLap.lapNumber})`
      : 'MoTeC FILE (no complete lap found)'
    : 'REFERENCE LAP';
  const refMeta = parsedFile
    ? `${parsedFile.header.driver ?? 'Driver unknown'} (${parsedFile.header.vehicleId ?? 'car unknown'})`
    : 'No MoTeC file loaded';

  const noGraphText = !parsedFile
    ? 'No data: load a MoTeC .ld file.'
    : !hasDistance
      ? 'No data: the file has no lap distance or speed channel, so laps cannot be compared by distance.'
      : null;

  return (
    <div className="lap-comparison-view" data-testid="lap-comparison-view">
      {/* File Import Bar */}
      <MotecImportBar />

      {/* Header Cards & Summary Delta */}
      <div className="comp-header-bar">
        <div className={`comp-lap-card comp-ref-card ${parsedFile ? 'comp-motec-active' : ''}`}>
          <span className="comp-role-tag">{refRoleTag}</span>
          <div className="comp-lap-time" data-testid="comp-ref-time">
            {refLapTimeS === null ? NO_VALUE : formatLapTime(refLapTimeS)}
          </div>
          <span className="comp-meta">{refMeta}</span>
        </div>

        <div className="comp-delta-center">
          <span className="comp-delta-label">TOTAL TIME DELTA (Δt)</span>
          <span
            className={`comp-delta-value ${overallDeltaS === null ? '' : overallDeltaS < 0 ? 'delta-faster' : 'delta-slower'}`}
            data-testid="comp-total-delta"
          >
            {overallDeltaS === null ? NO_VALUE : formatDelta(overallDeltaS, { withUnit: true })}
          </span>
          <span className="comp-delta-sub">
            {overallDeltaS === null
              ? 'No data: no comparison lap'
              : overallDeltaS < 0
                ? 'Comparison lap is faster'
                : 'Reference lap is faster'}
          </span>
        </div>

        <div className="comp-lap-card comp-target-card">
          <span className="comp-role-tag">COMPARISON LAP</span>
          <div className="comp-lap-time" data-testid="comp-cmp-time">
            {cmpLap ? formatLapTime(cmpLap.lapTimeS) : NO_VALUE}
          </div>
          {cmpCandidates.length > 0 ? (
            <select
              className="comp-meta"
              aria-label="Comparison lap"
              data-testid="comp-lap-select"
              value={cmpLap ? String(cmpLap.startSampleIdx) : ''}
              onChange={(e) => { setCmpLapKey(e.target.value === '' ? null : Number(e.target.value)); }}
            >
              <option value="">Choose a lap from this file…</option>
              {cmpCandidates.map((l) => (
                <option key={l.startSampleIdx} value={String(l.startSampleIdx)}>
                  {lapLabel(l)}
                </option>
              ))}
            </select>
          ) : (
            <span className="comp-meta">No data: no second valid lap in the file</span>
          )}
        </div>
      </div>

      {/* Navigation Toolbar: Zoom & View Modes */}
      <div className="comp-nav-toolbar">
        <div className="comp-nav-left">
          <span className="comp-toolbar-title">ZOOM &amp; SPAN:</span>
          <button
            type="button"
            className={`btn btn-xs ${isZoom(FULL) ? 'btn-primary' : ''}`}
            onClick={() => { setZoom(FULL); }}
            disabled={!hasDistance}
            title="Fit the complete lap"
          >
            Full Lap (100%)
          </button>
          {THIRDS.map((z, i) => (
            <button
              key={z.min}
              type="button"
              className={`btn btn-xs ${isZoom(z) ? 'btn-primary' : ''}`}
              onClick={() => { setZoom(z); }}
              disabled={!hasDistance}
              title={`Zoom to distance third ${i + 1} of the lap (not an official sector)`}
            >
              Third {i + 1}
            </button>
          ))}
        </div>

        {/* Cursor Readout */}
        <div className="comp-cursor-readout">
          {activeSample ? (
            <div className="cursor-active-stats">
              <span className="cursor-dist-label">
                DIST: <strong>{activeSample.dist.toFixed(0)}m</strong>
              </span>
              <span className="cursor-spd-ref">
                REF: <strong>{fmt(activeSample.refSpeedKmh, 1, ' km/h')}</strong>
              </span>
              <span className="cursor-spd-live">
                CMP: <strong>{fmt(activeSample.cmpSpeedKmh, 1, ' km/h')}</strong>
              </span>
              <span
                className={`cursor-delta ${activeSample.deltaS === null ? '' : activeSample.deltaS < 0 ? 'text-green' : 'text-red'}`}
              >
                Δt:{' '}
                <strong>
                  {activeSample.deltaS === null ? NO_VALUE : formatDelta(activeSample.deltaS, { withUnit: true })}
                </strong>
              </span>
              <span className="cursor-throt">
                THR: <strong>{fmt(activeSample.refThrottlePct, 0, '%')}</strong>
              </span>
              <span className="cursor-brake">
                BRK: <strong>{fmt(activeSample.refBrakePct, 0, '%')}</strong>
              </span>
            </div>
          ) : (
            <span className="cursor-hint">Hover over the graphs to inspect values</span>
          )}
        </div>

        <div className="comp-nav-right">
          <span className="comp-toolbar-title">LAYOUT:</span>
          <button
            type="button"
            className={`btn btn-xs ${viewMode === 'graphs-only' ? 'btn-primary' : ''}`}
            onClick={() => { setViewMode('graphs-only'); }}
            title="Full-width graphs only"
          >
            Graphs Only
          </button>
          <button
            type="button"
            className={`btn btn-xs ${viewMode === 'split' ? 'btn-primary' : ''}`}
            onClick={() => { setViewMode('split'); }}
            title="Graphs + lap table"
          >
            Split View
          </button>
          <button
            type="button"
            className={`btn btn-xs ${viewMode === 'table-only' ? 'btn-primary' : ''}`}
            onClick={() => { setViewMode('table-only'); }}
            title="Laps found in the file"
          >
            Lap Table
          </button>
        </div>
      </div>

      <div className={`comp-workspace-container mode-${viewMode}`}>
        {(viewMode === 'graphs-only' || viewMode === 'split') &&
          (noGraphText !== null ? (
            <div className="comp-fullwidth-graphs" data-testid="comp-no-data">
              <span className="cursor-hint">{noGraphText}</span>
            </div>
          ) : (
            <div
              className="comp-fullwidth-graphs"
              ref={graphContainerRef}
              onMouseMove={handleMouseMove}
              onMouseLeave={handleMouseLeave}
            >
              {/* Graph 1: Speed */}
              <div className="comp-graph-card comp-graph-speed">
                <div className="comp-graph-header">
                  <span className="comp-graph-title">SPEED (km/h vs Distance)</span>
                  <div className="comp-graph-legend">
                    {hasRefSpeed && (
                      <span className="legend-chip" style={{ color: '#f59e0b', fontWeight: 700 }}>
                        ━ Orange: Reference {refLap ? lapLabel(refLap) : '(whole file)'}
                      </span>
                    )}
                    {cmpLap && hasCmpSpeed && (
                      <span className="legend-chip" style={{ color: '#22d3ee', fontWeight: 700 }}>
                        ━ Cyan: Comparison {lapLabel(cmpLap)}
                      </span>
                    )}
                    {!hasRefSpeed && !hasCmpSpeed && <span className="legend-chip">No data: no speed channel</span>}
                  </div>
                </div>
                <div className="comp-svg-viewport">
                  <svg viewBox="0 0 1000 100" preserveAspectRatio="none" className="comp-telemetry-svg">
                    {[1, 2 / 3, 1 / 3].map((f) => (
                      <g key={f}>
                        <line
                          x1="0"
                          y1={speedY(speedAxisMaxKmh * f)}
                          x2="1000"
                          y2={speedY(speedAxisMaxKmh * f)}
                          stroke="#1e293b"
                          strokeWidth="0.5"
                          strokeDasharray="4 4"
                        />
                        <text x="8" y={speedY(speedAxisMaxKmh * f) - 2} fill="#64748b" fontSize="9">
                          {(speedAxisMaxKmh * f).toFixed(0)} km/h
                        </text>
                      </g>
                    ))}
                    {isOverlayEnabled &&
                      curves.speedRef.map((p) => (
                        <polyline key={`r${p.slice(0, 16)}`} points={p} fill="none" stroke="#f59e0b" strokeWidth="2.2" />
                      ))}
                    {curves.speedCmp.map((p) => (
                      <polyline key={`c${p.slice(0, 16)}`} points={p} fill="none" stroke="#22d3ee" strokeWidth="2.2" />
                    ))}
                    {cursorSvgX !== null && (
                      <line x1={cursorSvgX} y1="0" x2={cursorSvgX} y2="100" stroke="#ffffff" strokeWidth="1.2" strokeDasharray="2 2" />
                    )}
                  </svg>
                </div>
              </div>

              {/* Graph 2: Time delta (comparison minus reference at the same distance) */}
              <div className="comp-graph-card comp-graph-delta">
                <div className="comp-graph-header">
                  <span className="comp-graph-title">TIME DELTA Δt (comparison − reference)</span>
                  <span className="comp-graph-legend">
                    {cmpLap ? (
                      <>
                        <span className="legend-faster">Below 0: comparison ahead</span>
                        <span className="legend-slower">Above 0: comparison behind</span>
                      </>
                    ) : (
                      <span className="legend-chip">No data: choose a comparison lap</span>
                    )}
                  </span>
                </div>
                <div className="comp-svg-viewport">
                  <svg viewBox="0 0 1000 100" preserveAspectRatio="none" className="comp-telemetry-svg">
                    <line x1="0" y1="50" x2="1000" y2="50" stroke="#475569" strokeWidth="1" />
                    <text x="8" y="46" fill="#94a3b8" fontSize="9">
                      0.000s
                    </text>
                    <text x="8" y="16" fill="#22c55e" fontSize="9">
                      -{deltaAxisS.toFixed(1)}s
                    </text>
                    <text x="8" y="94" fill="#ef4444" fontSize="9">
                      +{deltaAxisS.toFixed(1)}s
                    </text>
                    {curves.delta.map((p) => (
                      <polyline key={p.slice(0, 16)} points={p} fill="none" stroke="#10b981" strokeWidth="2.2" />
                    ))}
                    {cursorSvgX !== null && (
                      <line x1={cursorSvgX} y1="0" x2={cursorSvgX} y2="100" stroke="#ffffff" strokeWidth="1.2" strokeDasharray="2 2" />
                    )}
                  </svg>
                </div>
              </div>

              {/* Graph 3: Throttle & Brake */}
              <div className="comp-graph-card comp-graph-pedals">
                <div className="comp-graph-header">
                  <span className="comp-graph-title">DRIVER INPUTS (Throttle &amp; Brake % vs Distance)</span>
                  <div className="comp-graph-legend">
                    {hasRefThrottle && (
                      <span className="legend-chip" style={{ color: '#22c55e', fontWeight: 700 }}>
                        ━ Throttle %
                      </span>
                    )}
                    {hasRefBrake && (
                      <span className="legend-chip" style={{ color: '#ef4444', fontWeight: 700 }}>
                        ━ Brake %
                      </span>
                    )}
                    {cmpLap && hasCmpPedals && <span className="legend-chip">┅ dashed: comparison lap</span>}
                    {!hasRefThrottle && !hasRefBrake && !hasCmpPedals && (
                      <span className="legend-chip">No data: no throttle or brake channel</span>
                    )}
                  </div>
                </div>
                <div className="comp-svg-viewport">
                  <svg viewBox="0 0 1000 80" preserveAspectRatio="none" className="comp-telemetry-svg">
                    <line x1="0" y1="45" x2="1000" y2="45" stroke="#1e293b" strokeWidth="0.5" strokeDasharray="4 4" />
                    <text x="8" y="20" fill="#64748b" fontSize="8">
                      100%
                    </text>
                    <text x="8" y="75" fill="#64748b" fontSize="8">
                      0%
                    </text>
                    {curves.throtRef.map((p) => (
                      <polyline key={`tr${p.slice(0, 16)}`} points={p} fill="none" stroke="#22c55e" strokeWidth="1.8" />
                    ))}
                    {curves.brakeRef.map((p) => (
                      <polyline key={`br${p.slice(0, 16)}`} points={p} fill="none" stroke="#ef4444" strokeWidth="1.8" />
                    ))}
                    {curves.throtCmp.map((p) => (
                      <polyline
                        key={`tc${p.slice(0, 16)}`}
                        points={p}
                        fill="none"
                        stroke="#22c55e"
                        strokeWidth="1.4"
                        strokeDasharray="5 3"
                      />
                    ))}
                    {curves.brakeCmp.map((p) => (
                      <polyline
                        key={`bc${p.slice(0, 16)}`}
                        points={p}
                        fill="none"
                        stroke="#ef4444"
                        strokeWidth="1.4"
                        strokeDasharray="5 3"
                      />
                    ))}
                    {cursorSvgX !== null && (
                      <line x1={cursorSvgX} y1="0" x2={cursorSvgX} y2="80" stroke="#ffffff" strokeWidth="1.2" strokeDasharray="2 2" />
                    )}
                  </svg>
                </div>
              </div>
            </div>
          ))}

        {/* Laps found in the file (real data only) */}
        {(viewMode === 'split' || viewMode === 'table-only') && (
          <div className="comp-corners-drawer">
            <div className="comp-corners-header">
              <span className="corners-title">LAPS IN THE MoTeC FILE</span>
              <span className="corners-count">{parsedFile ? `${parsedFile.laps.length} SEGMENTS` : NO_VALUE}</span>
            </div>
            <div className="comp-table-scroll">
              {parsedFile && parsedFile.laps.length > 0 ? (
                <table className="comp-table" data-testid="comp-lap-table">
                  <thead>
                    <tr>
                      <th>Lap</th>
                      <th>Time</th>
                      <th>Distance</th>
                      <th>Status</th>
                      <th>Role</th>
                    </tr>
                  </thead>
                  <tbody>
                    {parsedFile.laps.map((l) => {
                      const lapDist =
                        isFiniteNumber(l.startDistM) && isFiniteNumber(l.endDistM) ? l.endDistM - l.startDistM : null;
                      const isRef = l === refLap;
                      const isCmp = l === cmpLap;
                      const selectable = l.isValid && !isRef;
                      return (
                        <tr
                          key={l.startSampleIdx}
                          className={isRef || isCmp ? 'row-highlighted' : ''}
                          onClick={selectable ? () => { setCmpLapKey(l.startSampleIdx); } : undefined}
                          title={selectable ? 'Click to compare this lap with the reference' : (l.invalidReason ?? '')}
                        >
                          <td className="corner-name-cell">{l.lapNumber}</td>
                          <td>{formatLapTime(l.lapTimeS)}</td>
                          <td>{fmt(lapDist, 0, ' m')}</td>
                          <td>{l.isValid ? 'Valid' : `Not used: ${l.invalidReason ?? 'invalid'}`}</td>
                          <td>{isRef ? 'Reference' : isCmp ? 'Comparison' : ''}</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              ) : (
                <span className="cursor-hint">
                  {parsedFile ? 'No data: no lap boundaries were found in the file.' : 'No data: load a MoTeC .ld file.'}
                </span>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
