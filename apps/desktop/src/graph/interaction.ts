/**
 * Pointer, wheel and keyboard input → view actions (spec §62 zoom / pan, §63 scrolling back). Pure functions of
 * the input and the current x range, so the engine's event handlers stay thin and the mapping is testable.
 *
 * - Wheel: zoom around the pointer (wheel down zooms out); with Shift, or a horizontal wheel, pan.
 * - Drag: box zoom; Shift+drag or middle-button drag: pan (dragging right shows earlier data).
 * - Keys: ←/→ pan by a tenth of the view, +/− zoom around the centre, End returns to LIVE.
 */
import type { ViewAction } from './view';
import type { XRange } from './types';

/** Zoom per wheel pixel: a 100 px notch zooms by e^0.2 ≈ 1.22. */
export const WHEEL_ZOOM_PER_PIXEL = 0.002;
/** Pixels per wheel "line" (deltaMode 1) and per "page" (deltaMode 2). */
export const WHEEL_LINE_PX = 16;
export const WHEEL_PAGE_PX = 400;
/** Fraction of the visible span one arrow key pans. */
export const KEY_PAN_FRACTION = 0.1;
/** Zoom factor of one +/− key press. */
export const KEY_ZOOM_FACTOR = 2;
/** A drag narrower than this (CSS px) is a click, not a zoom box. */
export const MIN_ZOOM_BOX_PX = 4;

const DOM_DELTA_LINE = 1;
const DOM_DELTA_PAGE = 2;

/** The wheel event fields the mapping uses. */
export interface WheelInput {
  readonly deltaX: number;
  readonly deltaY: number;
  readonly deltaMode: number;
  readonly shiftKey: boolean;
}

function wheelPixels(delta: number, mode: number): number {
  if (mode === DOM_DELTA_LINE) return delta * WHEEL_LINE_PX;
  if (mode === DOM_DELTA_PAGE) return delta * WHEEL_PAGE_PX;
  return delta;
}

/** X units per CSS pixel of a plot `plotWidth` px wide showing `range`. */
function unitsPerPx(range: XRange, plotWidth: number): number {
  return plotWidth > 0 ? (range.max - range.min) / plotWidth : 0;
}

/** Action of a wheel event at x `anchor` over a plot `plotWidth` px wide, or null for no movement. */
export function wheelAction(input: WheelInput, anchor: number, range: XRange, plotWidth: number): ViewAction | null {
  const dy = wheelPixels(input.deltaY, input.deltaMode);
  const dx = wheelPixels(input.deltaX, input.deltaMode);
  const panPx = input.shiftKey ? dx + dy : dx;
  if (panPx !== 0 && (input.shiftKey || Math.abs(dx) > Math.abs(dy))) {
    return { type: 'pan', dx: panPx * unitsPerPx(range, plotWidth) };
  }
  if (dy === 0) return null;
  return { type: 'zoom', factor: Math.exp(dy * WHEEL_ZOOM_PER_PIXEL), anchor };
}

/** Pan action for a drag of `dxPx` CSS px (content follows the pointer, so dragging right shows earlier x). */
export function dragPanAction(dxPx: number, range: XRange, plotWidth: number): ViewAction | null {
  const dx = -dxPx * unitsPerPx(range, plotWidth);
  return dx === 0 || !Number.isFinite(dx) ? null : { type: 'pan', dx };
}

/** Zoom action for a box dragged between x values `x0` and `x1` that were `widthPx` CSS px apart. */
export function zoomBoxAction(x0: number, x1: number, widthPx: number): ViewAction | null {
  if (!(Math.abs(widthPx) >= MIN_ZOOM_BOX_PX) || !Number.isFinite(x0) || !Number.isFinite(x1)) return null;
  return { type: 'zoomTo', min: Math.min(x0, x1), max: Math.max(x0, x1) };
}

/** Action of a key press over a graph showing `range`, or null when the key is not a graph key. */
export function keyAction(key: string, range: XRange): ViewAction | null {
  const span = range.max - range.min;
  const centre = (range.min + range.max) / 2;
  switch (key) {
    case 'ArrowLeft':
      return { type: 'pan', dx: -span * KEY_PAN_FRACTION };
    case 'ArrowRight':
      return { type: 'pan', dx: span * KEY_PAN_FRACTION };
    case '+':
    case '=':
      return { type: 'zoom', factor: 1 / KEY_ZOOM_FACTOR, anchor: centre };
    case '-':
    case '_':
      return { type: 'zoom', factor: KEY_ZOOM_FACTOR, anchor: centre };
    case 'End':
      return { type: 'live' };
    default:
      return null;
  }
}
