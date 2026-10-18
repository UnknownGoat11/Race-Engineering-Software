import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { AlertDto } from '../../lib/ipc';
import { AlertsBanner } from './AlertsBanner';
import { ScopeDqContext } from './presentation';

const mockCriticalAlert: AlertDto = {
  id: 'alert-1',
  name: 'Tyre Overheating',
  level: 'critical',
  message: 'FL carcass 125 °C',
  raised: true,
  timestamp_ns: 1000000000,
};

const mockWarnAlert: AlertDto = {
  id: 'alert-2',
  name: 'Low Fuel',
  level: 'warning',
  message: 'Remaining 1.2 laps',
  raised: true,
  timestamp_ns: 2000000000,
};

const mockInfoAlert: AlertDto = {
  id: 'alert-3',
  name: 'Personal Best',
  level: 'info',
  message: 'Lap 12 1:44.215',
  raised: true,
  timestamp_ns: 3000000000,
};

describe('AlertsBanner', () => {
  it('renders null when alerts list is empty or scopeDq is ended', () => {
    const { container: emptyContainer } = render(
      <AlertsBanner alerts={[]} onDismiss={vi.fn()} />,
    );
    expect(emptyContainer.firstChild).toBeNull();

    const { container: endedContainer } = render(
      <ScopeDqContext.Provider value="ended">
        <AlertsBanner alerts={[mockCriticalAlert]} onDismiss={vi.fn()} />
      </ScopeDqContext.Provider>,
    );
    expect(endedContainer.firstChild).toBeNull();
  });

  it('renders critical alert with role="alert" and warning with role="status"', () => {
    render(
      <AlertsBanner
        alerts={[mockCriticalAlert, mockWarnAlert]}
        onDismiss={vi.fn()}
      />,
    );

    const criticalRow = screen.getByTestId('alert-alert-1');
    expect(criticalRow).toHaveAttribute('role', 'alert');
    expect(criticalRow).toHaveTextContent('Tyre Overheating: FL carcass 125 °C');

    const warnRow = screen.getByTestId('alert-alert-2');
    expect(warnRow).toHaveAttribute('role', 'status');
    expect(warnRow).toHaveTextContent('Low Fuel: Remaining 1.2 laps');
  });

  it('wraps decorative glyph in aria-hidden', () => {
    render(
      <AlertsBanner
        alerts={[mockCriticalAlert]}
        onDismiss={vi.fn()}
      />,
    );

    const glyph = screen.getByText('■');
    expect(glyph).toHaveAttribute('aria-hidden', 'true');
  });

  it('calls onDismiss with alert id when dismiss button is clicked', () => {
    const onDismiss = vi.fn();
    render(
      <AlertsBanner
        alerts={[mockCriticalAlert]}
        onDismiss={onDismiss}
      />,
    );

    const dismissBtn = screen.getByRole('button', { name: 'Dismiss alert Tyre Overheating' });
    expect(dismissBtn).toBeInTheDocument();

    fireEvent.click(dismissBtn);
    expect(onDismiss).toHaveBeenCalledWith('alert-1');
  });

  it('limits to 2 visible alerts with overflow toggle', () => {
    render(
      <AlertsBanner
        alerts={[mockCriticalAlert, mockWarnAlert, mockInfoAlert]}
        onDismiss={vi.fn()}
      />,
    );

    expect(screen.getByTestId('alert-alert-1')).toBeInTheDocument();
    expect(screen.getByTestId('alert-alert-2')).toBeInTheDocument();
    expect(screen.queryByTestId('alert-alert-3')).not.toBeInTheDocument();

    const overflowBtn = screen.getByTestId('alert-overflow-btn');
    expect(overflowBtn).toHaveTextContent('+1 more');

    fireEvent.click(overflowBtn);
    expect(screen.getByTestId('alert-alert-3')).toBeInTheDocument();
    expect(overflowBtn).toHaveTextContent('Show less');
  });

  it('renders Clear all button when provided and multiple alerts exist', () => {
    const onClearAll = vi.fn();
    render(
      <AlertsBanner
        alerts={[mockCriticalAlert, mockWarnAlert]}
        onDismiss={vi.fn()}
        onClearAll={onClearAll}
      />,
    );

    const clearAllBtn = screen.getByRole('button', { name: 'Clear all' });
    fireEvent.click(clearAllBtn);
    expect(onClearAll).toHaveBeenCalledTimes(1);
  });
});
