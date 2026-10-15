/**
 * The subset of `CanvasRenderingContext2D` the graph draws with. A real 2D context satisfies it; tests pass a
 * recording fake (src/test/recordingContext.ts) because jsdom has no canvas implementation.
 */

/** Paint style of a stroke or fill. */
export type CanvasPaint = string | CanvasGradient | CanvasPattern;

/** 2D drawing context subset (see the module comment). */
export interface Canvas2DLike {
  lineWidth: number;
  strokeStyle: CanvasPaint;
  fillStyle: CanvasPaint;
  lineJoin: CanvasLineJoin;
  lineCap: CanvasLineCap;
  font: string;
  textAlign: CanvasTextAlign;
  textBaseline: CanvasTextBaseline;
  save(): void;
  restore(): void;
  setTransform(a: number, b: number, c: number, d: number, e: number, f: number): void;
  clearRect(x: number, y: number, w: number, h: number): void;
  fillRect(x: number, y: number, w: number, h: number): void;
  beginPath(): void;
  rect(x: number, y: number, w: number, h: number): void;
  clip(): void;
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
  stroke(): void;
  setLineDash(segments: number[]): void;
  fillText(text: string, x: number, y: number): void;
  measureText(text: string): { readonly width: number };
}

/** Obtains a 2D context for a canvas; injectable so tests can record drawing. */
export type ContextFactory = (canvas: HTMLCanvasElement) => Canvas2DLike | null;

/**
 * The browser's 2D context of `canvas`, or null where none is available (jsdom; a canvas too large at a high
 * device pixel ratio; a context limit). A missing context is logged with the canvas size (silent-failure M4); the
 * engine then shows the graph as unavailable instead of a plot without its axes and STALE markers. Tests inject a
 * recording context instead.
 */
export const browserContext: ContextFactory = (canvas) => {
  let ctx: CanvasRenderingContext2D | null = null;
  let error: unknown = null;
  try {
    ctx = canvas.getContext('2d');
  } catch (e: unknown) {
    error = e;
  }
  if (ctx === null) {
    console.error('[graph] 2D canvas context unavailable', { width: canvas.width, height: canvas.height }, error);
  }
  return ctx;
};

/**
 * Sizes `canvas` to `widthCss × heightCss` CSS pixels at `dpr` device pixels per CSS pixel. Returns true when the
 * backing store changed (which also clears it).
 */
export function sizeCanvas(canvas: HTMLCanvasElement, widthCss: number, heightCss: number, dpr: number): boolean {
  const w = Math.max(0, Math.round(widthCss * dpr));
  const h = Math.max(0, Math.round(heightCss * dpr));
  canvas.style.width = `${widthCss}px`;
  canvas.style.height = `${heightCss}px`;
  if (canvas.width === w && canvas.height === h) return false;
  canvas.width = w;
  canvas.height = h;
  return true;
}
