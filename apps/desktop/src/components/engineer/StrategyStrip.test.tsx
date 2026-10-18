import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { TelemetryStore } from '../../telemetry/store';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { StrategyStrip } from './StrategyStrip';

describe('StrategyStrip', () => {
  const store = new TelemetryStore();

  it('renders strategy strip with provided session items', () => {
    render(
      <TelemetryStoreContext.Provider value={store}>
        <StrategyStrip
          sessionClockS={3600}
          completedLaps={42}
          trackTempC={32}
          airTempC={24}
          weather="Dry"
        />
      </TelemetryStoreContext.Provider>,
    );

    expect(screen.getByTestId('strategy-strip')).toBeInTheDocument();
    expect(screen.getByText('Session')).toBeInTheDocument();
    expect(screen.getByText('1:00:00')).toBeInTheDocument();
    expect(screen.getByText('Laps')).toBeInTheDocument();
    expect(screen.getByText('42')).toBeInTheDocument();
    expect(screen.getByText('Dry')).toBeInTheDocument();
  });

  it('hides sourceless items when props are null or undefined', () => {
    render(
      <TelemetryStoreContext.Provider value={store}>
        <StrategyStrip />
      </TelemetryStoreContext.Provider>,
    );

    expect(screen.getByTestId('strategy-strip')).toBeInTheDocument();
    expect(screen.queryByText('Session')).not.toBeInTheDocument();
    expect(screen.queryByText('Laps')).not.toBeInTheDocument();
    expect(screen.queryByText('Stint')).not.toBeInTheDocument();
  });
});
