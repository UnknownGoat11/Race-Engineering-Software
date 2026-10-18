import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { StatusBadge } from './StatusBadge';

describe('StatusBadge (§2.6)', () => {
  it('renders status levels with appropriate CSS classes and glyphs', () => {
    const { rerender } = render(<StatusBadge level="ok" label="LIVE" />);
    expect(screen.getByText('LIVE')).toBeInTheDocument();
    expect(screen.getByText('●')).toBeInTheDocument();

    rerender(<StatusBadge level="warn" label="DEGRADED" />);
    expect(screen.getByText('▲')).toBeInTheDocument();

    rerender(<StatusBadge level="critical" label="CRITICAL" />);
    expect(screen.getByText('■')).toBeInTheDocument();

    rerender(<StatusBadge level="info" label="INFO" />);
    expect(screen.getByText('ℹ')).toBeInTheDocument();

    rerender(<StatusBadge level="off" label="OFF" />);
    expect(screen.getByText('○')).toBeInTheDocument();
  });

  it('renders DQ levels with correct classes', () => {
    const { container } = render(<StatusBadge dq="stale" label="STALE 4.2s" />);
    expect(container.firstChild).toHaveClass('status-badge-stale');
    expect(screen.getByText('STALE 4.2s')).toBeInTheDocument();
  });

  it('renders car class badges', () => {
    const { container, rerender } = render(<StatusBadge carClass="hypercar" label="HY" />);
    expect(container.firstChild).toHaveClass('status-badge-hypercar');

    rerender(<StatusBadge carClass="lmp2" label="P2" />);
    expect(container.firstChild).toHaveClass('status-badge-lmp2');

    rerender(<StatusBadge carClass="gt3" label="GT3" />);
    expect(container.firstChild).toHaveClass('status-badge-gt3');
  });

  it('allows disabling or overriding glyph', () => {
    render(<StatusBadge level="ok" label="No Glyph" glyph={false} />);
    expect(screen.queryByText('●')).toBeNull();

    render(<StatusBadge level="ok" label="Custom Glyph" glyph="★" />);
    expect(screen.getByText('★')).toBeInTheDocument();
  });
});
