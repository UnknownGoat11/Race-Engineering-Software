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

  it('triggers onClick on click and on Enter key press', () => {
    const onClick = vi.fn();
    render(
      <GlanceTile
        id="lmu"
        label="LMU"
        level="ok"
        word="LIVE"
        onClick={onClick}
      />,
    );

    const tile = screen.getByRole('button');
    fireEvent.click(tile);
    expect(onClick).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(tile, { key: 'Enter' });
    expect(onClick).toHaveBeenCalledTimes(2);

    fireEvent.keyDown(tile, { key: ' ' });
    expect(onClick).toHaveBeenCalledTimes(3);
  });

  it('does not trigger tile onClick when interacting with child elements', () => {
    const onTileClick = vi.fn();
    const onChildClick = vi.fn();
    render(
      <GlanceTile
        id="code"
        label="ENGINEER CODE"
        level="ok"
        word="8F3A2C"
        onClick={onTileClick}
      >
        <button type="button" onClick={onChildClick} data-testid="inner-btn">
          Copy
        </button>
      </GlanceTile>,
    );

    const btn = screen.getByTestId('inner-btn');
    fireEvent.click(btn);

    expect(onChildClick).toHaveBeenCalledTimes(1);
    expect(onTileClick).not.toHaveBeenCalled();
  });
});
