import { describe, expect, it } from 'vitest';
import { layoutMarkers, type MarkerLayoutOptions } from './markers';
import type { GraphMarker } from './types';

/** 1 x unit = 1 px; labels are 6 px per character. */
const X = { scale: 1, offset: 0 };
const OPTIONS: MarkerLayoutOptions = {
  plotLeft: 0,
  plotRight: 400,
  rows: 2,
  measure: (text) => text.length * 6,
  gap: 4,
};

function marker(x: number, label: string, kind: GraphMarker['kind'] = 'sector'): GraphMarker {
  return { x, label, kind };
}

describe('layoutMarkers', () => {
  it('places separated labels in the first row, to the right of their line', () => {
    const out = layoutMarkers([marker(10, 'S1'), marker(200, 'S2')], X, OPTIONS);
    expect(out.map((m) => [m.px, m.row, m.labelX, m.labelVisible])).toEqual([
      [10, 0, 14, true],
      [200, 0, 204, true],
    ]);
  });

  it('moves an overlapping label to the next row and hides one that fits nowhere', () => {
    const out = layoutMarkers([marker(10, 'SECTOR 1'), marker(20, 'SECTOR 2'), marker(30, 'SECTOR 3')], X, OPTIONS);
    expect(out.map((m) => [m.row, m.labelVisible])).toEqual([
      [0, true],
      [1, true],
      [0, false],
    ]);
  });

  it('places lap labels before sector and event labels', () => {
    const out = layoutMarkers(
      [marker(10, 'PIT IN', 'event'), marker(12, 'S1', 'sector'), marker(14, 'LAP 12', 'lap')],
      X,
      { ...OPTIONS, rows: 1 },
    );
    const byLabel = Object.fromEntries(out.map((m) => [m.marker.label, m.labelVisible]));
    expect(byLabel).toEqual({ 'PIT IN': false, S1: false, 'LAP 12': true });
  });

  it('flips a label to the left of its line at the right edge of the plot', () => {
    const [m] = layoutMarkers([marker(395, 'LAP 7', 'lap')], X, OPTIONS);
    expect(m?.labelX).toBe(395 - 4 - 30);
    expect(m?.labelVisible).toBe(true);
  });

  it('drops markers outside the plot and markers with non-finite x', () => {
    const out = layoutMarkers([marker(-5, 'A'), marker(401, 'B'), marker(Number.NaN, 'C'), marker(50, 'D')], X, OPTIONS);
    expect(out.map((m) => m.marker.label)).toEqual(['D']);
  });

  it('maps x through the axis transform', () => {
    const out = layoutMarkers([marker(1000, 'S2')], { scale: 0.1, offset: 50 }, OPTIONS);
    expect(out[0]?.px).toBe(150);
  });

  it('keeps the line of an unlabelled marker', () => {
    const out = layoutMarkers([marker(10, '')], X, OPTIONS);
    expect(out).toHaveLength(1);
    expect(out[0]?.labelVisible).toBe(false);
  });
});
