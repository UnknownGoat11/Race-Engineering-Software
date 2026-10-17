/**
 * WebGL series renderer (D-011 "WebGL fallback if uPlot cannot hold 60 FPS"; the default, see DEFAULT_RENDERER).
 *
 * Why: the bench (bench/, 2026-10-14 review) showed that Canvas2D and uPlot are both limited by the GPU process,
 * not by JavaScript: Chromium rasterises every Canvas2D path on the GPU process's main thread (Skia geometry
 * generation for ~77 000 line vertices per frame), and when that falls behind the canvas rate limiter blocks the
 * page (frames alternate 7 ms / 28 ms on a 144 Hz display). Here the same decimated vertices
 * (`tracePolyline`, per device-pixel column min/max, spec §277) are uploaded once per frame into one vertex buffer
 * and drawn as 1-device-pixel line strips, which costs the GPU process almost nothing.
 *
 * Drawing rules are exactly the Canvas2D renderer's: the same decimation, lines broken at NaN and time gaps
 * (§219), Discrete channels as steps (§125), no smoothing (§318), clipped to the plot, a lone sample drawn as a
 * dot, references dashed (6/4 CSS px, measured along the line). Lines are 1 device px wide and antialiased by the
 * context's multisampling.
 *
 * Robustness: when WebGL is unavailable (no context, e.g. jsdom or a blocked GPU), its shaders do not compile or
 * link, the context is lost or a vertex upload runs out of GPU memory, the renderer switches permanently to a
 * `Canvas2DRenderer` in the same layer, logs the reason (with the shader / program log) and reports it through
 * `status()` / `onStatusChange`, so the engine redraws from a fresh read and shows that the fallback is active.
 * It never replays an earlier frame (its window arrays may have been reused by the source since).
 */
import { Canvas2DRenderer } from './canvas2dRenderer';
import { sizeCanvas } from './canvasContext';
import { tracePolyline, type PathSink } from './decimate';
import { REFERENCE_DASH_CSS, okStatus, type RenderFrame, type RendererStatus, type SeriesRenderer } from './renderer';
import { visibleRows } from './sample';
import { linearAffine } from './scale';

/** Floats per vertex: x, y (device px from the layer's top-left) and the distance along the run (device px). */
export const FLOATS_PER_VERTEX = 3;
const INITIAL_VERTICES = 8192;
const INITIAL_RUNS = 64;

/**
 * Collects polyline commands as line-strip runs in one growable vertex array (a `PathSink`, so it is fed by
 * `tracePolyline`). Each `moveTo` starts a run; a run whose vertices all coincide is a dot.
 */
export class StripBuilder implements PathSink {
  vertices = new Float32Array(INITIAL_VERTICES * FLOATS_PER_VERTEX);
  /** Vertices written. */
  count = 0;
  runFirst = new Int32Array(INITIAL_RUNS);
  runCount = new Int32Array(INITIAL_RUNS);
  /** Length of each run in device px (0 = a dot). */
  runLength = new Float32Array(INITIAL_RUNS);
  /** Runs written. */
  runs = 0;
  #lastX = 0;
  #lastY = 0;
  #dist = 0;

  /** Forgets every vertex and run (the arrays are kept). */
  reset(): void {
    this.count = 0;
    this.runs = 0;
  }

  moveTo(x: number, y: number): void {
    if (this.runs === this.runFirst.length) this.#growRuns();
    const r = this.runs;
    this.runFirst[r] = this.count;
    this.runCount[r] = 0;
    this.runLength[r] = 0;
    this.runs = r + 1;
    this.#dist = 0;
    this.#push(x, y);
  }

