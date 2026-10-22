import { useEffect, useState, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatDelta, NO_VALUE } from '../../lib/units';
import { useTelemetryStore } from '../../telemetry/hooks';
import type { TelemetryStore } from '../../telemetry/store';
import '../../styles/engineer.css';

/** Minisectors per lap (equal fractions of LAP_PROGRESS). */
const MINISECTOR_COUNT = 12;

const TRACKED_CHANNELS = [ch.LAP_PROGRESS, ch.DELTA_PERSONAL_BEST, ch.LAP_NUMBER] as const;

/**
 * Per-minisector time gained or lost against the personal best on the current lap, measured from the live
 * DELTA_PERSONAL_BEST channel: a minisector's delta is the running delta when the car leaves it minus the
 * running delta when it entered. A minisector the car has not yet completed this lap, or whose entry was not
 * observed (view opened mid-minisector, a progress jump, missing delta), has no value.
 */
interface MinisectorTracker {
  readonly lap: number | null;
  /** Minisector the car is in, or null before the first progress sample. */
  readonly index: number | null;
  /** Running delta when the current minisector was entered; null when that entry was not observed. */
  readonly entryDelta: number | null;
  /** Completed minisector deltas of the current lap (s); null where not measured. */
  readonly deltas: readonly (number | null)[];
}

const INITIAL_TRACKER: MinisectorTracker = {
  lap: null,
  index: null,
  entryDelta: null,
  deltas: Array.from({ length: MINISECTOR_COUNT }, () => null),
};

function minisectorOf(progress: number): number {
  const frac = Math.min(1, Math.max(0, progress));
  return Math.min(MINISECTOR_COUNT - 1, Math.floor(frac * MINISECTOR_COUNT));
}

/** Pure tracker step for one observation of (lap number, lap progress 0..1, running delta to PB in s). */
function advanceTracker(
  prev: MinisectorTracker,
  lap: number | null,
  progress: number | null,
  delta: number | null,
): MinisectorTracker {
  if (progress === null) return prev;
  const index = minisectorOf(progress);

  // A new lap: start a fresh row. Only minisector 1 can be measured if we see it from its start.
  if (prev.lap !== null && lap !== null && lap !== prev.lap) {
    return {
      lap,
      index,
      entryDelta: index === 0 ? delta : null,
      deltas: INITIAL_TRACKER.deltas,
    };
  }

  if (prev.index === null) {
    // First observation: the entry into this minisector was not seen.
    return { lap, index, entryDelta: null, deltas: prev.deltas };
  }

  if (index === prev.index) {
    // Still inside: a missing delta makes this minisector unmeasurable.
    return delta === null && prev.entryDelta !== null ? { ...prev, lap, entryDelta: null } : { ...prev, lap };
  }

  if (index === prev.index + 1) {
    const deltas = [...prev.deltas];
    deltas[prev.index] = prev.entryDelta !== null && delta !== null ? delta - prev.entryDelta : null;
    return { lap, index, entryDelta: delta, deltas };
  }

  // Skipped minisectors or moved backwards: neither the left nor the new minisector is measurable.
  return { lap, index, entryDelta: null, deltas: prev.deltas };
}

function readNumber(store: TelemetryStore, id: number): number | null {
  const v = store.getSample(id).value;
  return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

function observe(store: TelemetryStore, prev: MinisectorTracker): MinisectorTracker {
  return advanceTracker(
    prev,
    readNumber(store, ch.LAP_NUMBER),
    readNumber(store, ch.LAP_PROGRESS),
    readNumber(store, ch.DELTA_PERSONAL_BEST),
  );
}

function statusClass(delta: number | null): string {
  if (delta === null) return 'ms-nodata';
  if (delta < 0) return 'ms-faster';
  if (delta > 0) return 'ms-slower';
  return '';
}

/**
 * One 24 px row (audit V03): "Minisectors vs PB" plus 12 cells. Renders nothing until at least one minisector of
 * the current lap has been measured (an all-"—" ribbon only takes graph space).
 */
export function MinisectorRibbon(): JSX.Element | null {
  const store = useTelemetryStore();
  const [tracker, setTracker] = useState<MinisectorTracker>(() => observe(store, INITIAL_TRACKER));

  useEffect(
    () =>
      store.subscribeChannels(TRACKED_CHANNELS, () => {
        setTracker((prev) => observe(store, prev));
      }),
    [store],
  );

  if (tracker.deltas.every((d) => d === null)) return null;

  return (
    <div className="engineer-minisector-ribbon" data-testid="engineer-minisector-ribbon">
      <span className="ms-ribbon-tag">Minisectors vs PB</span>
      <div className="ms-ribbon-grid">
        {tracker.deltas.map((delta, idx) => {
          const text = delta !== null ? formatDelta(delta) : NO_VALUE;
          const active = idx === tracker.index;
          return (
            <div
              key={idx}
              className={`num ms-ribbon-cell ${statusClass(delta)} ${active ? 'ms-cell-active' : ''}`}
              title={`Minisector ${String(idx + 1)}${active ? ' (current)' : ''}: ${delta !== null ? `${text} s` : 'not measured'}`}
              aria-current={active ? 'true' : undefined}
              data-testid={`ms-cell-${String(idx + 1)}`}
            >
              {text}
            </div>
          );
        })}
      </div>
    </div>
  );
}
