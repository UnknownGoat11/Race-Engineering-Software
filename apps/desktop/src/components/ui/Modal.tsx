/**
 * Accessible Modal Dialog Component (REDESIGN.md W4.1, A01).
 *
 * Implements native HTML <dialog> with showModal():
 * - Native browser top-layer backdrop and inert background
 * - Accessible labelling via aria-labelledby and role="dialog" | "alertdialog"
 * - Tab key focus trapping cycling strictly inside the dialog
 * - Native Escape handling with cancellation
 * - Initial focus placement on designated element or first focusable child
 * - Automatic focus restoration to the opener element upon closing
 */
import {
  useEffect,
  useId,
  useRef,
  type JSX,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from 'react';
import '../../styles/modal.css';

// JSDOM fallback for showModal and close in test environments
if (typeof HTMLDialogElement !== 'undefined') {
  if (typeof HTMLDialogElement.prototype.showModal !== 'function') {
    HTMLDialogElement.prototype.showModal = function () {
      this.setAttribute('open', '');
    };
  }
  if (typeof HTMLDialogElement.prototype.close !== 'function') {
    HTMLDialogElement.prototype.close = function () {
      this.removeAttribute('open');
    };
  }
}

const FOCUSABLE_SELECTOR =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * Traverses ancestors of the dialog and sets inert on all sibling subtrees so that
 * the background content is inert in both JSDOM and browsers.
 */
function setBackgroundInert(dialog: HTMLDialogElement): () => void {
  const inertElements: HTMLElement[] = [];
  let current: HTMLElement | null = dialog;

  while (current && current !== dialog.ownerDocument.body) {
    const parentNode: HTMLElement | null = current.parentElement;
    if (parentNode) {
      for (const child of Array.from(parentNode.children)) {
        if (child !== current && child instanceof HTMLElement && !child.contains(dialog)) {
          if (!child.hasAttribute('inert')) {
            child.setAttribute('inert', '');
            inertElements.push(child);
          }
        }
      }
    }
    current = parentNode;
  }

  return () => {
    for (const el of inertElements) {
      el.removeAttribute('inert');
    }
  };
}

export interface ModalProps {
  readonly isOpen: boolean;
  readonly onClose: () => void;
  readonly title?: string | undefined;
  readonly ariaLabel?: string | undefined;
  readonly ariaLabelledBy?: string | undefined;
  readonly ariaDescribedBy?: string | undefined;
  readonly role?: 'dialog' | 'alertdialog' | undefined;
  readonly size?: 'sm' | 'md' | 'lg' | 'xl' | undefined;
  readonly initialFocusRef?: { readonly current: HTMLElement | null } | undefined;
  /** Key to trigger re-focusing when inner phase or content changes (e.g. phase change). */
  readonly focusKey?: unknown;
  readonly closeOnBackdropClick?: boolean | undefined;
  readonly testId?: string | undefined;
  readonly className?: string | undefined;
  readonly children: ReactNode;
}

export function Modal({
  isOpen,
  onClose,
  title,
  ariaLabel,
  ariaLabelledBy,
  ariaDescribedBy,
  role = 'dialog',
  size = 'md',
  initialFocusRef,
  focusKey,
  closeOnBackdropClick = true,
  testId = 'modal',
  className = '',
  children,
}: ModalProps): JSX.Element | null {
  const dialogRef = useRef<HTMLDialogElement | null>(null);
  const openerRef = useRef<HTMLElement | null>(null);
  const autoTitleId = useId();
  const effectiveTitleId = ariaLabelledBy ?? (title ? autoTitleId : undefined);

  // Dialog open/close lifecycle and opener restoration
  useEffect(() => {
    if (!isOpen) return;

    openerRef.current = document.activeElement as HTMLElement | null;
    const dialog = dialogRef.current;
    if (!dialog) return;

    if (typeof dialog.showModal === 'function') {
      if (!dialog.open) {
        dialog.showModal();
      }
    } else {
      dialog.setAttribute('open', '');
    }

    const restoreInert = setBackgroundInert(dialog);

    return () => {
      restoreInert();
      if (typeof dialog.close === 'function') {
        if (dialog.open) {
          dialog.close();
        }
      } else {
        dialog.removeAttribute('open');
      }

      if (openerRef.current && document.contains(openerRef.current)) {
        openerRef.current.focus();
        openerRef.current = null;
      }
    };
  }, [isOpen]);

  // Initial focus and re-focus on phase/focusKey changes
  useEffect(() => {
    if (!isOpen) return;

    const dialog = dialogRef.current;
    if (!dialog) return;

    const applyFocus = () => {
      if (initialFocusRef?.current) {
        initialFocusRef.current.focus();
        return;
      }
      const focusables = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
        (el) => !el.hasAttribute('disabled') && el.getAttribute('aria-hidden') !== 'true',
      );
      if (focusables.length > 0) {
        focusables[0]?.focus();
      } else {
        dialog.focus();
      }
    };

    applyFocus();
    const timer = setTimeout(applyFocus, 0);
    return () => {
      clearTimeout(timer);
    };
  }, [isOpen, focusKey, initialFocusRef]);

  const handleKeyDown = (e: KeyboardEvent<HTMLDialogElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }

    if (e.key === 'Tab') {
      const dialog = dialogRef.current;
      if (!dialog) return;

      const allFocusable = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
        (el) => !el.hasAttribute('disabled') && el.getAttribute('aria-hidden') !== 'true',
      );

      if (allFocusable.length === 0) {
        e.preventDefault();
        dialog.focus();
        return;
      }

      const first = allFocusable[0];
      const last = allFocusable[allFocusable.length - 1];

      if (e.shiftKey) {
        if (document.activeElement === first || !dialog.contains(document.activeElement)) {
          e.preventDefault();
          last?.focus();
        }
      } else {
        if (document.activeElement === last || !dialog.contains(document.activeElement)) {
          e.preventDefault();
          first?.focus();
        }
      }
    }
  };

  const handleBackdropClick = (e: MouseEvent<HTMLDialogElement>) => {
    if (!closeOnBackdropClick) return;
    if (e.target === dialogRef.current) {
      onClose();
    }
  };

  if (!isOpen) return null;

  return (
    <dialog
      ref={dialogRef}
      className={`modal-dialog modal-dialog--${size} ${className}`}
      role={role}
      aria-modal="true"
      aria-labelledby={effectiveTitleId}
      aria-label={ariaLabel}
      aria-describedby={ariaDescribedBy}
      onKeyDown={handleKeyDown}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      onClick={handleBackdropClick}
      data-testid={testId}
      tabIndex={-1}
    >
      <div
        className="modal-dialog-inner"
        onClick={(e) => {
          e.stopPropagation();
        }}
      >
        {title && (
          <div className="card-header modal-header">
            <span id={autoTitleId} className="card-header-title">
              {title}
            </span>
          </div>
        )}
        {children}
      </div>
    </dialog>
  );
}
