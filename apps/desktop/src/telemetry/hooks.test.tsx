import { act, render, screen } from '@testing-library/react';
import type { JSX, ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { FrequencyClass, Quality, ch } from '../generated/channels';
import { FakeClock, FakeFrames } from '../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../test/tlvEncoder';
import {
  AGE_REFRESH_INTERVAL_MS,
  TelemetryStoreContext,
  useChannelSample,
  useChannelValues,
  useLiveBatchAgeMs,
  useStreamAgeMs,
  useStreamBasis,
  useStreamState,
  useTelemetryStatus,
} from './hooks';
import { TelemetryStore } from './store';

function setup(): {
  store: TelemetryStore;
  clock: FakeClock;
  frames: FakeFrames;
  send: (et: number, records: readonly FixtureRecord[]) => void;
  wrapper: (props: { children: ReactNode }) => JSX.Element;
} {
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
  let seq = 0;
  const send = (et: number, records: readonly FixtureRecord[]): void => {
    store.applyBatch(encodeUiBatch(0, seq++, [encodeTlvBlock({ sourceEtS: et }, records)]));
  };
  const wrapper = ({ children }: { children: ReactNode }): JSX.Element => (
    <TelemetryStoreContext value={store}>{children}</TelemetryStoreContext>
  );
  return { store, clock, frames, send, wrapper };
}

afterEach(() => {
  vi.useRealTimers();
});

describe('useChannelSample', () => {
  it('re-renders only when its channel changes, at most once per frame', () => {
    const { frames, send, wrapper } = setup();
    let renders = 0;
    function Speed(): JSX.Element {
      renders += 1;
      const sample = useChannelSample(ch.SPEED);
      return <span data-testid="speed">{sample.value === null ? 'none' : String(sample.value)}</span>;
    }
    render(<Speed />, { wrapper });
    expect(renders).toBe(1);
    expect(screen.getByTestId('speed').textContent).toBe('none');

    send(1, [rec.f32(ch.SPEED, 10)]);
    send(1.01, [rec.f32(ch.SPEED, 11)]);
    send(1.02, [rec.f32(ch.SPEED, 12)]);
    act(() => {
      frames.runFrame();
    });
    expect(renders).toBe(2); // three batches, one frame, one render
    expect(screen.getByTestId('speed').textContent).toBe('12');

    send(1.03, [rec.f32(ch.RPM, 9000)]); // another channel
    act(() => {
      frames.runFrame();
    });
    expect(renders).toBe(2);

    send(1.04, [rec.f32(ch.SPEED, 12)]); // same value again
    act(() => {
      frames.runFrame();
    });
    expect(renders).toBe(2);
  });

  it('throws a clear error without a store in context', () => {
    function Orphan(): JSX.Element {
      useChannelSample(ch.SPEED);
      return <span />;
    }
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    expect(() => render(<Orphan />)).toThrow(/TelemetryStoreContext/);
  });
});

describe('useChannelValues', () => {
  it('returns a stable array and ignores unrelated channels, even with a new ids array each render', () => {
    const { frames, send, wrapper } = setup();
    let renders = 0;
    const seen: (readonly unknown[])[] = [];
    function Pair(): JSX.Element {
      renders += 1;
      const samples = useChannelValues([ch.SPEED, ch.GEAR]);
      seen.push(samples);
      return <span>{samples.map((s) => String(s.value)).join(',')}</span>;
    }
    const { container } = render(<Pair />, { wrapper });
    send(1, [rec.f32(ch.RPM, 1)]);
    act(() => {
      frames.runFrame();
    });
    expect(renders).toBe(1);
    send(1.01, [rec.f32(ch.SPEED, 50), rec.i32(ch.GEAR, 4)]);
    act(() => {
      frames.runFrame();
    });
    expect(renders).toBe(2);
    expect(container.textContent).toBe('50,4');
    expect(seen[0]).not.toBe(seen[1]);
  });
});

describe('useStreamState and useLiveBatchAgeMs', () => {
  it('follows the physics stream through LIVE, DEGRADED and STALE with a ticking age', () => {
    vi.useFakeTimers();
    const { clock, frames, send, wrapper } = setup();
    function Stream(): JSX.Element {
      const state = useStreamState(FrequencyClass.Physics);
      const age = useLiveBatchAgeMs(state !== 'live');
      return <span data-testid="stream">{`${state}:${age === null ? '-' : String(age)}`}</span>;
    }
    render(<Stream />, { wrapper });
    expect(screen.getByTestId('stream').textContent).toBe('noData:-');
    send(1, [rec.f32(ch.SPEED, 1)]);
    act(() => {
      frames.runFrame();
    });
    expect(screen.getByTestId('stream').textContent).toBe('live:-');
    act(() => {
      clock.advance(1_200);
      frames.runFrame();
    });
    act(() => {
      vi.advanceTimersByTime(AGE_REFRESH_INTERVAL_MS);
    });
    expect(screen.getByTestId('stream').textContent).toBe('stale:1200');
    act(() => {
      clock.advance(300);
      vi.advanceTimersByTime(AGE_REFRESH_INTERVAL_MS);
    });
    expect(screen.getByTestId('stream').textContent).toBe('stale:1500');
  });

  it('useStreamBasis and useStreamAgeMs follow the reported LMU source age of a class (review F3)', () => {
    vi.useFakeTimers();
    const { clock, frames, send, wrapper } = setup();
    function Scoring(): JSX.Element {
      const state = useStreamState(FrequencyClass.Scoring);
      const basis = useStreamBasis(FrequencyClass.Scoring);
      const age = useStreamAgeMs(FrequencyClass.Scoring, state !== 'live');
      return <span data-testid="scoring">{`${state}:${basis}:${age === null ? '-' : String(age)}`}</span>;
    }
    render(<Scoring />, { wrapper });
    expect(screen.getByTestId('scoring').textContent).toBe('noData:linkOnly:-');
    send(1, [rec.f64(ch.BEST_LAP_TIME, 97.5)]);
    act(() => {
      frames.runFrame();
    });
    // Only the link is measured: a batch just arrived, so the class reads LIVE, and says so.
    expect(screen.getByTestId('scoring').textContent).toBe('live:linkOnly:-');
    // The core reports that LMU's scoring buffer has not been refreshed for 2 s: STALE although batches flow.
    send(1.01, [rec.f32(ch.SRC_SCORING_AGE, 2)]);
    act(() => {
      frames.runFrame();
    });
    act(() => {
      vi.advanceTimersByTime(AGE_REFRESH_INTERVAL_MS);
    });
    expect(screen.getByTestId('scoring').textContent).toBe('stale:endToEnd:2000');
    act(() => {
      clock.advance(100);
      vi.advanceTimersByTime(AGE_REFRESH_INTERVAL_MS);
    });
    expect(screen.getByTestId('scoring').textContent).toBe('stale:endToEnd:2100');
    // A source age that is no longer VALID is not used: back to the link alone.
    send(1.02, [rec.quality(ch.SRC_SCORING_AGE, Quality.Unavailable)]);
    act(() => {
      frames.runFrame();
    });
    expect(screen.getByTestId('scoring').textContent).toBe('live:linkOnly:-');
  });

  it('exposes the store status (state and counters)', () => {
    const { frames, send, wrapper } = setup();
    function Status(): JSX.Element {
      const status = useTelemetryStatus();
      return <span data-testid="status">{`${status.state}:${status.batchesApplied}`}</span>;
    }
    render(<Status />, { wrapper });
    expect(screen.getByTestId('status').textContent).toBe('waiting:0');
    send(1, []);
    act(() => {
      frames.runFrame();
    });
    expect(screen.getByTestId('status').textContent).toBe('receiving:1');
  });
});
