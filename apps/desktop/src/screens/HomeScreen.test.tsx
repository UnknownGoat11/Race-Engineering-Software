import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as ipc from '../lib/ipc';
import { HomeScreen } from './HomeScreen';

const { mockCard } = vi.hoisted(() => ({
  mockCard: {
    driver_display_name: 'Driver',
    vehicle_name: 'Ferrari 499P',
    vehicle_class: 'HYPERCAR',
    track_name: 'Circuit de la Sarthe',
    session_type: 'RACE',
    session_duration_s: 3600,
    connection_time_s: 120,
    redeemed_code: 'K7Q2XM',
  },
}));

vi.mock('../lib/ipc', async (importOriginal) => {
  const actual = await importOriginal<typeof ipc>();
  return {
    ...actual,
    driverNewCode: vi.fn().mockResolvedValue(undefined),
    engineerJoin: vi.fn().mockResolvedValue(mockCard),
  };
});

describe('HomeScreen', () => {
  const onChangeMode = vi.fn().mockResolvedValue(undefined);
  const onEnterSession = vi.fn();
  const writeTextSpy = vi.fn().mockResolvedValue(undefined);

  beforeEach(() => {
    vi.clearAllMocks();
    Object.assign(navigator, {
      clipboard: {
        writeText: writeTextSpy,
      },
    });
  });

  it('renders session lobby options for hosting and joining', () => {
    render(
      <HomeScreen
        currentMode="driver"
        onChangeMode={onChangeMode}
        onEnterSession={onEnterSession}
        displayName="Alice Driver"
      />
    );

    expect(screen.getByTestId('home-screen')).toBeInTheDocument();
    expect(screen.getByText('Alice Driver')).toBeInTheDocument();
    expect(screen.getByTestId('home-card-host')).toBeInTheDocument();
    expect(screen.getByTestId('home-card-join')).toBeInTheDocument();
  });

  it('requests a relay code when hosting and never shows a locally invented code', async () => {
    render(
      <HomeScreen
        currentMode="driver"
        onChangeMode={onChangeMode}
        onEnterSession={onEnterSession}
        displayName="Alice Driver"
      />
    );

    fireEvent.click(screen.getByTestId('home-host-btn'));

    await waitFor(() => {
      expect(ipc.driverNewCode).toHaveBeenCalled();
    });
    // No relay code in core status yet: nothing to copy, no banner.
    expect(screen.queryByTestId('home-active-banner')).not.toBeInTheDocument();
    expect(screen.queryByTestId('home-code-value')).not.toBeInTheDocument();
  });

  it('shows the error when the relay cannot issue a code', async () => {
    vi.mocked(ipc.driverNewCode).mockRejectedValueOnce(new Error('relay unreachable'));
    render(
      <HomeScreen
        currentMode="driver"
        onChangeMode={onChangeMode}
        onEnterSession={onEnterSession}
      />
    );

    fireEvent.click(screen.getByTestId('home-host-btn'));

    expect(await screen.findByRole('alert')).toHaveTextContent('relay unreachable');
    expect(screen.queryByTestId('home-code-value')).not.toBeInTheDocument();
  });

  it('switches mode when hosting as engineer', async () => {
    render(
      <HomeScreen
        currentMode="driver"
        onChangeMode={onChangeMode}
        onEnterSession={onEnterSession}
      />
    );

    const engineerBtns = screen.getAllByRole('button', { name: 'Engineer (Pitwall)' });
    const btn = engineerBtns[0];
    if (btn) {
      fireEvent.click(btn);
    }
    fireEvent.click(screen.getByTestId('home-host-btn'));

    await waitFor(() => {
      expect(onChangeMode).toHaveBeenCalledWith('engineer');
    });
  });

  it('allows entering a code and joining a session', async () => {
    render(
      <HomeScreen
        currentMode="engineer"
        onChangeMode={onChangeMode}
        onEnterSession={onEnterSession}
      />
    );

    const codeInput = screen.getByTestId('home-join-code-input');
    fireEvent.change(codeInput, { target: { value: 'k7q2xm' } });
    expect(codeInput).toHaveValue('K7Q2XM');

    const joinBtn = screen.getByTestId('home-join-submit-btn');
    fireEvent.click(joinBtn);

    await waitFor(() => {
      expect(ipc.engineerJoin).toHaveBeenCalledWith('K7Q2XM');
      expect(onEnterSession).toHaveBeenCalledWith('pitwall');
    });
  });
});
