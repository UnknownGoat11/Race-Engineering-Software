/**
 * PREVIEW HARNESS ONLY. Stand-in for `@tauri-apps/api/event`, swapped in by the alias in
 * preview/vite.config.ts (never by the app build). `listen` subscribes to the fake backend's core events.
 */
import { fakeBackend } from '../backend';

export type UnlistenFn = () => void;

export interface Event<T> {
  readonly event: string;
  readonly id: number;
  readonly payload: T;
}

let nextEventId = 1;

export function listen<T>(event: string, handler: (event: Event<T>) => void): Promise<UnlistenFn> {
  const unlisten = fakeBackend.listen(event, (payload) => {
    handler({ event, id: nextEventId++, payload: payload as T });
  });
  return Promise.resolve(unlisten);
}
