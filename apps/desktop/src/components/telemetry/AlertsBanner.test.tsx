import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { AlertDto } from '../../lib/ipc';
import { formatAgoText as alertAgeText } from '../../lib/units';
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
    const { unmount } = render(<AlertsBanner alerts={[mockCriticalAlert]} onDismiss={vi.fn()} />);

    const criticalRow = screen.getByTestId('alert-alert-1');
    expect(criticalRow).toHaveAttribute('role', 'alert');
    expect(criticalRow).toHaveTextContent('Tyre Overheating: FL carcass 125 °C');
    unmount();
    render(<AlertsBanner alerts={[mockWarnAlert]} onDismiss={vi.fn()} />);

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

  it('shows one row by default with a +N toggle', () => {
    const second = { ...mockWarnAlert, id: 'alert-4', name: 'Tyre pressure' };
    render(
      <AlertsBanner
        alerts={[mockCriticalAlert, mockWarnAlert, second]}
        onDismiss={vi.fn()}
      />,
    );

    expect(screen.getByTestId('alert-alert-1')).toBeInTheDocument();
    expect(screen.queryByTestId('alert-alert-2')).not.toBeInTheDocument();

    const overflowBtn = screen.getByTestId('alert-overflow-btn');
    expect(overflowBtn).toHaveTextContent('+2 more');
    // The toggle sits inside the strip row.
    expect(screen.getByTestId('alert-alert-1')).toContainElement(overflowBtn);

    fireEvent.click(overflowBtn);
    expect(screen.getByTestId('alert-alert-2')).toBeInTheDocument();
    expect(screen.getByTestId('alert-alert-4')).toBeInTheDocument();
    expect(overflowBtn).toHaveTextContent('Show less');
  });

  it('info alerts never enter the strip (they go to the session log)', () => {
    const { container } = render(<AlertsBanner alerts={[mockInfoAlert]} onDismiss={vi.fn()} />);
    expect(container.firstChild).toBeNull();
    render(<AlertsBanner alerts={[mockInfoAlert, mockWarnAlert]} onDismiss={vi.fn()} />);
    expect(screen.queryByTestId('alert-alert-3')).not.toBeInTheDocument();
    expect(screen.queryByTestId('alert-overflow-btn')).not.toBeInTheDocument();
  });

  it('shows the alert age from its receipt time', () => {
    render(
      <AlertsBanner alerts={[{ ...mockWarnAlert, receivedAtMs: 1_000 }]} onDismiss={vi.fn()} nowMs={1_000 + 4 * 60_000} />,
    );
    expect(screen.getByTestId('alert-alert-2')).toHaveTextContent('4 min ago');
    expect(alertAgeText(0, 30_000)).toBe('under 1 min ago');
    expect(alertAgeText(0, 2 * 3_600_000)).toBe('2 h ago');
    expect(alertAgeText(undefined, 5)).toBeNull();
  });

  it('renders Clear all inside the strip when several alerts are active', () => {
    const onClearAll = vi.fn();
    render(
      <AlertsBanner
        alerts={[mockCriticalAlert, mockWarnAlert, mockInfoAlert]}
        onDismiss={vi.fn()}
        onClearAll={onClearAll}
      />,
    );

    const clearAllBtn = screen.getByRole('button', { name: 'Clear all' });
    expect(screen.getByTestId('alert-alert-1')).toContainElement(clearAllBtn);
    fireEvent.click(clearAllBtn);
    expect(onClearAll).toHaveBeenCalledTimes(1);
    // Only the alerts the strip shows are cleared; the info alert (log only) is untouched.
    expect(onClearAll).toHaveBeenCalledWith(['alert-1', 'alert-2']);
  });
});
