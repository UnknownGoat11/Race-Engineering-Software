import { describe, expect, it, vi } from 'vitest';
import { GraphSyncGroup, type SyncEvent } from './syncBus';
import { liveView } from './view';

describe('GraphSyncGroup', () => {
  it('broadcasts the cursor of one graph to every graph in the group', () => {
    const group = new GraphSyncGroup();
    const a: SyncEvent[] = [];
    const b: SyncEvent[] = [];
    group.subscribe((e) => a.push(e));
    group.subscribe((e) => b.push(e));
    group.setCursor('graph-a', 12.5);
    expect(a).toEqual([{ kind: 'cursor', source: 'graph-a', x: 12.5 }]);
    expect(b).toEqual(a);
    expect(group.cursor).toBe(12.5);
  });

  it('does not notify when the cursor did not change', () => {
    const group = new GraphSyncGroup();
    const listener = vi.fn();
    group.subscribe(listener);
    group.setCursor('a', 1);
    group.setCursor('b', 1);
    group.setCursor('a', null);
    group.setCursor('a', Number.NaN);
    expect(listener).toHaveBeenCalledTimes(2);
    expect(group.cursor).toBeNull();
  });

  it('shares the view (zoom, pan, LIVE) and skips identical views', () => {
    const group = new GraphSyncGroup();
    const listener = vi.fn();
    group.subscribe(listener);
    group.setView('a', liveView(30));
    group.setView('b', liveView(30));
    group.setView('b', { follow: false, span: 10, end: 100 });
    expect(listener).toHaveBeenCalledTimes(2);
    expect(listener).toHaveBeenLastCalledWith({ kind: 'view', source: 'b', view: { follow: false, span: 10, end: 100 } });
    expect(group.view).toEqual({ follow: false, span: 10, end: 100 });
  });

  it('stops notifying after unsubscribe', () => {
    const group = new GraphSyncGroup();
    const listener = vi.fn();
    const unsubscribe = group.subscribe(listener);
    expect(group.size).toBe(1);
    unsubscribe();
    group.setCursor('a', 3);
    expect(listener).not.toHaveBeenCalled();
    expect(group.size).toBe(0);
  });

  it('lets a listener unsubscribe while being notified', () => {
    const group = new GraphSyncGroup();
    const second = vi.fn();
    const unsubscribeFirst = group.subscribe(() => {
      unsubscribeFirst();
    });
    group.subscribe(second);
    group.setCursor('a', 1);
    expect(second).toHaveBeenCalledTimes(1);
    expect(group.size).toBe(1);
  });
});
