import { describe, expect, it } from 'vitest';
import {
  GRAPH_KEYBOARD_HELP,
  GRAPH_KEYBOARD_SHORTCUTS,
  KEY_PAN_FRACTION,
  KEY_ZOOM_FACTOR,
  MIN_ZOOM_BOX_PX,
  WHEEL_LINE_PX,
  WHEEL_ZOOM_PER_PIXEL,
  dragPanAction,
  keyAction,
  wheelAction,
  zoomBoxAction,
} from './interaction';

const RANGE = { min: 100, max: 110 };
const PLOT_WIDTH = 500;

describe('wheelAction', () => {
  it('zooms around the pointer: wheel down zooms out, wheel up zooms in', () => {
    expect(wheelAction({ deltaX: 0, deltaY: 100, deltaMode: 0, shiftKey: false }, 104, RANGE, PLOT_WIDTH)).toEqual({
      type: 'zoom',
      factor: Math.exp(100 * WHEEL_ZOOM_PER_PIXEL),
      anchor: 104,
    });
    const zoomIn = wheelAction({ deltaX: 0, deltaY: -100, deltaMode: 0, shiftKey: false }, 104, RANGE, PLOT_WIDTH);
    expect(zoomIn?.type === 'zoom' && zoomIn.factor < 1).toBe(true);
  });

  it('converts line-mode deltas to pixels', () => {
    const a = wheelAction({ deltaX: 0, deltaY: 3, deltaMode: 1, shiftKey: false }, 104, RANGE, PLOT_WIDTH);
    expect(a).toEqual({ type: 'zoom', factor: Math.exp(3 * WHEEL_LINE_PX * WHEEL_ZOOM_PER_PIXEL), anchor: 104 });
  });

  it('pans with Shift or a mostly horizontal wheel, in x units per pixel', () => {
    // 10 s over 500 px = 0.02 s per px.
    expect(wheelAction({ deltaX: 0, deltaY: 50, deltaMode: 0, shiftKey: true }, 104, RANGE, PLOT_WIDTH)).toEqual({ type: 'pan', dx: 1 });
    expect(wheelAction({ deltaX: -100, deltaY: 5, deltaMode: 0, shiftKey: false }, 104, RANGE, PLOT_WIDTH)).toEqual({ type: 'pan', dx: -2 });
  });

  it('ignores a wheel event without movement', () => {
    expect(wheelAction({ deltaX: 0, deltaY: 0, deltaMode: 0, shiftKey: false }, 104, RANGE, PLOT_WIDTH)).toBeNull();
  });
});

describe('dragPanAction', () => {
  it('moves the content with the pointer: dragging right shows earlier data', () => {
    expect(dragPanAction(50, RANGE, PLOT_WIDTH)).toEqual({ type: 'pan', dx: -1 });
    expect(dragPanAction(-25, RANGE, PLOT_WIDTH)).toEqual({ type: 'pan', dx: 0.5 });
    expect(dragPanAction(0, RANGE, PLOT_WIDTH)).toBeNull();
    expect(dragPanAction(10, RANGE, 0)).toBeNull();
  });
});

describe('zoomBoxAction', () => {
  it('zooms to the dragged interval in either direction, ignoring clicks', () => {
    expect(zoomBoxAction(108, 102, -120)).toEqual({ type: 'zoomTo', min: 102, max: 108 });
    expect(zoomBoxAction(102, 102.01, MIN_ZOOM_BOX_PX - 1)).toBeNull();
    expect(zoomBoxAction(Number.NaN, 102, 50)).toBeNull();
  });
});

describe('keyAction and keyboard accessibility', () => {
  it('maps arrows to pans, +/- and up/down to centred zooms, and End to LIVE', () => {
    expect(keyAction('ArrowLeft', RANGE)).toEqual({ type: 'pan', dx: -10 * KEY_PAN_FRACTION });
    expect(keyAction('ArrowRight', RANGE)).toEqual({ type: 'pan', dx: 10 * KEY_PAN_FRACTION });
    expect(keyAction('+', RANGE)).toEqual({ type: 'zoom', factor: 1 / KEY_ZOOM_FACTOR, anchor: 105 });
    expect(keyAction('=', RANGE)).toEqual({ type: 'zoom', factor: 1 / KEY_ZOOM_FACTOR, anchor: 105 });
    expect(keyAction('ArrowUp', RANGE)).toEqual({ type: 'zoom', factor: 1 / KEY_ZOOM_FACTOR, anchor: 105 });
    expect(keyAction('-', RANGE)).toEqual({ type: 'zoom', factor: KEY_ZOOM_FACTOR, anchor: 105 });
    expect(keyAction('_', RANGE)).toEqual({ type: 'zoom', factor: KEY_ZOOM_FACTOR, anchor: 105 });
    expect(keyAction('ArrowDown', RANGE)).toEqual({ type: 'zoom', factor: KEY_ZOOM_FACTOR, anchor: 105 });
    expect(keyAction('End', RANGE)).toEqual({ type: 'live' });
    expect(keyAction('a', RANGE)).toBeNull();
  });

  it('provides documented keyboard shortcuts and accessible help string (REDESIGN.md §4 W4.3)', () => {
    expect(GRAPH_KEYBOARD_SHORTCUTS.length).toBeGreaterThanOrEqual(3);
    expect(GRAPH_KEYBOARD_SHORTCUTS.some((s) => s.keys.includes('←'))).toBe(true);
    expect(GRAPH_KEYBOARD_HELP).toContain('Left and Right');
    expect(GRAPH_KEYBOARD_HELP).toContain('zoom');
    expect(GRAPH_KEYBOARD_HELP).toContain('End');
  });
});
