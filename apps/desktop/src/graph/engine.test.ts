import { describe, expect, it } from 'vitest';
import { Unit, ch } from '../generated/channels';
import { unitAxisDisplay } from '../lib/units';
import { FakeClock, FakeFrames } from '../test/fakeScheduler';
import { RecordingContext } from '../test/recordingContext';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../test/tlvEncoder';
import { SPEC_DEGRADED_AFTER_MS } from '../telemetry/freshness';
import { TelemetryStore } from '../telemetry/store';
import { UI_BATCH_FLAG_RESET } from '../telemetry/uiBatch';
import { GraphEngine, NO_DATA_TEXT, referenceKey, seriesColorSlot, seriesKey, type GraphEngineOptions } from './engine';
import { axisKey } from './layout';
import { LiveRingSource } from './liveSource';
import { CURRENT_DEGRADED_LABEL, CURRENT_LABEL, CURRENT_STALE_LABEL } from './overlay';
import type { RenderFrame, SeriesRenderer } from './renderer';
import { StaticSource } from './staticSource';
import { GraphSyncGroup } from './syncBus';
import type { GraphDataSource } from './types';

const ROW_DT = 0.01;
const WIDTH = 800;
const HEIGHT = 200;

class FakeRenderer implements SeriesRenderer {
  readonly kind = 'canvas2d' as const;
  readonly frames: RenderFrame[] = [];
  host: HTMLElement | null = null;
  destroyed = false;
  size: readonly number[] = [];
  mount(host: HTMLElement): void {
    this.host = host;
  }
  setSize(width: number, height: number, dpr: number): void {
    this.size = [width, height, dpr];
  }
  render(frame: RenderFrame): void {
    this.frames.push(frame);
  }
  destroy(): void {
    this.destroyed = true;
  }
  get last(): RenderFrame | undefined {
    return this.frames.at(-1);
  }
}

interface Harness {
  readonly store: TelemetryStore;
  readonly clock: FakeClock;
  readonly frames: FakeFrames;
  /** Over-canvas context of the most recently attached engine. */
  readonly contexts: RecordingContext[];
  send(blocks: readonly (readonly [number, readonly FixtureRecord[]])[]): void;
  /** Runs store notifications and the engine frames they request. */
  pump(): void;
  engine(options: Partial<GraphEngineOptions> & Pick<GraphEngineOptions, 'series'>): {
    engine: GraphEngine;
    renderer: FakeRenderer;
    container: HTMLDivElement;
    over: () => RecordingContext;
  };
}

function harness(): Harness {
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
  const contexts: RecordingContext[] = [];
  let seq = -1;
  return {
    store,
    clock,
    frames,
    contexts,
    send(blocks) {
      seq += 1;
      const encoded = blocks.map(([sourceEtS, records]) => encodeTlvBlock({ sourceEtS }, records));
      const result = store.applyBatch(encodeUiBatch(seq === 0 ? UI_BATCH_FLAG_RESET : 0, seq, encoded));
      if (!result.ok) throw new Error(`batch refused: ${JSON.stringify(result.error)}`);
    },
    pump() {
      frames.runFrame();
      frames.runFrame();
    },
    engine(options) {
      const renderer = new FakeRenderer();
      const own: RecordingContext[] = [];
      const engine = new GraphEngine({
        source: new LiveRingSource(store, { xMode: 'time' }),
        xMode: 'time',
        renderer: () => renderer,
        scheduleFrame: frames.schedule,
        context: () => {
          const c = new RecordingContext();
          own.push(c);
          contexts.push(c);
          return c;
        },
        ...options,
      });
      const container = document.createElement('div');
      engine.attach(container);
      engine.resize(WIDTH, HEIGHT);
      // Contexts are created under-canvas first, then over-canvas.
      return { engine, renderer, container, over: () => own[1] ?? new RecordingContext() };
    },
  };
}

/** `n` rows from `t0` every 10 ms; speed = row index, throttle 0.5, gear 3. */
function rows(t0: number, n: number, extra: (i: number) => FixtureRecord[] = () => []): [number, FixtureRecord[]][] {
  return Array.from(
    { length: n },
    (_, i) => [t0 + i * ROW_DT, [rec.f32(ch.SPEED, i), rec.f32(ch.THROTTLE, 0.5), rec.i32(ch.GEAR, 3), ...extra(i)]] as [number, FixtureRecord[]],
  );
}

