import { useState, useMemo, type JSX } from 'react';
import type { TyreDegradationPoint, VehicleDynamicsState } from '../types';
import { DEFAULT_LOCKUP_EVENTS, TyreDegQuickSummary } from './TyreDegQuickSummary';

export interface TyreDegradationViewProps {
  readonly state?: VehicleDynamicsState | undefined;
}

// Generate realistic 25-lap wear telemetry points for Spa-Francorchamps
const DEFAULT_DEG_POINTS: readonly TyreDegradationPoint[] = Array.from({ length: 25 }, (_, i) => {
  const lap = i + 1;
  // Non-linear wear acceleration after lap 18
  const baseFl = 100 - lap * 0.88 - (lap > 18 ? (lap - 18) * 0.45 : 0);
  const baseFr = 100 - lap * 0.79 - (lap > 18 ? (lap - 18) * 0.38 : 0);
  const baseRl = 100 - lap * 0.65 - (lap > 18 ? (lap - 18) * 0.25 : 0);
  const baseRr = 100 - lap * 0.61 - (lap > 18 ? (lap - 18) * 0.22 : 0);
  const grip = Math.max(0.7, 1.0 - lap * 0.009);
  const lapTime = 136.4 + (lap > 18 ? (lap - 18) * 0.18 : lap * 0.04);

  return {
    lapNumber: lap,
    wearFl: Number(Math.max(40, baseFl).toFixed(1)),
    wearFr: Number(Math.max(40, baseFr).toFixed(1)),
    wearRl: Number(Math.max(40, baseRl).toFixed(1)),
    wearRr: Number(Math.max(40, baseRr).toFixed(1)),
    gripIndex: Number(grip.toFixed(3)),
    lapTimeS: Number(lapTime.toFixed(3)),
  };
});

