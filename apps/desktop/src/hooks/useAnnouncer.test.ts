import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useAnnouncer } from './useAnnouncer';

describe('useAnnouncer', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('initializes with empty messages', () => {
    const { result } = renderHook(() => useAnnouncer());
    expect(result.current.politeMessage).toBe('');
    expect(result.current.assertiveMessage).toBe('');
  });

  it('sets polite message by default', () => {
    const { result } = renderHook(() => useAnnouncer());

    act(() => {
      result.current.announce('Streaming started');
    });

    expect(result.current.politeMessage).toBe('Streaming started');
    expect(result.current.assertiveMessage).toBe('');
  });

  it('sets assertive message when priority is assertive', () => {
    const { result } = renderHook(() => useAnnouncer());

    act(() => {
      result.current.announce('Recording failed', 'assertive');
    });

    expect(result.current.politeMessage).toBe('');
    expect(result.current.assertiveMessage).toBe('Recording failed');
  });

  it('throttles duplicate messages within the throttle window', () => {
    const { result } = renderHook(() => useAnnouncer({ defaultThrottleMs: 1000 }));

    act(() => {
      result.current.announce('Engineer joined');
    });
    expect(result.current.politeMessage).toBe('Engineer joined');

    // Simulate rapid repeated 4 Hz tick attempt with same message
    act(() => {
      vi.advanceTimersByTime(250);
      result.current.announce('Engineer joined');
    });
    expect(result.current.politeMessage).toBe('Engineer joined');

    // After throttle window expires, announcing again succeeds
    act(() => {
      vi.advanceTimersByTime(800);
      result.current.clearAnnouncements();
    });
    expect(result.current.politeMessage).toBe('');

    act(() => {
      result.current.announce('Engineer joined');
    });
    expect(result.current.politeMessage).toBe('Engineer joined');
  });

  it('allows different messages immediately without throttle delay', () => {
    const { result } = renderHook(() => useAnnouncer({ defaultThrottleMs: 1000 }));

    act(() => {
      result.current.announce('Connected to live stream');
    });
    expect(result.current.politeMessage).toBe('Connected to live stream');

    act(() => {
      vi.advanceTimersByTime(100);
      result.current.announce('Driver stream is stale');
    });
    expect(result.current.politeMessage).toBe('Driver stream is stale');
  });

  it('automatically clears message after clearDelayMs', () => {
    const { result } = renderHook(() =>
      useAnnouncer({ defaultClearDelayMs: 3000 }),
    );

    act(() => {
      result.current.announce('Refresh requested');
    });
    expect(result.current.politeMessage).toBe('Refresh requested');

    act(() => {
      vi.advanceTimersByTime(2999);
    });
    expect(result.current.politeMessage).toBe('Refresh requested');

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(result.current.politeMessage).toBe('');
  });

  it('ignores empty and whitespace-only messages', () => {
    const { result } = renderHook(() => useAnnouncer());

    act(() => {
      result.current.announce('');
      result.current.announce('   ');
    });

    expect(result.current.politeMessage).toBe('');
    expect(result.current.assertiveMessage).toBe('');
  });

  it('clearAnnouncements manually resets active messages and timers', () => {
    const { result } = renderHook(() => useAnnouncer());

    act(() => {
      result.current.announce('Test message');
    });
    expect(result.current.politeMessage).toBe('Test message');

    act(() => {
      result.current.clearAnnouncements();
    });
    expect(result.current.politeMessage).toBe('');
  });
});
