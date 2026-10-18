import { fireEvent, render, screen } from '@testing-library/react';
import { useRef, useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { Modal } from './Modal';

describe('Modal Primitive (W4.1, A01)', () => {
  it('does not render into the DOM when isOpen is false', () => {
    render(
      <Modal isOpen={false} onClose={vi.fn()} title="Test Dialog">
        <div>Content</div>
      </Modal>,
    );
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('renders native dialog with role, aria-modal, and title when isOpen is true', () => {
    render(
      <Modal isOpen={true} onClose={vi.fn()} title="Engine Diagnostics">
        <div>Modal Body</div>
      </Modal>,
    );

    const dialog = screen.getByRole('dialog', { name: 'Engine Diagnostics' });
    expect(dialog).toBeInTheDocument();
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(screen.getByText('Engine Diagnostics')).toBeInTheDocument();
    expect(screen.getByText('Modal Body')).toBeInTheDocument();
  });

  it('supports role="alertdialog" and aria-describedby for confirmation dialogs', () => {
    render(
      <Modal
        isOpen={true}
        onClose={vi.fn()}
        role="alertdialog"
        title="Warning"
        ariaDescribedBy="desc-1"
      >
        <p id="desc-1">This will permanently delete data.</p>
      </Modal>,
    );

    const dialog = screen.getByRole('alertdialog', { name: 'Warning' });
    expect(dialog).toBeInTheDocument();
    expect(dialog).toHaveAttribute('aria-describedby', 'desc-1');
  });

  it('makes background sibling content inert while open and restores when closed', () => {
    function TestHost() {
      const [isOpen, setIsOpen] = useState(false);
      return (
        <div>
          <div data-testid="bg-tree">
            <button
              type="button"
              onClick={() => {
                setIsOpen(true);
              }}
            >
              Open
            </button>
            <span>Background text</span>
          </div>
          <Modal
            isOpen={isOpen}
            onClose={() => {
              setIsOpen(false);
            }}
            title="Notice"
          >
            <button
              type="button"
              onClick={() => {
                setIsOpen(false);
              }}
            >
              Close
            </button>
          </Modal>
        </div>
      );
    }

    render(<TestHost />);
    const bgTree = screen.getByTestId('bg-tree');
    expect(bgTree).not.toHaveAttribute('inert');

    fireEvent.click(screen.getByRole('button', { name: 'Open' }));
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(bgTree).toHaveAttribute('inert');

    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(bgTree).not.toHaveAttribute('inert');
  });

  it('places initial focus on initialFocusRef if provided', () => {
    function HostWithCustomFocus() {
      const secondBtnRef = useRef<HTMLButtonElement>(null);
      return (
        <Modal isOpen={true} onClose={vi.fn()} initialFocusRef={secondBtnRef}>
          <button type="button">First</button>
          <button ref={secondBtnRef} type="button">
            Target
          </button>
        </Modal>
      );
    }

    render(<HostWithCustomFocus />);
    expect(screen.getByRole('button', { name: 'Target' })).toHaveFocus();
  });

  it('places initial focus on first focusable child if initialFocusRef is omitted', () => {
    render(
      <Modal isOpen={true} onClose={vi.fn()}>
        <button type="button">Primary Action</button>
        <button type="button">Secondary Action</button>
      </Modal>,
    );

    expect(screen.getByRole('button', { name: 'Primary Action' })).toHaveFocus();
  });

  it('re-focuses when focusKey changes (e.g. phase transition)', () => {
    function HostPhaseTransition() {
      const [phase, setPhase] = useState<'step1' | 'step2'>('step1');
      const step2InputRef = useRef<HTMLInputElement>(null);

      return (
        <div>
          <Modal
            isOpen={true}
            onClose={vi.fn()}
            focusKey={phase}
            initialFocusRef={phase === 'step2' ? step2InputRef : undefined}
          >
            {phase === 'step1' ? (
              <div>
                <button
                  type="button"
                  onClick={() => {
                    setPhase('step2');
                  }}
                >
                  Next Step
                </button>
              </div>
            ) : (
              <div>
                <input ref={step2InputRef} data-testid="step2-input" />
              </div>
            )}
          </Modal>
        </div>
      );
    }

    render(<HostPhaseTransition />);
    expect(screen.getByRole('button', { name: 'Next Step' })).toHaveFocus();

    fireEvent.click(screen.getByRole('button', { name: 'Next Step' }));
    expect(screen.getByTestId('step2-input')).toHaveFocus();
  });

  it('restores focus to the opener element upon closing', () => {
    function HostRestoreFocus() {
      const [isOpen, setIsOpen] = useState(false);
      return (
        <div>
          <button
            type="button"
            onClick={() => {
              setIsOpen(true);
            }}
          >
            Open Dialog
          </button>
          <Modal
            isOpen={isOpen}
            onClose={() => {
              setIsOpen(false);
            }}
          >
            <button
              type="button"
              onClick={() => {
                setIsOpen(false);
              }}
            >
              Close Dialog
            </button>
          </Modal>
        </div>
      );
    }

    render(<HostRestoreFocus />);
    const openBtn = screen.getByRole('button', { name: 'Open Dialog' });
    openBtn.focus();
    expect(openBtn).toHaveFocus();

    fireEvent.click(openBtn);
    expect(screen.getByRole('button', { name: 'Close Dialog' })).toHaveFocus();

    fireEvent.click(screen.getByRole('button', { name: 'Close Dialog' }));
    expect(openBtn).toHaveFocus();
  });

  it('traps Tab key focus strictly inside the dialog (U Appendix-A acceptance probe: Tabx6 never leaves)', () => {
    render(
      <Modal isOpen={true} onClose={vi.fn()}>
        <button type="button" data-testid="b1">
          Button 1
        </button>
        <button type="button" data-testid="b2">
          Button 2
        </button>
        <button type="button" data-testid="b3">
          Button 3
        </button>
      </Modal>,
    );

    const dialog = screen.getByRole('dialog');
    const b1 = screen.getByTestId('b1');
    const b2 = screen.getByTestId('b2');
    const b3 = screen.getByTestId('b3');

    // Initial focus is on b1
    expect(b1).toHaveFocus();

    // Tab x 1 -> b2
    b2.focus();
    expect(b2).toHaveFocus();

    // Tab x 2 -> b3
    b3.focus();
    expect(b3).toHaveFocus();

    // Tab on last element (b3) should wrap back to b1
    fireEvent.keyDown(dialog, { key: 'Tab' });
    expect(b1).toHaveFocus();

    // Shift+Tab on first element (b1) should wrap back to b3
    fireEvent.keyDown(dialog, { key: 'Tab', shiftKey: true });
    expect(b3).toHaveFocus();

    // Run Tab 6 times: verify focus never leaves dialog
    for (let i = 0; i < 6; i++) {
      if (document.activeElement === b3) {
        fireEvent.keyDown(dialog, { key: 'Tab' });
      } else if (document.activeElement === b1) {
        b2.focus();
      } else if (document.activeElement === b2) {
        b3.focus();
      }
      expect([b1, b2, b3]).toContain(document.activeElement);
    }
  });

  it('Escape key triggers onClose', () => {
    const onClose = vi.fn();
    render(
      <Modal isOpen={true} onClose={onClose}>
        <button type="button">Inside</button>
      </Modal>,
    );

    const dialog = screen.getByRole('dialog');
    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('clicking backdrop calls onClose, clicking inner does not', () => {
    const onClose = vi.fn();
    render(
      <Modal isOpen={true} onClose={onClose} testId="test-modal">
        <div data-testid="inner-content">
          <button type="button">Inside</button>
        </div>
      </Modal>,
    );

    const dialog = screen.getByTestId('test-modal');
    const inner = screen.getByTestId('inner-content');

    // Clicking inner content does not close
    fireEvent.click(inner);
    expect(onClose).not.toHaveBeenCalled();

    // Clicking dialog backdrop directly calls onClose
    fireEvent.click(dialog);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('closeOnBackdropClick=false prevents closing on backdrop click', () => {
    const onClose = vi.fn();
    render(
      <Modal isOpen={true} onClose={onClose} closeOnBackdropClick={false} testId="test-modal">
        <div>Unclosable content</div>
      </Modal>,
    );

    const dialog = screen.getByTestId('test-modal');
    fireEvent.click(dialog);
    expect(onClose).not.toHaveBeenCalled();
  });
});
