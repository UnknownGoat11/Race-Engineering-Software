/**
 * Two-step confirmation dialog for actions that stop a stream, a recording or an engineer link
 * (owner rule OR-5, REDESIGN.md C20, W4.1): "End session" → Modal: "End for everyone? <consequence> [Cancel] [Confirm]".
 *
 * - Renders a native accessible Modal dialog (role="alertdialog", aria-modal="true", inert background).
 * - The consequence is always stated and is the confirm button's accessible description (`aria-describedby`).
 * - Focus moves to Confirm when the step opens and returns to the trigger when it closes.
 * - Escape or Cancel closes it; it also auto-cancels after `timeoutMs` (default 5 s).
 * - While `onConfirm`'s promise runs, Confirm is disabled and busy. A rejection goes to `onError`, or, without
 *   one, to the global error reporter (`reportError`), never swallowed silently.
 */
import { useCallback, useEffect, useId, useRef, useState, type JSX } from 'react';
import { COPY } from '../../lib/labels';
import { Modal } from './Modal';
import '../../styles/modal.css';

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
  const mounted = useRef(true);
  const consequenceId = useId();
  const promptId = useId();
  const reasonId = useId();

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const restoreFocusRef = useRef(false);

  const close = useCallback((focusTrigger: boolean) => {
    restoreFocusRef.current = focusTrigger;
    setPhase('idle');
    if (focusTrigger) {
      triggerRef.current?.focus();
    }
  }, []);

  useEffect(() => {
    if (phase === 'idle' && restoreFocusRef.current) {
      restoreFocusRef.current = false;
      triggerRef.current?.focus();
    }
  }, [phase]);

  // Auto-cancel while waiting for the confirm click.
  useEffect(() => {
    if (phase !== 'confirming') return undefined;
    const timer = setTimeout(() => {
      close(false);
    }, timeoutMs);
    return () => {
      clearTimeout(timer);
    };
  }, [phase, timeoutMs, close]);

  // A trigger disabled while the step is open closes it.
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

  const buttonClass = ['btn', size === 'sm' ? 'btn-sm' : '', variant === 'danger' ? 'btn-danger' : '']
    .filter((c) => c !== '')
    .join(' ');

  const pending = phase === 'pending';
  const isConfirming = phase !== 'idle';

  return (
    <>
      <span className="confirm-action" data-testid={testId}>
        <button
          ref={triggerRef}
          type="button"
          className={buttonClass}
          disabled={disabled || isConfirming}
          aria-describedby={disabled && disabledReason !== undefined ? reasonId : undefined}
          onClick={() => {
            triggerRef.current?.focus();
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

      <Modal
        isOpen={isConfirming}
        onClose={() => {
          close(true);
        }}
        role="alertdialog"
        ariaLabelledBy={promptId}
        ariaDescribedBy={consequenceId}
        initialFocusRef={confirmRef}
        size="sm"
        testId={`${testId}-modal`}
      >
        <div className="confirm-modal-body" role="group" aria-label={prompt}>
          <div className="card-header confirm-modal-header">
            <span id={promptId} className="card-header-title confirm-action-prompt">
              {prompt}
            </span>
          </div>
          <div className="card-body confirm-modal-content">
            <p id={consequenceId} className="confirm-action-consequence">
              {consequence}
            </p>
            <div className="confirm-modal-actions">
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
            </div>
          </div>
        </div>
      </Modal>
    </>
  );
}
