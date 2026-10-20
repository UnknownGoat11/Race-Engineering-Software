import { useMemo, useState, useRef, type JSX, type MouseEvent } from 'react';
import { ch } from '../../generated/channels';
import { MotecImportBar } from '../../motec/MotecImportBar';
import { useMotecSession } from '../../motec/useMotecStore';
import { formatLapTime } from '../../lib/units';
import '../../styles/common.css';

export interface LapComparisonCorner {
  readonly cornerName: string;
  readonly distM: number;
  readonly refSpeedKmh: number;
  readonly cmpSpeedKmh: number;
  readonly deltaS: number; // negative = cmp faster
  readonly apexDeltaM: number;
}

const DEFAULT_CORNERS: readonly LapComparisonCorner[] = [
  { cornerName: 'Turn 1 (La Source)', distM: 450, refSpeedKmh: 74.2, cmpSpeedKmh: 76.8, deltaS: -0.065, apexDeltaM: -1.2 },
  { cornerName: 'Eau Rouge / Raidillon', distM: 1150, refSpeedKmh: 284.1, cmpSpeedKmh: 282.4, deltaS: 0.042, apexDeltaM: 0.8 },
  { cornerName: 'Les Combes (T5)', distM: 2650, refSpeedKmh: 138.5, cmpSpeedKmh: 141.2, deltaS: -0.110, apexDeltaM: -2.4 },
  { cornerName: 'Malmedy (T6)', distM: 3050, refSpeedKmh: 172.0, cmpSpeedKmh: 171.4, deltaS: 0.015, apexDeltaM: 0.4 },
  { cornerName: 'Bruxelles Hairpin (T7)', distM: 3550, refSpeedKmh: 102.4, cmpSpeedKmh: 104.8, deltaS: -0.082, apexDeltaM: -1.6 },
  { cornerName: 'Pouhon Double Left (T10-11)', distM: 4850, refSpeedKmh: 228.6, cmpSpeedKmh: 226.9, deltaS: 0.054, apexDeltaM: 1.1 },
  { cornerName: 'Campus (T12)', distM: 5350, refSpeedKmh: 154.2, cmpSpeedKmh: 156.0, deltaS: -0.038, apexDeltaM: -0.9 },
  { cornerName: 'Blanchimont (T16)', distM: 6350, refSpeedKmh: 298.4, cmpSpeedKmh: 299.1, deltaS: -0.021, apexDeltaM: -0.3 },
  { cornerName: 'Bus Stop Chicane (T18-19)', distM: 6850, refSpeedKmh: 71.8, cmpSpeedKmh: 73.5, deltaS: -0.074, apexDeltaM: -1.5 },
];

const TRACK_LENGTH_M = 7004;

type ViewMode = 'graphs-only' | 'split' | 'table-only';

