import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CONFIRM_TIMEOUT_MS, ConfirmAction } from './ConfirmAction';

const CONSEQUENCE = 'Every engineer is disconnected and needs a new code to rejoin.';

function setup(props: Partial<Parameters<typeof ConfirmAction>[0]> = {}) {
  const onConfirm = vi.fn();
  render(
    <ConfirmAction
      label="End session"
      prompt="End for everyone?"
      consequence={CONSEQUENCE}
      onConfirm={onConfirm}
      {...props}
    />,
  );
  return { onConfirm: (props.onConfirm as ReturnType<typeof vi.fn> | undefined) ?? onConfirm };
}

afterEach(() => {
  vi.useRealTimers();
});

describe('ConfirmAction', () => {
  it('one click never runs the action; it asks and states the consequence', () => {
    const { onConfirm } = setup();
    fireEvent.click(screen.getByRole('button', { name: 'End session' }));
    expect(onConfirm).not.toHaveBeenCalled();
    expect(screen.getByRole('group', { name: 'End for everyone?' })).toBeInTheDocument();
    expect(screen.getByText(CONSEQUENCE)).toBeInTheDocument();
    const confirm = screen.getByRole('button', { name: 'Confirm' });
    expect(confirm).toHaveAccessibleDescription(CONSEQUENCE);
    expect(confirm).toHaveFocus();
  });

  it('confirm runs the action once and returns to idle', async () => {
    const { onConfirm } = setup();
    fireEvent.click(screen.getByRole('button', { name: 'End session' }));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Confirm' }));
      await Promise.resolve();
    });
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'End session' })).toBeInTheDocument();
  });

  it('cancel and Escape close without acting and return focus', () => {
    const { onConfirm } = setup();
    fireEvent.click(screen.getByRole('button', { name: 'End session' }));
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onConfirm).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'End session' })).toHaveFocus();

    fireEvent.click(screen.getByRole('button', { name: 'End session' }));
    fireEvent.keyDown(screen.getByRole('button', { name: 'Confirm' }), { key: 'Escape' });
    expect(onConfirm).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'End session' })).toHaveFocus();
  });

  it('auto-cancels after 5 s', () => {
    vi.useFakeTimers();
    const { onConfirm } = setup();
    fireEvent.click(screen.getByRole('button', { name: 'End session' }));
    act(() => {
      vi.advanceTimersByTime(CONFIRM_TIMEOUT_MS - 1);
    });
    expect(screen.getByRole('button', { name: 'Confirm' })).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(screen.queryByRole('button', { name: 'Confirm' })).not.toBeInTheDocument();
    expect(onConfirm).not.toHaveBeenCalled();
    expect(CONFIRM_TIMEOUT_MS).toBe(5_000);
  });

  it('is busy while the action runs and cannot be confirmed twice', async () => {
    let resolve!: () => void;
    const onConfirm = vi.fn(() => new Promise<void>((r) => (resolve = r)));
    setup({ onConfirm });
    fireEvent.click(screen.getByRole('button', { name: 'End session' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }));
    const confirm = screen.getByRole('button', { name: 'Confirm' });
    expect(confirm).toBeDisabled();
    expect(confirm).toHaveAttribute('aria-busy', 'true');
    fireEvent.click(confirm);
    expect(onConfirm).toHaveBeenCalledTimes(1);
    await act(async () => {
      resolve();
      await Promise.resolve();
    });
    expect(screen.getByRole('button', { name: 'End session' })).toBeInTheDocument();
  });

  it('a failing action goes to onError (async and sync)', async () => {
    const onError = vi.fn();
    const err = new Error('nope');
    setup({ onConfirm: vi.fn().mockRejectedValue(err), onError });
    fireEvent.click(screen.getByRole('button', { name: 'End session' }));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Confirm' }));
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onError).toHaveBeenCalledWith(err);
  });

  it('a sync throw without onError is reported, not swallowed', () => {
    const report = vi.fn();
    const original = globalThis.reportError;
    globalThis.reportError = report;
    try {
      const err = new Error('sync');
      setup({
        onConfirm: () => {
          throw err;
        },
      });
      fireEvent.click(screen.getByRole('button', { name: 'End session' }));
      fireEvent.click(screen.getByRole('button', { name: 'Confirm' }));
      expect(report).toHaveBeenCalledWith(err);
      expect(screen.getByRole('button', { name: 'End session' })).toBeInTheDocument();
    } finally {
      globalThis.reportError = original;
    }
  });

  it('disabled trigger explains why and cannot open', () => {
    setup({ disabled: true, disabledReason: 'No session to end.' });
    const trigger = screen.getByRole('button', { name: 'End session' });
    expect(trigger).toBeDisabled();
    expect(trigger).toHaveAccessibleDescription('No session to end.');
    fireEvent.click(trigger);
    expect(screen.queryByRole('group')).not.toBeInTheDocument();
  });

  it('defaults the prompt to the label', () => {
    render(<ConfirmAction label="Leave session" consequence="x" onConfirm={() => undefined} />);
    fireEvent.click(screen.getByRole('button', { name: 'Leave session' }));
    expect(screen.getByRole('group', { name: 'Leave session?' })).toBeInTheDocument();
  });

  it('supports ariaLabel for custom accessible name on the trigger', () => {
    setup({ label: 'Remove', ariaLabel: 'Remove Ana (@pitwall_ana)' });
    expect(screen.getByRole('button', { name: 'Remove Ana (@pitwall_ana)' })).toBeInTheDocument();
  });
});