  lineTo(x: number, y: number): void {
    if (this.runs === 0) {
      this.moveTo(x, y);
      return;
    }
    const dx = x - this.#lastX;
    const dy = y - this.#lastY;
    // Not Math.hypot, which is slow in V8: this runs for every vertex of every repaint (bench CPU profile,
    // 2026-10-14: `traceLines` self time incl. the inlined sink 1.67 s → 1.21 s per 8 s run).
    this.#dist += Math.sqrt(dx * dx + dy * dy);
    this.runLength[this.runs - 1] = this.#dist;
    this.#push(x, y);
  }

  #push(x: number, y: number): void {
    if ((this.count + 1) * FLOATS_PER_VERTEX > this.vertices.length) {
      const next = new Float32Array(this.vertices.length * 2);
      next.set(this.vertices);
      this.vertices = next;
    }
    const o = this.count * FLOATS_PER_VERTEX;
    this.vertices[o] = x;
    this.vertices[o + 1] = y;
    this.vertices[o + 2] = this.#dist;
    this.count += 1;
    this.runCount[this.runs - 1] = (this.runCount[this.runs - 1] ?? 0) + 1;
    this.#lastX = x;
    this.#lastY = y;
  }

  #growRuns(): void {
    const size = this.runFirst.length * 2;
    const first = new Int32Array(size);
    const count = new Int32Array(size);
    const length = new Float32Array(size);
    first.set(this.runFirst);
    count.set(this.runCount);
    length.set(this.runLength);
    this.runFirst = first;
    this.runCount = count;
    this.runLength = length;
  }
}

/** RGBA in 0…1, not premultiplied. */
export type Rgba = readonly [number, number, number, number];

/** Resolves a CSS colour to RGBA (injectable for tests; the default paints it into a 1×1 canvas). */
export type ColorParser = (css: string) => Rgba | null;

/** Colour of a series whose CSS colour cannot be parsed (logged; the engine validates its tokens before). */
const FALLBACK_RGBA: Rgba = [1, 1, 1, 1];
const CHANNEL_MAX = 255;
/** Two different sentinels: an invalid colour assignment is ignored, so `fillStyle` keeps the sentinel. */
const SENTINEL_A = '#010203';
const SENTINEL_B = '#030201';

/** True when assigning `css` to a 2D context's fillStyle is accepted (an invalid colour is silently ignored). */
export function canvasAcceptsColor(ctx: { fillStyle: string | CanvasGradient | CanvasPattern }, css: string): boolean {
  ctx.fillStyle = SENTINEL_A;
  ctx.fillStyle = css;
  if (ctx.fillStyle !== SENTINEL_A) return true;
  // `css` may be the sentinel itself.
  ctx.fillStyle = SENTINEL_B;
  ctx.fillStyle = css;
  return ctx.fillStyle !== SENTINEL_B;
}

/**
 * The browser's CSS colour parser: any colour syntax the page can use (hex, rgb(), oklch() …). Returns null for an
 * empty or invalid colour (never opaque black, silent-failure H5).
 */
export const browserColorParser: ColorParser = (css) => {
  try {
    const canvas = document.createElement('canvas');
    canvas.width = 1;
    canvas.height = 1;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    if (ctx === null || css.trim() === '' || !canvasAcceptsColor(ctx, css)) return null;
    ctx.fillRect(0, 0, 1, 1);
    const [r = 0, g = 0, b = 0, a = 0] = ctx.getImageData(0, 0, 1, 1).data;
    return [r / CHANNEL_MAX, g / CHANNEL_MAX, b / CHANNEL_MAX, a / CHANNEL_MAX];
  } catch {
    return null;
  }
};

const VERTEX_SHADER = `
attribute vec3 a_vertex;
uniform vec2 u_size;
varying float v_dist;
void main() {
  v_dist = a_vertex.z;
  vec2 clip = a_vertex.xy / u_size * 2.0 - 1.0;
  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);
  gl_PointSize = 1.0;
}`;

const FRAGMENT_SHADER = `
precision highp float;
uniform vec4 u_color;
uniform vec2 u_dash;
varying float v_dist;
void main() {
  if (u_dash.y > 0.0 && mod(v_dist, u_dash.y) >= u_dash.x) discard;
  gl_FragColor = u_color;
}`;

