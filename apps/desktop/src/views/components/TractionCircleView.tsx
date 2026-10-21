import { useMemo, useState, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { useLiveReadings, useLiveRing } from '../liveData';
import { NOT_CONNECTED_REASON, ViewNoData } from './ViewNoData';

export interface TractionCircleViewProps {
  /** Plot scale: G at the outer edge of the plot. */
  readonly maxG?: number | undefined;
}

interface GPoint {
  /** Lateral G, positive toward the LEFT (registry G_LAT, ISO). */
  readonly latG: number;
  /** Longitudinal G, positive accelerating (registry G_LONG). */
  readonly longG: number;
}

const LIVE_CHANNELS: readonly number[] = [ch.G_LAT, ch.G_LONG];
const RING_CHANNELS: readonly number[] = [ch.G_LAT, ch.G_LONG];
/** Most points drawn for the trail (older rows are subsampled). */
const MAX_TRAIL_POINTS = 400;
const TRAIL_OPTIONS_S = [5, 15, 60] as const;

function lateralText(latG: number | null): string {
  if (latG === null) return NO_VALUE;
  if (latG === 0) return '0.00 G';
  return latG > 0 ? `${latG.toFixed(2)} G (Left)` : `${Math.abs(latG).toFixed(2)} G (Right)`;
}

function longitudinalText(longG: number | null): string {
  if (longG === null) return NO_VALUE;
  return longG < 0 ? `${longG.toFixed(2)} G (Brake)` : `+${longG.toFixed(2)} G (Drive)`;
}

function peakText(value: number | null, signed: boolean): string {
  if (value === null) return NO_VALUE;
  return signed && value > 0 ? `+${value.toFixed(2)} G` : `${value.toFixed(2)} G`;
}

export function TractionCircleView({ maxG = 2.5 }: TractionCircleViewProps): JSX.Element {
  const [trailS, setTrailS] = useState<number>(15);
  const live = useLiveReadings(LIVE_CHANNELS);
  const ring = useLiveRing(RING_CHANNELS);

  const liveLatG = live.num(ch.G_LAT);
  const liveLongG = live.num(ch.G_LONG);
  const combinedG =
    liveLatG !== null && liveLongG !== null ? Math.sqrt(liveLatG * liveLatG + liveLongG * liveLongG) : null;

  // Real samples from the live ring: the trail (last `trailS` seconds) and the peaks over the whole ring window.
  const { trail, peaks, windowPoints } = useMemo(() => {
    const lat = ring.series.get(ch.G_LAT) ?? [];
    const lon = ring.series.get(ch.G_LONG) ?? [];
    const newest = ring.times.length > 0 ? (ring.times[ring.times.length - 1] ?? null) : null;
    const all: GPoint[] = [];
    const recent: GPoint[] = [];
    for (let i = 0; i < ring.rows; i++) {
      const la = lat[i];
      const lo = lon[i];
      if (la === null || la === undefined || lo === null || lo === undefined) continue;
      const point = { latG: la, longG: lo };
      all.push(point);
      const t = ring.times[i];
      if (newest !== null && t !== null && t !== undefined && newest - t <= trailS) recent.push(point);
    }
    const step = Math.max(1, Math.ceil(recent.length / MAX_TRAIL_POINTS));
    const sampled = step === 1 ? recent : recent.filter((_, i) => i % step === 0);
    let brake: number | null = null;
    let accel: number | null = null;
    let left: number | null = null;
    let right: number | null = null;
    for (const p of all) {
      if (brake === null || p.longG < brake) brake = p.longG;
      if (accel === null || p.longG > accel) accel = p.longG;
      if (left === null || p.latG > left) left = p.latG;
      if (right === null || p.latG < right) right = p.latG;
    }
    return {
      trail: sampled,
      windowPoints: all.length,
      peaks: {
        brake: brake !== null && brake < 0 ? brake : null,
        accel: accel !== null && accel > 0 ? accel : null,
        left: left !== null && left > 0 ? left : null,
        right: right !== null && right < 0 ? Math.abs(right) : null,
      },
    };
  }, [ring, trailS]);

  const svgCenter = 150;
  const svgScale = 120 / maxG;
  // G_LAT is positive to the LEFT: a left-hand load plots on the left half of the diagram.
  const toSvgX = (latG: number): number => svgCenter - latG * svgScale;
  const toSvgY = (longG: number): number => svgCenter - longG * svgScale;

  const rings = [0.5, 1.0, 1.5, 2.0, 2.5].filter((g) => g <= maxG);

  return (
    <div className="view-card traction-circle-view" data-testid="traction-circle-view">
      <div className="view-card-header">
        <div className="view-card-title-group">
          <span className="view-card-glyph">🎯</span>
          <h2 className="view-card-title">Traction Circle (G-G Diagram)</h2>
          <span className="badge badge-accent">LIVE G_LAT / G_LONG</span>
        </div>
        <div className="view-card-controls">
          <select
            className="view-select-sm"
            value={trailS}
            onChange={(e) => {
              setTrailS(Number(e.target.value));
            }}
            aria-label="Scatter trail length"
          >
            {TRAIL_OPTIONS_S.map((s) => (
              <option key={s} value={s}>
                Trail: last {s} s
              </option>
            ))}
          </select>
        </div>
      </div>

      {!live.connected && <ViewNoData title="G-G diagram" reason={NOT_CONNECTED_REASON} testId="traction-not-connected" />}

      <div className="traction-circle-content-grid">
        <div className="traction-circle-plot-card">
          <div className="traction-circle-svg-wrapper">
            <svg viewBox="0 0 300 300" className="traction-circle-svg" role="img" aria-label="G-G diagram plot">
              <line x1="20" y1={svgCenter} x2="280" y2={svgCenter} stroke="#334155" strokeWidth="1.2" />
              <line x1={svgCenter} y1="20" x2={svgCenter} y2="280" stroke="#334155" strokeWidth="1.2" />

              {rings.map((g) => {
                const r = g * svgScale;
                return (
                  <g key={`ring-${String(g)}`}>
                    <circle cx={svgCenter} cy={svgCenter} r={r} fill="none" stroke="#1e293b" strokeWidth="1" strokeDasharray="3 3" />
                    <text x={svgCenter + 4} y={svgCenter - r + 11} fill="#64748b" fontSize="9" fontFamily="monospace">
                      {g.toFixed(1)}G
                    </text>
                  </g>
                );
              })}

              <text x="25" y="32" fill="#10b981" fontSize="10" fontWeight="bold">ACCEL + LEFT</text>
              <text x="200" y="32" fill="#10b981" fontSize="10" fontWeight="bold">ACCEL + RIGHT</text>
              <text x="25" y="278" fill="#ef4444" fontSize="10" fontWeight="bold">BRAKE + LEFT</text>
              <text x="200" y="278" fill="#ef4444" fontSize="10" fontWeight="bold">BRAKE + RIGHT</text>

              <g data-testid="traction-trail">
                {trail.map((pt, idx) => (
                  <circle
                    key={`pt-${String(idx)}`}
                    cx={toSvgX(pt.latG)}
                    cy={toSvgY(pt.longG)}
                    r="2"
                    fill="#22d3ee"
                    fillOpacity="0.55"
                  />
                ))}
              </g>

              {liveLatG !== null && liveLongG !== null && (
                <g data-testid="traction-live-dot">
                  <line
                    x1={svgCenter}
                    y1={svgCenter}
                    x2={toSvgX(liveLatG)}
                    y2={toSvgY(liveLongG)}
                    stroke="#22d3ee"
                    strokeWidth="1.5"
                    strokeOpacity="0.6"
                  />
                  <circle cx={toSvgX(liveLatG)} cy={toSvgY(liveLongG)} r="4.5" fill="#ef4444" stroke="#ffffff" strokeWidth="1.5" />
                </g>
              )}
            </svg>
          </div>
          <div className="traction-circle-legend">
            <span className="legend-item">
              <span className="legend-dot dot-cyan" /> Live ring samples (last {trailS} s, {trail.length} drawn)
            </span>
            <span className="legend-item">
              <span className="legend-dot dot-red" /> Current
            </span>
          </div>
        </div>

        <div className="traction-circle-stats-col">
          <div className="stat-card stat-card-live-g">
            <div className="stat-card-label">CURRENT CHASSIS LOAD</div>
            <div className="stat-main-row">
              <span className="stat-hero-number" data-testid="traction-combined">
                {combinedG !== null ? `${combinedG.toFixed(2)} G` : NO_VALUE}
              </span>
              <span className="stat-sub-badge">COMBINED</span>
            </div>
            <div className="stat-split-row">
              <div className="stat-split-item">
                <span className="split-label">Lateral G:</span>
                <span className="split-value num-cyan" data-testid="traction-lateral">
                  {lateralText(liveLatG)}
                </span>
              </div>
              <div className="stat-split-item">
                <span className="split-label">Longitudinal G:</span>
                <span
                  className={`split-value ${liveLongG !== null && liveLongG < 0 ? 'num-red' : 'num-green'}`}
                  data-testid="traction-longitudinal"
                >
                  {longitudinalText(liveLongG)}
                </span>
              </div>
            </div>
          </div>

          <div className="stat-card">
            <div className="stat-card-label">
              PEAKS IN LIVE WINDOW{ring.windowS !== null ? ` (LAST ${String(ring.windowS)} S)` : ''}
            </div>
            <div className="envelope-grid" data-testid="traction-peaks">
              <div className="envelope-box">
                <span className="envelope-tag">PEAK BRAKING</span>
                <span className="envelope-val num-red">{peakText(peaks.brake, false)}</span>
              </div>
              <div className="envelope-box">
                <span className="envelope-tag">PEAK LAT LEFT</span>
                <span className="envelope-val num-cyan">{peakText(peaks.left, false)}</span>
              </div>
              <div className="envelope-box">
                <span className="envelope-tag">PEAK LAT RIGHT</span>
                <span className="envelope-val num-cyan">{peakText(peaks.right, false)}</span>
              </div>
              <div className="envelope-box">
                <span className="envelope-tag">PEAK ACCEL</span>
                <span className="envelope-val num-green">{peakText(peaks.accel, true)}</span>
              </div>
            </div>
            <span className="envelope-loc">{windowPoints} samples in window</span>
          </div>
        </div>
      </div>
    </div>
  );
}
