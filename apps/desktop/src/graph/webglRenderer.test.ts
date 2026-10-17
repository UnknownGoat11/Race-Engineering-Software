import { describe, expect, it, vi } from 'vitest';
import { tracePolyline } from './decimate';
import type { RenderFrame, RenderSeries, SeriesRenderer } from './renderer';
import { linearAffine } from './scale';
import type { GraphWindow } from './types';
import {
  FLOATS_PER_VERTEX,
  StripBuilder,
  WebGLRenderer,
  browserColorParser,
  canvasAcceptsColor,
  type Rgba,
} from './webglRenderer';

interface GlCall {
  readonly op: string;
  readonly args: readonly unknown[];
}

const GL_CONSTANTS = {
  VERTEX_SHADER: 1,
  FRAGMENT_SHADER: 2,
  COMPILE_STATUS: 3,
  LINK_STATUS: 4,
  ARRAY_BUFFER: 5,
  FLOAT: 6,
  BLEND: 7,
  ONE: 8,
  ONE_MINUS_SRC_ALPHA: 9,
  SCISSOR_TEST: 10,
  COLOR_BUFFER_BIT: 11,
  DYNAMIC_DRAW: 12,
  LINE_STRIP: 13,
  POINTS: 14,
  OUT_OF_MEMORY: 15,
} as const satisfies Record<string, number>;
const GL_CONSTANT_BY_NAME: Readonly<Record<string, number>> = GL_CONSTANTS;

/** A recording WebGL context: every method call is logged; queries answer "success". */
function fakeGl(options: { multiDraw?: boolean; compileFails?: boolean; outOfMemory?: boolean } = {}): {
  gl: WebGLRenderingContext;
  calls: GlCall[];
} {
  const calls: GlCall[] = [];
  const answers: Record<string, (...args: unknown[]) => unknown> = {
    getShaderParameter: () => options.compileFails !== true,
    getShaderInfoLog: () => 'ERROR: 0:3: syntax error',
    getProgramParameter: () => true,
    getError: () => (options.outOfMemory === true ? GL_CONSTANTS.OUT_OF_MEMORY : 0),
    getAttribLocation: () => 0,
    getUniformLocation: (_program, name) => ({ name }),
    getExtension: (name) => {
      if (name === 'WEBGL_multi_draw' && options.multiDraw === true) {
        return {
          multiDrawArraysWEBGL: (...args: unknown[]) => {
            calls.push({ op: 'multiDrawArraysWEBGL', args: args.map((a) => (a instanceof Int32Array ? [...a] : a)) });
          },
        };
      }
      if (name === 'WEBGL_lose_context') return { loseContext: () => calls.push({ op: 'loseContext', args: [] }) };
      return null;
    },
  };
  const gl = new Proxy(
    {},
    {
      get(_target, prop) {
        if (typeof prop !== 'string') return undefined;
        const constant = GL_CONSTANT_BY_NAME[prop];
        if (constant !== undefined) return constant;
        return (...args: unknown[]) => {
          calls.push({ op: prop, args: args.map((a) => (a instanceof Float32Array ? Array.from(a) : a)) });
          const answer = answers[prop];
          return answer === undefined ? {} : answer(...args);
        };
      },
    },
  ) as WebGLRenderingContext;
  return { gl, calls };
}

/** A renderer stub that records what it is asked to do. */
function fallbackStub(): { renderer: SeriesRenderer; log: string[]; frames: RenderFrame[] } {
  const log: string[] = [];
  const frames: RenderFrame[] = [];
  const renderer: SeriesRenderer = {
    kind: 'canvas2d',
    mount: () => log.push('mount'),
    setSize: (w, h, dpr) => log.push(`setSize ${w}x${h}@${dpr}`),
    render: (frame) => {
      log.push('render');
      frames.push(frame);
    },
    destroy: () => log.push('destroy'),
  };
  return { renderer, log, frames };
}

function windowOf(xs: readonly number[]): GraphWindow {
  const x = Float64Array.from(xs);
  return { xs: x, ts: x, count: xs.length, monotonicX: true, column: () => null };
}

function series(window: GraphWindow, ys: readonly number[], extra: Partial<RenderSeries> = {}): RenderSeries {
  return {
    key: 's',
    window,
    ys: Float64Array.from(ys),
    yRange: { min: 0, max: 2 },
    color: 'red',
    dashed: false,
    step: false,
    ...extra,
  };
}

/** Plot 30 × 10 CSS px at (2, 1) in a 40 × 20 layer; x 0…3. */
function frame(s: readonly RenderSeries[]): RenderFrame {
  return {
    plot: { left: 2, top: 1, width: 30, height: 10 },
    xRange: { min: 0, max: 3 },
    maxGapT: Number.POSITIVE_INFINITY,
    series: s,
  };
}

