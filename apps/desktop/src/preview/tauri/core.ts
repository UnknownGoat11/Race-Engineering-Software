/**
 * PREVIEW HARNESS ONLY. Stand-in for `@tauri-apps/api/core`, swapped in by the alias in preview/vite.config.ts
 * (never by the app build). Implements the subset the app uses — `isTauri`, `invoke`, `Channel` — on top of the
 * scenario-driven fake backend.
 */
import { fakeBackend } from '../backend';

/** The preview pretends to run inside Tauri so the app takes its normal (non-fallback) code paths. */
export function isTauri(): boolean {
  return true;
}

export function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  return fakeBackend.invoke(cmd, args) as Promise<T>;
}

let nextChannelId = 1;

/** Minimal `Channel`: the fake backend calls `onmessage` with each raw UiBatch buffer. */
export class Channel<T = unknown> {
  readonly id: number = nextChannelId++;
  onmessage: (response: T) => void;

  constructor(onmessage?: (response: T) => void) {
    this.onmessage = onmessage ?? (() => undefined);
  }

  toJSON(): string {
    return `__CHANNEL__:${this.id}`;
  }
}
