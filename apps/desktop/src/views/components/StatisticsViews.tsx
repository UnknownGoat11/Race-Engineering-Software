import type { JSX } from 'react';

export function StatisticsTableView(): JSX.Element {
  return (
    <div className="statistics-table-view" data-testid="statistics-table-view">
      <div className="stat-header">
        <span className="badge badge-subtle">V1.1 DESCRIPTIVE STATISTICS</span>
        <h2 className="stat-title">Session & Stint Statistics Table</h2>
        <p className="stat-desc">
          Mathematical aggregation of lap pacing, sector efficiency, consistency metrics, and theoretical optimums.
        </p>
      </div>

      <div className="stat-cards-row">
        <div className="stat-metric-card">
          <span className="card-label">BEST LAP (ACTUAL)</span>
          <span className="card-val num-cyan">1:34.420</span>
          <span className="card-sub">Lap 54 (Stint 4)</span>
        </div>
        <div className="stat-metric-card">
          <span className="card-label">THEORETICAL OPTIMUM (Σ S_MIN)</span>
          <span className="card-val text-purple">1:34.112</span>
          <span className="card-sub">Potential gain: -0.308s</span>
        </div>
        <div className="stat-metric-card">
          <span className="card-label">TRIMMED MEAN (TOP 80%)</span>
          <span className="card-val">1:34.865</span>
          <span className="card-sub">Excludes traffic/pit in-out</span>
        </div>
        <div className="stat-metric-card">
          <span className="card-label">CONSISTENCY (STD DEV)</span>
          <span className="card-val num-cyan">±0.076s</span>
          <span className="card-sub">98.2% pacing predictability</span>
        </div>
      </div>

      <div className="stat-sectors-table-card">
        <div className="table-card-header">
          <span className="table-card-title">SECTOR STATISTICS MATRIX</span>
        </div>
        <table className="stat-table">
          <thead>
            <tr>
              <th>Sector</th>
              <th>Actual Best</th>
              <th>Session Median</th>
              <th>Trimmed Mean</th>
              <th>Std Dev (σ)</th>
              <th>Top Speed Trap</th>
              <th>Efficiency Index</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td className="font-bold">Sector 1</td>
              <td className="text-purple font-mono font-bold">29.140s</td>
              <td className="font-mono">29.380s</td>
              <td className="font-mono">29.285s</td>
              <td className="font-mono">±0.038s</td>
              <td className="font-mono">312.4 km/h</td>
              <td>99.2% (Elite)</td>
            </tr>
            <tr>
              <td className="font-bold">Sector 2</td>
              <td className="text-purple font-mono font-bold">38.450s</td>
              <td className="font-mono">38.820s</td>
              <td className="font-mono">38.640s</td>
              <td className="font-mono">±0.052s</td>
              <td className="font-mono">284.6 km/h</td>
              <td>98.5% (High)</td>
            </tr>
            <tr>
              <td className="font-bold">Sector 3</td>
              <td className="text-purple font-mono font-bold">26.522s</td>
              <td className="font-mono">26.810s</td>
              <td className="font-mono">26.680s</td>
              <td className="font-mono">±0.041s</td>
              <td className="font-mono">298.1 km/h</td>
              <td>98.8% (Elite)</td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  );
}