const RED: Rgba = [1, 0, 0, 0.5];
const parseColor = (css: string): Rgba | null => (css === 'red' ? RED : null);

function mounted(options: { multiDraw?: boolean; dpr?: number } = {}): {
  renderer: WebGLRenderer;
  host: HTMLElement;
  calls: GlCall[];
  fallback: ReturnType<typeof fallbackStub>;
} {
  const { gl, calls } = fakeGl(options);
  const fallback = fallbackStub();
  const renderer = new WebGLRenderer({ context: () => gl, parseColor, fallback: () => fallback.renderer });
  const host = document.createElement('div');
  renderer.mount(host);
  renderer.setSize(40, 20, options.dpr ?? 1);
  return { renderer, host, calls, fallback };
}

const ops = (calls: readonly GlCall[], op: string): GlCall[] => calls.filter((c) => c.op === op);

describe('StripBuilder', () => {
  it('starts a run at every moveTo and accumulates the distance along it', () => {
    const b = new StripBuilder();
    b.moveTo(0, 0);
    b.lineTo(3, 4);
    b.lineTo(3, 10);
    b.moveTo(20, 1);
    expect(b.runs).toBe(2);
    expect(b.count).toBe(4);
    expect([b.runFirst[0], b.runCount[0], b.runLength[0]]).toEqual([0, 3, 11]);
    expect([b.runFirst[1], b.runCount[1], b.runLength[1]]).toEqual([3, 1, 0]);
    // x, y, distance per vertex.
    expect(Array.from(b.vertices.subarray(0, 4 * FLOATS_PER_VERTEX))).toEqual([0, 0, 0, 3, 4, 5, 3, 10, 11, 20, 1, 0]);
  });

  it('treats a lineTo without a run as a moveTo, and a zero-length run as a dot', () => {
    const b = new StripBuilder();
    b.lineTo(5, 5);
    b.lineTo(5, 5);
    expect(b.runs).toBe(1);
    expect(b.runLength[0]).toBe(0);
  });

  it('grows past its initial vertex and run capacity without losing data', () => {
    const b = new StripBuilder();
    const runs = 1000;
    for (let r = 0; r < runs; r++) {
      b.moveTo(r, 0);
      for (let k = 1; k < 20; k++) b.lineTo(r, k);
    }
    expect(b.runs).toBe(runs);
    expect(b.count).toBe(runs * 20);
    expect(b.runFirst[runs - 1]).toBe((runs - 1) * 20);
    expect(b.runLength[runs - 1]).toBe(19);
    expect(b.vertices[(runs * 20 - 1) * FLOATS_PER_VERTEX + 1]).toBe(19);
    b.reset();
    expect([b.runs, b.count]).toEqual([0, 0]);
  });
});

