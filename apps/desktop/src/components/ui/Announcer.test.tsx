import { act, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { useAnnouncer } from '../../hooks/useAnnouncer';
import { Announcer, AnnouncerProvider } from './Announcer';

function TestConsumer({ message, priority }: { message: string; priority?: 'polite' | 'assertive' }) {
  const { announce } = useAnnouncer();
  return (
    <button
      type="button"
      onClick={() => {
        announce(message, priority);
      }}
    >
      Trigger
    </button>
  );
}

describe('Announcer component', () => {
  it('renders polite and assertive live regions with sr-only styling', () => {
    render(<Announcer />);

    const root = screen.getByTestId('screen-announcer');
    expect(root).toHaveClass('sr-only');

    const polite = screen.getByTestId('screen-announcer-polite');
    expect(polite).not.toHaveAttribute('role');
    expect(polite).toHaveAttribute('aria-live', 'polite');
    expect(polite).toHaveAttribute('aria-atomic', 'true');
    expect(polite.textContent).toBe('');

    const assertive = screen.getByTestId('screen-announcer-assertive');
    expect(assertive).not.toHaveAttribute('role');
    expect(assertive).toHaveAttribute('aria-live', 'assertive');
    expect(assertive).toHaveAttribute('aria-atomic', 'true');
    expect(assertive.textContent).toBe('');
  });

  it('renders explicit message props directly and applies semantic roles', () => {
    render(
      <Announcer
        politeMessage="Copied to clipboard"
        assertiveMessage="Recording failed"
      />,
    );

    const polite = screen.getByTestId('screen-announcer-polite');
    expect(polite).toHaveAttribute('role', 'status');
    expect(polite.textContent).toBe('Copied to clipboard');

    const assertive = screen.getByTestId('screen-announcer-assertive');
    expect(assertive).toHaveAttribute('role', 'alert');
    expect(assertive.textContent).toBe('Recording failed');
  });

  it('connects to AnnouncerProvider context seamlessly', () => {
    render(
      <AnnouncerProvider>
        <TestConsumer message="Engineer joined" />
      </AnnouncerProvider>,
    );

    const polite = screen.getByTestId('screen-announcer-polite');
    expect(polite.textContent).toBe('');

    const button = screen.getByRole('button', { name: 'Trigger' });
    act(() => {
      button.click();
    });

    expect(polite.textContent).toBe('Engineer joined');
  });
});
