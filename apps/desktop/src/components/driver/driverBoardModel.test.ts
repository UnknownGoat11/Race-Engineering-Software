import { describe, expect, it } from 'vitest';
import { flagStripView } from './driverBoardModel';

describe('flagStripView without a flag summary', () => {
  it('raw yellow state 7 (race halt) is a red flag, not a yellow', () => {
    expect(flagStripView(null, 7, null).kind).toBe('critical');
  });

  it('raw yellow states 1-6 are a yellow / FCY period', () => {
    for (const state of [1, 2, 3, 4, 5, 6]) expect(flagStripView(null, state, null).kind).toBe('warn');
  });

  it('no caution and unknown states stay neutral, never an invented flag', () => {
    expect(flagStripView(null, 0, null).kind).toBe('unknown');
    expect(flagStripView(null, -1, null).kind).toBe('unknown');
    expect(flagStripView(null, null, null).kind).toBe('unknown');
  });
});