/** The subset of WEBGL_multi_draw used here. */
interface MultiDraw {
  multiDrawArraysWEBGL(
    mode: number,
    firsts: Int32Array,
    firstsOffset: number,
    counts: Int32Array,
    countsOffset: number,
    drawcount: number,
  ): void;
}

interface GlState {
  readonly gl: WebGLRenderingContext;
  readonly program: WebGLProgram;
  readonly buffer: WebGLBuffer;
  readonly aVertex: number;
  readonly uSize: WebGLUniformLocation | null;
  readonly uColor: WebGLUniformLocation | null;
  readonly uDash: WebGLUniformLocation | null;
  readonly multiDraw: MultiDraw | null;
}

/** A shader, or why it could not be compiled (with the driver's info log). */
function compile(gl: WebGLRenderingContext, type: number, source: string, name: string): WebGLShader | string {
  const shader = gl.createShader(type);
  if (shader === null) return `${name} shader could not be created`;
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (gl.getShaderParameter(shader, gl.COMPILE_STATUS) !== true) {
    const log = gl.getShaderInfoLog(shader) ?? '';
    gl.deleteShader(shader);
    return `${name} shader did not compile: ${log}`;
  }
  return shader;
}

/** Frees everything a failed `initGl` created and the context itself (browsers cap live WebGL contexts). */
function releaseGl(
  gl: WebGLRenderingContext,
  vs: WebGLShader | string,
  fs: WebGLShader | string,
  program: WebGLProgram | null,
  buffer: WebGLBuffer | null,
): void {
  if (typeof vs !== 'string') gl.deleteShader(vs);
  if (typeof fs !== 'string') gl.deleteShader(fs);
  if (program !== null) gl.deleteProgram(program);
  if (buffer !== null) gl.deleteBuffer(buffer);
  (gl.getExtension('WEBGL_lose_context') as { loseContext(): void } | null)?.loseContext();
}

/** The GL objects of the renderer, or why they could not be created (shader / link log included). */
function initGl(gl: WebGLRenderingContext): GlState | string {
  const vs = compile(gl, gl.VERTEX_SHADER, VERTEX_SHADER, 'vertex');
  const fs = compile(gl, gl.FRAGMENT_SHADER, FRAGMENT_SHADER, 'fragment');
  // Typed non-null by lib.dom, but null after a context loss.
  const program = gl.createProgram() as WebGLProgram | null;
  const buffer = gl.createBuffer() as WebGLBuffer | null;
  const fail = (reason: string): string => {
    releaseGl(gl, vs, fs, program, buffer);
    return reason;
  };
  if (typeof vs === 'string') return fail(vs);
  if (typeof fs === 'string') return fail(fs);
  if (program === null || buffer === null) return fail('WebGL program or buffer could not be created');
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);
  if (gl.getProgramParameter(program, gl.LINK_STATUS) !== true) {
    return fail(`WebGL program did not link: ${gl.getProgramInfoLog(program) ?? ''}`);
  }
  gl.useProgram(program);
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  const aVertex = gl.getAttribLocation(program, 'a_vertex');
  if (aVertex < 0) return fail('WebGL attribute a_vertex not found');
  gl.enableVertexAttribArray(aVertex);
  gl.vertexAttribPointer(aVertex, FLOATS_PER_VERTEX, gl.FLOAT, false, 0, 0);
  gl.enable(gl.BLEND);
  // Premultiplied output (the context default): colour × alpha is done on the CPU.
  gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  return {
    gl,
    program,
    buffer,
    aVertex,
    uSize: gl.getUniformLocation(program, 'u_size'),
    uColor: gl.getUniformLocation(program, 'u_color'),
    uDash: gl.getUniformLocation(program, 'u_dash'),
    multiDraw: gl.getExtension('WEBGL_multi_draw') as MultiDraw | null,
  };
}

