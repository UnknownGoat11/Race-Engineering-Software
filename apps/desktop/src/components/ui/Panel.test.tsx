import { render, screen } from '@testing-library/react';
import type { JSX, ReactNode } from 'react';
import { describe, expect, it } from 'vitest';
import { ScopeDqContext } from '../telemetry/presentation';
import { Panel } from './Panel';

function withScopeDq(element: JSX.Element, dq: 'live' | 'stale' | 'untrusted' | 'ended' | 'unavailable' | null) {
  const wrapper = ({ children }: { children: ReactNode }) => (
    <ScopeDqContext value={dq}>{children}</ScopeDqContext>
  );
  return render(element, { wrapper });
}

describe('Panel component (§2.6)', () => {
  it('renders title, children, and default LIVE glyph', () => {
    render(
      <Panel title="TEST PANEL">
        <div>Panel content</div>
      </Panel>,
    );

    expect(screen.getByText('TEST PANEL')).toBeInTheDocument();
    expect(screen.getByText('Panel content')).toBeInTheDocument();
    expect(screen.getByText('●')).toBeInTheDocument();
  });

  it('renders STALE state with age tag and sets data-dq attribute', () => {
    const { container } = render(
      <Panel title="TEST PANEL" dq="stale" ageS={6.2}>
        <div>Data values</div>
      </Panel>,
    );

    expect(container.firstChild).toHaveAttribute('data-dq', 'stale');
    expect(screen.getByText('STALE 6.2 s')).toBeInTheDocument();
    expect(screen.getByText('Data values')).toBeInTheDocument();
  });

  it('handles UNAVAILABLE state: hides children and displays unavailable reason', () => {
    const { container } = render(
      <Panel title="ENERGY" dq="unavailable" reason="Not reported by this car">
        <div>Should be hidden</div>
      </Panel>,
    );

    expect(container.firstChild).toHaveAttribute('data-dq', 'unavailable');
    expect(screen.getByText('Unavailable — Not reported by this car')).toBeInTheDocument();
    expect(screen.queryByText('Should be hidden')).toBeNull();
  });

  it('handles UNTRUSTED state: keeps children and displays warning notice', () => {
    const { container } = render(
      <Panel title="TIMING" dq="untrusted">
        <div>Some values</div>
      </Panel>,
    );

    expect(container.firstChild).toHaveAttribute('data-dq', 'untrusted');
    expect(screen.getByText('Untrusted — data format mismatch')).toBeInTheDocument();
    expect(screen.getByText('Some values')).toBeInTheDocument();
  });

  it('inherits ScopeDq from ScopeDqContext when not explicitly provided', () => {
    const { container } = withScopeDq(
      <Panel title="SCOPED PANEL">
        <div>Scoped content</div>
      </Panel>,
      'stale',
    );

    expect(container.firstChild).toHaveAttribute('data-dq', 'stale');
    expect(screen.getByText('STALE')).toBeInTheDocument();
  });

  it('renders meta, actions, and footer when provided', () => {
    render(
      <Panel
        title="RICH PANEL"
        meta={<span>META CHIP</span>}
        actions={<button type="button">Action</button>}
        footer={<span>Footer text</span>}
      >
        <div>Content</div>
      </Panel>,
    );

    expect(screen.getByText('META CHIP')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Action' })).toBeInTheDocument();
    expect(screen.getByText('Footer text')).toBeInTheDocument();
  });
});
