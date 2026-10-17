import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { FrequencyClass, ch } from '../../generated/channels';
import { FakeClock, FakeFrames } from '../../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec } from '../../test/tlvEncoder';
import { AGE_REFRESH_INTERVAL_MS, TelemetryStoreContext } from '../../telemetry/hooks';
import { TelemetryStore } from '../../telemetry/store';
import { DataQualityIndicator, StreamQualityIndicator } from './DataQualityIndicator';

afterEach(() => {
  vi.useRealTimers();
});

describe('DataQualityIndicator (spec §87, §126)', () => {
  it.each([
    ['live', 'LIVE', 'Telemetry: live'],
    ['degraded', 'DEGRADED', 'Telemetry: degraded'],
    ['stale', 'STALE', 'Telemetry: stale'],
    ['invalid', 'INVALID', 'Telemetry: invalid'],
    ['noData', 'NO DATA', 'Telemetry: no data'],
    ['disconnected', 'DISCONNECTED', 'Telemetry: disconnected'],
  ] as const)('%s', (state, label, name) => {
    render(<DataQualityIndicator label="Telemetry" state={state} />);
    const indicator = screen.getByRole('group');
    expect(indicator).toHaveTextContent(label);
    expect(indicator).toHaveAccessibleName(name);
    expect(indicator).toHaveAttribute('data-state', state);
  });

  it('shows the age in readable units', () => {
    const { rerender } = render(<DataQualityIndicator label="Timing" state="degraded" ageMs={420} />);
    expect(screen.getByRole('group')).toHaveTextContent('420 ms');
    expect(screen.getByRole('group')).toHaveAccessibleName('Timing: degraded, last update 420 milliseconds ago');
    rerender(<DataQualityIndicator label="Timing" state="stale" ageMs={3_420} />);
    expect(screen.getByRole('group')).toHaveTextContent('3.4 s');
    rerender(<DataQualityIndicator label="Timing" state="stale" ageMs={125_000} />);
    expect(screen.getByRole('group')).toHaveTextContent('2:05');
    expect(screen.getByRole('group')).toHaveAccessibleName('Timing: stale, last update 2 minutes 5 seconds ago');
  });

  it('labels a state that measures only the link LINK ONLY, visibly and for screen readers (review F3)', () => {
    const { rerender } = render(<DataQualityIndicator label="Timing" state="live" basis="linkOnly" />);
    const indicator = screen.getByRole('group');
    expect(indicator).toHaveTextContent('LIVE');
    expect(indicator).toHaveTextContent('LINK ONLY');
    expect(indicator).toHaveAttribute('data-basis', 'linkOnly');
    expect(indicator).toHaveAccessibleName('Timing: live, link freshness only, game data age not reported');
    rerender(<DataQualityIndicator label="Timing" state="stale" ageMs={3_420} basis="linkOnly" />);
    expect(indicator).toHaveAccessibleName(
      'Timing: stale, last update 3.4 seconds ago, link freshness only, game data age not reported',
    );
    // States that claim no freshness carry no basis label.
    for (const state of ['noData', 'disconnected', 'invalid'] as const) {
      rerender(<DataQualityIndicator label="Timing" state={state} basis="linkOnly" />);
      expect(indicator).not.toHaveTextContent('LINK ONLY');
    }
    rerender(<DataQualityIndicator label="Timing" state="live" basis="endToEnd" />);
    expect(indicator).not.toHaveTextContent('LINK ONLY');
  });
});

describe('StreamQualityIndicator', () => {
  it('shows NO DATA, then LIVE, then STALE with the age of the last live batch', () => {
    vi.useFakeTimers();
    const clock = new FakeClock();
    const frames = new FakeFrames();
    const store = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
    render(
      <TelemetryStoreContext value={store}>
        <StreamQualityIndicator label="Telemetry" frequency={FrequencyClass.Physics} />
      </TelemetryStoreContext>,
    );
    const indicator = screen.getByRole('group');
    expect(indicator).toHaveTextContent('NO DATA');
    store.applyBatch(encodeUiBatch(0, 1, [encodeTlvBlock({ sourceEtS: 1 }, [rec.f32(ch.SPEED, 1)])]));
    act(() => {
      frames.runFrame();
    });
    expect(indicator).toHaveTextContent('LIVE');
    expect(indicator).not.toHaveTextContent('ms');
    act(() => {
      clock.advance(2_500);
      frames.runFrame();
    });
    act(() => {
      vi.advanceTimersByTime(AGE_REFRESH_INTERVAL_MS);
    });
    expect(indicator).toHaveTextContent('STALE');
    expect(indicator).toHaveTextContent('2.5 s');
    // The Rust core does not report the LMU telemetry age here, so the indicator says what it measures.
    expect(indicator).toHaveTextContent('LINK ONLY');
  });

  it('includes the reported LMU source age in the class age and drops LINK ONLY (review F3)', () => {
    vi.useFakeTimers();
    const clock = new FakeClock();
    const frames = new FakeFrames();
    const store = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
    render(
      <TelemetryStoreContext value={store}>
        <StreamQualityIndicator label="Telemetry" frequency={FrequencyClass.Physics} />
      </TelemetryStoreContext>,
    );
    const indicator = screen.getByRole('group');
    // LMU's telemetry buffer was last refreshed 0.5 s before this batch was built.
    store.applyBatch(
      encodeUiBatch(0, 1, [
        encodeTlvBlock({ sourceEtS: 1 }, [rec.f32(ch.SPEED, 1), rec.f32(ch.SRC_TELEMETRY_AGE, 0.5)]),
      ]),
    );
    act(() => {
      frames.runFrame();
    });
    act(() => {
      vi.advanceTimersByTime(AGE_REFRESH_INTERVAL_MS);
    });
    expect(indicator).toHaveTextContent('DEGRADED');
    expect(indicator).toHaveTextContent('500 ms');
    expect(indicator).not.toHaveTextContent('LINK ONLY');
    expect(indicator).toHaveAttribute('data-basis', 'endToEnd');
  });
});
