import { act, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { TelemetryStore } from '../../telemetry/store';
import { FakeClock, FakeFrames } from '../../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec } from '../../test/tlvEncoder';
import { MinisectorRibbon } from './MinisectorRibbon';

function setup(): (lap: number, progress: number, delta: number) => void {
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
  render(
    <TelemetryStoreContext value={store}>
      <MinisectorRibbon />
    </TelemetryStoreContext>,
  );
  let seq = 0;
  return (lap, progress, delta) => {
    store.applyBatch(
      encodeUiBatch(0, seq, [
        encodeTlvBlock({ sourceEtS: seq + 1 }, [
          rec.i32(ch.LAP_NUMBER, lap),
          rec.f32(ch.LAP_PROGRESS, progress),
          rec.f64(ch.DELTA_PERSONAL_BEST, delta),
        ]),
      ]),
    );
    seq += 1;
    act(() => {
      frames.runFrame();
    });
  };
}

describe('MinisectorRibbon', () => {
  it('shows no data in every cell before any minisector is measured (no hardcoded deltas, no fixed purple)', () => {
    setup();
    for (let i = 1; i <= 12; i++) {
      const cell = screen.getByTestId(`ms-cell-${String(i)}`);
      expect(cell).toHaveTextContent(NO_VALUE);
      expect(cell.className).not.toContain('ms-purple');
    }
    expect(screen.getByTestId('ms-active')).toHaveTextContent(NO_VALUE);
  });

  it('measures a minisector as the change of the live PB delta between its entry and exit', () => {
    const send = setup();
    send(3, 0.01, 0); // first sample: entry into M1 not observed
    send(3, 0.05, 0.05);
    send(3, 0.1, 0.1); // enters M2 at +0.100
    expect(screen.getByTestId('ms-cell-1')).toHaveTextContent(NO_VALUE);
    send(3, 0.17, 0.0); // leaves M2 at 0.000 -> M2 = -0.100
    const m2 = screen.getByTestId('ms-cell-2');
    expect(m2).toHaveTextContent('-0.100');
    expect(m2.className).toContain('ms-faster');
    expect(screen.getByTestId('ms-active')).toHaveTextContent('M3');
    send(3, 0.26, 0.2); // leaves M3 at +0.200 -> M3 = +0.200
    expect(screen.getByTestId('ms-cell-3')).toHaveTextContent('+0.200');
    expect(screen.getByTestId('ms-cell-3').className).toContain('ms-slower');
    send(4, 0.01, 0); // new lap: row resets
    expect(screen.getByTestId('ms-cell-2')).toHaveTextContent(NO_VALUE);
  });
});
