/**
 * Screen reader live region component and Provider (REDESIGN.md §4 W4.2, A02, A11, EN14).
 * Permanently mounted in DOM with .sr-only styling, presenting polite and assertive live regions.
 */
import type { JSX, ReactNode } from 'react';
import {
  AnnouncerContext,
  useAnnouncer,
  useAnnouncerContext,
  type UseAnnouncerOptions,
} from '../../hooks/useAnnouncer';

export interface AnnouncerProps {
  /** Explicit polite message (overrides context if provided) */
  readonly politeMessage?: string | null | undefined;
  /** Explicit assertive message (overrides context if provided) */
  readonly assertiveMessage?: string | null | undefined;
  readonly testId?: string | undefined;
}

export function Announcer(props?: AnnouncerProps): JSX.Element {
  const context = useAnnouncerContext();
  const polite = props?.politeMessage ?? context?.politeMessage ?? '';
  const assertive = props?.assertiveMessage ?? context?.assertiveMessage ?? '';
  const testId = props?.testId ?? 'screen-announcer';

  return (
    <div className="sr-only" data-testid={testId}>
      <div
        role={polite ? 'status' : undefined}
        aria-live="polite"
        aria-atomic="true"
        data-testid={`${testId}-polite`}
      >
        {polite}
      </div>
      <div
        role={assertive ? 'alert' : undefined}
        aria-live="assertive"
        aria-atomic="true"
        data-testid={`${testId}-assertive`}
      >
        {assertive}
      </div>
    </div>
  );
}

export interface AnnouncerProviderProps extends UseAnnouncerOptions {
  readonly children: ReactNode;
}

export function AnnouncerProvider({
  children,
  defaultThrottleMs,
  defaultClearDelayMs,
}: AnnouncerProviderProps): JSX.Element {
  const value = useAnnouncer({ defaultThrottleMs, defaultClearDelayMs });

  return (
    <AnnouncerContext.Provider value={value}>
      <Announcer />
      {children}
    </AnnouncerContext.Provider>
  );
}
