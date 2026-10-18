import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { EventsLog } from './EventsLog';

describe('EventsLog', () => {
  it('renders empty message when there are no events', () => {
    render(<EventsLog />);
    expect(screen.getByTestId('events-log')).toBeInTheDocument();
    expect(screen.getByText(/No events recorded yet/)).toBeInTheDocument();
  });

  it('renders combined events and alerts', () => {
    render(
      <EventsLog
        alerts={[
          {
            id: 'a1',
            name: 'Brake Fade',
            level: 'warning',
            raised: true,
            message: 'FL rotor above 650 °C',
            timestamp_ns: 1_000_000_000,
          },
        ]}
        events={[
          {
            id: 'e1',
            timeText: 'L12',
            kind: 'pit',
            message: 'Pit stop complete',
          },
        ]}
      />,
    );

    expect(screen.getByText('FL rotor above 650 °C', { exact: false })).toBeInTheDocument();
    expect(screen.getByText('Pit stop complete')).toBeInTheDocument();
    expect(screen.getByText('2 EVENTS')).toBeInTheDocument();
  });
});