describe('GraphEngine repaint policy', () => {
  it('draws once per change, follows the newest data in LIVE, and repaints only the chrome for cursor moves', () => {
    const h = harness();
    h.send(rows(100, 1000));
    const { engine, renderer, over } = h.engine({ series: [{ channelId: ch.SPEED }] });
    engine.flush();
    expect(renderer.frames).toHaveLength(1);
    const range = renderer.last?.xRange;
    expect(range?.max).toBeCloseTo(109.99, 6);
    expect((range?.max ?? 0) - (range?.min ?? 0)).toBeCloseTo(10, 9);
    engine.flush();
    expect(renderer.frames).toHaveLength(1);

    over().clear();
    engine.setCursor(105.004);
    engine.flush();
    expect(renderer.frames).toHaveLength(1);
    // Snapped to the nearest row (105.00) and labelled on the x axis.
    expect(engine.cursorX).toBeCloseTo(105, 9);
    expect(over().texts()).toContain('1:45.00');

    h.send(rows(110, 10));
    h.pump();
    expect(renderer.frames).toHaveLength(2);
    expect(renderer.last?.xRange.max).toBeCloseTo(110.09, 6);
  });

  it('shows NO DATA and draws no series before data arrives', () => {
    const h = harness();
    const { engine, renderer, over } = h.engine({ series: [{ channelId: ch.SPEED }] });
    engine.flush();
    expect(renderer.last?.series).toEqual([]);
    expect(over().texts()).toContain(NO_DATA_TEXT);
  });
});

describe('GraphEngine live view (spec §63)', () => {
  it('pauses when scrolled back, keeps the paused range while data arrives, and returns to LIVE', () => {
    const h = harness();
    h.send(rows(100, 3000));
    const { engine, renderer } = h.engine({ series: [{ channelId: ch.SPEED }] });
    engine.flush();
    engine.dispatch({ type: 'pan', dx: -10 });
    engine.flush();
    expect(engine.getSnapshot().follow).toBe(false);
    const paused = renderer.last?.xRange;
    expect(paused?.max).toBeCloseTo(119.99, 6);
    h.send(rows(130, 100));
    h.pump();
    expect(renderer.last?.xRange).toEqual(paused);
    engine.goLive();
    engine.flush();
    expect(engine.getSnapshot().follow).toBe(true);
    expect(renderer.last?.xRange.max).toBeCloseTo(130.99, 6);
  });
});

describe('GraphEngine synchronization (spec §81)', () => {
  it('shares the cursor and the x view across a group', () => {
    const h = harness();
    h.send(rows(100, 1000));
    const group = new GraphSyncGroup();
    const a = h.engine({ series: [{ channelId: ch.SPEED }], syncGroup: group, id: 'a' });
    const b = h.engine({ series: [{ channelId: ch.THROTTLE }], syncGroup: group, id: 'b' });
    a.engine.flush();
    b.engine.flush();
    a.engine.setCursor(104.333);
    expect(b.engine.cursorX).toBeCloseTo(104.33, 9);
    a.engine.dispatch({ type: 'zoom', factor: 0.5, anchor: 105 });
    expect(b.engine.view).toEqual(a.engine.view);
    a.engine.flush();
    b.engine.flush();
    expect(b.renderer.last?.xRange).toEqual(a.renderer.last?.xRange);
    // Both readouts refer to the same row.
    expect(b.engine.getSnapshot().cursorLabel).toBe(a.engine.getSnapshot().cursorLabel);
  });
});