/** Options of the WebGL renderer. */
export interface WebGLRendererOptions {
  /** Creates the WebGL context (tests inject a fake); default `canvas.getContext('webgl', …)`. */
  readonly context?: (canvas: HTMLCanvasElement) => WebGLRenderingContext | null;
  /** CSS colour parser (default: the browser's). */
  readonly parseColor?: ColorParser;
  /** Renderer used when WebGL is unavailable or lost (default: a Canvas2D renderer). */
  readonly fallback?: () => SeriesRenderer;
}

const defaultContext = (canvas: HTMLCanvasElement): WebGLRenderingContext | null => {
  try {
    return canvas.getContext('webgl', {
      antialias: true,
      alpha: true,
      premultipliedAlpha: true,
      depth: false,
      stencil: false,
      preserveDrawingBuffer: false,
    });
  } catch {
    return null;
  }
};

/** One series' share of the vertex buffer. */
interface SeriesSlice {
  firstRun: number;
  endRun: number;
  color: Rgba;
  dashed: boolean;
}

/** WebGL implementation of `SeriesRenderer` (see the module comment). */
export class WebGLRenderer implements SeriesRenderer {
  readonly kind = 'webgl' as const;
  readonly #canvas: HTMLCanvasElement;
  readonly #makeContext: (canvas: HTMLCanvasElement) => WebGLRenderingContext | null;
  readonly #parseColor: ColorParser;
  readonly #makeFallback: () => SeriesRenderer;
  readonly #colors = new Map<string, Rgba>();
  readonly #builder = new StripBuilder();
  #slices: SeriesSlice[] = [];
  #firsts = new Int32Array(INITIAL_RUNS);
  #counts = new Int32Array(INITIAL_RUNS);
  #state: GlState | null = null;
  #fallback: SeriesRenderer | null = null;
  #host: HTMLElement | null = null;
  #size: readonly [number, number, number] = [0, 0, 1];
  /** Largest vertex upload so far (bytes): a larger one may run out of GPU memory, so it is checked. */
  #uploadedBytes = 0;
  #status: RendererStatus = okStatus('webgl');
  readonly #statusListeners = new Set<() => void>();
  /** Colours already reported as unparseable (logged once each). */
  readonly #badColors = new Set<string>();
  readonly #onLost = (event: Event): void => {
    event.preventDefault();
    this.#useFallback('WebGL context lost');
  };

  constructor(options: WebGLRendererOptions = {}) {
    this.#canvas = document.createElement('canvas');
    this.#canvas.className = 'telemetry-graph__layer';
    this.#makeContext = options.context ?? defaultContext;
    this.#parseColor = options.parseColor ?? browserColorParser;
    this.#makeFallback = options.fallback ?? (() => new Canvas2DRenderer());
  }

  /** True while drawing with WebGL (false after falling back to Canvas2D). */
  get usingWebGL(): boolean {
    return this.#state !== null;
  }

  status(): RendererStatus {
    return this.#status;
  }

  onStatusChange(listener: () => void): () => void {
    this.#statusListeners.add(listener);
    return () => {
      this.#statusListeners.delete(listener);
    };
  }

  mount(host: HTMLElement): void {
    this.#host = host;
    host.appendChild(this.#canvas);
    const gl = this.#makeContext(this.#canvas);
    const state = gl === null ? 'WebGL context unavailable' : initGl(gl);
    if (typeof state === 'string') {
      this.#useFallback(state);
      return;
    }
    this.#state = state;
    this.#canvas.addEventListener('webglcontextlost', this.#onLost);
  }

  setSize(widthCss: number, heightCss: number, dpr: number): void {
    this.#size = [widthCss, heightCss, dpr];
    if (this.#fallback !== null) this.#fallback.setSize(widthCss, heightCss, dpr);
    else sizeCanvas(this.#canvas, widthCss, heightCss, dpr);
  }

  render(frame: RenderFrame): void {
    if (this.#fallback !== null) {
      this.#fallback.render(frame);
      return;
    }
    const state = this.#state;
    if (state === null) return;
    const { gl } = state;
    const width = this.#canvas.width;
    const height = this.#canvas.height;
    gl.viewport(0, 0, width, height);
    gl.disable(gl.SCISSOR_TEST);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    const dpr = this.#size[2];
    const { plot, xRange } = frame;
    const left = plot.left * dpr;
    const top = plot.top * dpr;
    const w = plot.width * dpr;
    const h = plot.height * dpr;
    if (!(w > 0) || !(h > 0) || !(xRange.max > xRange.min) || frame.series.length === 0) return;

    const builder = this.#builder;
    builder.reset();
    const slices = this.#slices;
    slices.length = 0;
    const x = linearAffine(xRange.min, xRange.max, left, left + w);
    for (const s of frame.series) {
      if (!(s.yRange.max > s.yRange.min)) continue;
      const { start, end } = visibleRows(s.window, xRange);
      if (end <= start) continue;
      const firstRun = builder.runs;
      tracePolyline(
        {
          xs: s.window.xs,
          ts: s.window.ts,
          ys: s.ys,
          start,
          end,
          x,
          // Larger values are higher on screen: min maps to the plot's bottom edge.
          y: linearAffine(s.yRange.min, s.yRange.max, top + h, top),
          maxGapT: frame.maxGapT,
          step: s.step,
        },
        builder,
      );
      if (builder.runs > firstRun) slices.push({ firstRun, endRun: builder.runs, color: this.#color(s.color), dashed: s.dashed });
    }
    if (builder.count === 0) return;

    gl.useProgram(state.program);
    gl.bindBuffer(gl.ARRAY_BUFFER, state.buffer);
    const upload = builder.vertices.subarray(0, builder.count * FLOATS_PER_VERTEX);
    gl.bufferData(gl.ARRAY_BUFFER, upload, gl.DYNAMIC_DRAW);
    if (upload.byteLength > this.#uploadedBytes) {
      // Only a larger upload can newly run out of memory; getError is a synchronous GPU round trip, so it is not
      // called every frame. A failed upload would leave the series blank while readouts show values.
      this.#uploadedBytes = upload.byteLength;
      if (gl.getError() === gl.OUT_OF_MEMORY) {
        this.#useFallback(`GL_OUT_OF_MEMORY uploading ${String(upload.byteLength)} bytes`);
        (this.#fallback as SeriesRenderer | null)?.render(frame);
        return;
      }
    }
    gl.vertexAttribPointer(state.aVertex, FLOATS_PER_VERTEX, gl.FLOAT, false, 0, 0);
    gl.uniform2f(state.uSize, width, height);
    gl.enable(gl.SCISSOR_TEST);
    // Scissor rectangles count from the bottom-left corner and are whole device pixels (rounded like the edges
    // of Canvas2D's antialiased clip, so neither renderer draws a column the other leaves empty).
    const sx = Math.round(left);
    const sy = Math.round(height - (top + h));
    gl.scissor(sx, sy, Math.round(left + w) - sx, Math.round(height - top) - sy);
    const [dashOn, dashOff] = REFERENCE_DASH_CSS;
    for (const slice of slices) {
      const [r, g, b, a] = slice.color;
      gl.uniform4f(state.uColor, r * a, g * a, b * a, a);
      gl.uniform2f(state.uDash, slice.dashed ? dashOn * dpr : 0, slice.dashed ? (dashOn + dashOff) * dpr : 0);
      this.#drawRuns(state, slice, true);
      this.#drawRuns(state, slice, false);
    }
  }

  destroy(): void {
    this.#canvas.removeEventListener('webglcontextlost', this.#onLost);
    this.#canvas.remove();
    this.#fallback?.destroy();
    this.#fallback = null;
    const state = this.#state;
    if (state !== null) {
      state.gl.deleteBuffer(state.buffer);
      state.gl.deleteProgram(state.program);
      // Free the context now instead of waiting for garbage collection (browsers cap live WebGL contexts).
      (state.gl.getExtension('WEBGL_lose_context') as { loseContext(): void } | null)?.loseContext();
    }
    this.#state = null;
    this.#host = null;
    this.#statusListeners.clear();
  }

  /** Draws a slice's line runs (`strips`) or its dots (runs of zero length) with as few calls as possible. */
  #drawRuns(state: GlState, slice: SeriesSlice, strips: boolean): void {
    const { gl } = state;
    const builder = this.#builder;
    if (this.#firsts.length < slice.endRun - slice.firstRun) {
      this.#firsts = new Int32Array(builder.runFirst.length);
      this.#counts = new Int32Array(builder.runFirst.length);
    }
    const firsts = this.#firsts;
    const counts = this.#counts;
    let n = 0;
    for (let r = slice.firstRun; r < slice.endRun; r++) {
      const isDot = (builder.runLength[r] ?? 0) === 0;
      if (isDot === strips) continue;
      firsts[n] = builder.runFirst[r] ?? 0;
      // A dot is its first vertex alone.
      counts[n] = isDot ? 1 : (builder.runCount[r] ?? 0);
      n += 1;
    }
    if (n === 0) return;
    const mode = strips ? gl.LINE_STRIP : gl.POINTS;
    if (state.multiDraw !== null) {
      state.multiDraw.multiDrawArraysWEBGL(mode, firsts, 0, counts, 0, n);
      return;
    }
    for (let k = 0; k < n; k++) gl.drawArrays(mode, firsts[k] ?? 0, counts[k] ?? 0);
  }

  #color(css: string): Rgba {
    let c = this.#colors.get(css);
    if (c === undefined) {
      const parsed = this.#parseColor(css);
      if (parsed === null && !this.#badColors.has(css)) {
        this.#badColors.add(css);
        console.error('[graph] WebGL renderer cannot parse a series colour; drawing it white', { color: css });
      }
      c = parsed ?? FALLBACK_RGBA;
      this.#colors.set(css, c);
    }
    return c;
  }

  #setStatus(status: RendererStatus): void {
    this.#status = status;
    for (const listener of [...this.#statusListeners]) listener();
  }

  /**
   * Switches to the Canvas2D fallback for good (logged with `reason`) and reports it; the engine redraws from a
   * fresh read. The fallback's own status (e.g. no 2D context either) is passed on.
   */
  #useFallback(reason: string): void {
    if (this.#fallback !== null) return;
    console.warn('[graph] WebGL renderer fell back to Canvas2D', { reason });
    this.#canvas.removeEventListener('webglcontextlost', this.#onLost);
    this.#canvas.remove();
    const state = this.#state;
    this.#state = null;
    if (state !== null) {
      state.gl.deleteBuffer(state.buffer);
      state.gl.deleteProgram(state.program);
    }
    const fallback = this.#makeFallback();
    this.#fallback = fallback;
    if (this.#host !== null) fallback.mount(this.#host);
    const [w, h, dpr] = this.#size;
    fallback.setSize(w, h, dpr);
    const fromFallback = (): RendererStatus => {
      const inner = fallback.status?.() ?? okStatus(fallback.kind);
      return inner.state === 'failed'
        ? { activeKind: inner.activeKind, state: 'failed', reason: `${reason}; fallback: ${inner.reason ?? 'failed'}` }
        : { activeKind: inner.activeKind, state: 'degraded', reason };
    };
    fallback.onStatusChange?.(() => {
      this.#setStatus(fromFallback());
    });
    this.#setStatus(fromFallback());
  }
}
