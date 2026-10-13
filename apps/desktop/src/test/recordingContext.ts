// Test support: a fake 2D canvas context that records every call and the style in effect at each one
// (jsdom has no canvas implementation). Text is measured as a fixed width per character.
import type { Canvas2DLike, CanvasPaint } from '../graph/canvasContext';

/** One recorded drawing call. */
export interface RecordedCall {
  readonly op: string;
  readonly args: readonly (number | string | readonly number[])[];
  /** strokeStyle (for stroke) or fillStyle (for fill operations and text) at the time of the call. */
  readonly style: string;
  /** Line dash in effect at the time of the call. */
  readonly dash: readonly number[];
}

/** Width of one character in `measureText`, px. */
export const RECORDING_CHAR_WIDTH = 6;

function paintName(p: CanvasPaint): string {
  return typeof p === 'string' ? p : 'pattern';
}

/** Recording context (see the file comment). */
export class RecordingContext implements Canvas2DLike {
  lineWidth = 1;
  strokeStyle: CanvasPaint = '#000';
  fillStyle: CanvasPaint = '#000';
  lineJoin: CanvasLineJoin = 'miter';
  lineCap: CanvasLineCap = 'butt';
  font = '10px sans-serif';
  textAlign: CanvasTextAlign = 'start';
  textBaseline: CanvasTextBaseline = 'alphabetic';
  readonly calls: RecordedCall[] = [];
  #dash: readonly number[] = [];

  #push(op: string, args: RecordedCall['args'], style: CanvasPaint): void {
    this.calls.push({ op, args, style: paintName(style), dash: this.#dash });
  }

  save(): void {
    this.#push('save', [], this.strokeStyle);
  }
  restore(): void {
    this.#push('restore', [], this.strokeStyle);
  }
  setTransform(a: number, b: number, c: number, d: number, e: number, f: number): void {
    this.#push('setTransform', [a, b, c, d, e, f], this.strokeStyle);
  }
  clearRect(x: number, y: number, w: number, h: number): void {
    this.#push('clearRect', [x, y, w, h], this.fillStyle);
  }
  fillRect(x: number, y: number, w: number, h: number): void {
    this.#push('fillRect', [x, y, w, h], this.fillStyle);
  }
  beginPath(): void {
    this.#push('beginPath', [], this.strokeStyle);
  }
  rect(x: number, y: number, w: number, h: number): void {
    this.#push('rect', [x, y, w, h], this.strokeStyle);
  }
  clip(): void {
    this.#push('clip', [], this.strokeStyle);
  }
  moveTo(x: number, y: number): void {
    this.#push('moveTo', [x, y], this.strokeStyle);
  }
  lineTo(x: number, y: number): void {
    this.#push('lineTo', [x, y], this.strokeStyle);
  }
  stroke(): void {
    this.#push('stroke', [], this.strokeStyle);
  }
  setLineDash(segments: number[]): void {
    this.#dash = [...segments];
  }
  fillText(text: string, x: number, y: number): void {
    this.#push('fillText', [text, x, y], this.fillStyle);
  }
  measureText(text: string): { readonly width: number } {
    return { width: text.length * RECORDING_CHAR_WIDTH };
  }

  /** Recorded calls of one operation. */
  ops(op: string): RecordedCall[] {
    return this.calls.filter((c) => c.op === op);
  }

  /** Text drawn with fillText, in order. */
  texts(): string[] {
    return this.ops('fillText').map((c) => String(c.args[0]));
  }

  /** Forgets every recorded call. */
  clear(): void {
    this.calls.length = 0;
  }
}
