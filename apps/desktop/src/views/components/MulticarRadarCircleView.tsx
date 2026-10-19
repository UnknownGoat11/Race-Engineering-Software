import type { JSX } from 'react';

export interface CircularCarMarker {
  readonly carNumber: string;
  readonly driverName: string;
  readonly vehicleClass: 'Hypercar' | 'LMP2' | 'GT3';
  readonly lapDistanceFrac: number; // 0.0 to 1.0 around track
  readonly isPlayer: boolean;
  readonly gapToPlayerS: number;
  readonly speedKmh: number;
}

const DEFAULT_FIELD: readonly CircularCarMarker[] = [
  { carNumber: '7', driverName: 'Player (Toyota GR010)', vehicleClass: 'Hypercar', lapDistanceFrac: 0.72, isPlayer: true, gapToPlayerS: 0, speedKmh: 284 },
  { carNumber: '51', driverName: 'Ferrari 499P #51', vehicleClass: 'Hypercar', lapDistanceFrac: 0.76, isPlayer: false, gapToPlayerS: 2.1, speedKmh: 279 },
  { carNumber: '6', driverName: 'Porsche 963 #6', vehicleClass: 'Hypercar', lapDistanceFrac: 0.69, isPlayer: false, gapToPlayerS: -1.8, speedKmh: 288 },
  { carNumber: '28', driverName: 'JOTA Oreca #28', vehicleClass: 'LMP2', lapDistanceFrac: 0.54, isPlayer: false, gapToPlayerS: -8.4, speedKmh: 245 },
  { carNumber: '23', driverName: 'United Oreca #23', vehicleClass: 'LMP2', lapDistanceFrac: 0.51, isPlayer: false, gapToPlayerS: -9.8, speedKmh: 242 },
  { carNumber: '92', driverName: 'Manthey Porsche 911', vehicleClass: 'GT3', lapDistanceFrac: 0.38, isPlayer: false, gapToPlayerS: -16.2, speedKmh: 220 },
  { carNumber: '55', driverName: 'AF Corse 296 GT3', vehicleClass: 'GT3', lapDistanceFrac: 0.35, isPlayer: false, gapToPlayerS: -17.6, speedKmh: 218 },
  { carNumber: '33', driverName: 'Corvette Z06 #33', vehicleClass: 'GT3', lapDistanceFrac: 0.88, isPlayer: false, gapToPlayerS: 12.4, speedKmh: 224 },
  { carNumber: '2', driverName: 'Cadillac V-Series.R #2', vehicleClass: 'Hypercar', lapDistanceFrac: 0.94, isPlayer: false, gapToPlayerS: 15.1, speedKmh: 290 },
];

function classToColor(c: 'Hypercar' | 'LMP2' | 'GT3'): string {
  switch (c) {
    case 'Hypercar':
      return '#22d3ee'; // Cyan
    case 'LMP2':
      return '#3b82f6'; // Blue
    case 'GT3':
      return '#f97316'; // Orange
  }
}

