/**
 * Graph renderer benchmark (D-011, spec §61 "choose the best option after benchmarking", §121 performance).
 *
 * 4 synchronized graphs × 6 series showing the full 60 s live window at 100 Hz (24 000 points per graph),
 * live-scrolling at display rate, rendered with each renderer in turn through the production TelemetryGraph /
 * GraphEngine / LiveRingSource / TelemetryStore path. Results are shown on screen and published as JSON on
 * `window.__benchResults` for automation. URL parameters: `renderers=uplot,canvas2d`, `seconds=20`, `warmup=3`.
 *
 * All data here is SYNTHETIC BENCHMARK DATA (bench/synthetic.ts) and is labelled as such on screen.
 */
import { useCallback, useEffect, useMemo, useState, type JSX } from 'react';
import { Canvas2DRenderer } from '../src/graph/canvas2dRenderer';
import type { GraphEngine } from '../src/graph/engine';
import { LiveRingSource } from '../src/graph/liveSource';
import type { SeriesRenderer } from '../src/graph/renderer';
import { GraphSyncGroup } from '../src/graph/syncBus';
import { TelemetryGraph } from '../src/graph/TelemetryGraph';
import type { RendererKind } from '../src/graph/types';
import { UplotRenderer } from '../src/graph/uplotRenderer';
import { WebGLRenderer } from '../src/graph/webglRenderer';
import { TelemetryStore } from '../src/telemetry/store';
import { manualLoop, measure, type BenchResults, type ManualLoop, type RunResult } from './runner';
import { BENCH_GRAPHS, BENCH_RATE_HZ, BENCH_WINDOW_S, syntheticBatch, syntheticBatchOf, syntheticBlocks } from './synthetic';

declare global {
  interface Window {
    /** Benchmark results for automation (see the module comment). */
    __benchResults?: BenchResults;
  }
}

const DEFAULT_MEASURE_S = 20;
const DEFAULT_WARMUP_S = 3;
const ALL_RENDERERS: readonly RendererKind[] = ['webgl', 'canvas2d', 'uplot'];
/** Live rows encoded ahead of the run beyond warm-up + measurement (s); later rows are encoded on the fly. */
const PREENCODE_SLACK_S = 5;
/** Frames to wait after mounting graphs before the run starts (layout and first paint). */
const SETTLE_FRAMES = 3;

/** Default feed: UI batches at 60 per second, as the app receives them (D-012). `ingest=frame` feeds every display frame. */
const DEFAULT_INGEST_HZ = 60;

function params(): { renderers: RendererKind[]; measureSeconds: number; warmupSeconds: number; ingestHz: number } {
  const q = new URLSearchParams(window.location.search);
  const renderers = (q.get('renderers') ?? ALL_RENDERERS.join(','))
    .split(',')
    .filter((r): r is RendererKind => (ALL_RENDERERS as readonly string[]).includes(r));
  const seconds = Number(q.get('seconds') ?? DEFAULT_MEASURE_S);
  const warmup = Number(q.get('warmup') ?? DEFAULT_WARMUP_S);
  const ingestParam = q.get('ingest') ?? String(DEFAULT_INGEST_HZ);
  const ingestHz = ingestParam === 'frame' ? 0 : Number(ingestParam);
  return {
    renderers: renderers.length > 0 ? renderers : [...ALL_RENDERERS],
    measureSeconds: Number.isFinite(seconds) && seconds > 0 ? seconds : DEFAULT_MEASURE_S,
    warmupSeconds: Number.isFinite(warmup) && warmup >= 0 ? warmup : DEFAULT_WARMUP_S,
    ingestHz: Number.isFinite(ingestHz) && ingestHz >= 0 ? ingestHz : DEFAULT_INGEST_HZ,
  };
}

/** One run's live state: a fresh store, frame loop, sync group and renderer factory. */
interface Session {
  readonly renderer: RendererKind;
  readonly store: TelemetryStore;
  readonly loop: ManualLoop;
  readonly group: GraphSyncGroup;
  readonly ready: Promise<void>[];
  readonly engines: GraphEngine[];
  readonly makeRenderer: () => SeriesRenderer;
}

