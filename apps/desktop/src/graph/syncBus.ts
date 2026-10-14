/**
 * Graph synchronization (spec §81): every graph of a group shares the x view (zoom, pan, LIVE) and the cursor x,
 * so moving the cursor on one graph moves the vertical cursor on all of them, and their readouts refer to the
 * same sample. A plain observable outside React: graphs publish with their own id and ignore their own events.
 */
import type { ViewState } from './view';

/** What changed in a sync group. */
export type SyncEvent =
  | { readonly kind: 'cursor'; readonly source: string; readonly x: number | null }
  | { readonly kind: 'view'; readonly source: string; readonly view: ViewState };

/** A sync group listener. */
export type SyncListener = (event: SyncEvent) => void;

/** Shared cursor and view of a group of graphs. Create one per group and pass it to each graph. */
export class GraphSyncGroup {
  #cursor: number | null = null;
  #view: ViewState | null = null;
  readonly #listeners = new Set<SyncListener>();

  /** Current shared cursor x (null when no graph of the group is hovered). */
  get cursor(): number | null {
    return this.#cursor;
  }

  /** Current shared view, or null until a graph published one. */
  get view(): ViewState | null {
    return this.#view;
  }

  /** Publishes the cursor x of graph `source`; listeners are told only when it changed. */
  setCursor(source: string, x: number | null): void {
    const next = x !== null && Number.isFinite(x) ? x : null;
    if (next === this.#cursor) return;
    this.#cursor = next;
    this.#emit({ kind: 'cursor', source, x: next });
  }

  /** Publishes the view of graph `source`; listeners are told only when it changed. */
  setView(source: string, view: ViewState): void {
    const current = this.#view;
    if (current !== null && current.follow === view.follow && current.span === view.span && current.end === view.end) {
      return;
    }
    this.#view = view;
    this.#emit({ kind: 'view', source, view });
  }

  /** Adds a listener; returns the function that removes it. */
  subscribe(listener: SyncListener): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /** Number of subscribed graphs (diagnostics and tests). */
  get size(): number {
    return this.#listeners.size;
  }

  #emit(event: SyncEvent): void {
    for (const listener of [...this.#listeners]) listener(event);
  }
}
