import { useState, type JSX } from 'react';
import type { HypercarVetState, VehicleDynamicsState } from '../types';
import { DEFAULT_HYPERCAR_VET, HypercarVetQuickWidget } from './HypercarVetQuickWidget';

export interface HypercarHybridViewProps {
  readonly state?: VehicleDynamicsState | undefined;
}

interface BrakingRegenZone {
  readonly corner: string;
  readonly entrySpeedKmh: number;
  readonly apexSpeedKmh: number;
  readonly energyHarvestedMj: number;
  readonly peakRegenKw: number;
  readonly brakeBiasMigrationPct: number;
}

const DEFAULT_REGEN_ZONES: readonly BrakingRegenZone[] = [
  { corner: 'T1 - La Source', entrySpeedKmh: 288, apexSpeedKmh: 74, energyHarvestedMj: 1.94, peakRegenKw: -148, brakeBiasMigrationPct: 58.2 },
  { corner: 'T5 - Les Combes', entrySpeedKmh: 324, apexSpeedKmh: 138, energyHarvestedMj: 2.38, peakRegenKw: -152, brakeBiasMigrationPct: 57.5 },
  { corner: 'T8 - Bruxelles', entrySpeedKmh: 215, apexSpeedKmh: 104, energyHarvestedMj: 0.92, peakRegenKw: -110, brakeBiasMigrationPct: 56.0 },
  { corner: 'T12 - Fagnes', entrySpeedKmh: 245, apexSpeedKmh: 142, energyHarvestedMj: 0.88, peakRegenKw: -115, brakeBiasMigrationPct: 56.5 },
  { corner: 'T18 - Bus Stop Chicane', entrySpeedKmh: 312, apexSpeedKmh: 72, energyHarvestedMj: 2.45, peakRegenKw: -155, brakeBiasMigrationPct: 58.0 },
];

