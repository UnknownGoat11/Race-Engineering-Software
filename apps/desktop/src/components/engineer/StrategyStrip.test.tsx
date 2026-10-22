import { act, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ch } from '../../generated/channels';
import { formatWeather as weatherText, NO_VALUE, UNIT_SPACE } from '../../lib/units';
import { TelemetryStore } from '../../telemetry/store';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { FakeClock, FakeFrames } from '../../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec } from '../../test/tlvEncoder';
import { StrategyStrip } from './StrategyStrip';

describe('StrategyStrip', () => {
  const store = new TelemetryStore();

  it('renders strategy strip with provided session items', () => {
    render(
      <TelemetryStoreContext.Provider value={store}>
        <StrategyStrip sessionClockS={3600} completedLaps={42} trackTempC={32} airTempC={24} rainFrac={0} />
      </TelemetryStoreContext.Provider>,
    );

    expect(screen.getByTestId('strategy-strip')).toBeInTheDocument();
    expect(screen.getByText('Session')).toBeInTheDocument();
    expect(screen.getByText('1:00:00')).toBeInTheDocument();
    expect(screen.getByText('Laps')).toBeInTheDocument();
    expect(screen.getByText('42')).toBeInTheDocument();
    expect(screen.getByText('No rain')).toBeInTheDocument();
  });

  it('hides sourceless items when props are null or undefined; the flag reads plain "—"', () => {
    render(
      <TelemetryStoreContext.Provider value={store}>
        <StrategyStrip />
      </TelemetryStoreContext.Provider>,
    );

    expect(screen.getByTestId('strategy-strip')).toBeInTheDocument();
    expect(screen.queryByText('Session')).not.toBeInTheDocument();
    expect(screen.queryByText('Laps')).not.toBeInTheDocument();
    expect(screen.queryByText('Stint')).not.toBeInTheDocument();
    // Unknown rain: no invented "No rain" (U03).
    expect(screen.queryByText('No rain')).not.toBeInTheDocument();
    expect(screen.getByTestId('strategy-flag')).toHaveTextContent(`Flag${NO_VALUE}`);
    expect(screen.getByTestId('strategy-flag').querySelector('.badge')).toBeNull();
  });

  it('rain: "No rain" only when 0 was measured, "Rain" above 0, unknown omitted (U03)', () => {
    expect(weatherText(0)).toBe('No rain');
    expect(weatherText(0.3)).toBe('Rain');
    expect(weatherText(null)).toBeNull();
    expect(weatherText(undefined)).toBeNull();
    expect(weatherText(Number.NaN)).toBeNull();
  });

  it('an active caution gets a pill', () => {
    render(
      <TelemetryStoreContext.Provider value={store}>
        <StrategyStrip flagText="Pending" flagLevel="warn" />
      </TelemetryStoreContext.Provider>,
    );
    const badge = screen.getByTestId('strategy-flag').querySelector('.badge');
    expect(badge).toHaveTextContent('Pending');
    expect(badge?.className).toContain('badge-status-warn');
  });

  it('fuel, VE and tyre laps are laps, fuel to add is litres (U07, E06)', () => {
    const clock = new FakeClock();
    const frames = new FakeFrames();
    const live = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
    render(
      <TelemetryStoreContext value={live}>
        <StrategyStrip />
      </TelemetryStoreContext>,
    );
    live.applyBatch(
      encodeUiBatch(0, 0, [
        encodeTlvBlock({ sourceEtS: 1 }, [
          rec.f32(ch.FUEL_LAPS_REMAINING, 13.7),
          rec.f32(ch.VE_LAPS_REMAINING, 15.2),
          rec.f32(ch.FUEL_TO_ADD, 12),
          rec.u32(ch.TYRE_SET_LAPS, 9),
        ]),
      ]),
    );
    act(() => {
      frames.runFrame();
    });
    const fuel = screen.getByTestId('strategy-fuel');
    expect(fuel.textContent).toContain(`13.7${UNIT_SPACE}laps`);
    expect(fuel).toHaveTextContent('Limit');
    expect(fuel.textContent).not.toMatch(/\bL\b/);
    expect(screen.getByTestId('strategy-ve')).not.toHaveTextContent('Limit');
    expect(screen.getByTestId('strategy-fuel-add').textContent).toBe(`Fuel to add12.0${UNIT_SPACE}L`);
    expect(screen.getByTestId('strategy-tyres').textContent).toBe(`Tyres9${UNIT_SPACE}laps`);
  });
});
