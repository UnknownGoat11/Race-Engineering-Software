import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { IdentityCardDto } from '../lib/ipc';
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

  it('displays error when join fails', async () => {
    vi.mocked(ipc.engineerJoin).mockRejectedValue(new Error('Session full (max 2 engineers)'));

    render(<JoinSessionModal />);

    const input = screen.getByTestId('join-code-input');
    fireEvent.change(input, { target: { value: '8F3A2C' } });
    fireEvent.click(screen.getByTestId('btn-join-session'));

    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent('Session full (max 2 engineers)');
    });
  });
});
