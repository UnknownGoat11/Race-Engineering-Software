import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { TelemetryStore } from '../../telemetry/store';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { RightColumnTabs } from './RightColumnTabs';

describe('RightColumnTabs', () => {
  const store = new TelemetryStore();

  it('renders pinned mini-row and initial timing and fuel panels', () => {
    render(
      <TelemetryStoreContext.Provider value={store}>
        <RightColumnTabs initialTab="timing_fuel" />
      </TelemetryStoreContext.Provider>,
    );

    expect(screen.getByTestId('pinned-mini-row')).toBeInTheDocument();
    expect(screen.getByTestId('tab-timing')).toBeInTheDocument();
    expect(screen.getByTestId('tab-tyres')).toBeInTheDocument();
    expect(screen.getByTestId('tab-power')).toBeInTheDocument();
    expect(screen.getByTestId('panel-timing')).toBeInTheDocument();
    expect(screen.getByTestId('panel-fuel')).toBeInTheDocument();
  });

  it('switches to tyres & brakes tab when clicked', () => {
    render(
      <TelemetryStoreContext.Provider value={store}>
        <RightColumnTabs initialTab="timing_fuel" />
      </TelemetryStoreContext.Provider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Tyres & Brakes' }));
    expect(screen.getByTestId('panel-tyres')).toBeInTheDocument();
    expect(screen.getByTestId('panel-brakes')).toBeInTheDocument();
  });

  it('switches to power (inputs & energy) tab when clicked', () => {
    render(
      <TelemetryStoreContext.Provider value={store}>
        <RightColumnTabs initialTab="timing_fuel" />
      </TelemetryStoreContext.Provider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Inputs & Energy' }));
    expect(screen.getByTestId('panel-energy')).toBeInTheDocument();
    expect(screen.getByTestId('panel-driver-inputs')).toBeInTheDocument();
  });
});
