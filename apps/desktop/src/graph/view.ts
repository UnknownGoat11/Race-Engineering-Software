/**
 * The x view of a graph (zoom, pan, LIVE follow) as pure state + reducer (spec §62 zoom/pan, §63 scrolling back
 * within the live buffer).
 *
 * A view either FOLLOWS the data (LIVE) or holds a fixed range. On a scrolling (time) axis, LIVE pins the right
 * edge to the newest x and keeps the span; scrolling back or box-zooming pauses it, and panning forward to the
 * newest data resumes it. On a domain (lap) axis, LIVE shows the whole domain; zooming fixes a range.
 */
import { clamp } from './scale';
import type { XViewKind } from './xAxis';
import type { XRange } from './types';

/** Zoom and pan state. `end` and `span` are in x units. */
export interface ViewState {
  /** LIVE: the range follows the data (scroll axis) or shows the whole domain (domain axis). */
  readonly follow: boolean;
  /** Visible width. */
  readonly span: number;
  /** Right edge while not following (ignored while following). */
  readonly end: number;
}

/** What limits the view; supplied by the engine from the axis mode and the data. */
export interface ViewBounds {
  readonly kind: XViewKind;
  /** Scroll axis: x extent of the data. Domain axis: the domain. Null when there is nothing to show. */
  readonly extent: XRange | null;
  readonly minSpan: number;
  readonly maxSpan: number;
}

/** View actions. Zoom factors above 1 zoom out, below 1 zoom in. */
export type ViewAction =
  | { readonly type: 'zoom'; readonly factor: number; readonly anchor: number }
  | { readonly type: 'zoomTo'; readonly min: number; readonly max: number }
  | { readonly type: 'pan'; readonly dx: number }
  | { readonly type: 'live' }
  | { readonly type: 'setSpan'; readonly span: number }
  /** Re-applies the bounds after the data moved (e.g. the live ring dropped the rows a paused view showed). */
  | { readonly type: 'clamp' };

/** A LIVE view of `span`. */
export function liveView(span: number): ViewState {
  return { follow: true, span, end: 0 };
}

/** The x range a view shows, or null when it follows data that does not exist yet. */
export function resolveRange(view: ViewState, bounds: ViewBounds): XRange | null {
  if (!view.follow) return { min: view.end - view.span, max: view.end };
  const extent = bounds.extent;
  if (extent === null) return null;
  if (bounds.kind === 'domain') return extent;
  return { min: extent.max - view.span, max: extent.max };
}

function clampSpan(span: number, bounds: ViewBounds): number {
  const lo = Math.min(bounds.minSpan, bounds.maxSpan);
  return clamp(Number.isFinite(span) ? span : bounds.maxSpan, lo, bounds.maxSpan);
}

/**
 * A fixed (not following) view of `[end - span, end]`, kept within the data: on a scroll axis the right edge
 * stays between the oldest x (plus the span, when the data is wider than it) and the newest x; on a domain axis
 * the range stays inside the domain. A fixed range that reaches the newest x of a scroll axis resumes LIVE only
 * when `resumeAtNewest` (panning forward), not after a box zoom.
 */
function fixedView(span: number, end: number, bounds: ViewBounds, resumeAtNewest: boolean): ViewState {
  const extent = bounds.extent;
  if (extent === null) return { follow: false, span, end };
  if (bounds.kind === 'domain') {
    if (span >= extent.max - extent.min) return { follow: true, span: extent.max - extent.min, end: extent.max };
    return { follow: false, span, end: clamp(end, extent.min + span, extent.max) };
  }
  const lowest = Math.min(extent.min + span, extent.max);
  const clamped = clamp(end, lowest, extent.max);
  if (resumeAtNewest && clamped >= extent.max) return { follow: true, span, end: clamped };
  return { follow: false, span, end: clamped };
}

/** Applies `action` to `view` within `bounds`. Pure: the same inputs always give the same view. */
export function reduceView(view: ViewState, action: ViewAction, bounds: ViewBounds): ViewState {
  const range = resolveRange(view, bounds);
  switch (action.type) {
    case 'live':
      return view.follow ? view : { ...view, follow: true };
    case 'clamp': {
      if (view.follow) return view;
      const next = fixedView(view.span, view.end, bounds, false);
      return next.follow === view.follow && next.span === view.span && next.end === view.end ? view : next;
    }
    case 'setSpan': {
      const span = clampSpan(action.span, bounds);
      if (view.follow) return { ...view, span };
      return fixedView(span, view.end, bounds, false);
    }
    case 'zoom': {
      if (range === null || !(action.factor > 0)) return view;
      const span = clampSpan(view.span * action.factor, bounds);
      if (view.follow && bounds.kind === 'scroll') {
        // LIVE keeps following: the newest data stays at the right edge.
        return { ...view, span };
      }
      const current = range.max - range.min;
      const anchor = clamp(action.anchor, range.min, range.max);
      const ratio = current > 0 ? span / current : 1;
      const end = anchor + (range.max - anchor) * ratio;
      return fixedView(span, end, bounds, false);
    }
    case 'zoomTo': {
      const min = Math.min(action.min, action.max);
      const max = Math.max(action.min, action.max);
      if (!Number.isFinite(min) || !Number.isFinite(max)) return view;
      const span = clampSpan(max - min, bounds);
      const centre = (min + max) / 2;
      return fixedView(span, centre + span / 2, bounds, false);
    }
    case 'pan': {
      if (range === null || !Number.isFinite(action.dx) || action.dx === 0) return view;
      if (view.follow && bounds.kind === 'domain') return view;
      if (view.follow && action.dx > 0) return view;
      const span = range.max - range.min;
      return fixedView(span, range.max + action.dx, bounds, true);
    }
  }
}
