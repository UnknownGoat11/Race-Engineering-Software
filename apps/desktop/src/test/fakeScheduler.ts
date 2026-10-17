// Test support: a manual clock with timers and a manual animation-frame queue, injected into TelemetryStore.
import type { ScheduleFrame, ScheduleTimeout } from '../telemetry/store';

interface PendingTimer {
  readonly id: number;
  readonly at: number;
  readonly callback: () => void;
}

/** A clock that only moves when told to, firing due timers in order. */
export class FakeClock {
  #now = 0;
  #nextId = 0;
  #timers: PendingTimer[] = [];

  readonly now = (): number => this.#now;

  readonly scheduleTimeout: ScheduleTimeout = (callback, delayMs) => {
    const id = this.#nextId++;
    this.#timers.push({ id, at: this.#now + delayMs, callback });
    return () => {
      this.#timers = this.#timers.filter((t) => t.id !== id);
    };
  };

  /** Pending timer count. */
  get pendingTimers(): number {
    return this.#timers.length;
  }

  /** Advances time by `ms`, running every timer that falls due, in due order, at its due time. */
  advance(ms: number): void {
    const end = this.#now + ms;
    for (;;) {
      const due = this.#timers.filter((t) => t.at <= end).sort((a, b) => a.at - b.at || a.id - b.id)[0];
      if (due === undefined) break;
      this.#timers = this.#timers.filter((t) => t.id !== due.id);
      this.#now = Math.max(this.#now, due.at);
      due.callback();
    }
    this.#now = end;
  }
}

/** Animation frames that run only when `runFrame` is called. */
export class FakeFrames {
  #nextId = 0;
  #pending: { readonly id: number; readonly callback: () => void }[] = [];
  /** Number of frames that have been requested so far. */
  requested = 0;

  readonly schedule: ScheduleFrame = (callback) => {
    const id = this.#nextId++;
    this.requested += 1;
    this.#pending.push({ id, callback });
    return () => {
      this.#pending = this.#pending.filter((p) => p.id !== id);
    };
  };

  /** Requested frames not yet run. */
  get pending(): number {
    return this.#pending.length;
  }

  /** Runs every frame callback requested before this call. */
  runFrame(): void {
    const callbacks = this.#pending;
    this.#pending = [];
    for (const p of callbacks) p.callback();
  }
}
