import { useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { mpsToKmh } from '../../lib/units';
import { useLiveRing } from '../liveData';
import { NOT_CONNECTED_REASON, ViewNoData } from './ViewNoData';

interface Bin {
  readonly label: string;
  readonly test: (v: number) => boolean;
}

interface BinResult {
  readonly label: string;
  readonly pct: number;
}

const RING_CHANNELS: readonly number[] = [ch.SPEED, ch.THROTTLE, ch.BRAKE, ch.GEAR];

const SPEED_BINS: readonly Bin[] = [
  { label: '0-50 km/h', test: (v) => v < 50 },
  { label: '50-100 km/h', test: (v) => v >= 50 && v < 100 },
  { label: '100-150 km/h', test: (v) => v >= 100 && v < 150 },
  { label: '150-200 km/h', test: (v) => v >= 150 && v < 200 },
  { label: '200-250 km/h', test: (v) => v >= 200 && v < 250 },
  { label: '250-300 km/h', test: (v) => v >= 250 && v < 300 },
  { label: '300+ km/h', test: (v) => v >= 300 },
];

const PEDAL_BINS: readonly Bin[] = [
  { label: '0%', test: (v) => v <= 0 },
  { label: '1-25%', test: (v) => v > 0 && v <= 0.25 },
  { label: '26-50%', test: (v) => v > 0.25 && v <= 0.5 },
  { label: '51-75%', test: (v) => v > 0.5 && v <= 0.75 },
  { label: '76-99%', test: (v) => v > 0.75 && v < 1 },
  { label: '100%', test: (v) => v >= 1 },
];

function histogram(values: readonly (number | null)[] | undefined, bins: readonly Bin[], map?: (v: number) => number): {
  readonly bins: readonly BinResult[];
  readonly samples: number;
} {
  const counts = bins.map(() => 0);
  let samples = 0;
  for (const raw of values ?? []) {
    if (raw === null) continue;
    const v = map ? map(raw) : raw;
    const idx = bins.findIndex((b) => b.test(v));
    if (idx >= 0) {
      counts[idx] = (counts[idx] ?? 0) + 1;
      samples++;
    }
  }
  return {
    samples,
    bins: bins.map((b, i) => ({ label: b.label, pct: samples > 0 ? ((counts[i] ?? 0) / samples) * 100 : 0 })),
  };
}

function gearHistogram(values: readonly (number | null)[] | undefined): {
  readonly bins: readonly BinResult[];
  readonly samples: number;
} {
  const counts = new Map<number, number>();
  let samples = 0;
  for (const raw of values ?? []) {
    if (raw === null) continue;
    const g = Math.round(raw);
    if (g <= 0) continue;
    counts.set(g, (counts.get(g) ?? 0) + 1);
    samples++;
  }
  const gears = [...counts.keys()].sort((a, b) => a - b);
  return {
    samples,
    bins: gears.map((g) => ({ label: `Gear ${String(g)}`, pct: ((counts.get(g) ?? 0) / samples) * 100 })),
  };
}

function HistCard({
  title,
  result,
  testId,
}: {
  readonly title: string;
  readonly result: { readonly bins: readonly BinResult[]; readonly samples: number };
  readonly testId: string;
}): JSX.Element {
  return (
    <div className="hist-card" data-testid={testId}>
      <div className="hist-card-header">
        <span className="hist-card-title">{title}</span>
        <span className="hist-card-sub">{result.samples} samples</span>
      </div>
      {result.samples === 0 ? (
        <ViewNoData title={title} reason="No live samples of this channel in the window yet." />
      ) : (
        <div className="hist-bars-list">
          {result.bins.map((bin) => (
            <div key={bin.label} className="hist-bar-row">
              <span className="hist-bar-label">{bin.label}</span>
              <div className="hist-bar-track">
                <div className="hist-bar-fill view-hist-bar-fill" style={{ width: `${Math.min(100, bin.pct)}%` }} />
              </div>
              <span className="hist-bar-val">{bin.pct.toFixed(1)}%</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** Distributions of real samples in the live ring (the last `windowS` seconds the app keeps). */
export function MultihistogramView(): JSX.Element {
  const ring = useLiveRing(RING_CHANNELS);
  const { speed, throttle, brake, gears } = useMemo(
    () => ({
      speed: histogram(ring.series.get(ch.SPEED), SPEED_BINS, mpsToKmh),
      throttle: histogram(ring.series.get(ch.THROTTLE), PEDAL_BINS),
      brake: histogram(ring.series.get(ch.BRAKE), PEDAL_BINS),
      gears: gearHistogram(ring.series.get(ch.GEAR)),
    }),
    [ring],
  );

  return (
    <div className="multihistogram-view" data-testid="multihistogram-view">
      <div className="hist-header">
        <h2 className="hist-title">Multi-Histogram Channel Distributions</h2>
        <p className="hist-desc">
          Share of live samples per band over the live window
          {ring.windowS !== null ? ` (last ${String(ring.windowS)} s of session time)` : ''}.
        </p>
      </div>

      {ring.windowS === null ? (
        <ViewNoData title="Histograms" reason={NOT_CONNECTED_REASON} testId="hist-not-connected" />
      ) : (
        <div className="hist-grid">
          <HistCard title="SPEED DISTRIBUTION" result={speed} testId="hist-speed" />
          <HistCard title="THROTTLE PEDAL" result={throttle} testId="hist-throttle" />
          <HistCard title="BRAKE PEDAL" result={brake} testId="hist-brake" />
          <HistCard title="GEAR USAGE" result={gears} testId="hist-gear" />
        </div>
      )}
    </div>
  );
}