function newSession(renderer: RendererKind): Session {
  const loop = manualLoop();
  const store = new TelemetryStore({ scheduleFrame: loop.schedule, liveWindowS: BENCH_WINDOW_S });
  const ready: Promise<void>[] = [];
  return {
    renderer,
    store,
    loop,
    group: new GraphSyncGroup(),
    ready,
    engines: [],
    makeRenderer: () => {
      if (renderer === 'canvas2d') return new Canvas2DRenderer();
      if (renderer === 'webgl') return new WebGLRenderer();
      const r = new UplotRenderer();
      ready.push(r.ready);
      return r;
    },
  };
}

function nextFrames(n: number): Promise<void> {
  return new Promise((resolve) => {
    let left = n;
    const step = (): void => {
      left -= 1;
      if (left <= 0) resolve();
      else requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  });
}

function Graphs({ session }: { session: Session }): JSX.Element {
  // One live source per graph (sources reuse their window buffers between reads).
  const graphs = useMemo(
    () =>
      BENCH_GRAPHS.map((channels) => ({
        series: channels.map((channelId) => ({ channelId })),
        // Generated data: the graphs label it "Synthetic Test Data", never as LMU telemetry.
        source: new LiveRingSource(session.store, { xMode: 'time', synthetic: true }),
      })),
    [session],
  );
  const onEngine = useCallback(
    (engine: GraphEngine | null) => {
      if (engine !== null && !session.engines.includes(engine)) session.engines.push(engine);
    },
    [session],
  );
  return (
    <div className="bench__graphs">
      {graphs.map((g, i) => (
        <div className="bench__graph" key={`${session.renderer}-${i}`}>
          <TelemetryGraph
            title={`Graph ${i + 1} (${session.renderer})`}
            source={g.source}
            xMode="time"
            series={g.series}
            syncGroup={session.group}
            renderer={session.makeRenderer}
            initialSpan={BENCH_WINDOW_S}
            scheduleFrame={session.loop.schedule}
            onEngine={onEngine}
          />
        </div>
      ))}
    </div>
  );
}

function fmt(v: number, digits = 2): string {
  return Number.isFinite(v) ? v.toFixed(digits) : 'n/a';
}

function ResultsTable({ runs }: { runs: readonly RunResult[] }): JSX.Element {
  return (
    <table className="bench__table">
      <thead>
        <tr>
          <th>Renderer</th>
          <th>Frames</th>
          <th>Interval p50 / p95 / p99 / max (ms)</th>
          <th>Repaints � interval p50 / p95 (ms) � &gt; 18 ms</th>
          <th>Long frames</th>
          <th>&gt;50 ms</th>
          <th>JS work p50 / p95 / p99 (ms)</th>
          <th>Work &gt; 16.7 ms</th>
          <th>Ingest p95 (ms)</th>
          <th>JS heap start / end / max (MB)</th>
        </tr>
      </thead>
      <tbody>
        {runs.map((r) => (
          <tr key={r.renderer}>
            <td>{r.renderer}</td>
            <td>{r.frames}</td>
            <td>
              {fmt(r.intervalMs.p50)} / {fmt(r.intervalMs.p95)} / {fmt(r.intervalMs.p99)} / {fmt(r.intervalMs.max)}
            </td>
            <td>
              {r.repaints} � {fmt(r.repaintIntervalMs.p50)} / {fmt(r.repaintIntervalMs.p95)} � {r.repaintIntervalMs.long}
            </td>
            <td>{r.longFrames}</td>
            <td>{r.framesOver50ms}</td>
            <td>
              {fmt(r.workMs.p50)} / {fmt(r.workMs.p95)} / {fmt(r.workMs.p99)}
            </td>
            <td>{r.workOverBudget}</td>
            <td>{fmt(r.ingestMs.p95)}</td>
            <td>
              {r.heap === null
                ? 'not exposed by this browser'
                : `${fmt(r.heap.startMB, 1)} / ${fmt(r.heap.endMB, 1)} / ${fmt(r.heap.maxMB, 1)}`}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** The benchmark page: runs every renderer in turn and shows the results. */
export function Bench(): JSX.Element {
  const [config] = useState(params);
  const [session, setSession] = useState<Session | null>(null);
  const [progress, setProgress] = useState('starting');
  const [runs, setRuns] = useState<RunResult[]>([]);
  const [status, setStatus] = useState<BenchResults['status']>('idle');

  useEffect(() => {
    const results: BenchResults = {
      label: 'SYNTHETIC BENCHMARK DATA',
      status: 'running',
      config: {
        graphs: BENCH_GRAPHS.length,
        seriesPerGraph: BENCH_GRAPHS[0]?.length ?? 0,
        windowSeconds: BENCH_WINDOW_S,
        rateHz: BENCH_RATE_HZ,
        warmupSeconds: config.warmupSeconds,
        measureSeconds: config.measureSeconds,
        ingest: config.ingestHz === 0 ? 'frame' : config.ingestHz,
        renderers: config.renderers,
      },
      environment: {
        userAgent: navigator.userAgent,
        devicePixelRatio: window.devicePixelRatio,
        viewport: [window.innerWidth, window.innerHeight],
      },
      runs: [],
    };
    window.__benchResults = results;
    let cancelled = false;
    const run = async (): Promise<void> => {
      setStatus('running');
      for (const renderer of config.renderers) {
        if (cancelled) return;
        const s = newSession(renderer);
        const backfillRows = BENCH_WINDOW_S * BENCH_RATE_HZ;
        let seq = 0;
        const backfill = s.store.applyBatch(syntheticBatch(0, backfillRows, seq));
        if (!backfill.ok) throw new Error(`backfill refused: ${JSON.stringify(backfill.error)}`);
        let fed = backfillRows;
        // TLV encoding is the harness's cost, not the app's: encode the live rows before measuring.
        const liveRows = Math.ceil((config.warmupSeconds + config.measureSeconds + PREENCODE_SLACK_S) * BENCH_RATE_HZ);
        const liveBlocks = syntheticBlocks(backfillRows, backfillRows + liveRows);
        const blocksFor = (from: number, to: number): Uint8Array[] =>
          to - backfillRows <= liveBlocks.length
            ? liveBlocks.slice(from - backfillRows, to - backfillRows)
            : syntheticBlocks(from, to);
        setSession(s);
        await nextFrames(SETTLE_FRAMES);
        await Promise.all(s.ready);
        const result = await measure({
          renderer,
          loop: s.loop,
          engines: () => s.engines,
          warmupSeconds: config.warmupSeconds,
          measureSeconds: config.measureSeconds,
          ingestHz: config.ingestHz,
          onProgress: setProgress,
          ingest: (elapsedS) => {
            const due = backfillRows + Math.floor(elapsedS * BENCH_RATE_HZ);
            if (due <= fed) return;
            seq += 1;
            const applied = s.store.applyBatch(syntheticBatchOf(blocksFor(fed, due), seq));
            if (!applied.ok) throw new Error(`batch refused: ${JSON.stringify(applied.error)}`);
            fed = due;
          },
        });
        results.runs.push(result);
        setRuns([...results.runs]);
        setSession(null);
        await nextFrames(SETTLE_FRAMES);
        s.store.dispose();
      }
      results.status = 'done';
      setStatus('done');
      setProgress('done');
    };
    run().catch((error: unknown) => {
      results.status = 'error';
      results.error = String(error);
      setStatus('error');
      setProgress(`error: ${String(error)}`);
    });
    return () => {
      cancelled = true;
    };
  }, [config]);

  return (
    <main className="bench">
      <header className="bench__header">
        <h1 className="bench__title">Graph renderer benchmark (D-011)</h1>
        <span className="bench__synthetic" role="note">
          SYNTHETIC BENCHMARK DATA
        </span>
      </header>
      <p className="bench__config">
        {BENCH_GRAPHS.length} synchronized graphs × {BENCH_GRAPHS[0]?.length ?? 0} series × {BENCH_WINDOW_S} s at{' '}
        {BENCH_RATE_HZ} Hz, live-scrolling · warm-up {config.warmupSeconds} s · measured {config.measureSeconds} s per
        renderer · feed: {config.ingestHz === 0 ? 'every display frame' : `${config.ingestHz} batches/s`} · renderers:{' '}
        {config.renderers.join(', ')}
      </p>
      <p className="bench__status" data-status={status}>
        Status: {status} — {progress}
      </p>
      {runs.length > 0 && <ResultsTable runs={runs} />}
      {session !== null && <Graphs session={session} />}
    </main>
  );
}
