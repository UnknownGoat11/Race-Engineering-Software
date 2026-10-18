/**
 * Accessible live announcer hook & context (REDESIGN.md §4 W4.2, A02, A11, EN14).
 * Manages polite and assertive announcements for screen reader users:
 * - Emits on state *transitions* only (never on high-frequency 4 Hz data ticks)
 * - Throttles duplicate announcements within a configurable window
 * - Provides polite announcements by default, assertive for critical errors
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';

export type AnnouncePriority = 'polite' | 'assertive';

export interface AnnounceOptions {
  readonly priority?: AnnouncePriority | undefined;
  readonly throttleMs?: number | undefined;
  readonly clearDelayMs?: number | undefined;
}

export interface AnnouncerContextValue {
  readonly politeMessage: string;
  readonly assertiveMessage: string;
  readonly announce: (
    message: string,
    options?: AnnouncePriority | AnnounceOptions,
  ) => void;
  readonly clearAnnouncements: () => void;
}

const DEFAULT_THROTTLE_MS = 1000;
const DEFAULT_CLEAR_DELAY_MS = 5000;

export const AnnouncerContext = createContext<AnnouncerContextValue | null>(null);

export function useAnnouncerContext(): AnnouncerContextValue | null {
  return useContext(AnnouncerContext);
}

export interface UseAnnouncerOptions {
  readonly defaultThrottleMs?: number | undefined;
  readonly defaultClearDelayMs?: number | undefined;
}

/**
 * Hook to announce messages to screen readers.
 * Uses AnnouncerContext when available, or manages its own local state.
 */
export function useAnnouncer(options?: UseAnnouncerOptions): AnnouncerContextValue {
  const context = useContext(AnnouncerContext);
  const defaultThrottle = options?.defaultThrottleMs ?? DEFAULT_THROTTLE_MS;
  const defaultClearDelay = options?.defaultClearDelayMs ?? DEFAULT_CLEAR_DELAY_MS;

  const [politeMessage, setPoliteMessage] = useState<string>('');
  const [assertiveMessage, setAssertiveMessage] = useState<string>('');

  const lastAnnounceRef = useRef<{ message: string; timestamp: number }>({
    message: '',
    timestamp: 0,
  });
  const politeClearTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const assertiveClearTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (politeClearTimerRef.current) clearTimeout(politeClearTimerRef.current);
      if (assertiveClearTimerRef.current) clearTimeout(assertiveClearTimerRef.current);
    };
  }, []);

  const clearAnnouncements = useCallback(() => {
    setPoliteMessage('');
    setAssertiveMessage('');
    if (politeClearTimerRef.current) {
      clearTimeout(politeClearTimerRef.current);
      politeClearTimerRef.current = null;
    }
    if (assertiveClearTimerRef.current) {
      clearTimeout(assertiveClearTimerRef.current);
      assertiveClearTimerRef.current = null;
    }
  }, []);

  const localAnnounce = useCallback(
    (message: string, opts?: AnnouncePriority | AnnounceOptions) => {
      const text = message.trim();
      if (!text) return;

      const priority: AnnouncePriority =
        typeof opts === 'string' ? opts : (opts?.priority ?? 'polite');
      const throttleMs =
        typeof opts === 'object' && opts.throttleMs !== undefined
          ? opts.throttleMs
          : defaultThrottle;
      const clearDelayMs =
        typeof opts === 'object' && opts.clearDelayMs !== undefined
          ? opts.clearDelayMs
          : defaultClearDelay;

      const now = Date.now();
      if (
        lastAnnounceRef.current.message === text &&
        now - lastAnnounceRef.current.timestamp < throttleMs
      ) {
        return;
      }

      lastAnnounceRef.current = { message: text, timestamp: now };

      if (priority === 'assertive') {
        setAssertiveMessage(text);
        if (assertiveClearTimerRef.current) clearTimeout(assertiveClearTimerRef.current);
        assertiveClearTimerRef.current = setTimeout(() => {
          setAssertiveMessage('');
          assertiveClearTimerRef.current = null;
        }, clearDelayMs);
      } else {
        setPoliteMessage(text);
        if (politeClearTimerRef.current) clearTimeout(politeClearTimerRef.current);
        politeClearTimerRef.current = setTimeout(() => {
          setPoliteMessage('');
          politeClearTimerRef.current = null;
        }, clearDelayMs);
      }
    },
    [defaultThrottle, defaultClearDelay],
  );

  const localValue = useMemo<AnnouncerContextValue>(
    () => ({
      politeMessage,
      assertiveMessage,
      announce: localAnnounce,
      clearAnnouncements,
    }),
    [politeMessage, assertiveMessage, localAnnounce, clearAnnouncements],
  );

  if (context) {
    return context;
  }

  return localValue;
}
