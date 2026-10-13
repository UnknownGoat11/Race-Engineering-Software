/**
 * Benchmark measurement (D-011: uPlot vs a minimal Canvas2D renderer). The harness owns the animation-frame loop:
 * each display frame it (1) feeds the synthetic rows that became due into the TelemetryStore and (2) runs the
 * shared frame loop that the store and all four graph engines schedule into. It records, per frame, the interval
 * since the previous frame (what the user sees, including React, layout and paint), the JS work of the frame and
 * the ingest share of it, and samples the JS heap where the browser exposes it (Chromium `performance.memory`).
 */
import { createFrameLoop } from '../src/graph/frameLoop';
import { summarize, type DurationSummary } from '../src/graph/frameStats';
import type { GraphEngine } from '../src/graph/engine';
import type { RendererKind } from '../src/graph/types';
import type { ScheduleFrame } from '../src/telemetry/store';

/** Frames slower than this multiple of the median interval missed at least one display refresh. */
export const LONG_FRAME_FACTOR = 1.5;
/** Frames slower than this are visible stutter regardless of the refresh rate (ms). */
export const VERY_LONG_FRAME_MS = 50;
/** JS budget of one 60 Hz frame (ms). */
export const FRAME_BUDGET_MS = 1_000 / 60;
const BYTES_PER_MB = 1024 * 1024;
/** How often the on-screen progress text updates (ms). */
const PROGRESS_INTERVAL_MS = 500;

/** Heap statistics in MB (null where the browser does not expose them). */
export interface HeapSummary {
  readonly startMB: number;
  readonly endMB: number;
  readonly maxMB: number;
}

/** Result of one renderer run. */
export interface RunResult {
  readonly renderer: RendererKind;
  readonly measuredSeconds: number;
  readonly frames: number;
  /** Display-frame intervals (ms). */
  readonly intervalMs: DurationSummary;
  /** JS work per frame: ingest + store notification + every graph engine frame (ms). */
  readonly workMs: DurationSummary;
  /** Ingest share of the work: wrapping the pre-encoded rows in a UiBatch + decode + ring append (ms). */
  readonly ingestMs: DurationSummary;
  /** Frames longer than LONG_FRAME_FACTOR × the median interval. */
  readonly longFrames: number;
  /** Frames longer than VERY_LONG_FRAME_MS. */
  readonly framesOver50ms: number;
  /** Frames whose JS work exceeded one 60 Hz frame. */
  readonly workOverBudget: number;
  /** Estimated display refresh rate (1000 / median interval). */
  readonly refreshHz: number;
  /** Per-graph engine frame durations (ms). */
  readonly engineFrameMs: readonly DurationSummary[];
  readonly heap: HeapSummary | null;
}

/** Everything published on `window.__benchResults`. */
export interface BenchResults {
  readonly label: 'SYNTHETIC BENCHMARK DATA';
  status: 'idle' | 'running' | 'done' | 'error';
  readonly config: {
    readonly graphs: number;
    readonly seriesPerGraph: number;
    readonly windowSeconds: number;
    readonly rateHz: number;
    readonly warmupSeconds: number;
    readonly measureSeconds: number;
    readonly renderers: readonly RendererKind[];
  };
  readonly environment: {
    readonly userAgent: string;
    readonly devicePixelRatio: number;
    readonly viewport: readonly [number, number];
  };
  runs: RunResult[];
  error?: string;
}

interface MemoryInfo {
  readonly usedJSHeapSize: number;
}

function heapBytes(): number | null {
  const memory = (performance as Performance & { readonly memory?: MemoryInfo }).memory;
  return memory === undefined ? null : memory.usedJSHeapSize;
}

/** A frame loop the harness runs manually inside its own animation frame. */
export interface ManualLoop {
  readonly schedule: ScheduleFrame;
  /** Runs everything scheduled so far (and what that schedules in turn). */
  run(): void;
}

/** Creates the manual loop. */
export function manualLoop(): ManualLoop {
  let pending: (() => void) | null = null;
  const schedule = createFrameLoop({
    requestFrame: (callback) => {
      pending = callback;
      return 1;
    },
    cancelFrame: () => {
      pending = null;
    },
  });
  return {
    schedule,
    run() {
      const callback = pending;
      pending = null;
      callback?.();
    },
  };
}

/** Inputs of one measured run. */
export interface RunInput {
  readonly renderer: RendererKind;
  readonly loop: ManualLoop;
  /** Feeds the rows due at `elapsedS` seconds after the live phase started. */
  readonly ingest: (elapsedS: number) => void;
  readonly engines: () => readonly GraphEngine[];
  readonly warmupSeconds: number;
  readonly measureSeconds: number;
  readonly onProgress: (text: string) => void;
}

/** Runs one renderer: warm-up, then `measureSeconds` of recorded frames. */
export function measure(input: RunInput): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    const intervals: number[] = [];
    const work: number[] = [];
    const ingest: number[] = [];
    let heapStart: number | null = null;
    let heapEnd: number | null = null;
    let heapMax = 0;
    let liveStart: number | null = null;
    let measureStart: number | null = null;
    let last: number | null = null;
    let lastProgress = 0;
    const tick = (now: number): void => {
      try {
        step(now);
      } catch (error: unknown) {
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    };
    const step = (now: number): void => {
      liveStart ??= now;
      const elapsedS = (now - liveStart) / 1_000;
      const a = performance.now();
      input.ingest(elapsedS);
      const b = performance.now();
      input.loop.run();
      const c = performance.now();
      const measuring = elapsedS >= input.warmupSeconds;
      if (measuring) {
        if (measureStart === null) {
          measureStart = now;
          for (const e of input.engines()) e.frameTimes.reset();
          heapStart = heapBytes();
        } else if (last !== null) {
          intervals.push(now - last);
          work.push(c - a);
          ingest.push(b - a);
        }
        const heap = heapBytes();
        if (heap !== null) {
          heapMax = Math.max(heapMax, heap);
          heapEnd = heap;
        }
      }
      last = now;
      if (now - lastProgress > PROGRESS_INTERVAL_MS) {
        lastProgress = now;
        const phase = measuring ? 'measuring' : 'warming up';
        input.onProgress(`${input.renderer}: ${phase} ${elapsedS.toFixed(1)} s`);
      }
      if (measureStart !== null && (now - measureStart) / 1_000 >= input.measureSeconds) {
        const intervalMs = summarize(intervals, Number.POSITIVE_INFINITY);
        const longThreshold = intervalMs.p50 * LONG_FRAME_FACTOR;
        resolve({
          renderer: input.renderer,
          measuredSeconds: (now - measureStart) / 1_000,
          frames: intervals.length,
          intervalMs: summarize(intervals, longThreshold),
          workMs: summarize(work, FRAME_BUDGET_MS),
          ingestMs: summarize(ingest, FRAME_BUDGET_MS),
          longFrames: intervals.filter((d) => d > longThreshold).length,
          framesOver50ms: intervals.filter((d) => d > VERY_LONG_FRAME_MS).length,
          workOverBudget: work.filter((d) => d > FRAME_BUDGET_MS).length,
          refreshHz: intervalMs.p50 > 0 ? 1_000 / intervalMs.p50 : Number.NaN,
          engineFrameMs: input.engines().map((e) => e.frameStats(FRAME_BUDGET_MS)),
          heap:
            heapStart === null || heapEnd === null
              ? null
              : { startMB: heapStart / BYTES_PER_MB, endMB: heapEnd / BYTES_PER_MB, maxMB: heapMax / BYTES_PER_MB },
        });
        return;
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}