export function StatisticsTableGraphsView(): JSX.Element {
  const points = [
    { lap: 1, timeS: 94.6 },
    { lap: 2, timeS: 94.4 },
    { lap: 3, timeS: 94.5 },
    { lap: 4, timeS: 94.7 },
    { lap: 5, timeS: 94.6 },
    { lap: 6, timeS: 94.8 },
    { lap: 7, timeS: 95.1 },
    { lap: 8, timeS: 95.0 },
    { lap: 9, timeS: 95.3 },
    { lap: 10, timeS: 95.5 },
    { lap: 11, timeS: 95.7 },
    { lap: 12, timeS: 96.1 },
    { lap: 13, timeS: 96.4 },
    { lap: 14, timeS: 96.8 },
  ];

  const minTime = 94.0;
  const maxTime = 97.5;
  const polyline = points
    .map((p, i) => {
      const x = 40 + (i / (points.length - 1)) * 420;
      const y = 30 + ((p.timeS - minTime) / (maxTime - minTime)) * 140;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(' ');

  return (
    <div className="statistics-graphs-view" data-testid="statistics-table-graphs-view">
      <div className="stat-header">
        <span className="badge badge-subtle">V1.1 TREND REGRESSION</span>
        <h2 className="stat-title">Statistics Table Graphs (Tyre Degradation Pace Curve)</h2>
        <p className="stat-desc">
          Regression slope of lap time decay across completed stint laps illustrating the tyre thermal and wear cliff.
        </p>
      </div>

      <div className="stat-chart-card">
        <svg viewBox="0 0 500 200" className="stat-graph-svg" preserveAspectRatio="none">
          {/* Y Axis grid lines */}
          {[94.0, 95.0, 96.0, 97.0].map((t) => {
            const y = 30 + ((t - minTime) / (maxTime - minTime)) * 140;
            return (
              <g key={t}>
                <line x1="40" y1={y} x2="480" y2={y} stroke="#334155" strokeWidth="0.8" strokeDasharray="3 3" />
                <text x="35" y={y + 3} fill="#94a3b8" fontSize="9" textAnchor="end">{t.toFixed(1)}s</text>
              </g>
            );
          })}

          {/* X Axis lap lines */}
          {points.map((p, i) => {
            const x = 40 + (i / (points.length - 1)) * 420;
            return (
              <g key={p.lap}>
                <line x1={x} y1="30" x2={x} y2="170" stroke="#1e293b" strokeWidth="0.8" />
                <text x={x} y="185" fill="#64748b" fontSize="8" textAnchor="middle">L{p.lap}</text>
              </g>
            );
          })}

          {/* Polyline */}
          <polyline points={polyline} fill="none" stroke="#22d3ee" strokeWidth="2.5" />

          {/* Points */}
          {points.map((p, i) => {
            const x = 40 + (i / (points.length - 1)) * 420;
            const y = 30 + ((p.timeS - minTime) / (maxTime - minTime)) * 140;
            return <circle key={p.lap} cx={x} cy={y} r="3" fill="#ffffff" stroke="#22d3ee" strokeWidth="1.5" />;
          })}
        </svg>
      </div>

      <div className="stat-insights-bar">
        <span className="stat-pill-highlight">Pace Decay Rate: <strong>+0.082s / lap</strong></span>
        <span className="stat-pill-highlight">Tyre Cliff Onset: <strong>Lap 11</strong></span>
        <span className="stat-pill-highlight">Optimal Pit Window: <strong>Laps 14-16</strong></span>
      </div>
    </div>
  );
}

export function StatisticsRedBlueView(): JSX.Element {
  return (
    <div className="statistics-red-blue-view" data-testid="statistics-red-blue-view">
      <div className="stat-header">
        <span className="badge badge-subtle">V1.1 MULTI-CAR BENCHMARK</span>
        <h2 className="stat-title">Statistics: RED, BLUE &amp; SES Benchmark</h2>
        <p className="stat-desc">
          Direct comparative matrix: Current car (BLUE) vs Benchmark Pole (RED) vs Session Field Average (SES).
        </p>
      </div>

      <div className="red-blue-benchmark-grid">
        <div className="benchmark-card card-blue">
          <span className="benchmark-tag">BLUE CAR (YOU)</span>
          <div className="benchmark-lap font-mono">1:34.420</div>
          <span className="benchmark-delta">P1 in Class (Hypercar)</span>
          <ul className="benchmark-metrics">
            <li>S1: 29.140s (Purple)</li>
            <li>S2: 38.450s (+0.120s)</li>
            <li>S3: 26.522s (Purple)</li>
            <li>Top Speed: 338.4 km/h</li>
          </ul>
        </div>

        <div className="benchmark-card card-red">
          <span className="benchmark-tag">RED CAR (SESSION BENCHMARK)</span>
          <div className="benchmark-lap font-mono">1:34.380</div>
          <span className="benchmark-delta text-red">-0.040s Faster</span>
          <ul className="benchmark-metrics">
            <li>S1: 29.210s (+0.070s)</li>
            <li>S2: 38.330s (Purple)</li>
            <li>S3: 26.580s (+0.058s)</li>
            <li>Top Speed: 341.2 km/h</li>
          </ul>
        </div>

        <div className="benchmark-card card-ses">
          <span className="benchmark-tag">SES (FIELD AVERAGE)</span>
          <div className="benchmark-lap font-mono">1:35.840</div>
          <span className="benchmark-delta text-green">+1.420s Slower</span>
          <ul className="benchmark-metrics">
            <li>S1: 29.620s (+0.480s)</li>
            <li>S2: 38.980s (+0.530s)</li>
            <li>S3: 26.890s (+0.368s)</li>
            <li>Top Speed: 331.0 km/h</li>
          </ul>
        </div>
      </div>
    </div>
  );
}
