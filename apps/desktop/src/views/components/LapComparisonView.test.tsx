import { describe, expect, it, beforeEach } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { LapComparisonView } from './LapComparisonView';
import { motecStore } from '../../motec/useMotecStore';
import { createMotecLdBuffer, generateMultiLapMotecBuffer } from '../../motec/motecWriter';
import { NO_VALUE } from '../../lib/units';

describe('LapComparisonView', () => {
  beforeEach(() => {
    motecStore.clear();
  });

  it('shows no data (no invented reference, comparison, delta or corners) without a file', () => {
    render(<LapComparisonView />);
    expect(screen.getByText('REFERENCE LAP')).toBeInTheDocument();
    expect(screen.getByText('COMPARISON LAP')).toBeInTheDocument();
    expect(screen.getByText('TOTAL TIME DELTA (Δt)')).toBeInTheDocument();
    expect(screen.getByTestId('comp-ref-time')).toHaveTextContent(NO_VALUE);
    expect(screen.getByTestId('comp-cmp-time')).toHaveTextContent(NO_VALUE);
    expect(screen.getByTestId('comp-total-delta')).toHaveTextContent(NO_VALUE);
    expect(screen.getByTestId('comp-no-data')).toBeInTheDocument();
    expect(screen.queryByText(/La Source|Eau Rouge|Live Player/)).toBeNull();
  });

  it('uses the quickest lap as reference and compares a second real lap from the file', () => {
    act(() => {
      motecStore.loadBuffer(generateMultiLapMotecBuffer(), 'session.ld');
    });
    render(<LapComparisonView />);
    expect(screen.getByTestId('comp-ref-time')).toHaveTextContent('2:14.500');
    expect(screen.getByTestId('comp-total-delta')).toHaveTextContent(NO_VALUE);

    fireEvent.change(screen.getByTestId('comp-lap-select'), { target: { value: '1600' } }); // lap 2 starts at 160 s
    expect(screen.getByTestId('comp-cmp-time')).toHaveTextContent('2:20.000');
    expect(screen.getByTestId('comp-total-delta')).toHaveTextContent('+5.500');
    expect(screen.getByTestId('comp-lap-table')).toHaveTextContent(/Not used: starts at the beginning of the file/);
  });

  it('shows no data for graphs when the file has no distance', () => {
    act(() => {
      motecStore.loadBuffer(createMotecLdBuffer([{ name: 'Engine RPM', unit: 'rpm', samples: [1, 2, 3] }]), 'rpm.ld');
    });
    render(<LapComparisonView />);
    expect(screen.getByTestId('comp-no-data')).toHaveTextContent(/no lap distance or speed channel/);
  });
});
