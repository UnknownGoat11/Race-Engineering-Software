/**
 * Inline two-step confirmation for actions that stop a stream, a recording or an engineer link (owner rule OR-5,
 * REDESIGN.md C20): "End session" → "End for everyone? <consequence> [Confirm] [Cancel]".
 *
 * - The consequence is always stated and is the confirm button's accessible description (`aria-describedby`).
 * - Focus moves to Confirm when the step opens and back to the trigger when it closes without confirming.
 * - Escape or Cancel closes it; it also cancels itself after `timeoutMs` (default 5 s) so a stray first click
 *   cannot be confirmed much later.
 * - While `onConfirm`'s promise runs, Confirm is disabled and busy. A rejection goes to `onError`, or, without
 *   one, to the global error reporter (`reportError`, forwarded to the app log), never swallowed silently.
 *
 * W4.1 swaps the internals to the `<Modal>` dialog without changing this API.
 */
import { useCallback, useEffect, useId, useRef, useState, type JSX, type KeyboardEvent } from 'react';
import { COPY } from '../../lib/labels';

/** Default auto-cancel delay of the confirm step. */
export const CONFIRM_TIMEOUT_MS = 5_000;

export interface ConfirmActionProps {
  /** Trigger button text, e.g. "End session". */
  readonly label: string;
  /** Question shown in the confirm step, e.g. "End for everyone?". Defaults to `${label}?`. */
  readonly prompt?: string;
  /** What will happen, in one sentence. Required: a confirm without a stated consequence is not allowed. */
  readonly consequence: string;
  readonly confirmLabel?: string;
  readonly cancelLabel?: string;
  /** The action. A returned promise is awaited (Confirm stays busy until it settles). */
  readonly onConfirm: () => unknown;
  readonly onError?: (error: unknown) => void;
  readonly disabled?: boolean;
  /** Why the trigger is disabled (its accessible description). */
  readonly disabledReason?: string;
  readonly variant?: 'danger' | 'default';
  readonly size?: 'sm' | 'md';
  readonly timeoutMs?: number;
  readonly testId?: string;
}

type Phase = 'idle' | 'confirming' | 'pending';

function reportUnhandled(error: unknown): void {
  if (typeof globalThis.reportError === 'function') {
    globalThis.reportError(error);
  } else {
    // No reporter (very old runtime): surface it as an unhandled rejection rather than dropping it.
    void Promise.reject(error instanceof Error ? error : new Error(String(error)));
  }
}

export function ConfirmAction(props: ConfirmActionProps): JSX.Element {
  const {
    label,
    consequence,
    onConfirm,
    onError,
    disabled = false,
    disabledReason,
    variant = 'danger',
    size = 'sm',
    timeoutMs = CONFIRM_TIMEOUT_MS,
    testId = 'confirm-action',
  } = props;
  const prompt = props.prompt ?? `${label}?`;
  const confirmLabel = props.confirmLabel ?? COPY.confirm;
  const cancelLabel = props.cancelLabel ?? COPY.cancel;

  const [phase, setPhase] = useState<Phase>('idle');
  const triggerRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(false);
  const mounted = useRef(true);
  const consequenceId = useId();
  const reasonId = useId();

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const close = useCallback((focusTrigger: boolean) => {
    restoreFocus.current = focusTrigger;
    setPhase('idle');
  }, []);

  // Focus management on phase changes.
  useEffect(() => {
    if (phase === 'confirming') {
      confirmRef.current?.focus();
    } else if (phase === 'idle' && restoreFocus.current) {
      restoreFocus.current = false;
      triggerRef.current?.focus();
    }
  }, [phase]);

  // Auto-cancel while waiting for the second click.
  useEffect(() => {
    if (phase !== 'confirming') return undefined;
    const timer = setTimeout(() => {
      close(false);
    }, timeoutMs);
    return () => {
      clearTimeout(timer);
    };
  }, [phase, timeoutMs, close]);

  // A trigger disabled while the step is open closes it (e.g. the session ended meanwhile). Adjusting state
  // during render (not in an effect) avoids rendering the stale confirm step even once.
  if (disabled && phase === 'confirming') {
    setPhase('idle');
  }

  const handleConfirm = (): void => {
    setPhase('pending');
    let result: unknown;
    try {
      result = onConfirm();
    } catch (error: unknown) {
      setPhase('idle');
      (onError ?? reportUnhandled)(error);
      return;
    }
    Promise.resolve(result)
      .catch((error: unknown) => {
        (onError ?? reportUnhandled)(error);
      })
      .finally(() => {
        if (mounted.current) setPhase('idle');
      });
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    if (event.key === 'Escape' && phase === 'confirming') {
      event.stopPropagation();
      close(true);
    }
  };

  const buttonClass = ['btn', size === 'sm' ? 'btn-sm' : '', variant === 'danger' ? 'btn-danger' : '']
    .filter((c) => c !== '')
    .join(' ');

  if (phase === 'idle') {
    return (
      <span className="confirm-action" data-testid={testId}>
        <button
          ref={triggerRef}
          type="button"
          className={buttonClass}
          disabled={disabled}
          aria-describedby={disabled && disabledReason !== undefined ? reasonId : undefined}
          onClick={() => {
            setPhase('confirming');
          }}
          data-testid={`${testId}-trigger`}
        >
          {label}
        </button>
        {disabled && disabledReason !== undefined && (
          <span id={reasonId} className="confirm-action-reason">
            {disabledReason}
          </span>
        )}
      </span>
    );
  }

  const pending = phase === 'pending';
  return (
    <div
      className="confirm-action confirm-action-open"
      role="group"
      aria-label={prompt}
      onKeyDown={handleKeyDown}
      data-testid={testId}
    >
      <span className="confirm-action-prompt">{prompt}</span>
      <span id={consequenceId} className="confirm-action-consequence">
        {consequence}
      </span>
      <button
        ref={confirmRef}
        type="button"
        className={buttonClass}
        disabled={pending}
        aria-busy={pending}
        aria-describedby={consequenceId}
        onClick={handleConfirm}
        data-testid={`${testId}-confirm`}
      >
        {confirmLabel}
      </button>
      <button
        type="button"
        className={size === 'sm' ? 'btn btn-sm' : 'btn'}
        disabled={pending}
        onClick={() => {
          close(true);
        }}
        data-testid={`${testId}-cancel`}
      >
        {cancelLabel}
      </button>
    </div>
  );
}
