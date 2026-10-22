import { act, render, screen } from '@testing-library/react';
import type { JSX, ReactNode } from 'react';
import { describe, expect, it } from 'vitest';
import { ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { TelemetryStore } from '../../telemetry/store';
import { FakeClock, FakeFrames } from '../../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../../test/tlvEncoder';
import { BrakePanel } from './BrakePanel';
import { DriverInputsPanel } from './DriverInputsPanel';
import { SuspensionPanel } from './SuspensionPanel';

function harness(): { send: (r: readonly FixtureRecord[]) => void; renderWithStore: (el: JSX.Element) => void } {
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
  let seq = 0;
  return {
    send: (records) => {
      store.applyBatch(encodeUiBatch(0, seq, [encodeTlvBlock({ sourceEtS: seq }, records)]));
      seq += 1;
      act(() => {
        frames.runFrame();
      });
    },
    renderWithStore: (el) => {
      const wrapper = ({ children }: { children: ReactNode }): JSX.Element => (
        <TelemetryStoreContext value={store}>{children}</TelemetryStoreContext>
      );
      render(el, { wrapper });
    },
  };
}

describe('SuspensionPanel damage (registry `missing` channels)', () => {
  it('shows NO_VALUE, never INTACT / AERO 100%, when damage is not reported', () => {
    const { send, renderWithStore } = harness();
    renderWithStore(<SuspensionPanel />);
    send([rec.f32(ch.RIDE_HEIGHT_FL, 0.05)]);
    for (const w of ['fl', 'fr', 'rl', 'rr']) {
      expect(screen.getByTestId(`susp-dmg-${w}`)).toHaveTextContent(`DMG ${NO_VALUE}`);
    }
    expect(screen.getByText(`AERO DAMAGE: ${NO_VALUE}`)).toBeInTheDocument();
    expect(screen.queryByText('INTACT')).toBeNull();
    expect(screen.queryByText('AERO: 100%')).toBeNull();
    expect(screen.getByText('50.0 mm')).toBeInTheDocument();
  });
});

describe('BrakePanel wear (registry `missing` channels)', () => {
  it('shows Wear: NO_VALUE, never 0.0%, when brake wear is not reported', () => {
    const { renderWithStore } = harness();
    renderWithStore(<BrakePanel />);
    for (const w of ['fl', 'fr', 'rl', 'rr']) {
      expect(screen.getByTestId(`brake-wear-${w}`)).toHaveTextContent(`wear ${NO_VALUE}`);
    }
    expect(screen.queryByText(/0\.0%/)).toBeNull();
  });
});

describe('DriverInputsPanel', () => {
  it('has no heuristic suggested-gear readout', () => {
    const { send, renderWithStore } = harness();
    renderWithStore(<DriverInputsPanel />);
    send([rec.i32(ch.GEAR, 4), rec.f32(ch.RPM, 8400), rec.f32(ch.RPM_MAX, 8500)]);
    expect(screen.getByTestId('inputs-gear')).toHaveTextContent('4');
    expect(screen.queryByText('SUGG')).toBeNull();
    expect(screen.queryByText(/▲|▼/)).toBeNull();
    expect(screen.queryByTitle(/suggested gear/i)).toBeNull();
  });
});
