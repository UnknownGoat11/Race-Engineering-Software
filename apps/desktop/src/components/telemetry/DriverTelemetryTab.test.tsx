import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ch } from '../../generated/channels';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { TelemetryStore } from '../../telemetry/store';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../../test/tlvEncoder';
import { flagStripView, limiterView } from '../driver/driverBoardModel';
import { DriverTelemetryTab } from './DriverTelemetryTab';

function storeWith(records: readonly FixtureRecord[]): TelemetryStore {
  const store = new TelemetryStore();
  store.applyBatch(encodeUiBatch(0, 0, [encodeTlvBlock({ sourceEtS: 100 }, records)]));
  return store;
}

function renderBoard(records: readonly FixtureRecord[]): void {
  render(
    <TelemetryStoreContext value={storeWith(records)}>
      <DriverTelemetryTab />
    </TelemetryStoreContext>,
  );
}

describe('flagStripView (U04)', () => {
  it('shows green for flags.summary 0', () => {
    expect(flagStripView(0, null, null)).toEqual({ kind: 'clear', glyph: '●', text: 'Green flag' });
  });

  it('shows a neutral "Flag —" strip when the summary is unknown, never an invented green', () => {
    const view = flagStripView(null, null, null);
    expect(view.kind).toBe('unknown');
    expect(view.text).toBe('Flag —');
    // Known-zero yellow and blue channels do not prove green either (a red flag is not in them).
    expect(flagStripView(null, 0, 0).kind).toBe('unknown');
  });

  it('still shows a yellow or blue flag from the fallback channels when the summary is unknown', () => {
    expect(flagStripView(null, 3, 0).kind).toBe('warn');
    expect(flagStripView(null, 0, 1).kind).toBe('blue');
  });

  it('uses shape glyphs, never emoji', () => {
    for (const value of [0, 1, 2, 3, 4, 5, null]) {
      const { glyph, text } = flagStripView(value, null, null);
      expect(`${glyph}${text}`).not.toMatch(/\p{Extended_Pictographic}/u);
    }
  });
});

describe('limiterView (U05)', () => {
  it('reads "—" / "No data" when neither limiter channel has a value', () => {
    expect(limiterView(null, null)).toEqual({ state: 'unknown', word: '—', caption: 'No data' });
  });

  it('prefers SPEED_LIMITER_ACTIVE (0x010D) over the legacy 0x010C', () => {
    expect(limiterView(0, 1).state).toBe('off');
    expect(limiterView(1, 0).state).toBe('on');
    expect(limiterView(null, 1).state).toBe('on');
  });
});

describe('DriverTelemetryTab', () => {
  it('renders the neutral flag strip and the limiter no-data state with an empty store', async () => {
    renderBoard([rec.f64(ch.LAP_TIME, 94.215)]);

    await waitFor(() => {
      expect(screen.getByTestId('driver-flag-strip')).toHaveAttribute('data-flag', 'unknown');
    });
    expect(screen.getByTestId('driver-flag-strip')).toHaveTextContent('Flag —');
    expect(screen.getByTestId('driver-limiter-state')).toHaveTextContent('—');
    expect(screen.getByTestId('driver-limiter-caption')).toHaveTextContent('No data');
  });

  it('renders the green strip for flags.summary 0 and reads the boolean limiter channel', async () => {
    renderBoard([rec.u8(ch.FLAG_SUMMARY, 0), rec.bool(ch.SPEED_LIMITER_ACTIVE, true), rec.bool(ch.SPEED_LIMITER, false)]);

    await waitFor(() => {
      expect(screen.getByTestId('driver-flag-strip')).toHaveAttribute('data-flag', 'clear');
    });
    expect(screen.getByTestId('driver-limiter-state')).toHaveTextContent('ON');
    expect(screen.getByTestId('driver-limiter-caption')).toHaveTextContent('Limiter on');
  });

  it('puts the qualifier and the last lap in the delta block (D03, D04)', async () => {
    renderBoard([rec.f64(ch.DELTA_PERSONAL_BEST, -0.342), rec.f64(ch.LAST_LAP_TIME, 95.5)]);

    await waitFor(() => {
      expect(screen.getByTestId('driver-delta-value')).toHaveTextContent('-0.342s');
    });
    const hero = screen.getByTestId('driver-delta-hero');
    expect(hero).toContainElement(screen.getByTestId('driver-delta-qualifier'));
    expect(screen.getByTestId('driver-delta-qualifier')).toHaveTextContent('Faster');
    expect(hero).toContainElement(screen.getByTestId('driver-last-lap'));
    expect(screen.getByTestId('driver-limiter-tile')).not.toHaveTextContent(/last lap/i);
  });

  it('renders the status row it is given at the top of the board (D01)', async () => {
    render(
      <TelemetryStoreContext value={storeWith([])}>
        <DriverTelemetryTab statusRow={<div data-testid="status-row-probe" />} />
      </TelemetryStoreContext>,
    );
    await waitFor(() => {
      expect(screen.getByTestId('status-row-probe')).toBeInTheDocument();
    });
  });
});
