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

/**
 * Thrown by `subscribe` when a graph's x semantics differ from the group's (silent-failure M6): a cursor in
 * seconds applied as metres on another graph would show real values at the wrong position.
 */
export class SyncGroupMismatchError extends Error {
  constructor(
    readonly groupAxis: string,
    readonly memberAxis: string,
  ) {
    super(`sync group shares x as '${groupAxis}', this graph uses '${memberAxis}'`);
    this.name = 'SyncGroupMismatchError';
  }
}

/** Shared cursor and view of a group of graphs. Create one per group and pass it to each graph. */
export class GraphSyncGroup {
  #cursor: number | null = null;
  #view: ViewState | null = null;
  readonly #listeners = new Set<SyncListener>();
  /** X semantics of the members (e.g. "time|live"), fixed by the first member while the group has members. */
  #axis: string | null = null;

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

  /** X semantics the group's members share, or null while it has none (or none declared one). */
  get axis(): string | null {
    return this.#axis;
  }

  /**
   * Adds a listener; returns the function that removes it. `axis` names the member's x semantics (the engine
   * passes "<xMode>|<origin>"); a member whose axis differs from the group's is refused with a
   * `SyncGroupMismatchError`. The group forgets its axis when its last member leaves.
   */
  subscribe(listener: SyncListener, axis?: string): () => void {
    if (axis !== undefined) {
      if (this.#axis !== null && this.#axis !== axis) throw new SyncGroupMismatchError(this.#axis, axis);
      this.#axis = axis;
    }
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
      if (this.#listeners.size === 0) {
        this.#axis = null;
        this.#view = null;
        this.#cursor = null;
      }
    };
  }

  /** Number of subscribed graphs (diagnostics and tests). */
  get size(): number {
    return this.#listeners.size;
  }

  /**
   * Delivers `event` to every listener. One failing listener must not keep the cursor / view from the other graphs
   * or throw into the publishing graph's input handler (silent-failure L4): its error is logged with context.
   */
  #emit(event: SyncEvent): void {
    for (const listener of [...this.#listeners]) {
      try {
        listener(event);
      } catch (error: unknown) {
        console.error('[graph] sync group listener failed', { event: event.kind, source: event.source, axis: this.#axis }, error);
      }
    }
  }
}
