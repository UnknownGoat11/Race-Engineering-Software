import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { EngineerView } from '../../lib/statusModel';
import { StateBanner } from './StateBanner';

describe('StateBanner', () => {
  it('returns null when view is live and no caution flag is active', () => {
    const liveView: EngineerView = { kind: 'live', ageS: 0.1, degraded: false };
    const { container } = render(<StateBanner view={liveView} />);
    expect(container.firstChild).toBeNull();
  });

  it('renders incompatible banner when schema is incompatible (T05)', () => {
    const incompatibleView: EngineerView = { kind: 'incompatible', versions: null };
    render(<StateBanner view={incompatibleView} />);
    expect(screen.getByTestId('state-banner-incompatible')).toBeInTheDocument();
    expect(screen.getByText('UPDATE REQUIRED')).toBeInTheDocument();
  });

  it('renders ended banner with rejoin button when session ends (T06)', () => {
    const endedView: EngineerView = { kind: 'ended', reason: 'driver_quit' };
    const onRejoin = vi.fn();
    render(<StateBanner view={endedView} onRejoin={onRejoin} />);

    expect(screen.getByTestId('state-banner-ended')).toBeInTheDocument();
    expect(screen.getByText('ENDED')).toBeInTheDocument();
    expect(screen.getByText(/Session ended/)).toBeInTheDocument();

    const btn = screen.getByTestId('btn-rejoin');
    fireEvent.click(btn);
    expect(onRejoin).toHaveBeenCalledOnce();
  });

  it('renders away banner when driver is away (T08)', () => {
    const awayView: EngineerView = { kind: 'away', sinceMs: 120_000, ageS: 120, synthetic: false };
    render(<StateBanner view={awayView} />);
    expect(screen.getByTestId('state-banner-away')).toBeInTheDocument();
    expect(screen.getByText(/Driver is in garage \/ away/)).toBeInTheDocument();
  });

  it('renders caution banner when FCY is active', () => {
    const liveView: EngineerView = { kind: 'live', ageS: 0.1, degraded: false };
    render(<StateBanner view={liveView} caution="fcy" cautionDetail="Pending" />);
    expect(screen.getByTestId('state-banner-caution')).toBeInTheDocument();
    expect(screen.getByText('FCY / SC')).toBeInTheDocument();
    // Only what LMU reports: no invented "neutralized pace" claims.
    expect(screen.getByTestId('state-banner-caution')).toHaveTextContent('Full-course yellow or safety car · Pending');
  });

  it('no banner for a measured green or an unknown caution', () => {
    const liveView: EngineerView = { kind: 'live', ageS: 0.1, degraded: false };
    const { container, rerender } = render(<StateBanner view={liveView} caution="none" />);
    expect(container.firstChild).toBeNull();
    rerender(<StateBanner view={liveView} caution={null} />);
    expect(container.firstChild).toBeNull();
  });
});