export function TyreDegradationView({ state }: TyreDegradationViewProps): JSX.Element {
  const [selectedWheel, setSelectedWheel] = useState<'ALL' | 'FL' | 'FR' | 'RL' | 'RR'>('ALL');
  const [severityFilter, setSeverityFilter] = useState<'ALL' | 'MILD' | 'MODERATE' | 'SEVERE'>('ALL');

  const history = state?.tyreDegradationHistory ?? DEFAULT_DEG_POINTS;
  const lockupEvents = state?.lockupEvents ?? DEFAULT_LOCKUP_EVENTS;

  // Filter lockup events
  const filteredEvents = useMemo(() => {
    return lockupEvents.filter((ev) => {
      if (selectedWheel !== 'ALL' && ev.wheel !== selectedWheel) return false;
      if (severityFilter !== 'ALL' && ev.severity !== severityFilter) return false;
      return true;
    });
  }, [lockupEvents, selectedWheel, severityFilter]);

  // Compute SVG wear curve path coordinates (Laps 1 to 35, Wear 100% to 50%)
  const svgW = 600;
  const svgH = 220;
  const padLeft = 45;
  const padBottom = 30;
  const padTop = 20;
  const padRight = 20;
  const chartW = svgW - padLeft - padRight;
  const chartH = svgH - padTop - padBottom;

  const maxLaps = 35;
  const minWear = 50;
  const maxWear = 100;

  const lapToX = (lap: number) => padLeft + ((lap - 1) / (maxLaps - 1)) * chartW;
  const wearToY = (wear: number) => padTop + ((maxWear - wear) / (maxWear - minWear)) * chartH;

  const flPath = useMemo(() => {
    return history.map((pt, i) => `${i === 0 ? 'M' : 'L'} ${lapToX(pt.lapNumber).toFixed(1)} ${wearToY(pt.wearFl).toFixed(1)}`).join(' ');
  }, [history]);

  const frPath = useMemo(() => {
    return history.map((pt, i) => `${i === 0 ? 'M' : 'L'} ${lapToX(pt.lapNumber).toFixed(1)} ${wearToY(pt.wearFr).toFixed(1)}`).join(' ');
  }, [history]);

  const rlPath = useMemo(() => {
    return history.map((pt, i) => `${i === 0 ? 'M' : 'L'} ${lapToX(pt.lapNumber).toFixed(1)} ${wearToY(pt.wearRl).toFixed(1)}`).join(' ');
  }, [history]);

  const rrPath = useMemo(() => {
    return history.map((pt, i) => `${i === 0 ? 'M' : 'L'} ${lapToX(pt.lapNumber).toFixed(1)} ${wearToY(pt.wearRr).toFixed(1)}`).join(' ');
  }, [history]);

  const cliffY = wearToY(75); // 75% grip cliff line

  return (
    <div className="view-card tyre-degradation-view" data-testid="tyre-degradation-view">
      <div className="view-card-header">
        <div className="view-card-title-group">
          <span className="view-card-glyph">🛞</span>
          <h2 className="view-card-title">Tyre Degradation Curves & Wheel Lock-Up Flatspot Logger</h2>
          <span className="badge badge-accent">V1.1 STINT ANALYTICS</span>
        </div>
        <div className="view-card-controls">
          <span className="view-control-label">FILTER WHEEL:</span>
          <div className="btn-group">
            {(['ALL', 'FL', 'FR', 'RL', 'RR'] as const).map((w) => (
              <button
                key={w}
                type="button"
                className={`btn btn-xs ${selectedWheel === w ? 'btn-active' : ''}`}
                onClick={() => setSelectedWheel(w)}
              >
                {w}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Quick Glance Summary Strip */}
      <div className="tyre-deg-quick-banner">
        <TyreDegQuickSummary wheels={state?.wheels} lockupEvents={lockupEvents} />
      </div>

      <div className="tyre-deg-content-grid">
        {/* Left Column: Multi-Lap Degradation SVG Curves */}
        <div className="tyre-deg-chart-card">
          <div className="chart-card-header">
            <span className="chart-card-title">TYRE WEAR DEGRADATION VS STINT LAPS</span>
            <div className="chart-legend-row">
              <span className="legend-chip chip-fl"><span className="chip-dot" /> FL (Front Left)</span>
              <span className="legend-chip chip-fr"><span className="chip-dot" /> FR (Front Right)</span>
              <span className="legend-chip chip-rl"><span className="chip-dot" /> RL (Rear Left)</span>
              <span className="legend-chip chip-rr"><span className="chip-dot" /> RR (Rear Right)</span>
              <span className="legend-chip chip-cliff"><span className="chip-line" /> 75% Grip Cliff</span>
            </div>
          </div>

          <div className="deg-svg-wrapper">
            <svg viewBox={`0 0 ${svgW} ${svgH}`} className="deg-curve-svg" role="img" aria-label="Tyre degradation graph">
              {/* Horizontal Grid Lines */}
              {[100, 90, 80, 75, 70, 60, 50].map((w) => {
                const y = wearToY(w);
                const isCliff = w === 75;
                return (
                  <g key={`grid-y-${w}`}>
                    <line
                      x1={padLeft}
                      y1={y}
                      x2={svgW - padRight}
                      y2={y}
                      stroke={isCliff ? '#ef4444' : '#1e293b'}
                      strokeWidth={isCliff ? '1.5' : '1'}
                      strokeDasharray={isCliff ? '4 3' : '2 2'}
                    />
                    <text
                      x={padLeft - 6}
                      y={y + 3}
                      fill={isCliff ? '#ef4444' : '#64748b'}
                      fontSize="10"
                      textAnchor="end"
                      fontWeight={isCliff ? 'bold' : 'normal'}
                    >
                      {w}%
                    </text>
                  </g>
                );
              })}

              {/* Vertical Lap Grid Lines */}
              {[5, 10, 15, 20, 25, 30, 35].map((lap) => {
                const x = lapToX(lap);
                return (
                  <g key={`grid-x-${lap}`}>
                    <line x1={x} y1={padTop} x2={x} y2={svgH - padBottom} stroke="#1e293b" strokeWidth="1" strokeDasharray="2 2" />
                    <text x={x} y={svgH - padBottom + 14} fill="#64748b" fontSize="10" textAnchor="middle">
                      L{lap}
                    </text>
                  </g>
                );
              })}

              {/* Cliff Danger Area Shading below 75% */}
              <rect
                x={padLeft}
                y={cliffY}
                width={chartW}
                height={svgH - padBottom - cliffY}
                fill="#ef4444"
                fillOpacity="0.04"
              />
              <text x={svgW - padRight - 10} y={cliffY - 6} fill="#ef4444" fontSize="10" textAnchor="end" fontWeight="bold">
                ⚠️ CLIFF THRESHOLD (RAPID PACE LOSS)
              </text>

              {/* 4 Tyre Wear Traces */}
              {(selectedWheel === 'ALL' || selectedWheel === 'RR') && (
                <path d={rrPath} fill="none" stroke="#a855f7" strokeWidth="2.5" />
              )}
              {(selectedWheel === 'ALL' || selectedWheel === 'RL') && (
                <path d={rlPath} fill="none" stroke="#f59e0b" strokeWidth="2.5" />
              )}
              {(selectedWheel === 'ALL' || selectedWheel === 'FR') && (
                <path d={frPath} fill="none" stroke="#10b981" strokeWidth="2.5" />
              )}
              {(selectedWheel === 'ALL' || selectedWheel === 'FL') && (
                <path d={flPath} fill="none" stroke="#22d3ee" strokeWidth="2.5" />
              )}

              {/* Projected Cliff Intercept Point on FL */}
              <circle cx={lapToX(22)} cy={cliffY} r="5" fill="#ef4444" stroke="#ffffff" strokeWidth="1.5" />
              <text x={lapToX(22)} y={cliffY + 18} fill="#ef4444" fontSize="9" textAnchor="middle" fontWeight="bold">
                FL Cliff: Lap 22
              </text>
            </svg>
          </div>

          {/* Double-Stint Strategy Assessment Card */}
          <div className="double-stint-card">
            <div className="double-stint-header">
              <span className="double-stint-title">DOUBLE-STINT VIABILITY ASSESSMENT</span>
              <span className="badge badge-accent">2 x 26 LAPS = 52 LAPS TOTAL</span>
            </div>
            <div className="double-stint-grid">
              <div className="double-stint-stat">
                <span className="ds-label">Pit Stop Time Saved:</span>
                <span className="ds-val num-green">-28.5s (Fuel-Only Stop)</span>
              </div>
              <div className="double-stint-stat">
                <span className="ds-label">Degradation Lap Loss:</span>
                <span className="ds-val num-amber">+0.68s / lap in 2nd Stint</span>
              </div>
              <div className="double-stint-stat">
                <span className="ds-label">Net Stint Time Delta:</span>
                <span className="ds-val num-cyan font-bold">-10.8s Total Advantage</span>
              </div>
              <div className="double-stint-stat">
                <span className="ds-label">Safety Recommendation:</span>
                <span className="ds-val text-white font-semibold">✅ VIABLE (Maintain brake bias 54.5%)</span>
              </div>
            </div>
          </div>
        </div>

        {/* Right Column: Chronological Lock-Up & Flatspot Logger */}
        <div className="tyre-deg-log-card">
          <div className="log-card-header">
            <div className="log-title-group">
              <span className="log-card-title">LOCK-UP & FLATSPOT EVENT LOG</span>
              <span className="badge badge-subtle">{filteredEvents.length} Recorded</span>
            </div>
            <div className="log-filters">
              <select
                className="view-select-sm"
                value={severityFilter}
                onChange={(e) => setSeverityFilter(e.target.value as 'ALL' | 'MILD' | 'MODERATE' | 'SEVERE')}
                aria-label="Filter lockup events by severity"
              >
                <option value="ALL">Severity: All</option>
                <option value="MILD">Mild (&lt;30% slip)</option>
                <option value="MODERATE">Moderate (30-50%)</option>
                <option value="SEVERE">Severe (&gt;50%)</option>
              </select>
            </div>
          </div>

          <div className="lockup-table-wrapper">
            <table className="lockup-table">
              <thead>
                <tr>
                  <th>Lap</th>
                  <th>Corner</th>
                  <th>Wheel</th>
                  <th>Duration</th>
                  <th>Slip</th>
                  <th>Damage</th>
                  <th>Severity</th>
                </tr>
              </thead>
              <tbody>
                {filteredEvents.map((ev) => (
                  <tr key={ev.id} className={`lockup-row-severity-${ev.severity.toLowerCase()}`}>
                    <td className="font-bold text-white">L{ev.lapNumber}</td>
                    <td className="text-white">{ev.cornerName}</td>
                    <td className="num-cyan font-bold">{ev.wheel}</td>
                    <td>{ev.durationMs} ms</td>
                    <td className="num-red font-semibold">{ev.peakSlipPct.toFixed(1)}%</td>
                    <td className="num-amber">+{ev.flatspotDamagePct.toFixed(0)}%</td>
                    <td>
                      <span className={`severity-pill pill-${ev.severity.toLowerCase()}`}>
                        {ev.severity}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* Flatspot Actionable Guidance */}
          <div className="flatspot-guidance-card">
            <span className="guidance-title">🔧 RACE ENGINEER ACTION PLAN:</span>
            <ul className="guidance-list">
              <li>
                <strong>FL Flatspot on Lap 12 (T1):</strong> 14% flatspot damage. Vibration index is 3.8/10. If vibration exceeds 5.0, shift brake bias rearward by 0.5% to reduce front lockup recurrence.
              </li>
              <li>
                <strong>Front Left Wear:</strong> Running at 0.88%/lap. Avoid aggressive trail-braking over T14 curbing to protect tyre structure through Eau Rouge.
              </li>
            </ul>
          </div>
        </div>
      </div>
    </div>
  );
}
