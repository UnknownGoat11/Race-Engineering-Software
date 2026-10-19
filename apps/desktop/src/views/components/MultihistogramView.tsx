import type { JSX } from 'react';

export function MultihistogramView(): JSX.Element {
  const speedBins = [
    { label: '50-100 km/h', pct: 8.2 },
    { label: '100-150 km/h', pct: 14.5 },
    { label: '150-200 km/h', pct: 21.0 },
    { label: '200-250 km/h', pct: 28.4 },
    { label: '250-300 km/h', pct: 22.1 },
    { label: '300+ km/h', pct: 5.8 },
  ];

  const throttleBins = [
    { label: '0% (Off / Coast)', pct: 16.4, color: '#94a3b8' },
    { label: '1-25% (Modulation)', pct: 4.2, color: '#38bdf8' },
    { label: '26-50% (Mid-corner)', pct: 6.8, color: '#0ea5e9' },
    { label: '51-75% (Traction)', pct: 11.2, color: '#0284c7' },
    { label: '76-99% (Exit Roll)', pct: 8.4, color: '#0369a1' },
    { label: '100% (WOT Full)', pct: 53.0, color: '#22c55e' },
  ];

  const brakeBins = [
    { label: '0% (No Brake)', pct: 78.5, color: '#64748b' },
    { label: '1-20% (Trail)', pct: 6.2, color: '#38bdf8' },
    { label: '21-50% (Medium)', pct: 4.8, color: '#f59e0b' },
    { label: '51-80% (Hard)', pct: 5.1, color: '#f97316' },
    { label: '80-100% (Threshold)', pct: 5.4, color: '#ef4444' },
  ];

  const gearUsage = [
    { gear: '1st', pct: 4.2 },
    { gear: '2nd', pct: 9.8 },
    { gear: '3rd', pct: 18.4 },
    { gear: '4th', pct: 24.6 },
    { gear: '5th', pct: 26.2 },
    { gear: '6th', pct: 16.8 },
  ];

  return (
    <div className="multihistogram-view" data-testid="multihistogram-view">
      <div className="hist-header">
        <span className="badge badge-subtle">V1.1 FREQUENCY ANALYSIS</span>
        <h2 className="hist-title">Multi-Histogram Channel Distributions</h2>
        <p className="hist-desc">
          Telemetry sample frequency distribution across Speed bands, Throttle duty cycles, Braking pressures, and Gear selection.
        </p>
      </div>

      <div className="hist-grid">
        {/* Speed Histogram */}
        <div className="hist-card">
          <div className="hist-card-header">
            <span className="hist-card-title">SPEED DISTRIBUTION (KM/H)</span>
            <span className="hist-card-sub">Apex vs Straight distribution</span>
          </div>
          <div className="hist-bars-list">
            {speedBins.map((bin) => (
              <div key={bin.label} className="hist-bar-row">
                <span className="hist-bar-label">{bin.label}</span>
                <div className="hist-bar-track">
                  <div className="hist-bar-fill fill-cyan" style={{ width: `${(bin.pct / 30) * 100}%` }} />
                </div>
                <span className="hist-bar-val">{bin.pct.toFixed(1)}%</span>
              </div>
            ))}
          </div>
        </div>

        {/* Throttle Histogram */}
        <div className="hist-card">
          <div className="hist-card-header">
            <span className="hist-card-title">THROTTLE DUTY CYCLE (% PEDAL)</span>
            <span className="hist-card-sub">WOT: 53.0% of lap</span>
          </div>
          <div className="hist-bars-list">
            {throttleBins.map((bin) => (
              <div key={bin.label} className="hist-bar-row">
                <span className="hist-bar-label">{bin.label}</span>
                <div className="hist-bar-track">
                  <div className="hist-bar-fill" style={{ width: `${bin.pct}%`, backgroundColor: bin.color }} />
                </div>
                <span className="hist-bar-val">{bin.pct.toFixed(1)}%</span>
              </div>
            ))}
          </div>
        </div>

        {/* Brake Pressure Histogram */}
        <div className="hist-card">
          <div className="hist-card-header">
            <span className="hist-card-title">BRAKE PRESSURE BINS</span>
            <span className="hist-card-sub">Threshold braking: 5.4%</span>
          </div>
          <div className="hist-bars-list">
            {brakeBins.map((bin) => (
              <div key={bin.label} className="hist-bar-row">
                <span className="hist-bar-label">{bin.label}</span>
                <div className="hist-bar-track">
                  <div className="hist-bar-fill" style={{ width: `${(bin.pct / 80) * 100}%`, backgroundColor: bin.color }} />
                </div>
                <span className="hist-bar-val">{bin.pct.toFixed(1)}%</span>
              </div>
            ))}
          </div>
        </div>

        {/* Gear Usage Histogram */}
        <div className="hist-card">
          <div className="hist-card-header">
            <span className="hist-card-title">GEAR UTILIZATION RATIO</span>
            <span className="hist-card-sub">Dominant gears: 4th & 5th</span>
          </div>
          <div className="hist-bars-list">
            {gearUsage.map((bin) => (
              <div key={bin.gear} className="hist-bar-row">
                <span className="hist-bar-label">{bin.gear}</span>
                <div className="hist-bar-track">
                  <div className="hist-bar-fill fill-amber" style={{ width: `${(bin.pct / 30) * 100}%` }} />
                </div>
                <span className="hist-bar-val">{bin.pct.toFixed(1)}%</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
