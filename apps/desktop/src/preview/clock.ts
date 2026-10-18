/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). Clocks of the preview page.
 *
 * - `Date` is anchored at the preview world's wall clock (`PREVIEW_NOW_UNIX_S` at page load), so values the app
 *   derives from `Date.now()` (code expiry countdowns, "connected since") agree with the scenario's DTOs.
 * - `hold()` pins `performance.now()` and `Date` for the screenshot: the fake backend has stopped at its planned
 *   capture time, and with the clocks held the app's freshness timers keep seeing the stream as it was at that
 *   moment (instead of ageing it to DEGRADED / STALE while the capture script reads the page).
 *
 * Installed by src/preview/main.tsx before the app renders. Never used by the app build.
 */
import { PREVIEW_NOW_UNIX_S } from './status';

let installed = false;
let heldPerfMs: number | null = null;
let heldDateMs: number | null = null;

/** Installs the anchored `Date` and the holdable `performance.now`. Idempotent. */
export function installPreviewClock(): void {
  if (installed) return;
  installed = true;
  const realPerfNow = performance.now.bind(performance);
  const pageStartPerf = realPerfNow();
  const anchorMs = PREVIEW_NOW_UNIX_S * 1000;
  const nowMs = (): number => heldDateMs ?? anchorMs + (realPerfNow() - pageStartPerf);

  Object.defineProperty(performance, 'now', {
    configurable: true,
    value: (): number => heldPerfMs ?? realPerfNow(),
  });

  const RealDate = Date;
  class PreviewDate extends RealDate {
    constructor(...args: unknown[]) {
      if (args.length === 0) super(nowMs());
      else super(...(args as [string | number | Date]));
    }

    static override now(): number {
      return nowMs();
    }
  }
  globalThis.Date = PreviewDate as DateConstructor;
}

/**
 * Pins both clocks: `performance.now()` at its current value, `Date` at the preview world time of the capture
 * (`elapsedS` after page load, the fake backend's planned capture time).
 */
export function holdPreviewClock(elapsedS: number): void {
  heldPerfMs = performance.now();
  heldDateMs = PREVIEW_NOW_UNIX_S * 1000 + elapsedS * 1000;
}
