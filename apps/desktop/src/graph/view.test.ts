import { describe, expect, it } from 'vitest';
import { liveView, reduceView, resolveRange, type ViewBounds, type ViewState } from './view';

const SCROLL: ViewBounds = { kind: 'scroll', extent: { min: 100, max: 160 }, minSpan: 0.1, maxSpan: 120 };
const DOMAIN: ViewBounds = { kind: 'domain', extent: { min: 0, max: 5000 }, minSpan: 5, maxSpan: 5000 };

describe('resolveRange', () => {
  it('pins a LIVE scroll view to the newest x', () => {
    expect(resolveRange(liveView(30), SCROLL)).toEqual({ min: 130, max: 160 });
  });

  it('shows the whole domain for a LIVE domain view', () => {
    expect(resolveRange(liveView(30), DOMAIN)).toEqual({ min: 0, max: 5000 });
  });

  it('has no range while following data that does not exist yet', () => {
    expect(resolveRange(liveView(30), { ...SCROLL, extent: null })).toBeNull();
  });

  it('shows a fixed range regardless of the data', () => {
    expect(resolveRange({ follow: false, span: 10, end: 120 }, SCROLL)).toEqual({ min: 110, max: 120 });
  });
});

describe('reduceView on a scrolling (time) axis', () => {
  it('scrolling back pauses and keeps the span (spec §63)', () => {
    const v = reduceView(liveView(10), { type: 'pan', dx: -5 }, SCROLL);
    expect(v).toEqual({ follow: false, span: 10, end: 155 });
    expect(resolveRange(v, SCROLL)).toEqual({ min: 145, max: 155 });
  });

  it('cannot scroll back past the oldest data', () => {
    const v = reduceView(liveView(10), { type: 'pan', dx: -1000 }, SCROLL);
    expect(v).toEqual({ follow: false, span: 10, end: 110 });
  });

  it('panning forward to the newest data resumes LIVE', () => {
    const paused: ViewState = { follow: false, span: 10, end: 150 };
    expect(reduceView(paused, { type: 'pan', dx: 3 }, SCROLL)).toEqual({ follow: false, span: 10, end: 153 });
    expect(reduceView(paused, { type: 'pan', dx: 50 }, SCROLL)).toEqual({ follow: true, span: 10, end: 160 });
  });

  it('panning forward while LIVE changes nothing', () => {
    const live = liveView(10);
    expect(reduceView(live, { type: 'pan', dx: 5 }, SCROLL)).toBe(live);
  });

  it('wheel zoom while LIVE keeps following and changes the span only', () => {
    expect(reduceView(liveView(10), { type: 'zoom', factor: 0.5, anchor: 152 }, SCROLL)).toEqual(liveView(5));
  });

  it('wheel zoom while paused keeps the anchor x under the pointer', () => {
    const paused: ViewState = { follow: false, span: 20, end: 150 };
    const v = reduceView(paused, { type: 'zoom', factor: 0.5, anchor: 140 }, SCROLL);
    const r = resolveRange(v, SCROLL);
    expect(r).toEqual({ min: 135, max: 145 });
    // 140 sat at the middle before and still does.
  });

  it('clamps zoom to the minimum and maximum span', () => {
    expect(reduceView(liveView(10), { type: 'zoom', factor: 1e-9, anchor: 0 }, SCROLL).span).toBe(0.1);
    expect(reduceView(liveView(10), { type: 'zoom', factor: 1e9, anchor: 0 }, SCROLL).span).toBe(120);
  });

  it('box zoom fixes the selected range and pauses, even at the newest edge', () => {
    const v = reduceView(liveView(30), { type: 'zoomTo', min: 158, max: 150 }, SCROLL);
    expect(v).toEqual({ follow: false, span: 8, end: 158 });
    const atEdge = reduceView(liveView(30), { type: 'zoomTo', min: 155, max: 160 }, SCROLL);
    expect(atEdge.follow).toBe(false);
  });

  it('go LIVE resumes following', () => {
    expect(reduceView({ follow: false, span: 8, end: 120 }, { type: 'live' }, SCROLL)).toEqual({
      follow: true,
      span: 8,
      end: 120,
    });
  });

  it('ignores invalid zoom factors and non-finite input', () => {
    const live = liveView(10);
    expect(reduceView(live, { type: 'zoom', factor: 0, anchor: 150 }, SCROLL)).toBe(live);
    expect(reduceView(live, { type: 'zoom', factor: Number.NaN, anchor: 150 }, SCROLL)).toBe(live);
    expect(reduceView(live, { type: 'pan', dx: Number.NaN }, SCROLL)).toBe(live);
    expect(reduceView(live, { type: 'zoomTo', min: Number.NaN, max: 1 }, SCROLL)).toBe(live);
  });
});

describe('reduceView on a domain (lap) axis', () => {
  it('zooming in from the whole lap fixes a range around the anchor', () => {
    const v = reduceView(liveView(5000), { type: 'zoom', factor: 0.1, anchor: 2500 }, DOMAIN);
    expect(v.follow).toBe(false);
    expect(resolveRange(v, DOMAIN)).toEqual({ min: 2250, max: 2750 });
  });

  it('panning keeps the range inside the domain', () => {
    const v: ViewState = { follow: false, span: 500, end: 2750 };
    expect(resolveRange(reduceView(v, { type: 'pan', dx: -10_000 }, DOMAIN), DOMAIN)).toEqual({ min: 0, max: 500 });
    expect(resolveRange(reduceView(v, { type: 'pan', dx: 10_000 }, DOMAIN), DOMAIN)).toEqual({
      min: 4500,
      max: 5000,
    });
  });

  it('zooming out to the whole domain returns to LIVE', () => {
    const v: ViewState = { follow: false, span: 500, end: 2750 };
    expect(reduceView(v, { type: 'zoom', factor: 100, anchor: 2500 }, DOMAIN).follow).toBe(true);
  });

  it('panning a LIVE (whole domain) view changes nothing', () => {
    const live = liveView(5000);
    expect(reduceView(live, { type: 'pan', dx: -100 }, DOMAIN)).toBe(live);
  });
});

describe('reduceView clamp (the data moved under a paused view)', () => {
  it('keeps a paused view inside the ring when old rows are dropped', () => {
    const paused: ViewState = { follow: false, span: 10, end: 115 };
    const moved: ViewBounds = { ...SCROLL, extent: { min: 120, max: 180 } };
    expect(reduceView(paused, { type: 'clamp' }, moved)).toEqual({ follow: false, span: 10, end: 130 });
  });

  it('returns the same object when nothing changes, and leaves LIVE views alone', () => {
    const paused: ViewState = { follow: false, span: 10, end: 150 };
    expect(reduceView(paused, { type: 'clamp' }, SCROLL)).toBe(paused);
    const live = liveView(10);
    expect(reduceView(live, { type: 'clamp' }, SCROLL)).toBe(live);
  });

  it('does not resume LIVE when the newest data reaches the paused range', () => {
    const paused: ViewState = { follow: false, span: 10, end: 170 };
    expect(reduceView(paused, { type: 'clamp' }, SCROLL)).toEqual({ follow: false, span: 10, end: 160 });
  });
});
