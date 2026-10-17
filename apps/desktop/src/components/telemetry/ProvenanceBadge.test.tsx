import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ALL_PROVENANCES, CHANNEL_BY_ID, ch } from '../../generated/channels';
import { ProvenanceBadge } from './ProvenanceBadge';

describe('ProvenanceBadge (spec §107)', () => {
  it.each([
    ['Measured', 'MEASURED', 'Directly from LMU'],
    ['Derived', 'DERIVED', 'Calculated from measured data'],
    ['Estimated', 'ESTIMATED', 'Calculated with assumptions or statistical inference'],
  ] as const)('%s', (provenance, label, description) => {
    const { container } = render(<ProvenanceBadge provenance={provenance} />);
    const badge = screen.getByText(label, { exact: false });
    expect(badge).toHaveAttribute('title', description);
    expect(container.textContent).toBe(`${label}: ${description}`);
  });

  it('covers every provenance of the generated metadata', () => {
    expect(ALL_PROVENANCES).toEqual(['Measured', 'Derived', 'Estimated']);
    const speed = CHANNEL_BY_ID.get(ch.SPEED);
    if (speed === undefined) throw new Error('SPEED missing');
    render(<ProvenanceBadge provenance={speed.provenance} />);
    expect(screen.getByText('DERIVED', { exact: false })).toBeInTheDocument();
  });
});
