import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { IdentityCardDto } from '../lib/ipc';
import { IpcError } from '../lib/ipc';
import * as ipc from '../lib/ipc';
import { JoinSessionModal } from './JoinSessionModal';

vi.mock('../lib/ipc', async () => {
  const actual = await vi.importActual<typeof ipc>('../lib/ipc');
  return {
    ...actual,
    engineerJoin: vi.fn(),
    engineerConfirm: vi.fn(),
    engineerCancel: vi.fn(),
  };
});

const MOCK_CARD: IdentityCardDto = {
  session_id: 1042,
  driver_display_name: 'Jane Doe',
  driver_username: 'janedoe',
  car: 'Ferrari 499P',
  vehicle_class: 'Hypercar',
  track: 'Circuit de la Sarthe',
  session_type: 'Race',
  started_at_ns: 1_700_000_000_000_000,
  schema_version: 1,
};

describe('JoinSessionModal', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders code input and sanitizes input to uppercase 6-char alphanumeric', () => {
    render(<JoinSessionModal />);

    const input = screen.getByTestId<HTMLInputElement>('join-code-input');
    const submitBtn = screen.getByTestId('btn-join-session');

    expect(submitBtn).toBeDisabled();

    // Type lowercase and symbols
    fireEvent.change(input, { target: { value: '8f-3a!2c99' } });

    // Should sanitize to 6 uppercase chars: '8F3A2C'
    expect(input.value).toBe('8F3A2C');
    expect(submitBtn).toBeEnabled();
  });

  it('submits code, displays identity card, and confirms join', async () => {
    vi.mocked(ipc.engineerJoin).mockResolvedValue(MOCK_CARD);
    vi.mocked(ipc.engineerConfirm).mockResolvedValue(undefined);

    const onJoined = vi.fn();
    render(<JoinSessionModal onJoined={onJoined} />);

    const input = screen.getByTestId('join-code-input');
    fireEvent.change(input, { target: { value: '8F3A2C' } });

    fireEvent.click(screen.getByTestId('btn-join-session'));

    expect(ipc.engineerJoin).toHaveBeenCalledWith('8F3A2C');

    // Identity Card appears
    await waitFor(() => {
      expect(screen.getByTestId('driver-identity-card')).toBeInTheDocument();
      expect(screen.getByText('Jane Doe (@janedoe)')).toBeInTheDocument();
      expect(screen.getByText('Ferrari 499P')).toBeInTheDocument();
      expect(screen.getByText('Circuit de la Sarthe')).toBeInTheDocument();
      expect(screen.getByText('8F3A2C')).toBeInTheDocument();
      expect(screen.getByText('HY')).toBeInTheDocument();
      expect(
        screen.getByText('Only confirm if this is the driver you expect.'),
      ).toBeInTheDocument();
      expect(
        screen.getByText(
          "This code can't be reused. Ask the driver for a new one.",
        ),
      ).toBeInTheDocument();
    });

    // Confirm button
    fireEvent.click(screen.getByTestId('btn-confirm-identity'));

    await waitFor(() => {
      expect(ipc.engineerConfirm).toHaveBeenCalled();
      expect(onJoined).toHaveBeenCalled();
    });
  });

  it('allows declining driver identity card and returns to code input', async () => {
    vi.mocked(ipc.engineerJoin).mockResolvedValue(MOCK_CARD);
    vi.mocked(ipc.engineerCancel).mockResolvedValue(undefined);

    render(<JoinSessionModal />);

    const input = screen.getByTestId('join-code-input');
    fireEvent.change(input, { target: { value: '8F3A2C' } });
    fireEvent.click(screen.getByTestId('btn-join-session'));

    await waitFor(() => {
      expect(screen.getByTestId('driver-identity-card')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByTestId('btn-cancel-identity'));

    await waitFor(() => {
      expect(ipc.engineerCancel).toHaveBeenCalled();
      expect(screen.getByTestId('join-code-input')).toBeInTheDocument();
    });
  });

  it('displays mapped error and clears code field when join code expires (EN02)', async () => {
    vi.mocked(ipc.engineerJoin).mockRejectedValue(
      new IpcError('engineer_join', 'code_expired', 'The code expired'),
    );

    render(<JoinSessionModal />);

    const input = screen.getByTestId<HTMLInputElement>('join-code-input');
    fireEvent.change(input, { target: { value: '8F3A2C' } });
    fireEvent.click(screen.getByTestId('btn-join-session'));

    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(
        'That code has expired or was already used',
      );
      expect(screen.getByRole('alert')).toHaveTextContent(
        'Ask the driver for a new code.',
      );
      // Code field was reset because clearField is true for code_expired
      expect(input.value).toBe('');
    });
  });

  it('displays mapped error when session is full (OR-4)', async () => {
    vi.mocked(ipc.engineerJoin).mockRejectedValue(
      new IpcError('engineer_join', 'session_full', 'Driver already has 2 engineers connected'),
    );

    render(<JoinSessionModal />);

    const input = screen.getByTestId('join-code-input');
    fireEvent.change(input, { target: { value: '8F3A2C' } });
    fireEvent.click(screen.getByTestId('btn-join-session'));

    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(
        'The driver already has 2 engineers',
      );
      expect(screen.getByRole('alert')).toHaveTextContent(
        'The driver must remove an engineer first.',
      );
    });
  });

  it('has accessible label, autocomplete, inputMode, and paste hint for engineer code (A06)', () => {
    render(<JoinSessionModal />);

    const label = screen.getByText('Engineer code');
    expect(label).toHaveAttribute('for', 'engineer-code-input');

    const input = screen.getByTestId<HTMLInputElement>('join-code-input');
    expect(input).toHaveAttribute('id', 'engineer-code-input');
    expect(input).toHaveAttribute('autocomplete', 'one-time-code');
    expect(input).toHaveAttribute('inputmode', 'text');
    expect(
      screen.getByText('Paste 6-character code from clipboard (Ctrl+V)'),
    ).toBeInTheDocument();
  });

  it('handles Escape in phase 1 by cancelling joining', () => {
    const onCancel = vi.fn();
    render(<JoinSessionModal onCancel={onCancel} />);

    const dialog = screen.getByRole('dialog');
    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('handles Escape in phase 2 (identity card) by cancelling back to code input', async () => {
    vi.mocked(ipc.engineerJoin).mockResolvedValue(MOCK_CARD);
    vi.mocked(ipc.engineerCancel).mockResolvedValue(undefined);

    const onCancel = vi.fn();
    render(<JoinSessionModal onCancel={onCancel} />);

    const input = screen.getByTestId('join-code-input');
    fireEvent.change(input, { target: { value: '8F3A2C' } });
    fireEvent.click(screen.getByTestId('btn-join-session'));

    await waitFor(() => {
      expect(screen.getByTestId('driver-identity-card')).toBeInTheDocument();
    });

    // Pressing Escape on the identity card
    const dialog = screen.getByRole('dialog');
    fireEvent.keyDown(dialog, { key: 'Escape' });

    await waitFor(() => {
      // Calls engineerCancel and returns to code input
      expect(ipc.engineerCancel).toHaveBeenCalled();
      expect(screen.getByTestId('join-code-input')).toBeInTheDocument();
      // Does not close entire modal (does not call onCancel)
      expect(onCancel).not.toHaveBeenCalled();
    });
  });

  it('shifts focus to identity card heading on phase transition', async () => {
    vi.mocked(ipc.engineerJoin).mockResolvedValue(MOCK_CARD);

    render(<JoinSessionModal />);

    const input = screen.getByTestId('join-code-input');
    expect(input).toHaveFocus();

    fireEvent.change(input, { target: { value: '8F3A2C' } });
    fireEvent.click(screen.getByTestId('btn-join-session'));

    await waitFor(() => {
      expect(screen.getByTestId('driver-identity-card')).toBeInTheDocument();
      expect(screen.getByText('CONFIRM DRIVER IDENTITY')).toHaveFocus();
    });
  });
});