export function HypercarHybridView({ state }: HypercarHybridViewProps): JSX.Element {
  const [selectedMode, setSelectedMode] = useState<'BUILD' | 'BALANCED' | 'ATTACK' | 'DEFEND' | 'QUAL'>('BALANCED');
  const vet: HypercarVetState = {
    ...(state?.hypercarVet ?? DEFAULT_HYPERCAR_VET),
    deployMode: selectedMode,
  };

  const speedKmh = state?.speedKmh ?? 218.4;
  const fuelL = state?.fuelL ?? 42.5;
  const fuelCapacityL = state?.fuelCapacityL ?? 90.0;

  // Stint Limiter Analysis: Fuel Laps vs VET Laps
  const fuelBurnPerLap = 2.45; // Liters / lap
  const fuelLapsRemaining = fuelL / fuelBurnPerLap;
  const vetLapsRemaining = vet.stintLapsRemaining;
  const stintLimiter = vetLapsRemaining < fuelLapsRemaining ? 'VIRTUAL ENERGY TANK (VET)' : 'PHYSICAL FUEL TANK';

  return (
    <div className="view-card hypercar-hybrid-view" data-testid="hypercar-hybrid-view">
      <div className="view-card-header">
        <div className="view-card-title-group">
          <span className="view-card-glyph">⚡</span>
          <h2 className="view-card-title">Hypercar Virtual Energy Tank (VET) & Hybrid Deployment</h2>
          <span className="badge badge-accent">WEC / LMU REGULATION SPEC</span>
        </div>
        <div className="view-card-controls">
          <span className="view-control-label">DEPLOY STRATEGY:</span>
          <div className="btn-group">
            {(['BUILD', 'BALANCED', 'ATTACK', 'DEFEND', 'QUAL'] as const).map((m) => (
              <button
                key={m}
                type="button"
                className={`btn btn-xs ${selectedMode === m ? 'btn-active' : ''}`}
                onClick={() => setSelectedMode(m)}
              >
                {m}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Top Banner: Quick Glance Status Widget */}
      <div className="hypercar-quick-banner">
        <HypercarVetQuickWidget state={vet} speedKmh={speedKmh} />
      </div>

      <div className="hypercar-main-grid">
        {/* Left Column: Energy vs Fuel Stint Projection & Deployment Curve */}
        <div className="hypercar-col-left">
          {/* Stint Limiter Card */}
          <div className="stat-card">
            <div className="stat-card-label">STINT LIMITER & LONGEVITY ANALYSIS</div>
            <div className="limiter-hero-row">
              <span className="limiter-title">CRITICAL STINT LIMITER:</span>
              <span className="limiter-name badge badge-accent">{stintLimiter}</span>
            </div>

            <div className="limiter-comparison-grid">
              <div className="limiter-box">
                <span className="limiter-metric-label">VET ENERGY REMAINING</span>
                <span className="limiter-metric-val num-cyan">{vet.vetRemainingMj.toFixed(1)} MJ</span>
                <span className="limiter-sub">{vetLapsRemaining.toFixed(1)} Laps @ {vet.targetPerLapMj.toFixed(2)} MJ/lap</span>
                <div className="progress-bar-track">
                  <div
                    className="progress-bar-fill fill-cyan"
                    style={{ width: `${(vet.vetRemainingMj / vet.vetCapacityMj) * 100}%` }}
                  />
                </div>
              </div>

              <div className="limiter-box">
                <span className="limiter-metric-label">FUEL TANK REMAINING</span>
                <span className="limiter-metric-val num-green">{fuelL.toFixed(1)} L</span>
                <span className="limiter-sub">{fuelLapsRemaining.toFixed(1)} Laps @ {fuelBurnPerLap.toFixed(2)} L/lap</span>
                <div className="progress-bar-track">
                  <div
                    className="progress-bar-fill fill-green"
                    style={{ width: `${(fuelL / fuelCapacityL) * 100}%` }}
                  />
                </div>
              </div>
            </div>

            <div className="limiter-advisory">
              💡 <strong>Engineer Advisory:</strong> Virtual Energy Tank is {vetLapsRemaining < fuelLapsRemaining ? 'the tighter restriction' : 'comfortable'}. Target consumption is <strong>{vet.targetPerLapMj.toFixed(2)} MJ/lap</strong>. Last lap was <strong>{vet.actualLastLapMj.toFixed(2)} MJ</strong> ({vet.deltaLastLapMj < 0 ? 'saving 0.32 MJ' : 'exceeding target'}).
            </div>
          </div>

          {/* MGU-K Front Axle 190 km/h Deployment Curve SVG */}
          <div className="stat-card">
            <div className="stat-card-label">MGU-K ELECTRIC DEPLOYMENT VS SPEED (FIA 190 KM/H GATE)</div>
            <div className="deploy-curve-svg-wrapper">
              <svg viewBox="0 0 500 180" className="deploy-curve-svg" role="img" aria-label="MGU-K deployment curve">
                {/* Background Grid */}
                <line x1="50" y1="20" x2="50" y2="150" stroke="#334155" strokeWidth="1" />
                <line x1="50" y1="150" x2="480" y2="150" stroke="#334155" strokeWidth="1" />

                {/* X Axis: Speed 0 to 350 km/h */}
                {/* Speed 190 km/h corresponds to x = 50 + (190 / 350) * 430 = 50 + 233.4 = 283.4 */}
                {/* Forbidden Zone (0 to 190 km/h) */}
                <rect x="50" y="20" width="233.4" height="130" fill="#ef4444" fillOpacity="0.08" />
                <line x1="283.4" y1="20" x2="283.4" y2="150" stroke="#ef4444" strokeWidth="2" strokeDasharray="3 3" />
                <text x="110" y="85" fill="#ef4444" fontSize="11" fontWeight="bold">
                  FORBIDDEN BY FIA (&lt; 190 KM/H)
                </text>
                <text x="288" y="35" fill="#ef4444" fontSize="10" fontWeight="bold">
                  190 KM/H GATE
                </text>

                {/* Active Deployment Zone (190 to 350 km/h) */}
                <rect x="283.4" y="20" width="196.6" height="130" fill="#10b981" fillOpacity="0.08" />
                <text x="320" y="85" fill="#10b981" fontSize="11" fontWeight="bold">
                  MGU-K 200 kW DEPLOY
                </text>

                {/* Power curve path */}
                {/* 0 until 190 km/h (y = 150), then ramps up to 200 kW (y = 40) at 210 km/h, tapers down at top speed */}
                <path
                  d="M 50 150 L 283.4 150 L 295 40 L 440 40 L 480 70"
                  fill="none"
                  stroke="#22d3ee"
                  strokeWidth="3"
                />

                {/* Current Car Speed Marker */}
                {/* speedKmh: x = 50 + (speedKmh / 350) * 430 */}
                {(() => {
                  const curX = Math.min(480, Math.max(50, 50 + (speedKmh / 350) * 430));
                  const curY = speedKmh >= 190 ? 40 : 150;
                  return (
                    <g>
                      <line x1={curX} y1="20" x2={curX} y2="150" stroke="#ffffff" strokeWidth="1.5" strokeDasharray="2 2" />
                      <circle cx={curX} cy={curY} r="5" fill="#f59e0b" stroke="#ffffff" strokeWidth="1.5" />
                      <text x={curX - 25} y="15" fill="#f59e0b" fontSize="11" fontWeight="bold">
                        {speedKmh.toFixed(0)} km/h
                      </text>
                    </g>
                  );
                })()}

                {/* Axis Labels */}
                <text x="50" y="165" fill="#94a3b8" fontSize="10">0 km/h</text>
                <text x="270" y="165" fill="#f59e0b" fontSize="10" fontWeight="bold">190</text>
                <text x="450" y="165" fill="#94a3b8" fontSize="10">350 km/h</text>
                <text x="15" y="45" fill="#94a3b8" fontSize="10">200 kW</text>
                <text x="25" y="150" fill="#94a3b8" fontSize="10">0 kW</text>
              </svg>
            </div>
            <div className="deploy-legend">
              <span className="deploy-note">
                Front-axle electric motor is restricted to <strong>0 kW</strong> below 190 km/h (dry track) by WEC Hypercar technical regulations. Full <strong>200 kW (272 hp)</strong> electric all-wheel-drive engagement engages above 190 km/h.
              </span>
            </div>
          </div>
        </div>

        {/* Right Column: Braking Regeneration Breakdown & Lift & Coast Guide */}
        <div className="hypercar-col-right">
          {/* Turn-by-Turn Regeneration Ledger */}
          <div className="stat-card">
            <div className="stat-card-header-row">
              <span className="stat-card-label">BRAKING ZONE REGENERATION HARVEST</span>
              <span className="badge badge-accent">TOTAL: 8.57 MJ / LAP</span>
            </div>

            <div className="regen-table-wrapper">
              <table className="regen-table">
                <thead>
                  <tr>
                    <th>Braking Zone</th>
                    <th>Entry &rarr; Apex</th>
                    <th>Harvest (MJ)</th>
                    <th>Peak Regen</th>
                    <th>BBias Migr</th>
                  </tr>
                </thead>
                <tbody>
                  {DEFAULT_REGEN_ZONES.map((z) => (
                    <tr key={z.corner}>
                      <td className="font-semibold text-white">{z.corner}</td>
                      <td>{z.entrySpeedKmh} &rarr; {z.apexSpeedKmh} km/h</td>
                      <td className="num-cyan font-bold">+{z.energyHarvestedMj.toFixed(2)} MJ</td>
                      <td className="num-green">{z.peakRegenKw} kW</td>
                      <td>F {z.brakeBiasMigrationPct.toFixed(1)}%</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>

          {/* Lift and Coast Recommendations Card */}
          <div className="stat-card">
            <div className="stat-card-label">LIFT & COAST OPTIMIZATION ADVISOR</div>
            <div className="lift-coast-grid">
              <div className="lift-coast-card">
                <span className="lift-tier-title">MINIMAL L&C (0.2 MJ Saved)</span>
                <p className="lift-desc">Lift 30m before T1 and T5 braking boards. Minimal time penalty.</p>
                <div className="lift-metric-row">
                  <span className="lift-metric-tag">Lap Time Delta:</span>
                  <span className="lift-metric-num num-green">+0.06s</span>
                </div>
              </div>

              <div className="lift-coast-card">
                <span className="lift-tier-title">MODERATE L&C (0.5 MJ Saved)</span>
                <p className="lift-desc">Lift 60m before T5 (Kemmel straight) and 40m before Bus Stop.</p>
                <div className="lift-metric-row">
                  <span className="lift-metric-tag">Lap Time Delta:</span>
                  <span className="lift-metric-num num-amber">+0.18s</span>
                </div>
              </div>

              <div className="lift-coast-card">
                <span className="lift-tier-title">MAX AGGRESSIVE L&C (1.0 MJ Saved)</span>
                <p className="lift-desc">Extended lift into T1, T5, and Bus Stop with early braking roll.</p>
                <div className="lift-metric-row">
                  <span className="lift-metric-tag">Lap Time Delta:</span>
                  <span className="lift-metric-num num-red">+0.44s</span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
