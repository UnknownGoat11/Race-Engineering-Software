import { useMemo, useState, type JSX } from 'react';
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

function buildPolylinePoints(corners: readonly LapComparisonCorner[]): string {
  let delta = 0;
  const pts: string[] = [];
  for (const c of corners) {
    delta += c.deltaS;
    const x = (c.distM / 7004) * 500;
    const y = 50 + delta * 60;
    pts.push(`${x.toFixed(1)},${y.toFixed(1)}`);
  }
  return pts.join(' ');
}

const DEFAULT_POLYLINE_POINTS = buildPolylinePoints(DEFAULT_CORNERS);

export function LapComparisonView(): JSX.Element {
  const { parsedFile, isOverlayEnabled } = useMotecSession();

  const [staticRefLap] = useState({ lapNum: 14, timeS: 136.421, driver: 'Reference / Benchmark' });
  const [cmpLap] = useState({ lapNum: 16, timeS: 134.563, driver: 'Live Player / Target' });

  // Use imported MoTeC run if available
  const refLapTimeS = parsedFile?.fastestLap?.lapTimeS ?? parsedFile?.durationS ?? staticRefLap.timeS;
  const refDriverName = parsedFile
    ? `${parsedFile.header.driver} (${parsedFile.header.vehicleId})`
    : staticRefLap.driver;

  const overallDeltaS = cmpLap.timeS - refLapTimeS;

  const polylinePoints = DEFAULT_POLYLINE_POINTS;

  // Synthesize Speed comparison curves for SVG plot
  const speedCurves = useMemo(() => {
    const pointsCount = 100;
    const motecPts: string[] = [];
    const cmpPts: string[] = [];

    for (let i = 0; i <= pointsCount; i++) {
      const frac = i / pointsCount;
      const x = frac * 500;
      // Synthetic speed profile (60 to 330 km/h mapped to 85 to 15 px)
      const phase = frac * Math.PI * 10;
      const corner = Math.abs(Math.sin(phase));

      const motecSpd = 280 - corner * 170;
      const cmpSpd = motecSpd + (Math.sin(frac * 14) * 12 - 4);

      // Y mapping: 340 km/h = 10px, 60 km/h = 85px
      const motecY = 85 - ((motecSpd - 60) / 280) * 75;
      const cmpY = 85 - ((cmpSpd - 60) / 280) * 75;

      motecPts.push(`${x.toFixed(1)},${motecY.toFixed(1)}`);
      cmpPts.push(`${x.toFixed(1)},${cmpY.toFixed(1)}`);
    }

    return {
      motecLine: motecPts.join(' '),
      cmpLine: cmpPts.join(' '),
    };
  }, []);

  return (
    <div className="lap-comparison-view" data-testid="lap-comparison-view">
      {/* MoTeC .ld Import & Benchmark Bar */}
      <MotecImportBar />

      <div className="comp-header-bar">
        <div className={`comp-lap-card comp-ref-card ${parsedFile ? 'comp-motec-active' : ''}`}>
          <span className="comp-role-tag">
            {parsedFile ? '🟠 MoTeC (.LD) REFERENCE' : 'REFERENCE LAP'}
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
            {overallDeltaS < 0 ? 'Live car is faster' : 'MoTeC reference is faster'}
          </span>
        </div>

        <div className="comp-lap-card comp-cmp-card">
          <span className="comp-role-tag">COMPARISON LAP</span>
          <div className="comp-lap-time">{formatLapTime(cmpLap.timeS)}</div>
          <span className="comp-meta">Lap #{cmpLap.lapNum} • {cmpLap.driver}</span>
        </div>
      </div>

      {/* Comparison Content Grid: Left Column has Speed & Delta graphs; Right Column has Corner Breakdown Table */}
      <div className="comp-content-grid">
        <div className="comp-graphs-col">
          {/* MoTeC Speed Comparison Graph (Orange MoTeC line vs Cyan Live line) */}
          <div className="comp-graph-card">
            <div className="comp-graph-header">
              <span className="comp-graph-title">Speed Comparison (Speed vs Distance)</span>
              <span className="comp-graph-legend">
                <span className="legend-chip" style={{ color: '#f59e0b', fontWeight: 700 }}>
                  ━ Orange: MoTeC {parsedFile ? `(${parsedFile.header.driver.split(' ')[0]})` : 'Ref'}
                </span>
                <span className="legend-chip" style={{ color: '#22d3ee', fontWeight: 700 }}>
                  ━ Cyan: Live
                </span>
              </span>
            </div>

            <div className="comp-svg-viewport">
              <svg viewBox="0 0 500 100" className="comp-speed-svg" preserveAspectRatio="none">
                {/* Speed Grid Lines */}
                <line x1="0" y1="20" x2="500" y2="20" stroke="#334155" strokeWidth="0.5" strokeDasharray="3 3" />
                <text x="5" y="18" fill="#64748b" fontSize="7">300 km/h</text>
                <line x1="0" y1="50" x2="500" y2="50" stroke="#334155" strokeWidth="0.5" strokeDasharray="3 3" />
                <text x="5" y="48" fill="#64748b" fontSize="7">200 km/h</text>
                <line x1="0" y1="75" x2="500" y2="75" stroke="#334155" strokeWidth="0.5" strokeDasharray="3 3" />
                <text x="5" y="73" fill="#64748b" fontSize="7">100 km/h</text>

                {/* MoTeC Reference Speed Line (Orange) */}
                {isOverlayEnabled && (
                  <polyline
                    points={speedCurves.motecLine}
                    fill="none"
                    stroke="#f59e0b"
                    strokeWidth="2.5"
                    strokeDasharray={parsedFile ? 'none' : '4 2'}
                  />
                )}

                {/* Live / Comparison Speed Line (Cyan) */}
                <polyline
                  points={speedCurves.cmpLine}
                  fill="none"
                  stroke="#22d3ee"
                  strokeWidth="2.5"
                />
              </svg>
            </div>
          </div>

          {/* Synchronous Delta-t Waveform Graph */}
          <div className="comp-graph-card">
            <div className="comp-graph-header">
              <span className="comp-graph-title">Continuous Time Delta (Δt vs Metres)</span>
              <span className="comp-graph-legend">
                <span className="legend-chip legend-faster">Green: Gaining</span>
                <span className="legend-chip legend-slower">Red: Losing</span>
              </span>
            </div>

            <div className="comp-svg-viewport">
              <svg viewBox="0 0 500 100" className="comp-delta-svg" preserveAspectRatio="none">
                {/* Zero line */}
                <line x1="0" y1="50" x2="500" y2="50" stroke="#64748b" strokeWidth="1" strokeDasharray="4 4" />
                <text x="5" y="46" fill="#94a3b8" fontSize="8">±0.000s</text>
                <text x="5" y="20" fill="#22c55e" fontSize="8">-0.500s</text>
                <text x="5" y="85" fill="#ef4444" fontSize="8">+0.500s</text>

                {/* Delta line */}
                <polyline
                  points={`0,50 ${polylinePoints}`}
                  fill="none"
                  stroke="#10b981"
                  strokeWidth="2.5"
                />
              </svg>
            </div>
          </div>
        </div>

        {/* Corner by Corner Breakdown Table */}
        <div className="comp-corners-panel">
          <div className="comp-corners-header">
            <span className="corners-title">CORNER-BY-CORNER TELEMETRY BREAKDOWN</span>
          </div>
          <div className="comp-table-scroll">
            <table className="comp-table">
              <thead>
                <tr>
                  <th>Corner</th>
                  <th>Dist</th>
                  <th>Ref Vmin</th>
                  <th>Cmp Vmin</th>
                  <th>ΔSpeed</th>
                  <th>Δt</th>
                  <th>Braking</th>
                </tr>
              </thead>
              <tbody>
                {DEFAULT_CORNERS.map((c) => {
                  const speedDiff = c.cmpSpeedKmh - c.refSpeedKmh;
                  return (
                    <tr key={c.cornerName}>
                      <td className="corner-name-cell">{c.cornerName}</td>
                      <td>{c.distM}m</td>
                      <td>{c.refSpeedKmh.toFixed(0)}</td>
                      <td>{c.cmpSpeedKmh.toFixed(0)}</td>
                      <td className={speedDiff >= 0 ? 'text-green' : 'text-red'}>
                        {speedDiff >= 0 ? '+' : ''}{speedDiff.toFixed(1)}
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
      </div>
    </div>
  );
}
