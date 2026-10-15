/**
 * A frame scheduler shared by the TelemetryStore and the graph engines (spec §121–122): every callback requested
 * before the next animation frame runs in that one frame, and callbacks requested WHILE the frame runs (a store
 * notification invalidating a graph) run in the same frame too, so new data is drawn without an extra frame of
 * latency. Pass it as `scheduleFrame` to both `new TelemetryStore({ scheduleFrame })` and the graphs.
 */
import type { ScheduleFrame } from '../telemetry/store';

/** Platform hooks (injectable for tests). */
export interface FrameLoopHost {
  readonly requestFrame: (callback: () => void) => number;
  readonly cancelFrame: (handle: number) => void;
  /**
   * Receives an error thrown by a callback. Default: logged with `console.error` and rethrown from a microtask,
   * so it also reaches `window.onerror`, without stopping the other callbacks of the frame. (Graph engines catch
   * their own frame errors and show them; this only sees errors of other callbacks.)
   */
  readonly reportError?: (error: unknown) => void;
}

function rethrowLater(error: unknown): void {
  console.error('[graph] frame callback failed', error);
  queueMicrotask(() => {
    throw error instanceof Error ? error : new Error(String(error));
  });
}

/**
 * Passes over newly queued callbacks within one frame. A callback that keeps re-queueing itself (a bug) is
 * deferred to the next frame instead of blocking this one.
 */
const MAX_PASSES_PER_FRAME = 4;

interface Entry {
  callback: (() => void) | null;
}

/** Creates a frame loop on `host` (default `requestAnimationFrame`). */
export function createFrameLoop(host?: FrameLoopHost): ScheduleFrame {
  const platform: FrameLoopHost = host ?? {
    requestFrame: (callback) =>
      requestAnimationFrame(() => {
        callback();
      }),
    cancelFrame: (handle) => {
      cancelAnimationFrame(handle);
    },
  };
  const report = host?.reportError ?? rethrowLater;
  let queue: Entry[] = [];
  let pending: number | null = null;
  let running = false;

  const run = (): void => {
    pending = null;
    running = true;
    try {
      for (let pass = 0; pass < MAX_PASSES_PER_FRAME && queue.length > 0; pass++) {
        const current = queue;
        queue = [];
        for (const entry of current) {
          const callback = entry.callback;
          entry.callback = null;
          if (callback === null) continue;
          // One failing graph must not starve the others (they would never be scheduled again).
          try {
            callback();
          } catch (error: unknown) {
            report(error);
          }
        }
      }
    } finally {
      running = false;
      if (queue.length > 0) request();
    }
  };

  const request = (): void => {
    if (pending === null && !running) pending = platform.requestFrame(run);
  };

  return (callback) => {
    const entry: Entry = { callback };
    queue.push(entry);
    request();
    return () => {
      entry.callback = null;
      if (pending !== null && queue.every((e) => e.callback === null)) {
        platform.cancelFrame(pending);
        pending = null;
        queue = [];
      }
    };
  };
}

let shared: ScheduleFrame | null = null;

/** The application-wide frame loop (created on first use). */
export function sharedFrameLoop(): ScheduleFrame {
  shared ??= createFrameLoop();
  return shared;
}
