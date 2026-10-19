import type { JSX } from 'react';

export interface FieldSpreadCarData {
  readonly carNumber: string;
  readonly name: string;
  readonly color: string;
  readonly gapsOverLaps: readonly number[]; // gap to leader in seconds per lap
}

const DEFAULT_CARS: readonly FieldSpreadCarData[] = [
  {
    carNumber: '7',
    name: 'Toyota GR010 #7 (Player)',
    color: '#22d3ee', // Cyan
    gapsOverLaps: [0.0, 0.4, 0.2, 0.0, 0.0, 0.6, 1.2, 1.5, 0.8, 0.0, 24.5, 18.2, 12.4, 6.2, 0.0, 0.5],
  },
  {
    carNumber: '51',
    name: 'Ferrari 499P #51',
    color: '#ef4444', // Red
    gapsOverLaps: [1.2, 0.8, 0.0, 1.1, 1.8, 2.4, 2.9, 3.4, 3.8, 4.2, 4.8, 28.6, 22.4, 16.5, 10.2, 4.8],
  },
  {
    carNumber: '6',
    name: 'Porsche 963 #6',
    color: '#f59e0b', // Amber
    gapsOverLaps: [2.5, 2.1, 2.8, 3.5, 4.1, 4.6, 5.2, 5.8, 6.4, 7.1, 7.8, 8.5, 32.1, 26.4, 20.8, 15.2],
  },
  {
    carNumber: '2',
    name: 'Cadillac V-Series.R #2',
    color: '#3b82f6', // Blue
    gapsOverLaps: [4.2, 4.9, 5.5, 6.2, 6.9, 7.8, 8.6, 9.4, 10.2, 11.1, 12.0, 13.1, 14.2, 38.5, 33.2, 28.1],
  },
];

export function FieldSpreadChartView({ cars = DEFAULT_CARS }: { readonly cars?: readonly FieldSpreadCarData[] }): JSX.Element {
  const maxLaps = 16;
  const maxGapS = 45;

  const width = 600;
  const height = 260;
  const padLeft = 45;
  const padRight = 20;
  const padTop = 20;
  const padBottom = 30;

  const plotW = width - padLeft - padRight;
  const plotH = height - padTop - padBottom;

  return (
    <div className="field-spread-view" data-testid="field-spread-view">
      <div className="spread-header">
        <div className="spread-title-group">
          <span className="badge badge-subtle">V1.1 GAP CHART</span>
          <h2 className="spread-title">Multi-Car Race Field Spread (Gap to Leader vs Laps)</h2>
          <p className="spread-desc">
            Visualizes tactical undercuts, pit stop out-laps, and traffic hold-ups. The leader stays on the 0.0s line at top.
          </p>
        </div>
        <div className="spread-legend">
          {cars.map((c) => (
            <span key={c.carNumber} className="legend-chip" style={{ color: c.color }}>
              ● #{c.carNumber} {c.name}
            </span>
          ))}
        </div>
      </div>

      <div className="spread-chart-card">
        <svg viewBox={`0 0 ${width} ${height}`} className="spread-svg" preserveAspectRatio="none">
          {/* Horizontal Grid lines (0s, 10s, 20s, 30s, 40s) */}
          {[0, 10, 20, 30, 40].map((gap) => {
            const y = padTop + (gap / maxGapS) * plotH;
            return (
              <g key={gap}>
                <line x1={padLeft} y1={y} x2={width - padRight} y2={y} stroke="#334155" strokeWidth="0.8" strokeDasharray="3 3" />
                <text x={padLeft - 6} y={y + 3} fill="#94a3b8" fontSize="9" textAnchor="end">
                  +{gap}s
                </text>
              </g>
            );
          })}

          {/* Vertical Lap Grid lines */}
          {Array.from({ length: maxLaps }).map((_, i) => {
            const x = padLeft + (i / (maxLaps - 1)) * plotW;
            return (
              <g key={i}>
                <line x1={x} y1={padTop} x2={x} y2={height - padBottom} stroke="#1e293b" strokeWidth="0.8" />
                <text x={x} y={height - padBottom + 14} fill="#64748b" fontSize="9" textAnchor="middle">
                  L{i + 1}
                </text>
              </g>
            );
          })}

          {/* Car Lines */}
          {cars.map((car) => {
            const points = car.gapsOverLaps
              .map((gap, i) => {
                const x = padLeft + (i / (maxLaps - 1)) * plotW;
                const y = padTop + Math.min(plotH, (gap / maxGapS) * plotH);
                return `${x.toFixed(1)},${y.toFixed(1)}`;
              })
              .join(' ');

            return (
              <g key={car.carNumber}>
                <polyline points={points} fill="none" stroke={car.color} strokeWidth="2.5" />
                {car.gapsOverLaps.map((gap, i) => {
                  const x = padLeft + (i / (maxLaps - 1)) * plotW;
                  const y = padTop + Math.min(plotH, (gap / maxGapS) * plotH);
                  // Highlight pit stops (gap jumps > 15s)
                  const prevGap = car.gapsOverLaps[i - 1] ?? 0;
                  const isPit = gap > 20 && i > 0 && prevGap < 10;
                  return (
                    <circle
                      key={i}
                      cx={x}
                      cy={y}
                      r={isPit ? 4.5 : 2.5}
                      fill={isPit ? '#eab308' : car.color}
                      stroke="#0b0d10"
                      strokeWidth="1"
                    />
                  );
                })}
              </g>
            );
          })}
        </svg>
      </div>

      <div className="spread-insights-grid">
        <div className="insight-card">
          <span className="insight-tag">STRATEGY ALERT</span>
          <span className="insight-val">Lap 10 Undercut Successful</span>
          <p className="insight-text">
            Player pitted on Lap 10, jumping Ferrari #51 by 4.8 seconds after clean air out-lap.
          </p>
        </div>
        <div className="insight-card">
          <span className="insight-tag">TRAFFIC BOTTLENECK</span>
          <span className="insight-val">Laps 6-8 LMGT3 Train</span>
          <p className="insight-text">
            Front runners dropped 1.2s per lap while navigating through 5 LMGT3 backmarkers in Sector 2.
          </p>
        </div>
        <div className="insight-card">
          <span className="insight-tag">PIT STOP DELTA</span>
          <span className="insight-val">24.5s Total Loss</span>
          <p className="insight-text">
            Stationary pit stop duration: 18.2s (4 tyres + fuel splash). Pit lane transit: 6.3s.
          </p>
        </div>
      </div>
    </div>
  );
}
