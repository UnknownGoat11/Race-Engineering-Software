import { fireEvent, render, screen } from '@testing-library/react';
import type { JSX } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AppErrorBoundary, PanelErrorBoundary } from './ErrorBoundary';

const failure = { enabled: true };

function Unstable(): JSX.Element {
  if (failure.enabled) {
    throw new Error('boom');
  }
  return <p>recovered content</p>;
}

describe('PanelErrorBoundary', () => {
  beforeEach(() => {
    failure.enabled = true;
    // React reports caught errors via console.error; keep test output clean.
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
  });

  it('isolates a failing panel so sibling panels keep rendering', () => {
    render(
      <>
        <PanelErrorBoundary name="Fuel">
          <Unstable />
        </PanelErrorBoundary>
        <PanelErrorBoundary name="Timing">
          <p>timing content</p>
        </PanelErrorBoundary>
      </>,
    );

    expect(screen.getByRole('alert', { name: 'Fuel panel' })).toHaveTextContent('Panel unavailable');
    expect(screen.getByText('timing content')).toBeInTheDocument();
  });

  it('remounts the panel on Retry', () => {
    const onError = vi.fn();
    render(
      <PanelErrorBoundary name="Fuel" onError={onError}>
        <Unstable />
      </PanelErrorBoundary>,
    );
    expect(onError).toHaveBeenCalledTimes(1);

    failure.enabled = false;
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(screen.getByText('recovered content')).toBeInTheDocument();
    expect(screen.queryByText('Panel unavailable')).not.toBeInTheDocument();
  });

  it('shows the fallback again if the retry also fails', () => {
    render(
      <PanelErrorBoundary name="Fuel">
        <Unstable />
      </PanelErrorBoundary>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(screen.getByText('Panel unavailable')).toBeInTheDocument();
  });
});

describe('AppErrorBoundary', () => {
  it('renders a full-screen error instead of a blank window', () => {
    failure.enabled = true;
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    render(
      <AppErrorBoundary>
        <Unstable />
      </AppErrorBoundary>,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('User interface error');
    expect(screen.getByRole('button', { name: 'Reload interface' })).toBeInTheDocument();
  });
});
