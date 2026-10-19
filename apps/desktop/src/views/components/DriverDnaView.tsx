import type { JSX } from 'react';

export interface DriverDnaMetrics {
  readonly trailBrakingScore: number; // 0-100
  readonly throttleAggressionScore: number; // 0-100
  readonly steeringSmoothnessScore: number; // 0-100
  readonly apexSpeedCommitment: number; // 0-100
  readonly tyrePreservationScore: number; // 0-100
  readonly lapConsistencyScore: number; // 0-100
  readonly overallRating: number; // 0-100
  readonly driverTier: string; // 'PRO ELITE' | 'GOLD' | 'SILVER'
  readonly lapStdDevS: number; // e.g. 0.084 s
  readonly trailBrakingDecayRate: string; // e.g. "-18.4 %/100ms"
  readonly steeringMicroCorrectionsPerKm: number; // e.g. 3.2 / km
}

const DEFAULT_DNA: DriverDnaMetrics = {
  trailBrakingScore: 92,
  throttleAggressionScore: 84,
  steeringSmoothnessScore: 95,
  apexSpeedCommitment: 88,
  tyrePreservationScore: 91,
  lapConsistencyScore: 96,
  overallRating: 91,
  driverTier: 'PRO ELITE',
  lapStdDevS: 0.076,
  trailBrakingDecayRate: '-16.8 % / 100ms',
  steeringMicroCorrectionsPerKm: 2.8,
};

export function DriverDnaView({ metrics = DEFAULT_DNA }: { readonly metrics?: DriverDnaMetrics }): JSX.Element {
  // Convert 6 scores into 6 spider-web radar polygon points
  const scores = [
    metrics.trailBrakingScore,
    metrics.throttleAggressionScore,
    metrics.steeringSmoothnessScore,
    metrics.apexSpeedCommitment,
    metrics.tyrePreservationScore,
    metrics.lapConsistencyScore,
  ];

  const labels = [
    'Trail-Braking Slope',
    'Throttle Aggression',
    'Steering Smoothness',
    'Apex Speed',
    'Tyre Preservation',
    'Lap Consistency',
  ];

  const center = 100;
  const radius = 75;

  const points = scores.map((score, i) => {
    const angle = (Math.PI * 2 * i) / scores.length - Math.PI / 2;
    const r = (score / 100) * radius;
    const x = center + r * Math.cos(angle);
    const y = center + r * Math.sin(angle);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(' ');

  return (
    <div className="driver-dna-view" data-testid="driver-dna-view">
      <div className="dna-hero-card">
        <div className="dna-hero-left">
          <div className="dna-badge-row">
            <span className="badge badge-subtle">V1.1 DRIVER BIOMETRICS</span>
            <span className="badge badge-mode">{metrics.driverTier}</span>
          </div>
          <h2 className="dna-title">Driver DNA & Performance Signature</h2>
          <p className="dna-desc">
            Deep algorithmic assessment of pedal modulation, steering smoothness, slip-angle control, and stint consistency across 100 Hz TLV telemetry samples.
          </p>
          <div className="dna-rating-cluster">
            <div className="dna-rating-number num-cyan">{metrics.overallRating}</div>
            <div className="dna-rating-meta">
              <span className="rating-label">OVERALL DRIVER INDEX</span>
              <span className="rating-sub">TOP 2% OF WEC PRO SIM COMPETITORS</span>
            </div>
          </div>
        </div>

        {/* 6-Axis Spider Radar Chart */}
        <div className="dna-radar-container">
          <svg viewBox="0 0 200 200" className="dna-radar-svg">
            {/* Concentric Grid Webs (25%, 50%, 75%, 100%) */}
            {[0.25, 0.5, 0.75, 1.0].map((frac) => (
              <polygon
                key={frac}
                points={scores
                  .map((_, i) => {
                    const angle = (Math.PI * 2 * i) / scores.length - Math.PI / 2;
                    const r = frac * radius;
                    return `${(center + r * Math.cos(angle)).toFixed(1)},${(center + r * Math.sin(angle)).toFixed(1)}`;
                  })
                  .join(' ')}
                fill="none"
                stroke="#334155"
                strokeWidth={frac === 1 ? '1.5' : '0.8'}
              />
            ))}

            {/* Radial Spokes */}
            {scores.map((_, i) => {
              const angle = (Math.PI * 2 * i) / scores.length - Math.PI / 2;
              const x2 = center + radius * Math.cos(angle);
              const y2 = center + radius * Math.sin(angle);
              return <line key={i} x1={center} y1={center} x2={x2} y2={y2} stroke="#334155" strokeWidth="1" />;
            })}

            {/* Filled Active Performance Polygon */}
            <polygon points={points} fill="rgba(34, 211, 238, 0.25)" stroke="#22d3ee" strokeWidth="2.5" />

            {/* Data Points */}
            {scores.map((score, i) => {
              const angle = (Math.PI * 2 * i) / scores.length - Math.PI / 2;
              const r = (score / 100) * radius;
              const x = center + r * Math.cos(angle);
              const y = center + r * Math.sin(angle);
              return <circle key={i} cx={x} cy={y} r="3" fill="#ffffff" stroke="#22d3ee" strokeWidth="1.5" />;
            })}
          </svg>
        </div>
      </div>

      {/* 6 Dimension Cards */}
      <div className="dna-traits-grid">
        {scores.map((score, i) => (
          <div key={labels[i]} className="dna-trait-card">
            <div className="trait-card-header">
              <span className="trait-title">{labels[i]}</span>
              <span className="trait-score num-cyan">{score}/100</span>
            </div>
            <div className="trait-progress-track">
              <div className="trait-progress-fill" style={{ width: `${score}%` }} />
            </div>
            <div className="trait-details">
              {i === 0 && <span>Decay: {metrics.trailBrakingDecayRate} (Elite progressive release)</span>}
              {i === 1 && <span>Throttle delay: 0.12s from apex (minimal hesitation)</span>}
              {i === 2 && <span>Scrub rate: {metrics.steeringMicroCorrectionsPerKm} / km (high precision)</span>}
              {i === 3 && <span>Apex Vmin: +2.4 km/h above field median</span>}
              {i === 4 && <span>Camber thermal balance: optimal (no localized core overheating)</span>}
              {i === 5 && <span>Stint variance: ±{metrics.lapStdDevS.toFixed(3)}s over 14 laps</span>}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
