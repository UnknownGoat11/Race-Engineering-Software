import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { GlanceTile } from './GlanceTile';

describe('GlanceTile', () => {
  it('renders label, state word, detail, and level glyph', () => {
    render(
      <GlanceTile
        id="lmu"
        label="LMU"
        level="ok"
        word="LIVE"
        detail="100 Hz"
        reason="Connected to game telemetry"
      />,
    );

    expect(screen.getByText('LMU')).toBeInTheDocument();
    expect(screen.getByText('LIVE')).toBeInTheDocument();
    expect(screen.getByText('100 Hz')).toBeInTheDocument();
    expect(screen.getByText('Connected to game telemetry')).toBeInTheDocument();
    expect(screen.getByText('●')).toBeInTheDocument();
  });

  it('renders critical worst-state escalation styling', () => {
    const { container } = render(
      <GlanceTile
        id="recording"
        label="RECORDING"
        level="critical"
        word="FAILED"
        reason="Disk full"
      />,
    );

    const tile = container.querySelector('.glance-tile');
    expect(tile).toHaveClass('glance-tile-critical');
    expect(screen.getByText('■')).toBeInTheDocument();
  });

  it('renders warning worst-state escalation styling', () => {
    const { container } = render(
      <GlanceTile
        id="streaming"
        label="STREAMING"
        level="warning"
        word="DEGRADED"
        detail="RTT 250 ms"
      />,
    );

    const tile = container.querySelector('.glance-tile');
    expect(tile).toHaveClass('glance-tile-warning');
    expect(screen.getByText('◆')).toBeInTheDocument();
  });

  it('renders code format properly with slots or code', () => {
    const { rerender } = render(
      <GlanceTile
        id="code"
        label="ENGINEER CODE"
        level="ok"
        word="8F3A2C"
        isCode={true}
      />,
    );
    expect(screen.getByTestId('driver-connect-code')).toHaveTextContent('8F3A2C');

    rerender(
      <GlanceTile
        id="code"
        label="ENGINEER CODE"
        level="off"
        word="—"
        isCode={true}
      />,
    );
    expect(screen.getByTestId('driver-connect-code')).toHaveTextContent('——————');

    rerender(
      <GlanceTile
        id="code"
        label="ENGINEER CODE"
        level="off"
        word="SESSION FULL"
        isCode={true}
        isSessionFull={true}
      />,
    );
    expect(screen.getByTestId('driver-connect-code')).toHaveTextContent('SESSION FULL');
  });

  it('is a labelled group with a separate Details button, not a button itself (A02)', () => {
    const onClick = vi.fn();
    render(
      <GlanceTile id="code" label="ENGINEER CODE" level="ok" word="8F3A2C" reason="Expires in 4:59" onClick={onClick}>
        <button type="button" data-testid="inner-btn">
          Copy code
        </button>
      </GlanceTile>,
    );

    const tile = screen.getByRole('group', { name: 'ENGINEER CODE' });
    expect(tile).toHaveTextContent('Expires in 4:59');
    expect(tile).not.toHaveAttribute('tabindex');
    // Only real buttons are interactive: the inner button and the Details button.
    expect(screen.getAllByRole('button')).toHaveLength(2);

    fireEvent.click(screen.getByTestId('inner-btn'));
    expect(onClick).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'ENGINEER CODE details' }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('draws no glyph for an off tile (no box that reads as a missing font)', () => {
    const { container } = render(<GlanceTile id="recording" label="RECORDING" level="off" word="OFF" />);
    expect(container.querySelector('.glance-glyph')).toBeNull();
  });

  it('applies adaptive word size modifiers based on word length and word count to avoid collisions', () => {
    const { container, rerender } = render(
      <GlanceTile id="lmu" label="LMU" level="ok" word="LIVE" />,
    );
    expect(container.querySelector('.glance-word')).toHaveClass('glance-word--lg');

    rerender(
      <GlanceTile id="recording" label="RECORDING" level="warning" word="FAILED" />,
    );
    expect(container.querySelector('.glance-word')).toHaveClass('glance-word--md');

    rerender(
      <GlanceTile id="lmu" label="LMU" level="off" word="NOT RUNNING" />,
    );
    expect(container.querySelector('.glance-word')).toHaveClass('glance-word--sm');

    rerender(
      <GlanceTile id="streaming" label="STREAMING" level="off" word="NOT SET UP" />,
    );
    expect(container.querySelector('.glance-word')).toHaveClass('glance-word--sm');

    rerender(
      <GlanceTile id="recording" label="RECORDING" level="off" word="OFF IN SETTINGS" />,
    );
    expect(container.querySelector('.glance-word')).toHaveClass('glance-word--xs');
  });
});