export function MulticarRadarCircleView({ field = DEFAULT_FIELD }: { readonly field?: readonly CircularCarMarker[] }): JSX.Element {
  const center = 150;
  const trackRadius = 100;

  return (
    <div className="multicar-circle-view" data-testid="multicar-circle-view">
      <div className="circle-view-header">
        <div className="circle-title-group">
          <span className="badge badge-subtle">V1.1 POLAR FIELD RADAR</span>
          <h2 className="circle-title">Multi-Car Track Radar (Circular Space Projection)</h2>
          <p className="circle-desc">
            Visualizes relative track density, overtaking traffic bubbles, pit merge windows, and sector splits across the entire field.
          </p>
        </div>
        <div className="circle-legend-group">
          <span className="legend-chip" style={{ color: '#22d3ee' }}>● Hypercar (Cyan)</span>
          <span className="legend-chip" style={{ color: '#3b82f6' }}>● LMP2 (Blue)</span>
          <span className="legend-chip" style={{ color: '#f97316' }}>● LMGT3 (Orange)</span>
        </div>
      </div>

      <div className="circle-radar-workspace">
        {/* Polar SVG Circle */}
        <div className="circle-svg-card">
          <svg viewBox="0 0 300 300" className="polar-radar-svg">
            {/* Concentric helper rings */}
            <circle cx={center} cy={center} r={trackRadius - 20} fill="none" stroke="#1e293b" strokeWidth="1" strokeDasharray="3 3" />
            <circle cx={center} cy={center} r={trackRadius + 20} fill="none" stroke="#1e293b" strokeWidth="1" strokeDasharray="3 3" />

            {/* The Track Ring */}
            <circle cx={center} cy={center} r={trackRadius} fill="none" stroke="#334155" strokeWidth="14" />
            <circle cx={center} cy={center} r={trackRadius} fill="none" stroke="#475569" strokeWidth="2" />

            {/* Start / Finish Line at Top (angle = -PI/2) */}
            <line x1={center} y1={center - trackRadius - 12} x2={center} y2={center - trackRadius + 12} stroke="#ffffff" strokeWidth="3" />
            <text x={center} y={center - trackRadius - 16} fill="#ffffff" fontSize="9" fontWeight="bold" textAnchor="middle">S/F LINE</text>

            {/* Sector 1 & Sector 2 Split Marks */}
            {/* S1 (~ 33% = angle 0.33 * 2PI - PI/2) */}
            <line
              x1={center + (trackRadius - 8) * Math.cos(0.33 * Math.PI * 2 - Math.PI / 2)}
              y1={center + (trackRadius - 8) * Math.sin(0.33 * Math.PI * 2 - Math.PI / 2)}
              x2={center + (trackRadius + 8) * Math.cos(0.33 * Math.PI * 2 - Math.PI / 2)}
              y2={center + (trackRadius + 8) * Math.sin(0.33 * Math.PI * 2 - Math.PI / 2)}
              stroke="#e2e8f0"
              strokeWidth="2"
            />
            {/* S2 (~ 66% = angle 0.66 * 2PI - PI/2) */}
            <line
              x1={center + (trackRadius - 8) * Math.cos(0.66 * Math.PI * 2 - Math.PI / 2)}
              y1={center + (trackRadius - 8) * Math.sin(0.66 * Math.PI * 2 - Math.PI / 2)}
              x2={center + (trackRadius + 8) * Math.cos(0.66 * Math.PI * 2 - Math.PI / 2)}
              y2={center + (trackRadius + 8) * Math.sin(0.66 * Math.PI * 2 - Math.PI / 2)}
              stroke="#e2e8f0"
              strokeWidth="2"
            />

            {/* Pit Entry / Exit Arc */}
            <path
              d={`M ${center - 24} ${center - trackRadius - 4} A 18 18 0 0 1 ${center + 24} ${center - trackRadius - 4}`}
              fill="none"
              stroke="#eab308"
              strokeWidth="2"
              strokeDasharray="2 2"
            />
            <text x={center} y={center - 30} fill="#64748b" fontSize="8" textAnchor="middle">PIT LANE MERGE</text>

            {/* Render Each Car on the Circle */}
            {field.map((car) => {
              const angle = car.lapDistanceFrac * Math.PI * 2 - Math.PI / 2;
              const x = center + trackRadius * Math.cos(angle);
              const y = center + trackRadius * Math.sin(angle);
              const color = classToColor(car.vehicleClass);

              return (
                <g key={car.carNumber}>
                  {car.isPlayer && (
                    <circle cx={x} cy={y} r="10" fill="none" stroke="#22d3ee" strokeWidth="2" opacity="0.6">
                      <animate attributeName="r" values="8;14;8" dur="2s" repeatCount="indefinite" />
                    </circle>
                  )}
                  <circle cx={x} cy={y} r={car.isPlayer ? 6.5 : 5} fill={car.isPlayer ? '#ffffff' : color} stroke="#090b0e" strokeWidth="1.5" />
                  <text
                    x={x + 10 * Math.cos(angle)}
                    y={y + 10 * Math.sin(angle) + 3}
                    fill="#e2e8f0"
                    fontSize="8"
                    fontWeight={car.isPlayer ? 'bold' : 'normal'}
                    textAnchor={Math.cos(angle) >= 0 ? 'start' : 'end'}
                  >
                    #{car.carNumber} {car.isPlayer ? '(YOU)' : ''}
                  </text>
                </g>
              );
            })}
          </svg>
        </div>

        {/* Traffic Gap Analysis Table */}
        <div className="circle-traffic-table-card">
          <div className="table-card-header">
            <span className="table-card-title">PROXIMITY RADAR & PIT MERGE GAPS</span>
          </div>
          <table className="traffic-table">
            <thead>
              <tr>
                <th>Car #</th>
                <th>Driver / Team</th>
                <th>Class</th>
                <th>Gap to You</th>
                <th>Speed</th>
                <th>Track Position</th>
              </tr>
            </thead>
            <tbody>
              {field.map((car) => (
                <tr key={car.carNumber} className={car.isPlayer ? 'player-row' : ''}>
                  <td><span className="car-badge" style={{ borderColor: classToColor(car.vehicleClass) }}>#{car.carNumber}</span></td>
                  <td>{car.driverName}</td>
                  <td><span className="badge badge-subtle">{car.vehicleClass}</span></td>
                  <td className={car.isPlayer ? 'font-bold' : car.gapToPlayerS > 0 ? 'text-cyan' : 'text-orange'}>
                    {car.isPlayer ? 'CURRENT' : car.gapToPlayerS > 0 ? `+${car.gapToPlayerS.toFixed(1)}s (Ahead)` : `${car.gapToPlayerS.toFixed(1)}s (Behind)`}
                  </td>
                  <td>{car.speedKmh} km/h</td>
                  <td>{(car.lapDistanceFrac * 100).toFixed(0)}%</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
