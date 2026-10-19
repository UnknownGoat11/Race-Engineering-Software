import { useState, type JSX } from 'react';

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
  const [refLap] = useState({ lapNum: 14, timeS: 134.842, driver: 'Player (Best)' });
  const [cmpLap] = useState({ lapNum: 16, timeS: 134.563, driver: 'Target / Teammate' });
  const overallDeltaS = cmpLap.timeS - refLap.timeS; // -0.279 s

  const polylinePoints = DEFAULT_POLYLINE_POINTS;

  return (
    <div className="lap-comparison-view" data-testid="lap-comparison-view">
      <div className="comp-header-bar">
        <div className="comp-lap-card comp-ref-card">
          <span className="comp-role-tag">REFERENCE LAP</span>
          <div className="comp-lap-time">{refLap.timeS.toFixed(3)}s</div>
          <span className="comp-meta">Lap #{refLap.lapNum} • {refLap.driver}</span>
        </div>

        <div className="comp-delta-center">
          <span className="comp-delta-label">TOTAL TIME DELTA (Δt)</span>
          <span className={`comp-delta-value ${overallDeltaS < 0 ? 'delta-faster' : 'delta-slower'}`}>
            {overallDeltaS < 0 ? '' : '+'}{overallDeltaS.toFixed(3)}s
          </span>
          <span className="comp-delta-sub">Comparison lap is faster</span>
        </div>

        <div className="comp-lap-card comp-cmp-card">
          <span className="comp-role-tag">COMPARISON LAP</span>
          <div className="comp-lap-time">{cmpLap.timeS.toFixed(3)}s</div>
          <span className="comp-meta">Lap #{cmpLap.lapNum} • {cmpLap.driver}</span>
        </div>
      </div>

      {/* Synchronous Delta-t Waveform Graph */}
      <div className="comp-graph-card">
        <div className="comp-graph-header">
          <span className="comp-graph-title">Continuous Time Delta Over Lap Distance (Δt vs Metres)</span>
          <span className="comp-graph-legend">
            <span className="legend-chip legend-faster">Green: Gaining Time</span>
            <span className="legend-chip legend-slower">Red: Losing Time</span>
          </span>
        </div>

        <div className="comp-svg-viewport">
          <svg viewBox="0 0 500 100" className="comp-delta-svg" preserveAspectRatio="none">
            {/* Zero line */}
            <line x1="0" y1="50" x2="500" y2="50" stroke="#64748b" strokeWidth="1" strokeDasharray="4 4" />
            <text x="5" y="46" fill="#94a3b8" fontSize="8">±0.000s</text>
            <text x="5" y="20" fill="#22c55e" fontSize="8">-0.500s (Faster)</text>
            <text x="5" y="85" fill="#ef4444" fontSize="8">+0.500s (Slower)</text>

            {/* Delta line */}
            <polyline
              points={`0,50 ${polylinePoints}`}
              fill="none"
              stroke="#22d3ee"
              strokeWidth="2.5"
            />
          </svg>
        </div>
      </div>

      {/* Corner by Corner Breakdown Table */}
      <div className="comp-corners-panel">
        <div className="comp-corners-header">
          <span className="corners-title">CORNER-BY-CORNER TELEMETRY BREAKDOWN</span>
        </div>
        <table className="comp-table">
          <thead>
            <tr>
              <th>Sector / Corner</th>
              <th>Track Distance</th>
              <th>Ref Apex Vmin</th>
              <th>Cmp Apex Vmin</th>
              <th>Speed Delta</th>
              <th>Time Delta (Δt)</th>
              <th>Braking Point</th>
            </tr>
          </thead>
          <tbody>
            {DEFAULT_CORNERS.map((c) => {
              const speedDiff = c.cmpSpeedKmh - c.refSpeedKmh;
              return (
                <tr key={c.cornerName}>
                  <td className="corner-name-cell">{c.cornerName}</td>
                  <td>{c.distM} m</td>
                  <td>{c.refSpeedKmh.toFixed(1)} km/h</td>
                  <td>{c.cmpSpeedKmh.toFixed(1)} km/h</td>
                  <td className={speedDiff >= 0 ? 'text-green' : 'text-red'}>
                    {speedDiff >= 0 ? '+' : ''}{speedDiff.toFixed(1)} km/h
                  </td>
                  <td className={c.deltaS < 0 ? 'text-green font-bold' : 'text-red font-bold'}>
                    {c.deltaS < 0 ? '' : '+'}{c.deltaS.toFixed(3)}s
                  </td>
                  <td>{c.apexDeltaM < 0 ? `${Math.abs(c.apexDeltaM)}m later` : `${c.apexDeltaM}m earlier`}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
