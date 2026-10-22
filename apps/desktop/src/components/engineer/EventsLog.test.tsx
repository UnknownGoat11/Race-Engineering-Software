import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { AlertLogEntry } from '../../hooks/useAlerts';
import { EventsLog } from './EventsLog';

const brake = {
  id: 'a1',
  name: 'Brake Fade',
  level: 'warning' as const,
  raised: true,
  message: 'FL rotor above 650 °C',
  timestamp_ns: 1_000_000_000,
};

describe('EventsLog', () => {
  it('renders empty message when there are no events', () => {
    render(<EventsLog />);
    expect(screen.getByTestId('events-log')).toBeInTheDocument();
    expect(screen.getByText(/No events recorded yet/)).toBeInTheDocument();
  });

  it('renders the alert log newest first plus race events', () => {
    const log: AlertLogEntry[] = [
      { seq: 1, kind: 'raised', atMs: new Date(2026, 9, 21, 14, 2, 5).getTime(), alert: brake },
      { seq: 2, kind: 'dismissed', atMs: new Date(2026, 9, 21, 14, 3, 0).getTime(), alert: brake },
    ];
    render(
      <EventsLog
        log={log}
        events={[{ id: 'e1', timeText: 'L12', kind: 'pit', message: 'Pit stop complete' }]}
      />,
    );

    const rows = screen.getAllByRole('listitem');
    expect(rows).toHaveLength(3);
    expect(rows[0]).toHaveTextContent('14:03:00');
    expect(rows[0]).toHaveTextContent('Dismissed');
    expect(rows[1]).toHaveTextContent('14:02:05');
    expect(rows[1]).toHaveTextContent('Raised');
    expect(rows[1]).toHaveTextContent('Brake Fade: FL rotor above 650 °C');
    expect(screen.getByText('Pit stop complete')).toBeInTheDocument();
    expect(screen.getByTestId('events-log')).toHaveTextContent('3 entries');
  });
});
