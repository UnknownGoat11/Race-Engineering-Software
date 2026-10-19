import type { JSX } from 'react';

export interface StintRecord {
  readonly stintNumber: number;
  readonly compound: 'Hard' | 'Medium' | 'Soft' | 'Wet';
  readonly tyreSet: string;
  readonly laps: number;
  readonly avgLapTime: string;
  readonly bestLapTime: string;
  readonly pitStationaryS: number;
  readonly fuelBurnPerLapL: number;
  readonly tyreWearFLPct: number;
  readonly tyreWearFRPct: number;
  readonly tyreWearRLPct: number;
  readonly tyreWearRRPct: number;
  readonly degPerLapPct: number;
}

const DEFAULT_STINTS: readonly StintRecord[] = [
  {
    stintNumber: 1,
    compound: 'Medium',
    tyreSet: 'Set M-01',
    laps: 16,
    avgLapTime: '1:35.124',
    bestLapTime: '1:34.563',
    pitStationaryS: 18.4,
    fuelBurnPerLapL: 2.84,
    tyreWearFLPct: 18.2,
    tyreWearFRPct: 21.4,
    tyreWearRLPct: 16.8,
    tyreWearRRPct: 17.5,
    degPerLapPct: 0.084,
  },
  {
    stintNumber: 2,
    compound: 'Medium',
    tyreSet: 'Set M-02',
    laps: 17,
    avgLapTime: '1:35.280',
    bestLapTime: '1:34.612',
    pitStationaryS: 18.1,
    fuelBurnPerLapL: 2.81,
    tyreWearFLPct: 19.5,
    tyreWearFRPct: 22.8,
    tyreWearRLPct: 17.4,
    tyreWearRRPct: 18.2,
    degPerLapPct: 0.089,
  },
  {
    stintNumber: 3,
    compound: 'Hard',
    tyreSet: 'Set H-01',
    laps: 18,
    avgLapTime: '1:35.640',
    bestLapTime: '1:35.010',
    pitStationaryS: 19.0,
    fuelBurnPerLapL: 2.78,
    tyreWearFLPct: 14.1,
    tyreWearFRPct: 16.5,
    tyreWearRLPct: 12.8,
    tyreWearRRPct: 13.4,
    degPerLapPct: 0.058,
  },
  {
    stintNumber: 4,
    compound: 'Medium',
    tyreSet: 'Set M-03 (Current)',
    laps: 8,
    avgLapTime: '1:34.890',
    bestLapTime: '1:34.420',
    pitStationaryS: 0,
    fuelBurnPerLapL: 2.85,
    tyreWearFLPct: 6.2,
    tyreWearFRPct: 7.8,
    tyreWearRLPct: 5.4,
    tyreWearRRPct: 5.9,
    degPerLapPct: 0.082,
  },
];

export function StintsDataView(): JSX.Element {
  return (
    <div className="stints-data-view" data-testid="stints-data-view">
      <div className="stints-header">
        <span className="badge badge-subtle">V1.1 ENDURANCE STINT TRACKER</span>
        <h2 className="stints-title">Stints & Tyre Wear Ledger</h2>
        <p className="stints-desc">
          Chronological record of completed stints, tyre compound lifecycles, wear percentage per corner, degradation slopes, and pit stop times.
        </p>
      </div>

      <div className="stints-summary-cards">
        <div className="summary-card">
          <span className="summary-label">TOTAL STINTS</span>
          <span className="summary-val num-cyan">4 Stints</span>
          <span className="summary-sub">59 Laps Completed</span>
        </div>
        <div className="summary-card">
          <span className="summary-label">AVERAGE PIT STOP</span>
          <span className="summary-val">18.5s</span>
          <span className="summary-sub">Stationary tyre + fuel</span>
        </div>
        <div className="summary-card">
          <span className="summary-label">AVG FUEL BURN</span>
          <span className="summary-val num-cyan">2.82 L / lap</span>
          <span className="summary-sub">Target: &lt;2.85 L</span>
        </div>
        <div className="summary-card">
          <span className="summary-label">CRITICAL CORNER</span>
          <span className="summary-val text-amber">Front Right (FR)</span>
          <span className="summary-sub">High lateral load in long rights</span>
        </div>
      </div>

      <div className="stints-table-card">
        <table className="stints-table">
          <thead>
            <tr>
              <th>Stint #</th>
              <th>Compound</th>
              <th>Tyre Set</th>
              <th>Laps</th>
              <th>Avg Lap</th>
              <th>Best Lap</th>
              <th>Pit Stationary</th>
              <th>Fuel Rate</th>
              <th>FL Wear</th>
              <th>FR Wear</th>
              <th>RL Wear</th>
              <th>RR Wear</th>
              <th>Deg / Lap</th>
            </tr>
          </thead>
          <tbody>
            {DEFAULT_STINTS.map((s) => (
              <tr key={s.stintNumber} className={s.stintNumber === 4 ? 'current-stint-row' : ''}>
                <td className="font-bold">Stint #{s.stintNumber}</td>
                <td>
                  <span className={`compound-pill compound-${s.compound.toLowerCase()}`}>
                    {s.compound}
                  </span>
                </td>
                <td>{s.tyreSet}</td>
                <td>{s.laps} laps</td>
                <td className="font-mono">{s.avgLapTime}</td>
                <td className="font-mono text-cyan">{s.bestLapTime}</td>
                <td>{s.pitStationaryS > 0 ? `${s.pitStationaryS.toFixed(1)}s` : 'Active'}</td>
                <td>{s.fuelBurnPerLapL.toFixed(2)} L</td>
                <td>{s.tyreWearFLPct.toFixed(1)}%</td>
                <td className="text-amber font-bold">{s.tyreWearFRPct.toFixed(1)}%</td>
                <td>{s.tyreWearRLPct.toFixed(1)}%</td>
                <td>{s.tyreWearRRPct.toFixed(1)}%</td>
                <td>+{s.degPerLapPct.toFixed(3)}s</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
