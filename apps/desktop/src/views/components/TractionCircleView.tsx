import { useState, useMemo, type JSX } from 'react';
import type { VehicleDynamicsState } from '../types';

export interface TractionCircleViewProps {
  readonly state?: VehicleDynamicsState | undefined;
  readonly maxG?: number | undefined;
}

interface GPoint {
  readonly latG: number;
  readonly longG: number;
  readonly phase: 'braking' | 'trail-braking' | 'cornering' | 'acceleration' | 'neutral';
  readonly speedKmh?: number | undefined;
}

const DEFAULT_SCATTER: readonly GPoint[] = [
  // Straight braking into Turn 1
  { latG: 0.02, longG: -2.25, phase: 'braking', speedKmh: 285 },
  { latG: 0.05, longG: -2.18, phase: 'braking', speedKmh: 260 },
  { latG: 0.12, longG: -1.95, phase: 'braking', speedKmh: 230 },
  // Trail-braking turn-in (transition from pure brake to lateral)
  { latG: 0.45, longG: -1.65, phase: 'trail-braking', speedKmh: 195 },
  { latG: 0.85, longG: -1.35, phase: 'trail-braking', speedKmh: 165 },
  { latG: 1.25, longG: -0.92, phase: 'trail-braking', speedKmh: 135 },
  { latG: 1.62, longG: -0.45, phase: 'trail-braking', speedKmh: 110 },
  // Apex pure cornering
  { latG: 1.88, longG: -0.05, phase: 'cornering', speedKmh: 98 },
  { latG: 1.92, longG: 0.08, phase: 'cornering', speedKmh: 102 },
  { latG: 1.85, longG: 0.22, phase: 'cornering', speedKmh: 112 },
  // Corner exit acceleration
  { latG: 1.45, longG: 0.48, phase: 'acceleration', speedKmh: 130 },
  { latG: 0.95, longG: 0.68, phase: 'acceleration', speedKmh: 155 },
  { latG: 0.45, longG: 0.82, phase: 'acceleration', speedKmh: 185 },
  { latG: 0.10, longG: 0.88, phase: 'acceleration', speedKmh: 215 },
  // Left-hander transition (Chicane)
  { latG: -0.35, longG: -1.45, phase: 'trail-braking', speedKmh: 180 },
  { latG: -1.15, longG: -1.05, phase: 'trail-braking', speedKmh: 145 },
  { latG: -1.78, longG: -0.15, phase: 'cornering', speedKmh: 115 },
  { latG: -1.82, longG: 0.15, phase: 'cornering', speedKmh: 122 },
  { latG: -1.25, longG: 0.55, phase: 'acceleration', speedKmh: 150 },
  { latG: -0.55, longG: 0.75, phase: 'acceleration', speedKmh: 180 },
];

