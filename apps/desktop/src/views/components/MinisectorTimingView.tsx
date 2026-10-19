import { useState, type JSX } from 'react';

export type MinisectorStatus = 'purple' | 'green' | 'yellow';

export interface MinisectorCarRow {
  readonly carNumber: string;
  readonly driver: string;
  readonly vehicleClass: 'Hypercar' | 'LMP2' | 'GT3';
  readonly lapTime: string;
  readonly splits: readonly MinisectorStatus[];
  readonly isPlayer: boolean;
}

const DEFAULT_ROWS: readonly MinisectorCarRow[] = [
  {
    carNumber: '7',
    driver: 'Player (Toyota GR010)',
    vehicleClass: 'Hypercar',
    lapTime: '1:34.563',
    isPlayer: true,
    splits: [
      'purple', 'green', 'purple', 'green', 'purple', 'yellow', 'green', 'purple',
      'green', 'green', 'purple', 'yellow', 'green', 'purple', 'purple', 'green',
      'yellow', 'purple', 'green', 'green', 'purple', 'yellow', 'purple', 'green',
    ],
  },
  {
    carNumber: '51',
    driver: 'Ferrari 499P #51',
    vehicleClass: 'Hypercar',
    lapTime: '1:34.842',
    isPlayer: false,
    splits: [
      'green', 'purple', 'green', 'purple', 'green', 'green', 'purple', 'green',
      'purple', 'purple', 'green', 'green', 'purple', 'green', 'green', 'purple',
      'green', 'green', 'yellow', 'purple', 'green', 'purple', 'green', 'purple',
    ],
  },
  {
    carNumber: '6',
    driver: 'Porsche 963 #6',
    vehicleClass: 'Hypercar',
    lapTime: '1:35.104',
    isPlayer: false,
    splits: [
      'green', 'green', 'green', 'yellow', 'green', 'purple', 'green', 'green',
      'green', 'yellow', 'green', 'purple', 'green', 'green', 'green', 'yellow',
      'purple', 'green', 'green', 'yellow', 'green', 'green', 'yellow', 'green',
    ],
  },
  {
    carNumber: '28',
    driver: 'JOTA Oreca #28',
    vehicleClass: 'LMP2',
    lapTime: '1:39.420',
    isPlayer: false,
    splits: [
      'yellow', 'yellow', 'yellow', 'green', 'yellow', 'yellow', 'yellow', 'green',
      'yellow', 'yellow', 'green', 'yellow', 'yellow', 'yellow', 'yellow', 'green',
      'yellow', 'yellow', 'yellow', 'yellow', 'green', 'yellow', 'yellow', 'yellow',
    ],
  },
  {
    carNumber: '92',
    driver: 'Manthey Porsche 911',
    vehicleClass: 'GT3',
    lapTime: '1:47.880',
    isPlayer: false,
    splits: [
      'yellow', 'yellow', 'yellow', 'yellow', 'yellow', 'yellow', 'yellow', 'yellow',
      'yellow', 'yellow', 'yellow', 'yellow', 'yellow', 'yellow', 'yellow', 'yellow',
      'yellow', 'yellow', 'yellow', 'yellow', 'yellow', 'yellow', 'yellow', 'yellow',
    ],
  },
];

export function MinisectorTimingView(): JSX.Element {
  const [selectedSector, setSelectedSector] = useState<number | null>(null);

  return (
    <div className="minisector-timing-view" data-testid="minisector-timing-view">
      <div className="minisector-header">
        <div className="minisector-title-group">
          <span className="badge badge-subtle">V1.1 MICRO-SPLITS</span>
          <h2 className="minisector-title">Multi-Car Minisector Timing Board</h2>
          <p className="minisector-desc">
            Circuit divided into 24 micro-timing sectors. Purple = overall session best, Green = personal best, Yellow = slower.
          </p>
        </div>
        <div className="minisector-legend">
          <span className="mini-chip mini-purple">PURPLE: Overall Fastest</span>
          <span className="mini-chip mini-green">GREEN: Personal Best</span>
          <span className="mini-chip mini-yellow">YELLOW: Slower</span>
        </div>
      </div>

      <div className="minisector-table-wrapper">
        <table className="minisector-table">
          <thead>
            <tr>
              <th className="car-col">Car</th>
              <th className="driver-col">Driver</th>
              <th className="time-col">Lap Time</th>
              {Array.from({ length: 24 }).map((_, i) => (
                <th
                  key={i}
                  className={`split-col ${selectedSector === i + 1 ? 'split-selected' : ''}`}
                  onClick={() => {
                    setSelectedSector(i + 1);
                  }}
                  title={`Mini-sector ${i + 1}`}
                >
                  {i + 1}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {DEFAULT_ROWS.map((row) => (
              <tr key={row.carNumber} className={row.isPlayer ? 'player-row' : ''}>
                <td className="car-col">
                  <span className="car-badge">#{row.carNumber}</span>
                </td>
                <td className="driver-col">{row.driver}</td>
                <td className="time-col font-mono font-bold">{row.lapTime}</td>
                {row.splits.map((split, idx) => (
                  <td
                    key={idx}
                    className={`split-cell split-${split} ${selectedSector === idx + 1 ? 'split-cell-active' : ''}`}
                    title={`Sector ${idx + 1}: ${split.toUpperCase()}`}
                  >
                    <span className="split-glyph">■</span>
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {selectedSector !== null && (
        <div className="minisector-detail-banner">
          <span className="detail-tag">MINI-SECTOR #{selectedSector} ANALYSIS:</span>
          <span className="detail-text">
            Player: <strong>3.421s</strong> (Purple - Sector Best) | Gap to P2: <strong>-0.084s</strong> | Entry Speed: <strong>264 km/h</strong>
          </span>
          <button
            type="button"
            className="btn btn-xs"
            onClick={() => {
              setSelectedSector(null);
            }}
          >
            Clear Selection
          </button>
        </div>
      )}
    </div>
  );
}