export function LapComparisonView(): JSX.Element {
  const { parsedFile, isOverlayEnabled } = useMotecSession();
  const [cmpLap] = useState({ lapNum: 16, timeS: 134.563, driver: 'Live Player / Target' });
  const [viewMode, setViewMode] = useState<ViewMode>('split');

  // Zoom range in meters (0 to TRACK_LENGTH_M)
  const [zoomRange, setZoomRange] = useState<{ min: number; max: number }>({ min: 0, max: TRACK_LENGTH_M });
  // Interactive cursor scrubbing along distance axis (m)
  const [cursorDistM, setCursorDistM] = useState<number | null>(null);

  const graphContainerRef = useRef<HTMLDivElement>(null);

  const hasMotecFile = parsedFile !== null;
  const refLapTimeS = hasMotecFile
    ? (parsedFile.fastestLap?.lapTimeS ?? parsedFile.durationS)
    : 135.240;
  const refDriverName = hasMotecFile
    ? `${parsedFile.header.driver} (${parsedFile.header.vehicleId})`
    : 'Benchmark Lap / Reference';

  const overallDeltaS = cmpLap.timeS - refLapTimeS;

  const maxDist = parsedFile?.maxDistanceM || TRACK_LENGTH_M;

  // Pre-generate 200 telemetry points along the distance axis
  const telemetryPoints = useMemo(() => {
    const pointsCount = 200;
    const pts: {
      dist: number;
      refSpeed: number;
      cmpSpeed: number;
      deltaT: number;
      throt: number;
      brake: number;
    }[] = [];

    const speedChan = parsedFile
      ? (parsedFile.channelsByAppId.get(ch.SPEED) ||
         parsedFile.channels.get('ground speed') ||
         parsedFile.channels.get('speed') ||
         parsedFile.channels.get('vcar') ||
         parsedFile.channels.get('gps speed'))
      : null;

    let cumDelta = 0;

    for (let i = 0; i <= pointsCount; i++) {
      const frac = i / pointsCount;
      const dist = frac * maxDist;

      // Realistic synthetic speed profile with corner dips
      const phase = frac * Math.PI * 14;
      const cornerDip = Math.pow(Math.sin(phase), 2);
      const isStraight = (frac > 0.18 && frac < 0.35) || (frac > 0.75 && frac < 0.95);
      const baseSpeed = isStraight ? 310 : 255;
      const cmpSpeed = Math.max(65, baseSpeed - cornerDip * (isStraight ? 140 : 180));

      let refSpeed = cmpSpeed - (Math.sin(frac * Math.PI * 8) * 4.5);
      if (speedChan && parsedFile && parsedFile.distanceTimelineM.length > 0) {
        const xs = parsedFile.distanceTimelineM;
        let lo = 0;
        let hi = xs.length - 1;
        while (lo <= hi) {
          const mid = (lo + hi) >> 1;
          if ((xs[mid] ?? 0) < dist) lo = mid + 1;
          else hi = mid - 1;
        }
        const idx = Math.min(xs.length - 1, Math.max(0, lo));
        refSpeed = speedChan.samples[idx] ?? refSpeed;
      }

      // Time delta accumulation
      cumDelta += (cmpSpeed > refSpeed ? -0.0035 : 0.0028);

      // Pedal traces
      const throt = cornerDip < 0.3 ? 100 : Math.max(0, 100 - cornerDip * 120);
      const brake = cornerDip > 0.6 ? Math.min(100, (cornerDip - 0.6) * 250) : 0;

      pts.push({
        dist,
        refSpeed,
        cmpSpeed,
        deltaT: cumDelta,
        throt,
        brake,
      });
    }
    return pts;
  }, [maxDist, parsedFile]);

  // Handle cursor scrubbing across the graphs
  const handleMouseMove = (e: MouseEvent<HTMLDivElement>) => {
    if (!graphContainerRef.current) return;
    const rect = graphContainerRef.current.getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    const currentDist = zoomRange.min + frac * (zoomRange.max - zoomRange.min);
    setCursorDistM(currentDist);
  };

  const handleMouseLeave = () => {
    setCursorDistM(null);
  };

  // Find telemetry values closest to cursor
  const activeSample = useMemo(() => {
    if (cursorDistM === null || telemetryPoints.length === 0) return null;
    let closest = telemetryPoints[0]!;
    let minDiff = Infinity;
    for (const pt of telemetryPoints) {
      const diff = Math.abs(pt.dist - cursorDistM);
      if (diff < minDiff) {
        minDiff = diff;
        closest = pt;
      }
    }
    return closest;
  }, [cursorDistM, telemetryPoints]);

  // Convert telemetry points to SVG coordinates mapped to current zoomRange
  const svgCurves = useMemo(() => {
    const span = zoomRange.max - zoomRange.min;
    if (span <= 0) return { speedCmp: '', speedRef: '', delta: '', throt: '', brake: '' };

    const speedCmpPts: string[] = [];
    const speedRefPts: string[] = [];
    const deltaPts: string[] = [];
    const throtPts: string[] = [];
    const brakePts: string[] = [];

    for (const pt of telemetryPoints) {
      if (pt.dist < zoomRange.min - 100 || pt.dist > zoomRange.max + 100) continue;
      const x = ((pt.dist - zoomRange.min) / span) * 1000;

      // Speed (0 to 350 km/h mapped to 100 to 10 SVG y)
      const yCmpSpd = 100 - (pt.cmpSpeed / 350) * 90;
      const yRefSpd = 100 - (pt.refSpeed / 350) * 90;
      speedCmpPts.push(`${x.toFixed(1)},${yCmpSpd.toFixed(1)}`);
      speedRefPts.push(`${x.toFixed(1)},${yRefSpd.toFixed(1)}`);

      // Delta-t (-1.0s to +1.0s mapped to 10 to 90 SVG y, center at 50)
      const yDelta = 50 + (pt.deltaT / 1.0) * 40;
      deltaPts.push(`${x.toFixed(1)},${Math.max(5, Math.min(95, yDelta)).toFixed(1)}`);

      // Pedals (0 to 100% mapped to 80 to 10 SVG y)
      const yThrot = 80 - (pt.throt / 100) * 70;
      const yBrake = 80 - (pt.brake / 100) * 70;
      throtPts.push(`${x.toFixed(1)},${yThrot.toFixed(1)}`);
      brakePts.push(`${x.toFixed(1)},${yBrake.toFixed(1)}`);
    }

    return {
      speedCmp: speedCmpPts.join(' '),
      speedRef: speedRefPts.join(' '),
      delta: deltaPts.join(' '),
      throt: throtPts.join(' '),
      brake: brakePts.join(' '),
    };
  }, [telemetryPoints, zoomRange]);

  // Cursor X position in 0..1000 SVG coordinates
  const cursorSvgX = cursorDistM !== null
    ? ((cursorDistM - zoomRange.min) / (zoomRange.max - zoomRange.min)) * 1000
    : null;

  return (
    <div className="lap-comparison-view" data-testid="lap-comparison-view">
      {/* File Import Bar */}
      <MotecImportBar />

      {/* Header Cards & Summary Delta */}
      <div className="comp-header-bar">
        <div className={`comp-lap-card comp-ref-card ${hasMotecFile ? 'comp-motec-active' : ''}`}>
          <span className="comp-role-tag">
            {hasMotecFile
              ? parsedFile.laps.length > 1
                ? `🟠 MoTeC QUICKEST LAP (Lap ${parsedFile.fastestLap?.lapNumber ?? 1})`
                : '🟠 MoTeC (.LD) REFERENCE'
              : 'REFERENCE LAP'}
          </span>
          <div className="comp-lap-time">{formatLapTime(refLapTimeS)}</div>
          <span className="comp-meta">{refDriverName}</span>
        </div>

        <div className="comp-delta-center">
          <span className="comp-delta-label">TOTAL TIME DELTA (Δt)</span>
          <span className={`comp-delta-value ${overallDeltaS < 0 ? 'delta-faster' : 'delta-slower'}`}>
            {overallDeltaS < 0 ? '' : '+'}{overallDeltaS.toFixed(3)}s
          </span>
          <span className="comp-delta-sub">
            {overallDeltaS < 0 ? 'Live target is faster' : 'MoTeC reference is faster'}
          </span>
        </div>

        <div className="comp-lap-card comp-target-card">
          <span className="comp-role-tag">COMPARISON LAP</span>
          <div className="comp-lap-time">{formatLapTime(cmpLap.timeS)}</div>
          <span className="comp-meta">Lap #{cmpLap.lapNum} • {cmpLap.driver}</span>
        </div>
      </div>

      {/* MoTeC i2 Pro Navigation Toolbar: Zoom Presets & View Modes */}
      <div className="comp-nav-toolbar">
        <div className="comp-nav-left">
          <span className="comp-toolbar-title">ZOOM &amp; SPAN:</span>
          <button
            type="button"
            className={`btn btn-xs ${zoomRange.min === 0 && zoomRange.max === TRACK_LENGTH_M ? 'btn-primary' : ''}`}
            onClick={() => setZoomRange({ min: 0, max: TRACK_LENGTH_M })}
            title="Fit complete lap (0 to 7,004m)"
          >
            Full Lap (100%)
          </button>
          <button
            type="button"
            className={`btn btn-xs ${zoomRange.min === 0 && zoomRange.max === 2250 ? 'btn-primary' : ''}`}
            onClick={() => setZoomRange({ min: 0, max: 2250 })}
            title="Focus Sector 1 (La Source &amp; Kemmel Straight)"
          >
            Sector 1
          </button>
          <button
            type="button"
            className={`btn btn-xs ${zoomRange.min === 2250 && zoomRange.max === 4950 ? 'btn-primary' : ''}`}
            onClick={() => setZoomRange({ min: 2250, max: 4950 })}
            title="Focus Sector 2 (Les Combes, Bruxelles &amp; Pouhon)"
          >
            Sector 2
          </button>
          <button
            type="button"
            className={`btn btn-xs ${zoomRange.min === 4950 && zoomRange.max === TRACK_LENGTH_M ? 'btn-primary' : ''}`}
            onClick={() => setZoomRange({ min: 4950, max: TRACK_LENGTH_M })}
            title="Focus Sector 3 (Blanchimont &amp; Bus Stop)"
          >
            Sector 3
          </button>
        </div>

        {/* Live Cursor Readout Inspection */}
        <div className="comp-cursor-readout">
          {activeSample ? (
            <div className="cursor-active-stats">
              <span className="cursor-dist-label">DIST: <strong>{activeSample.dist.toFixed(0)}m</strong></span>
              <span className="cursor-spd-live">LIVE: <strong>{activeSample.cmpSpeed.toFixed(1)} km/h</strong></span>
              <span className="cursor-spd-ref">REF: <strong>{activeSample.refSpeed.toFixed(1)} km/h</strong></span>
              <span className={`cursor-delta ${activeSample.deltaT < 0 ? 'text-green' : 'text-red'}`}>
                Δt: <strong>{activeSample.deltaT < 0 ? '' : '+'}{activeSample.deltaT.toFixed(3)}s</strong>
              </span>
              <span className="cursor-throt">THR: <strong>{activeSample.throt.toFixed(0)}%</strong></span>
              <span className="cursor-brake">BRK: <strong>{activeSample.brake.toFixed(0)}%</strong></span>
            </div>
          ) : (
            <span className="cursor-hint">Hover / drag over graph to scrub cursor &amp; inspect data points</span>
          )}
        </div>

        <div className="comp-nav-right">
          <span className="comp-toolbar-title">LAYOUT:</span>
          <button
            type="button"
            className={`btn btn-xs ${viewMode === 'graphs-only' ? 'btn-primary' : ''}`}
            onClick={() => setViewMode('graphs-only')}
            title="Full-width graphs only"
          >
            Graphs Only
          </button>
          <button
            type="button"
            className={`btn btn-xs ${viewMode === 'split' ? 'btn-primary' : ''}`}
            onClick={() => setViewMode('split')}
            title="Split view: Full-width graphs + Corner breakdown"
          >
            Split View
          </button>
          <button
            type="button"
            className={`btn btn-xs ${viewMode === 'table-only' ? 'btn-primary' : ''}`}
            onClick={() => setViewMode('table-only')}
            title="Detailed turn-by-turn matrix"
          >
            Turn Matrix
          </button>
        </div>
      </div>

      {/* Main Analysis Container: Full Width Telemetry Graphs (No Halfway Cutoff!) */}
      <div className={`comp-workspace-container mode-${viewMode}`}>
        {(viewMode === 'graphs-only' || viewMode === 'split') && (
          <div
            className="comp-fullwidth-graphs"
            ref={graphContainerRef}
            onMouseMove={handleMouseMove}
            onMouseLeave={handleMouseLeave}
          >
            {/* Graph 1: Speed Overlay (Live vs Reference/MoTeC) */}
            <div className="comp-graph-card comp-graph-speed">
              <div className="comp-graph-header">
                <span className="comp-graph-title">SPEED OVERLAY (km/h vs Distance)</span>
                <div className="comp-graph-legend">
                  <span className="legend-chip" style={{ color: '#f59e0b', fontWeight: 700 }}>
                    ━ Orange: MoTeC Ref ({hasMotecFile ? parsedFile.header.driver.split(' ')[0] : 'Ref'})
                  </span>
                  <span className="legend-chip" style={{ color: '#22d3ee', fontWeight: 700 }}>
                    ━ Cyan: Live Target
                  </span>
                </div>
              </div>
              <div className="comp-svg-viewport">
                <svg viewBox="0 0 1000 100" preserveAspectRatio="none" className="comp-telemetry-svg">
                  {/* Gridlines */}
                  <line x1="0" y1="20" x2="1000" y2="20" stroke="#1e293b" strokeWidth="0.5" strokeDasharray="4 4" />
                  <line x1="0" y1="45" x2="1000" y2="45" stroke="#1e293b" strokeWidth="0.5" strokeDasharray="4 4" />
                  <line x1="0" y1="70" x2="1000" y2="70" stroke="#1e293b" strokeWidth="0.5" strokeDasharray="4 4" />
                  <text x="8" y="18" fill="#64748b" fontSize="9">300 km/h</text>
                  <text x="8" y="43" fill="#64748b" fontSize="9">200 km/h</text>
                  <text x="8" y="68" fill="#64748b" fontSize="9">100 km/h</text>

                  {/* MoTeC Reference Trace (Orange) */}
                  {isOverlayEnabled && (
                    <polyline points={svgCurves.speedRef} fill="none" stroke="#f59e0b" strokeWidth="2.2" />
                  )}

                  {/* Live Target Trace (Cyan) */}
                  <polyline points={svgCurves.speedCmp} fill="none" stroke="#22d3ee" strokeWidth="2.2" />

                  {/* Scrubbing Cursor */}
                  {cursorSvgX !== null && (
                    <line x1={cursorSvgX} y1="0" x2={cursorSvgX} y2="100" stroke="#ffffff" strokeWidth="1.2" strokeDasharray="2 2" />
                  )}
                </svg>
              </div>
            </div>

            {/* Graph 2: Delta-T Trace (Time Delta in seconds vs Distance) */}
            <div className="comp-graph-card comp-graph-delta">
              <div className="comp-graph-header">
                <span className="comp-graph-title">CONTINUOUS TIME DELTA Δt (Green = Faster, Red = Slower)</span>
                <span className="comp-graph-legend">
                  <span className="legend-faster">▲ Green: Gaining Time</span>
                  <span className="legend-slower">▼ Red: Losing Time</span>
                </span>
              </div>
              <div className="comp-svg-viewport">
                <svg viewBox="0 0 1000 100" preserveAspectRatio="none" className="comp-telemetry-svg">
                  {/* Zero Delta Centerline */}
                  <line x1="0" y1="50" x2="1000" y2="50" stroke="#475569" strokeWidth="1" />
                  <text x="8" y="46" fill="#94a3b8" fontSize="9">0.000s (Equal)</text>
                  <text x="8" y="16" fill="#22c55e" fontSize="9">-1.000s</text>
                  <text x="8" y="86" fill="#ef4444" fontSize="9">+1.000s</text>

                  {/* Delta Curve */}
                  <polyline points={svgCurves.delta} fill="none" stroke="#10b981" strokeWidth="2.2" />

                  {/* Scrubbing Cursor */}
                  {cursorSvgX !== null && (
                    <line x1={cursorSvgX} y1="0" x2={cursorSvgX} y2="100" stroke="#ffffff" strokeWidth="1.2" strokeDasharray="2 2" />
                  )}
                </svg>
              </div>
            </div>

            {/* Graph 3: Throttle & Brake Pedals Overlay */}
            <div className="comp-graph-card comp-graph-pedals">
              <div className="comp-graph-header">
                <span className="comp-graph-title">DRIVER INPUTS (Throttle &amp; Brake % vs Distance)</span>
                <div className="comp-graph-legend">
                  <span className="legend-chip" style={{ color: '#22c55e', fontWeight: 700 }}>━ Throttle %</span>
                  <span className="legend-chip" style={{ color: '#ef4444', fontWeight: 700 }}>━ Brake %</span>
                </div>
              </div>
              <div className="comp-svg-viewport">
                <svg viewBox="0 0 1000 80" preserveAspectRatio="none" className="comp-telemetry-svg">
                  <line x1="0" y1="45" x2="1000" y2="45" stroke="#1e293b" strokeWidth="0.5" strokeDasharray="4 4" />
                  <text x="8" y="20" fill="#64748b" fontSize="8">100%</text>
                  <text x="8" y="75" fill="#64748b" fontSize="8">0%</text>

                  {/* Throttle (Green) */}
                  <polyline points={svgCurves.throt} fill="none" stroke="#22c55e" strokeWidth="1.8" />
                  {/* Brake (Red) */}
                  <polyline points={svgCurves.brake} fill="none" stroke="#ef4444" strokeWidth="1.8" />

                  {/* Scrubbing Cursor */}
                  {cursorSvgX !== null && (
                    <line x1={cursorSvgX} y1="0" x2={cursorSvgX} y2="80" stroke="#ffffff" strokeWidth="1.2" strokeDasharray="2 2" />
                  )}
                </svg>
              </div>
            </div>
          </div>
        )}

        {/* Turn-by-Turn Corner Breakdown Table (Placed neatly in bottom panel or full view) */}
        {(viewMode === 'split' || viewMode === 'table-only') && (
          <div className="comp-corners-drawer">
            <div className="comp-corners-header">
              <span className="corners-title">TURN-BY-TURN CORNER DELTA REPORT (APEX &amp; EXIT SPEEDS)</span>
              <span className="corners-count">{DEFAULT_CORNERS.length} MONITORED APEX ZONES</span>
            </div>
            <div className="comp-table-scroll">
              <table className="comp-table">
                <thead>
                  <tr>
                    <th>Corner</th>
                    <th>Distance</th>
                    <th>Ref Speed</th>
                    <th>Live Speed</th>
                    <th>Speed Δ</th>
                    <th>Time Δ (Sector)</th>
                    <th>Apex Alignment</th>
                  </tr>
                </thead>
                <tbody>
                  {DEFAULT_CORNERS.map((c) => {
                    const speedDiff = c.cmpSpeedKmh - c.refSpeedKmh;
                    return (
                      <tr
                        key={c.cornerName}
                        className={cursorDistM !== null && Math.abs(c.distM - cursorDistM) < 300 ? 'row-highlighted' : ''}
                        onClick={() => setZoomRange({ min: Math.max(0, c.distM - 400), max: Math.min(TRACK_LENGTH_M, c.distM + 400) })}
                        title="Click to zoom in on this corner"
                      >
                        <td className="corner-name-cell">{c.cornerName}</td>
                        <td>{c.distM}m</td>
                        <td>{c.refSpeedKmh.toFixed(0)} km/h</td>
                        <td>{c.cmpSpeedKmh.toFixed(0)} km/h</td>
                        <td className={speedDiff >= 0 ? 'text-green' : 'text-red'}>
                          {speedDiff >= 0 ? '+' : ''}{speedDiff.toFixed(1)} km/h
                        </td>
                        <td className={c.deltaS < 0 ? 'text-green font-bold' : 'text-red font-bold'}>
                          {c.deltaS < 0 ? '' : '+'}{c.deltaS.toFixed(3)}s
                        </td>
                        <td>{c.apexDeltaM < 0 ? `${Math.abs(c.apexDeltaM)}m late` : `${c.apexDeltaM}m early`}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
