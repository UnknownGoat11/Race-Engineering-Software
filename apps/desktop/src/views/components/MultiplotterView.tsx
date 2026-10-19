import { useState, type JSX } from 'react';

export type PlotMode = 'speed_rpm' | 'steer_latg' | 'brake_decel';

export function MultiplotterView(): JSX.Element {
  const [plotMode, setPlotMode] = useState<PlotMode>('steer_latg');

  // Synthetic scatter points for each plot mode
  const points = Array.from({ length: 60 }).map((_, i) => {
    if (plotMode === 'speed_rpm') {
      const v = 60 + (i / 60) * 260; // 60 to 320 km/h
      const rpm = 4500 + (v / 320) * 4400 + (Math.sin(i) * 300);
      return { x: (v / 340) * 100, y: 100 - (rpm / 9500) * 100 };
    }
    if (plotMode === 'steer_latg') {
      const steer = -35 + (i / 60) * 70; // -35 to +35 deg
      const latG = (steer / 35) * 2.1 + (Math.cos(i) * 0.15); // -2.1 to +2.1G
      return { x: 50 + (steer / 40) * 45, y: 50 - (latG / 2.5) * 45 };
    }
    // brake vs decel
    const brake = (i / 60) * 1.0;
    const decelG = brake * 2.8 + (Math.sin(i * 2) * 0.18);
    return { x: brake * 100, y: 100 - (decelG / 3.2) * 100 };
  });

  return (
    <div className="multiplotter-view" data-testid="multiplotter-view">
      <div className="plotter-header">
        <div className="plotter-title-group">
          <span className="badge badge-subtle">V1.1 X-Y CORRELATION</span>
          <h2 className="plotter-title">Multiplotter (Channel Cross-Correlation Scatter)</h2>
          <p className="plotter-desc">
            Evaluate vehicle dynamics correlations: understeer/oversteer gradient, gear ratio matching, and braking friction efficiency.
          </p>
        </div>

        <div className="plotter-mode-pills" role="radiogroup" aria-label="Scatter Mode">
          <button
            type="button"
            className={`btn btn-xs ${plotMode === 'steer_latg' ? 'btn-primary' : ''}`}
            onClick={() => {
              setPlotMode('steer_latg');
            }}
          >
            Steering vs Lateral G (Understeer Gradient)
          </button>
          <button
            type="button"
            className={`btn btn-xs ${plotMode === 'speed_rpm' ? 'btn-primary' : ''}`}
            onClick={() => {
              setPlotMode('speed_rpm');
            }}
          >
            Speed vs RPM (Gearing)
          </button>
          <button
            type="button"
            className={`btn btn-xs ${plotMode === 'brake_decel' ? 'btn-primary' : ''}`}
            onClick={() => {
              setPlotMode('brake_decel');
            }}
          >
            Brake % vs Decel G (Pad Friction)
          </button>
        </div>
      </div>

      <div className="plotter-viewport-card">
        <div className="plotter-svg-wrapper">
          <svg viewBox="0 0 100 100" className="plotter-svg" preserveAspectRatio="none">
            {/* Grid */}
            <line x1="0" y1="50" x2="100" y2="50" stroke="#334155" strokeWidth="0.5" strokeDasharray="2 2" />
            <line x1="50" y1="0" x2="50" y2="100" stroke="#334155" strokeWidth="0.5" strokeDasharray="2 2" />

            {/* Regression trendline */}
            {plotMode === 'steer_latg' && (
              <line x1="10" y1="90" x2="90" y2="10" stroke="#22d3ee" strokeWidth="1" strokeDasharray="3 3" opacity="0.7" />
            )}
            {plotMode === 'speed_rpm' && (
              <line x1="10" y1="80" x2="95" y2="15" stroke="#22d3ee" strokeWidth="1" strokeDasharray="3 3" opacity="0.7" />
            )}
            {plotMode === 'brake_decel' && (
              <line x1="5" y1="95" x2="95" y2="10" stroke="#22d3ee" strokeWidth="1" strokeDasharray="3 3" opacity="0.7" />
            )}

            {/* Data Scatter Points */}
            {points.map((p, i) => (
              <circle key={i} cx={p.x} cy={p.y} r="1.5" fill="#38bdf8" opacity="0.85" />
            ))}
          </svg>
        </div>

        <div className="plotter-sidebar-stats">
          <div className="plot-stat-card">
            <span className="stat-label">Pearson Correlation (r)</span>
            <span className="stat-value num-cyan">0.964</span>
            <span className="stat-note">Strong linear mechanical grip correlation</span>
          </div>
          <div className="plot-stat-card">
            <span className="stat-label">Gradient Slope</span>
            <span className="stat-value">0.058 G / deg</span>
            <span className="stat-note">Neutral to slight progressive understeer at limit</span>
          </div>
          <div className="plot-stat-card">
            <span className="stat-label">Grip Saturation Point</span>
            <span className="stat-value text-amber">2.18 G</span>
            <span className="stat-note">Tyre slip angle saturation reached at ~28° steer</span>
          </div>
        </div>
      </div>
    </div>
  );
}
