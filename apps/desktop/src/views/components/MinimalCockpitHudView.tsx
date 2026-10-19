import type { JSX } from 'react';

export function MinimalCockpitHudView(): JSX.Element {
  return (
    <div className="minimal-hud-view" data-testid="minimal-ui-view">
      <div className="minimal-hud-container">
        {/* Top Flag / Session Status Strip */}
        <div className="minimal-flag-strip">
          <span className="flag-badge flag-green">TRACK CLEAR (GREEN)</span>
          <span className="session-time-text">RACE • LAP 54 / 72</span>
          <span className="position-pill">P1 OVERALL</span>
        </div>

        {/* Center Primary Delta & Speed Cluster */}
        <div className="minimal-center-cluster">
          <div className="minimal-delta-box">
            <span className="minimal-delta-label">DELTA (VS PERSONAL BEST)</span>
            <div className="minimal-delta-number text-green">-0.342</div>
            <span className="minimal-delta-sub">GAINING IN SECTOR 2</span>
          </div>

          <div className="minimal-gear-speed-box">
            <div className="minimal-gear">4</div>
            <div className="minimal-speed">218 <span className="speed-unit">KM/H</span></div>
          </div>
        </div>

        {/* Bottom Status Bars: Fuel, RPM, Tyre Temps */}
        <div className="minimal-bottom-grid">
          {/* Fuel to Finish */}
          <div className="minimal-stat-card">
            <span className="min-stat-label">FUEL REMAINING</span>
            <div className="min-stat-val text-cyan">42.5 L</div>
            <span className="min-stat-sub">15.0 LAPS TO FINISH (OK +2.4L)</span>
          </div>

          {/* Lap Times */}
          <div className="minimal-stat-card">
            <span className="min-stat-label">LAST LAP / BEST LAP</span>
            <div className="min-stat-val">1:34.890</div>
            <span className="min-stat-sub text-purple">BEST: 1:34.420</span>
          </div>

          {/* Tyre Carcass Quick Thermals */}
          <div className="minimal-stat-card">
            <span className="min-stat-label">TYRE TEMPS (FL / FR / RL / RR)</span>
            <div className="min-stat-val text-green">85° • 83° • 89° • 88°</div>
            <span className="min-stat-sub">OPTIMAL OPERATING WINDOW</span>
          </div>
        </div>
      </div>
    </div>
  );
}