describe('WebGLRenderer', () => {
  it('uploads exactly the decimated polyline of each series and draws strips and dots', () => {
    const { renderer, calls } = mounted();
    expect(renderer.usingWebGL).toBe(true);
    const line = series(windowOf([0, 1, 2, 3]), [0, 2, 1, 1]);
    // A lone sample (its neighbours are NaN) is a dot.
    const dot = series(windowOf([0, 1, 2]), [Number.NaN, 1, Number.NaN], { key: 'd' });
    calls.length = 0;
    renderer.render(frame([line, dot]));

    const expected = new StripBuilder();
    for (const s of [line, dot]) {
      tracePolyline(
        {
          xs: s.window.xs,
          ts: s.window.ts,
          ys: s.ys,
          start: 0,
          end: s.window.count,
          x: linearAffine(0, 3, 2, 32),
          y: linearAffine(0, 2, 11, 1),
          maxGapT: Number.POSITIVE_INFINITY,
          step: false,
        },
        expected,
      );
    }
    const uploads = ops(calls, 'bufferData');
    expect(uploads).toHaveLength(1);
    expect(uploads[0]?.args[1]).toEqual(Array.from(expected.vertices.subarray(0, expected.count * FLOATS_PER_VERTEX)));

    const draws = ops(calls, 'drawArrays').map((c) => c.args);
    // Line: one strip of the line's vertices. Dot: its first vertex as a point.
    expect(draws).toEqual([
      [GL_CONSTANTS.LINE_STRIP, 0, expected.runCount[0]],
      [GL_CONSTANTS.POINTS, expected.runFirst[1], 1],
    ]);
    // Premultiplied colour, no dash.
    expect(ops(calls, 'uniform4f')[0]?.args.slice(1)).toEqual([0.5, 0, 0, 0.5]);
    expect(ops(calls, 'uniform2f').find((c) => (c.args[0] as { name: string }).name === 'u_dash')?.args.slice(1)).toEqual([0, 0]);
  });

  it('clips to the plot with a whole-pixel scissor counted from the bottom-left corner', () => {
    const { renderer, calls } = mounted({ dpr: 1.5 });
    calls.length = 0;
    renderer.render(frame([series(windowOf([0, 3]), [0, 2])]));
    // Layer 60 × 30 device px; plot left 3, top 1.5, width 45, height 15 → bottom edge at 16.5 → y from bottom 13.5.
    expect(ops(calls, 'scissor')[0]?.args).toEqual([3, 14, 45, 15]);
  });

  it('dashes references along the line in device pixels', () => {
    const { renderer, calls } = mounted({ dpr: 2 });
    calls.length = 0;
    renderer.render(frame([series(windowOf([0, 3]), [0, 2], { dashed: true })]));
    const dash = ops(calls, 'uniform2f').filter((c) => (c.args[0] as { name: string }).name === 'u_dash');
    expect(dash[0]?.args.slice(1)).toEqual([12, 20]);
  });

  it('draws all runs of a series with one multi-draw call when WEBGL_multi_draw exists', () => {
    const { renderer, calls } = mounted({ multiDraw: true });
    calls.length = 0;
    // Two runs separated by a NaN row.
    renderer.render(frame([series(windowOf([0, 1, 2, 3, 4]), [0, 1, Number.NaN, 1, 2])]));
    expect(ops(calls, 'drawArrays')).toHaveLength(0);
    const multi = ops(calls, 'multiDrawArraysWEBGL');
    expect(multi).toHaveLength(1);
    expect(multi[0]?.args[0]).toBe(GL_CONSTANTS.LINE_STRIP);
    expect(multi[0]?.args[5]).toBe(2);
  });

  it('draws nothing (but clears) for an empty frame, a flat y range or an empty plot', () => {
    const { renderer, calls } = mounted();
    calls.length = 0;
    renderer.render(frame([]));
    renderer.render(frame([series(windowOf([0, 3]), [1, 1], { yRange: { min: 1, max: 1 } })]));
    renderer.render({ ...frame([series(windowOf([0, 3]), [0, 2])]), plot: { left: 0, top: 0, width: 0, height: 10 } });
    expect(ops(calls, 'clear')).toHaveLength(3);
    expect(ops(calls, 'bufferData')).toHaveLength(0);
  });

  it('resolves each colour once and draws an unparseable one opaque white, logged once (H5)', () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const seen: string[] = [];
    const { gl, calls } = fakeGl();
    const renderer = new WebGLRenderer({
      context: () => gl,
      parseColor: (css) => {
        seen.push(css);
        return null;
      },
      fallback: () => fallbackStub().renderer,
    });
    renderer.mount(document.createElement('div'));
    renderer.setSize(40, 20, 1);
    const f = frame([series(windowOf([0, 3]), [0, 2], { color: 'nonsense' })]);
    renderer.render(f);
    renderer.render(f);
    expect(seen).toEqual(['nonsense']);
    expect(ops(calls, 'uniform4f')[0]?.args.slice(1)).toEqual([1, 1, 1, 1]);
    expect(errors).toHaveBeenCalledTimes(1);
    expect(errors.mock.calls[0]?.[1]).toEqual({ color: 'nonsense' });
    errors.mockRestore();
  });

  it('falls back to the Canvas2D renderer when no WebGL context is available', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const fallback = fallbackStub();
    const renderer = new WebGLRenderer({ context: () => null, fallback: () => fallback.renderer });
    const host = document.createElement('div');
    renderer.mount(host);
    renderer.setSize(40, 20, 2);
    const f = frame([series(windowOf([0, 3]), [0, 2])]);
    renderer.render(f);
    expect(renderer.usingWebGL).toBe(false);
    expect(host.querySelector('canvas')).toBeNull();
    expect(fallback.log).toEqual(['mount', 'setSize 0x0@1', 'setSize 40x20@2', 'render']);
    expect(fallback.frames[0]).toBe(f);
    expect(renderer.status()).toEqual({ activeKind: 'canvas2d', state: 'degraded', reason: 'WebGL context unavailable' });
    vi.restoreAllMocks();
  });

  it('on context loss switches to the fallback, reports and logs why, and never replays an old frame (M1)', () => {
    const warnings = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { renderer, host, fallback } = mounted({ dpr: 1.25 });
    const changes = vi.fn();
    renderer.onStatusChange(changes);
    expect(renderer.status()).toEqual({ activeKind: 'webgl', state: 'ok', reason: null });
    const f = frame([series(windowOf([0, 3]), [0, 2])]);
    renderer.render(f);
    const canvas = host.querySelector('canvas');
    expect(canvas).not.toBeNull();
    const lost = new Event('webglcontextlost', { cancelable: true });
    canvas?.dispatchEvent(lost);
    expect(lost.defaultPrevented).toBe(true);
    expect(renderer.usingWebGL).toBe(false);
    expect(host.querySelector('canvas')).toBeNull();
    // The window arrays of the last frame may have been reused since: the engine redraws from a fresh read.
    expect(fallback.log).toEqual(['mount', 'setSize 40x20@1.25']);
    expect(changes).toHaveBeenCalledTimes(1);
    expect(renderer.status()).toEqual({ activeKind: 'canvas2d', state: 'degraded', reason: 'WebGL context lost' });
    expect(warnings).toHaveBeenCalledWith('[graph] WebGL renderer fell back to Canvas2D', { reason: 'WebGL context lost' });
    renderer.render(f);
    expect(fallback.log.at(-1)).toBe('render');
    warnings.mockRestore();
  });

  it('falls back with the shader log when a shader does not compile, releasing the partial GL objects (M1)', () => {
    const warnings = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { gl, calls } = fakeGl({ compileFails: true });
    const fallback = fallbackStub();
    const renderer = new WebGLRenderer({ context: () => gl, parseColor, fallback: () => fallback.renderer });
    renderer.mount(document.createElement('div'));
    expect(renderer.usingWebGL).toBe(false);
    expect(renderer.status().state).toBe('degraded');
    expect(renderer.status().reason).toContain('vertex shader did not compile: ERROR: 0:3: syntax error');
    expect(ops(calls, 'deleteProgram')).toHaveLength(1);
    expect(ops(calls, 'deleteBuffer')).toHaveLength(1);
    expect(ops(calls, 'loseContext')).toHaveLength(1);
    expect(warnings).toHaveBeenCalled();
    warnings.mockRestore();
  });

  it('checks a growing vertex upload for GL_OUT_OF_MEMORY and draws that frame with the fallback (M1)', () => {
    const warnings = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { gl, calls } = fakeGl({ outOfMemory: true });
    const fallback = fallbackStub();
    const renderer = new WebGLRenderer({ context: () => gl, parseColor, fallback: () => fallback.renderer });
    renderer.mount(document.createElement('div'));
    renderer.setSize(40, 20, 1);
    const f = frame([series(windowOf([0, 1, 2, 3]), [0, 2, 1, 1])]);
    renderer.render(f);
    expect(ops(calls, 'getError')).toHaveLength(1);
    expect(renderer.status().reason).toMatch(/^GL_OUT_OF_MEMORY/);
    expect(fallback.frames).toEqual([f]);
    warnings.mockRestore();
  });

  it('checks for upload errors only when the upload grows (getError is a synchronous GPU round trip)', () => {
    const { renderer, calls } = mounted();
    const f = frame([series(windowOf([0, 1, 2, 3]), [0, 2, 1, 1])]);
    renderer.render(f);
    renderer.render(f);
    renderer.render(f);
    expect(ops(calls, 'getError')).toHaveLength(1);
    expect(renderer.status().state).toBe('ok');
  });

  it('releases its context and layer on destroy', () => {
    const { renderer, host, calls } = mounted();
    renderer.destroy();
    expect(host.querySelector('canvas')).toBeNull();
    expect(ops(calls, 'deleteBuffer')).toHaveLength(1);
    expect(ops(calls, 'loseContext')).toHaveLength(1);
    expect(renderer.usingWebGL).toBe(false);
  });
});

describe('canvasAcceptsColor (silent-failure H5)', () => {
  /** A context that ignores invalid colours, as a real canvas does. */
  function ctx(valid: (css: string) => boolean): { fillStyle: string } {
    let value = '#000000';
    return {
      get fillStyle() {
        return value;
      },
      set fillStyle(css: string) {
        if (valid(css)) value = css;
      },
    };
  }

  it('tells an ignored (invalid or empty) colour from an accepted one, including the sentinels themselves', () => {
    const c = ctx((css) => css.startsWith('#'));
    expect(canvasAcceptsColor(c, '#ff0000')).toBe(true);
    expect(canvasAcceptsColor(c, '#010203')).toBe(true);
    expect(canvasAcceptsColor(c, '#030201')).toBe(true);
    expect(canvasAcceptsColor(c, '')).toBe(false);
    expect(canvasAcceptsColor(c, 'not-a-colour')).toBe(false);
  });

  it('never parses an invalid colour as opaque black', () => {
    expect(browserColorParser('')).toBeNull();
  });
});