describe('GraphEngine readouts', () => {
  it('reads values at the cursor and never invents one (NO DATA, UNAVAILABLE, NOT SUPPORTED)', () => {
    const h = harness();
    h.send([
      ...rows(100, 50),
      [100.5, [rec.quality(ch.SPEED, 3)]],
      ...rows(100.51, 50).map(([t, r]) => [t, r.filter((x) => x.channel !== ch.SPEED).concat(rec.f32(ch.SPEED, 7))] as [number, FixtureRecord[]]),
    ]);
    const { engine } = h.engine({
      series: [{ channelId: ch.SPEED }, { channelId: ch.GEAR }, { channelId: ch.TRACK_LENGTH }, { channelId: ch.TC_INTERVENTION }],
    });
    engine.flush();
    engine.setCursor(100.2);
    engine.flush();
    let r = engine.getSnapshot().readouts;
    expect(r[seriesKey(ch.SPEED)]).toMatchObject({ state: 'live', value: 20 });
    expect(r[seriesKey(ch.GEAR)]).toMatchObject({ state: 'live', value: 3 });
    expect(r[seriesKey(ch.TRACK_LENGTH)]).toMatchObject({ state: 'unavailable', value: null });
    expect(r[seriesKey(ch.TC_INTERVENTION)]).toMatchObject({ state: 'notSupported', value: null });
    engine.setCursor(100.5);
    engine.flush();
    r = engine.getSnapshot().readouts;
    expect(r[seriesKey(ch.SPEED)]).toMatchObject({ state: 'noData', value: null });
  });

  it('reads the current sample without a cursor, DEGRADED when late and STALE when the feed stops', () => {
    const h = harness();
    h.send(rows(100, 100));
    const { engine, over } = h.engine({ series: [{ channelId: ch.SPEED }] });
    engine.flush();
    expect(engine.getSnapshot().readouts[seriesKey(ch.SPEED)]).toMatchObject({ state: 'live', value: 99 });
    expect(over().texts()).toContain(CURRENT_LABEL);
    h.clock.advance(SPEC_DEGRADED_AFTER_MS + 1);
    h.pump();
    engine.flush();
    expect(engine.getSnapshot().readouts[seriesKey(ch.SPEED)]).toMatchObject({ state: 'degraded', value: 99 });
    expect(over().texts()).toContain(CURRENT_DEGRADED_LABEL);
    // History at a cursor is shown as it was recorded.
    engine.setCursor(100.5);
    engine.flush();
    expect(engine.getSnapshot().readouts[seriesKey(ch.SPEED)]).toMatchObject({ state: 'live', value: 50 });
    engine.setCursor(null);
    h.clock.advance(5_000);
    h.pump();
    engine.flush();
    expect(engine.getSnapshot().readouts[seriesKey(ch.SPEED)]).toMatchObject({ state: 'stale', value: null });
    expect(over().texts()).toContain(CURRENT_STALE_LABEL);
  });

  it('keeps the same snapshot object while nothing shown changes', () => {
    const h = harness();
    h.send(rows(100, 100));
    const { engine } = h.engine({ series: [{ channelId: ch.SPEED }] });
    engine.flush();
    const first = engine.getSnapshot();
    engine.update({ markers: [] });
    engine.flush();
    expect(engine.getSnapshot()).toBe(first);
  });
});

describe('GraphEngine axes and series', () => {
  it('groups y axes by unit, applies fixed scales and draws Discrete channels as steps', () => {
    const h = harness();
    h.send(rows(100, 100));
    const { engine, renderer } = h.engine({ series: [{ channelId: ch.SPEED }, { channelId: ch.GEAR }, { channelId: ch.THROTTLE }] });
    engine.flush();
    const speedDisplay = unitAxisDisplay(Unit.MetrePerSecond);
    expect(engine.getSnapshot().axes.map((a) => a.title)).toEqual([speedDisplay.label, 'Gear (-1 R, 0 N)', '%']);
    const keys = renderer.last?.series.map((s) => [s.key, s.step]);
    expect(keys).toEqual([
      [seriesKey(ch.SPEED), false],
      [seriesKey(ch.GEAR), true],
      [seriesKey(ch.THROTTLE), false],
    ]);
    engine.update({ yScales: { [axisKey(speedDisplay)]: { mode: 'fixed', min: 0, max: 360 } } });
    engine.flush();
    const speed = renderer.last?.series.find((s) => s.key === seriesKey(ch.SPEED));
    expect(speed?.yRange.min).toBe(0);
    expect(speed?.yRange.max).toBeCloseTo(100, 9);
    expect(engine.getSnapshot().axes[0]?.setting).toEqual({ mode: 'fixed', min: 0, max: 360 });
  });

  it('leaves disabled channels out of drawing, axes and readouts', () => {
    const h = harness();
    h.send(rows(100, 100));
    const { engine, renderer } = h.engine({ series: [{ channelId: ch.SPEED }, { channelId: ch.THROTTLE, enabled: false }] });
    engine.flush();
    expect(renderer.last?.series.map((s) => s.key)).toEqual([seriesKey(ch.SPEED)]);
    expect(engine.getSnapshot().axes).toHaveLength(1);
    expect(engine.getSnapshot().readouts[seriesKey(ch.THROTTLE)]).toBeNull();
  });

  it('overlays a reference series dashed, from its own source', () => {
    const h = harness();
    h.send(rows(100, 100));
    const reference = new StaticSource({ xs: [100, 100.5, 101], columns: new Map([[ch.SPEED, [10, 20, 30]]]) });
    const { engine, renderer } = h.engine({
      series: [{ channelId: ch.SPEED }],
      references: [{ key: 'best', label: 'Best lap', channelId: ch.SPEED, source: reference }],
    });
    engine.flush();
    const ref = renderer.last?.series.find((s) => s.key === referenceKey('best'));
    expect(ref?.dashed).toBe(true);
    expect(ref?.window).not.toBe(renderer.last?.series[0]?.window);
    engine.setCursor(100.5);
    engine.flush();
    expect(engine.getSnapshot().readouts[referenceKey('best')]).toMatchObject({ state: 'live', value: 20 });
  });

  it('draws markers with their labels', () => {
    const h = harness();
    h.send(rows(100, 1000));
    const { engine, over } = h.engine({
      series: [{ channelId: ch.SPEED }],
      markers: [
        { x: 105, kind: 'lap', label: 'Lap 4' },
        { x: 50, kind: 'sector', label: 'Off screen' },
      ],
    });
    engine.flush();
    expect(over().texts()).toContain('Lap 4');
    expect(over().texts()).not.toContain('Off screen');
  });

  it('assigns palette slots 1…8 by position unless a colour is given', () => {
    expect(seriesColorSlot({ channelId: 1 }, 0)).toBe(1);
    expect(seriesColorSlot({ channelId: 1 }, 9)).toBe(2);
    expect(seriesColorSlot({ channelId: 1, color: 5 }, 0)).toBe(5);
    expect(seriesColorSlot({ channelId: 1, color: 12 }, 0)).toBe(4);
  });
});

