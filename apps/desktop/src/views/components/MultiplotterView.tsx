import { useMemo, useState, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { mpsToKmh, NO_VALUE } from '../../lib/units';
import { useLiveRing } from '../liveData';
import { NOT_CONNECTED_REASON, ViewNoData } from './ViewNoData';

export type PlotMode = 'speed_rpm' | 'steer_latg' | 'brake_decel';

interface PlotDef {
  readonly label: string;
  readonly xId: number;
  readonly yId: number;
  readonly xName: string;
  readonly yName: string;
  readonly xMap: (v: number) => number;
  readonly yMap: (v: number) => number;
}

const RAD_TO_DEG = 180 / Math.PI;

const PLOTS: Readonly<Record<PlotMode, PlotDef>> = {
  steer_latg: {
    label: 'Steering angle vs Lateral G',
    xId: ch.STEERING_ANGLE,
    yId: ch.G_LAT,
    xName: 'Steering (deg)',
    yName: 'Lateral G (+ left)',
    xMap: (v) => v * RAD_TO_DEG,
    yMap: (v) => v,
  },
  speed_rpm: {
    label: 'Speed vs RPM',
    xId: ch.SPEED,
    yId: ch.RPM,
    xName: 'Speed (km/h)',
    yName: 'RPM',
    xMap: mpsToKmh,
    yMap: (v) => v,
  },
  brake_decel: {
    label: 'Brake pedal vs Deceleration',
    xId: ch.BRAKE,
    yId: ch.G_LONG,
    xName: 'Brake (%)',
    yName: 'Deceleration G',
    xMap: (v) => v * 100,
    yMap: (v) => -v,
  },
};

const RING_CHANNELS: readonly number[] = [ch.STEERING_ANGLE, ch.G_LAT, ch.SPEED, ch.RPM, ch.BRAKE, ch.G_LONG];
const MAX_POINTS = 600;
/** Fewer paired samples than this make correlation statistics meaningless. */
const MIN_STATS_SAMPLES = 10;

interface Stats {
  readonly r: number | null;
  readonly slope: number | null;
}

function regression(xs: readonly number[], ys: readonly number[]): Stats {
  const n = xs.length;
  if (n < MIN_STATS_SAMPLES) return { r: null, slope: null };
  let sx = 0;
  let sy = 0;
  for (let i = 0; i < n; i++) {
    sx += xs[i] ?? 0;
    sy += ys[i] ?? 0;
  }
  const mx = sx / n;
  const my = sy / n;
  let sxx = 0;
  let syy = 0;
  let sxy = 0;
  for (let i = 0; i < n; i++) {
    const dx = (xs[i] ?? 0) - mx;
    const dy = (ys[i] ?? 0) - my;
    sxx += dx * dx;
    syy += dy * dy;
    sxy += dx * dy;
  }
  return {
    r: sxx > 0 && syy > 0 ? sxy / Math.sqrt(sxx * syy) : null,
    slope: sxx > 0 ? sxy / sxx : null,
  };
}

/** X-Y scatter of real paired samples from the live ring, with Pearson r and least-squares slope. */
export function MultiplotterView(): JSX.Element {
  const [plotMode, setPlotMode] = useState<PlotMode>('steer_latg');
  const ring = useLiveRing(RING_CHANNELS);
  const plot = PLOTS[plotMode];

  const data = useMemo(() => {
    const xs: number[] = [];
    const ys: number[] = [];
    const xCol = ring.series.get(plot.xId) ?? [];
    const yCol = ring.series.get(plot.yId) ?? [];
    for (let i = 0; i < ring.rows; i++) {
      const x = xCol[i];
      const y = yCol[i];
      if (x === null || x === undefined || y === null || y === undefined) continue;
      xs.push(plot.xMap(x));
      ys.push(plot.yMap(y));
    }
    let xMin = Infinity;
    let xMax = -Infinity;
    let yMin = Infinity;
    let yMax = -Infinity;
    for (let i = 0; i < xs.length; i++) {
      const x = xs[i] ?? 0;
      const y = ys[i] ?? 0;
      if (x < xMin) xMin = x;
      if (x > xMax) xMax = x;
      if (y < yMin) yMin = y;
      if (y > yMax) yMax = y;
    }
    const xSpan = xMax - xMin || 1;
    const ySpan = yMax - yMin || 1;
    const step = Math.max(1, Math.ceil(xs.length / MAX_POINTS));
    const points: { x: number; y: number }[] = [];
    for (let i = 0; i < xs.length; i += step) {
      points.push({
        x: 5 + (((xs[i] ?? 0) - xMin) / xSpan) * 90,
        y: 95 - (((ys[i] ?? 0) - yMin) / ySpan) * 90,
      });
    }
    return {
      count: xs.length,
      points,
      stats: regression(xs, ys),
      range: xs.length > 0 ? { xMin, xMax, yMin, yMax } : null,
    };
  }, [ring, plot]);

  return (
    <div className="multiplotter-view" data-testid="multiplotter-view">
      <div className="plotter-header">
        <div className="plotter-title-group">
          <h2 className="plotter-title">Multiplotter (Channel Cross-Correlation Scatter)</h2>
          <p className="plotter-desc">
            Paired live samples over the live window{ring.windowS !== null ? ` (last ${String(ring.windowS)} s)` : ''}.
          </p>
        </div>

        <div className="plotter-mode-pills" role="radiogroup" aria-label="Scatter Mode">
          {(Object.keys(PLOTS) as PlotMode[]).map((mode) => (
            <button
              key={mode}
              type="button"
              className={`btn btn-xs ${plotMode === mode ? 'btn-primary' : ''}`}
              onClick={() => {
                setPlotMode(mode);
              }}
            >
              {PLOTS[mode].label}
            </button>
          ))}
        </div>
      </div>

      {ring.windowS === null ? (
        <ViewNoData title={plot.label} reason={NOT_CONNECTED_REASON} testId="plotter-not-connected" />
      ) : data.count === 0 ? (
        <ViewNoData
          title={plot.label}
          reason="No paired live samples of these channels in the window yet."
          testId="plotter-no-samples"
        />
      ) : (
        <div className="plotter-viewport-card">
          <div className="plotter-svg-wrapper">
            <svg viewBox="0 0 100 100" className="plotter-svg" preserveAspectRatio="none" data-testid="plotter-scatter">
              {data.points.map((p, i) => (
                <circle key={i} cx={p.x} cy={p.y} r="1.2" fill="#38bdf8" opacity="0.8" />
              ))}
            </svg>
          </div>

          <div className="plotter-sidebar-stats">
            <div className="plot-stat-card">
              <span className="stat-label">Samples</span>
              <span className="stat-value">{data.count}</span>
              <span className="stat-note">
                X: {plot.xName}
                {data.range ? ` ${data.range.xMin.toFixed(1)} .. ${data.range.xMax.toFixed(1)}` : ''}
              </span>
              <span className="stat-note">
                Y: {plot.yName}
                {data.range ? ` ${data.range.yMin.toFixed(2)} .. ${data.range.yMax.toFixed(2)}` : ''}
              </span>
            </div>
            <div className="plot-stat-card">
              <span className="stat-label">Pearson Correlation (r)</span>
              <span className="stat-value num-cyan" data-testid="plotter-r">
                {data.stats.r !== null ? data.stats.r.toFixed(3) : NO_VALUE}
              </span>
            </div>
            <div className="plot-stat-card">
              <span className="stat-label">Least-squares slope (Y per X unit)</span>
              <span className="stat-value">{data.stats.slope !== null ? data.stats.slope.toPrecision(3) : NO_VALUE}</span>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
