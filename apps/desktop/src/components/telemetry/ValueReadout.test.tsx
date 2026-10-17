import { act, render, screen } from '@testing-library/react';
import type { JSX, ReactNode } from 'react';
import { describe, expect, it } from 'vitest';
import { CHANNEL_BY_ID, Quality, ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { FakeClock, FakeFrames } from '../../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../../test/tlvEncoder';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { TelemetryStore, type ChannelSample } from '../../telemetry/store';
import { SampleReadout, ValueReadout } from './ValueReadout';

function setup(): {
  store: TelemetryStore;
  clock: FakeClock;
  frames: FakeFrames;
  send: (records: readonly FixtureRecord[]) => void;
  wrapper: (props: { children: ReactNode }) => JSX.Element;
} {
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
  let seq = 0;
  const send = (records: readonly FixtureRecord[]): void => {
    store.applyBatch(encodeUiBatch(0, seq, [encodeTlvBlock({ sourceEtS: seq }, records)]));
    seq += 1;
  };
  const wrapper = ({ children }: { children: ReactNode }): JSX.Element => (
    <TelemetryStoreContext value={store}>{children}</TelemetryStoreContext>
  );
  return { store, clock, frames, send, wrapper };
}

function readout(): HTMLElement {
  return screen.getByRole('group');
}

describe('ValueReadout', () => {
  it('shows NO DATA with an em dash, never 0, before anything arrived', () => {
    const { wrapper } = setup();
    render(<ValueReadout channelId={ch.SPEED} />, { wrapper });
    expect(readout()).toHaveAccessibleName('Speed: no data');
    expect(readout()).toHaveTextContent(NO_VALUE);
    expect(readout()).toHaveTextContent('NO DATA');
    expect(readout().textContent).not.toMatch(/\b0\b/);
    expect(readout()).toHaveAttribute('data-state', 'noData');
  });

  it('formats a live value with its display unit and an accessible unit name', () => {
    const { frames, send, wrapper } = setup();
    render(<ValueReadout channelId={ch.SPEED} />, { wrapper });
    send([rec.f32(ch.SPEED, 83.5)]);
    act(() => {
      frames.runFrame();
    });
    expect(readout()).toHaveTextContent('301');
    expect(readout()).toHaveTextContent('km/h');
    expect(readout()).toHaveAccessibleName('Speed: 301 kilometres per hour');
    expect(readout()).toHaveAttribute('data-state', 'live');
  });

  it.each([
    [Quality.Unavailable, 'UNAVAILABLE', 'unavailable'],
    [Quality.NotSupported, 'NOT SUPPORTED', 'not supported'],
    [Quality.Invalid, 'INVALID', 'invalid'],
    [Quality.Stale, 'STALE', 'stale'],
  ] as const)('renders quality %s as an em dash with an explicit state', (quality, label, spoken) => {
    const { frames, send, wrapper } = setup();
    render(<ValueReadout channelId={ch.TYRE_PRESSURE_FL} />, { wrapper });
    send([rec.quality(ch.TYRE_PRESSURE_FL, quality)]);
    act(() => {
      frames.runFrame();
    });
    expect(readout()).toHaveTextContent(NO_VALUE);
    expect(readout()).toHaveTextContent(label);
    expect(readout()).toHaveAccessibleName(`Tyre pressure FL: ${spoken}`);
    expect(readout().textContent).not.toContain('0.00');
  });

  it('shows DEGRADED with the value, then STALE without it, as the stream ages', () => {
    const { clock, frames, send, wrapper } = setup();
    render(<ValueReadout channelId={ch.TYRE_PRESSURE_FL} />, { wrapper });
    send([rec.f32(ch.TYRE_PRESSURE_FL, 182_300)]);
    act(() => {
      clock.advance(300);
      frames.runFrame();
    });
    expect(readout()).toHaveTextContent('1.82');
    expect(readout()).toHaveTextContent('DEGRADED');
    expect(readout()).toHaveAccessibleName('Tyre pressure FL: 1.82 bar, degraded');
    act(() => {
      clock.advance(1_000);
      frames.runFrame();
    });
    expect(readout()).not.toHaveTextContent('1.82');
    expect(readout()).toHaveTextContent('STALE');
  });

  it('accepts a label, a precision override and hides the unit on request', () => {
    const { frames, send, wrapper } = setup();
    render(<ValueReadout channelId={ch.TYRE_PRESSURE_FL} label="FL" decimals={3} showUnit={false} />, { wrapper });
    send([rec.f32(ch.TYRE_PRESSURE_FL, 182_345)]);
    act(() => {
      frames.runFrame();
    });
    expect(readout()).toHaveTextContent('1.823');
    expect(readout()).not.toHaveTextContent('bar');
    expect(readout()).toHaveAccessibleName('FL: 1.823 bar');
  });
});

describe('SampleReadout', () => {
  const base: ChannelSample = {
    id: ch.LAP_TIME,
    def: CHANNEL_BY_ID.get(ch.LAP_TIME) ?? null,
    state: 'live',
    quality: Quality.Valid,
    value: 83.456,
    receivedAtMs: 0,
    captureMonoNs: 0n,
    captureRelayNsEst: null,
    sourceEtS: 1,
  };

  it('uses the channel display kind (lap time as m:ss.mmm, spoken in minutes and seconds)', () => {
    render(<SampleReadout sample={base} />);
    expect(readout()).toHaveTextContent('1:23.456');
    expect(readout()).toHaveAccessibleName('Current lap time: 1 minute 23.456 seconds');
  });

  it('shows a running lap timer at 0 as 0:00.000, not NO DATA (review F6)', () => {
    render(<SampleReadout sample={{ ...base, value: 0 }} />);
    expect(readout()).toHaveTextContent('0:00.000');
    expect(readout()).not.toHaveTextContent('NO DATA');
    expect(readout()).toHaveAttribute('data-state', 'live');
  });

  it('labels a received value it cannot display INVALID, never NO DATA (review F6)', () => {
    // A negative lap time arrives VALID (the store only checks types): it is shown as an invalid value.
    render(<SampleReadout sample={{ ...base, value: -1 }} />);
    expect(readout()).toHaveTextContent(NO_VALUE);
    expect(readout()).toHaveTextContent('INVALID');
    expect(readout()).not.toHaveTextContent('NO DATA');
    expect(readout()).toHaveAttribute('data-state', 'invalid');
    expect(readout()).toHaveAccessibleName('Current lap time: invalid');
  });

  it('names channels unknown to this build', () => {
    render(
      <SampleReadout
        sample={{ ...base, id: 0x0eff, def: null, state: 'notSupported', quality: Quality.NotSupported, value: null }}
      />,
    );
    expect(readout()).toHaveAccessibleName('Channel 0x0EFF: not supported');
    expect(readout()).toHaveTextContent(NO_VALUE);
  });

  it('renders booleans as YES / NO and text as received', () => {
    const tc = CHANNEL_BY_ID.get(ch.TC_ACTIVE) ?? null;
    const name = CHANNEL_BY_ID.get(ch.VEHICLE_NAME) ?? null;
    const { rerender } = render(<SampleReadout sample={{ ...base, id: ch.TC_ACTIVE, def: tc, value: true }} />);
    expect(readout()).toHaveTextContent('YES');
    rerender(<SampleReadout sample={{ ...base, id: ch.VEHICLE_NAME, def: name, value: 'Porsche 963' }} />);
    expect(readout()).toHaveAccessibleName('Vehicle name: Porsche 963');
  });
});