export function TractionCircleView({
  state,
  maxG = 2.5,
}: TractionCircleViewProps): JSX.Element {
  const [trailLength, setTrailLength] = useState<number>(100);
  const [showFrictionEnvelope, setShowFrictionEnvelope] = useState<boolean>(true);

  const liveLatG = state?.latG ?? 1.45;
  const liveLongG = state?.longG ?? 0.62;

  // Compute live combined G magnitude: sqrt(lat^2 + long^2)
  const combinedG = Math.sqrt(liveLatG * liveLatG + liveLongG * liveLongG);
  const frictionLimit = 2.15; // Estimated limit for Hypercar/LMH downforce levels
  const gripUtilizationPct = Math.min(100, (combinedG / frictionLimit) * 100);

  // Map (-maxG .. +maxG) to SVG coordinate space (0 .. 300)
  const svgCenter = 150;
  const svgScale = 120 / maxG; // 120px for maxG

  const toSvgX = (lat: number) => svgCenter + lat * svgScale;
  const toSvgY = (long: number) => svgCenter - long * svgScale;

  const liveDotX = toSvgX(liveLatG);
  const liveDotY = toSvgY(liveLongG);

  // Generate friction envelope ellipse path
  // Lateral limit: ~2.0G, Braking limit: ~2.3G, Acceleration limit: ~0.9G
  const ellipsePoints = useMemo(() => {
    const pts: string[] = [];
    const steps = 64;
    for (let i = 0; i <= steps; i++) {
      const angle = (i / steps) * Math.PI * 2;
      const lat = Math.sin(angle) * 1.95;
      const long = Math.cos(angle) >= 0 ? Math.cos(angle) * 0.95 : Math.cos(angle) * 2.35;
      const x = toSvgX(lat);
      const y = toSvgY(long);
      pts.push(`${i === 0 ? 'M' : 'L'} ${x.toFixed(1)} ${y.toFixed(1)}`);
    }
    pts.push('Z');
    return pts.join(' ');
  }, [maxG]);

  const scatterPoints = DEFAULT_SCATTER;

  return (
    <div className="view-card traction-circle-view" data-testid="traction-circle-view">
      <div className="view-card-header">
        <div className="view-card-title-group">
          <span className="view-card-glyph">🎯</span>
          <h2 className="view-card-title">Traction Circle (G-G Diagram)</h2>
          <span className="badge badge-accent">V1.1 CHASSIS DYNAMICS</span>
        </div>
        <div className="view-card-controls">
          <label className="view-control-label">
            <input
              type="checkbox"
              checked={showFrictionEnvelope}
              onChange={(e) => setShowFrictionEnvelope(e.target.checked)}
            />
            Limit Envelope
          </label>
          <select
            className="view-select-sm"
            value={trailLength}
            onChange={(e) => setTrailLength(Number(e.target.value))}
            aria-label="Scatter trail depth"
          >
            <option value={50}>Trail: 50 pts</option>
            <option value={100}>Trail: 100 pts</option>
            <option value={200}>Trail: 200 pts</option>
          </select>
        </div>
      </div>

      <div className="traction-circle-content-grid">
        {/* Main High-Resolution G-G Polar / Cartesian Plot */}
        <div className="traction-circle-plot-card">
          <div className="traction-circle-svg-wrapper">
            <svg viewBox="0 0 300 300" className="traction-circle-svg" role="img" aria-label="G-G diagram plot">
              {/* Radial gradient background */}
              <defs>
                <radialGradient id="ggGripGlow" cx="50%" cy="50%" r="50%">
                  <stop offset="0%" stopColor="#22d3ee" stopOpacity="0.12" />
                  <stop offset="60%" stopColor="#3b82f6" stopOpacity="0.05" />
                  <stop offset="100%" stopColor="#0f172a" stopOpacity="0.0" />
                </radialGradient>
              </defs>

              <rect x="0" y="0" width="300" height="300" fill="url(#ggGripGlow)" />

              {/* Crosshair Axes */}
              <line x1="20" y1={svgCenter} x2="280" y2={svgCenter} stroke="#334155" strokeWidth="1.2" />
              <line x1={svgCenter} y1="20" x2={svgCenter} y2="280" stroke="#334155" strokeWidth="1.2" />

              {/* Concentric G Rings */}
              {[0.5, 1.0, 1.5, 2.0, 2.5].map((g) => {
                const r = g * svgScale;
                return (
                  <g key={`ring-${g}`}>
                    <circle
                      cx={svgCenter}
                      cy={svgCenter}
                      r={r}
                      fill="none"
                      stroke={g === 2.0 ? '#0284c7' : '#1e293b'}
                      strokeWidth={g === 2.0 ? '1.5' : '1'}
                      strokeDasharray={g === 2.0 ? 'none' : '3 3'}
                    />
                    <text
                      x={svgCenter + 4}
                      y={svgCenter - r + 11}
                      fill="#64748b"
                      fontSize="9"
                      fontFamily="monospace"
                    >
                      {g.toFixed(1)}G
                    </text>
                  </g>
                );
              })}

              {/* Quadrant Regime Labels */}
              <text x="210" y="32" fill="#10b981" fontSize="10" fontWeight="bold">ACCEL + RIGHT</text>
              <text x="25" y="32" fill="#10b981" fontSize="10" fontWeight="bold">ACCEL + LEFT</text>
              <text x="210" y="278" fill="#ef4444" fontSize="10" fontWeight="bold">BRAKE + RIGHT</text>
              <text x="25" y="278" fill="#ef4444" fontSize="10" fontWeight="bold">BRAKE + LEFT</text>

              {/* Theoretical Friction Limit Envelope */}
              {showFrictionEnvelope && (
                <path
                  d={ellipsePoints}
                  fill="none"
                  stroke="#f59e0b"
                  strokeWidth="2"
                  strokeDasharray="4 2"
                  strokeOpacity="0.8"
                />
              )}

              {/* Telemetry Scatter Points */}
              {scatterPoints.map((pt, idx) => {
                const color =
                  pt.phase === 'braking'
                    ? '#ef4444'
                    : pt.phase === 'trail-braking'
                    ? '#f59e0b'
                    : pt.phase === 'cornering'
                    ? '#22d3ee'
                    : '#10b981';
                return (
                  <circle
                    key={`pt-${idx}`}
                    cx={toSvgX(pt.latG)}
                    cy={toSvgY(pt.longG)}
                    r="3"
                    fill={color}
                    fillOpacity="0.75"
                    stroke="#0f172a"
                    strokeWidth="0.5"
                  />
                );
              })}

              {/* Dynamic Live Position Indicator with Vector Line */}
              <line
                x1={svgCenter}
                y1={svgCenter}
                x2={liveDotX}
                y2={liveDotY}
                stroke="#22d3ee"
                strokeWidth="1.5"
                strokeOpacity="0.6"
              />
              <circle cx={liveDotX} cy={liveDotY} r="7" fill="#ef4444" fillOpacity="0.3" />
              <circle cx={liveDotX} cy={liveDotY} r="4.5" fill="#ef4444" stroke="#ffffff" strokeWidth="1.5" />
            </svg>
          </div>

          <div className="traction-circle-legend">
            <span className="legend-item"><span className="legend-dot dot-red" /> Braking</span>
            <span className="legend-item"><span className="legend-dot dot-amber" /> Trail-Braking</span>
            <span className="legend-item"><span className="legend-dot dot-cyan" /> Apex Cornering</span>
            <span className="legend-item"><span className="legend-dot dot-green" /> Traction Accel</span>
            <span className="legend-item"><span className="legend-line line-amber" /> Grip Envelope</span>
          </div>
        </div>

        {/* Right Column: Grip Telemetry & Technique Analytics */}
        <div className="traction-circle-stats-col">
          {/* Live G Status Card */}
          <div className="stat-card stat-card-live-g">
            <div className="stat-card-label">CURRENT CHASSIS LOAD</div>
            <div className="stat-main-row">
              <span className="stat-hero-number">{combinedG.toFixed(2)} G</span>
              <span className="stat-sub-badge">COMBINED</span>
            </div>
            <div className="stat-split-row">
              <div className="stat-split-item">
                <span className="split-label">Lateral G:</span>
                <span className="split-value num-cyan">
                  {liveLatG >= 0 ? `+${liveLatG.toFixed(2)} G (Right)` : `${liveLatG.toFixed(2)} G (Left)`}
                </span>
              </div>
              <div className="stat-split-item">
                <span className="split-label">Longitudinal G:</span>
                <span className={`split-value ${liveLongG < 0 ? 'num-red' : 'num-green'}`}>
                  {liveLongG < 0 ? `${liveLongG.toFixed(2)} G (Brake)` : `+${liveLongG.toFixed(2)} G (Drive)`}
                </span>
              </div>
            </div>

            {/* Grip Utilization Bar */}
            <div className="grip-util-container">
              <div className="grip-util-header">
                <span className="grip-util-title">Friction Limit Utilization</span>
                <span className="grip-util-pct">{gripUtilizationPct.toFixed(1)}%</span>
              </div>
              <div className="grip-util-track">
                <div
                  className="grip-util-fill"
                  style={{
                    width: `${gripUtilizationPct}%`,
                    backgroundColor:
                      gripUtilizationPct > 90
                        ? '#ef4444'
                        : gripUtilizationPct > 70
                        ? '#10b981'
                        : '#3b82f6',
                  }}
                />
              </div>
            </div>
          </div>

          {/* Peak G Envelope Breakdown */}
          <div className="stat-card">
            <div className="stat-card-label">SESSION PEAK ENVELOPE</div>
            <div className="envelope-grid">
              <div className="envelope-box">
                <span className="envelope-tag">PEAK BRAKING</span>
                <span className="envelope-val num-red">-2.35 G</span>
                <span className="envelope-loc">T1 Braking Zone</span>
              </div>
              <div className="envelope-box">
                <span className="envelope-tag">PEAK LAT LEFT</span>
                <span className="envelope-val num-cyan">1.94 G</span>
                <span className="envelope-loc">Eau Rouge / Raidillon</span>
              </div>
              <div className="envelope-box">
                <span className="envelope-tag">PEAK LAT RIGHT</span>
                <span className="envelope-val num-cyan">1.88 G</span>
                <span className="envelope-loc">Pouhon (Double-Apex)</span>
              </div>
              <div className="envelope-box">
                <span className="envelope-tag">PEAK ACCEL</span>
                <span className="envelope-val num-green">+0.88 G</span>
                <span className="envelope-loc">Kemmel Straight Launch</span>
              </div>
            </div>
          </div>

          {/* Driving Technique & Trail-Braking Rating */}
          <div className="stat-card">
            <div className="stat-card-label">TRAIL-BRAKING & TECHNIQUE ANALYSIS</div>
            <div className="technique-item-row">
              <span className="technique-label">Trail-Braking Transition:</span>
              <span className="technique-score score-opt">94 / 100 (Optimal Roll-in)</span>
            </div>
            <div className="technique-item-row">
              <span className="technique-label">Friction Circle Area Fill:</span>
              <span className="technique-score score-good">86.2% Coverage</span>
            </div>
            <div className="technique-item-row">
              <span className="technique-label">Brake Release Linearity:</span>
              <span className="technique-score score-opt">Continuous (-12% decay/s)</span>
            </div>
            <div className="technique-item-row">
              <span className="technique-label">Apex Min-Speed Balance:</span>
              <span className="technique-score score-good">+2.4 km/h vs Ref</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