function pointer(type: string, init: { clientX: number; button?: number; shiftKey?: boolean }): Event {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, ...init });
  Object.defineProperty(event, 'pointerId', { value: 1 });
  return event;
}

describe('GraphEngine input', () => {
  it('zooms with the wheel, box-zooms with a drag (pausing LIVE) and returns to LIVE with End', () => {
    const h = harness();
    h.send(rows(100, 3000));
    const { engine, container } = h.engine({ series: [{ channelId: ch.SPEED }] });
    engine.flush();
    const canvas = container.querySelector('canvas.telemetry-graph__input');
    expect(canvas).not.toBeNull();
    if (canvas === null) return;

    canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: -500, clientX: 400, bubbles: true, cancelable: true }));
    engine.flush();
    expect(engine.view.span).toBeLessThan(10);
    expect(engine.view.follow).toBe(true);

    canvas.dispatchEvent(pointer('pointerdown', { clientX: 200, button: 0 }));
    canvas.dispatchEvent(pointer('pointermove', { clientX: 300 }));
    canvas.dispatchEvent(pointer('pointerup', { clientX: 300 }));
    engine.flush();
    expect(engine.view.follow).toBe(false);
    expect(engine.getSnapshot().follow).toBe(false);

    canvas.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true, cancelable: true }));
    engine.flush();
    expect(engine.view.follow).toBe(true);
  });

  it('moves the cursor with the pointer and hides it when the pointer leaves', () => {
    const h = harness();
    h.send(rows(100, 1000));
    const { engine, container } = h.engine({ series: [{ channelId: ch.SPEED }] });
    engine.flush();
    const canvas = container.querySelector('canvas.telemetry-graph__input');
    canvas?.dispatchEvent(pointer('pointermove', { clientX: 400 }));
    expect(engine.cursorX).not.toBeNull();
    canvas?.dispatchEvent(pointer('pointerleave', { clientX: 400 }));
    expect(engine.cursorX).toBeNull();
  });
});

describe('GraphEngine lifecycle', () => {
  it('removes its layers, listeners and subscriptions on detach and can attach again', () => {
    const h = harness();
    h.send(rows(100, 100));
    let listeners = 0;
    const inner = new LiveRingSource(h.store, { xMode: 'time' });
    const source: GraphDataSource = {
      origin: 'live',
      extent: () => inner.extent(),
      read: (range, ids) => inner.read(range, ids),
      current: () => inner.current(),
      subscribe: (listener) => {
        listeners += 1;
        const off = inner.subscribe(listener);
        return () => {
          listeners -= 1;
          off();
        };
      },
    };
    const renderer = new FakeRenderer();
    const engine = new GraphEngine({
      source,
      xMode: 'time',
      series: [{ channelId: ch.SPEED }],
      renderer: () => renderer,
      scheduleFrame: h.frames.schedule,
      context: () => new RecordingContext(),
    });
    const container = document.createElement('div');
    const pendingBefore = h.frames.pending;
    const detach = engine.attach(container);
    expect(container.children).toHaveLength(3);
    expect(listeners).toBe(1);
    expect(h.frames.pending).toBe(pendingBefore + 1);
    detach();
    expect(container.children).toHaveLength(0);
    expect(listeners).toBe(0);
    expect(renderer.destroyed).toBe(true);
    // The engine's requested frame is cancelled.
    expect(h.frames.pending).toBe(pendingBefore);
    engine.attach(container)();
    expect(listeners).toBe(0);
  });
});
