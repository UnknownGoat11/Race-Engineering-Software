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

  it('hosts a session and displays deliverable code with copy button', async () => {
    render(
      <HomeScreen
        currentMode="driver"
        onChangeMode={onChangeMode}
        onEnterSession={onEnterSession}
        displayName="Alice Driver"
      />
    );

    const hostBtn = screen.getByTestId('home-host-btn');
    fireEvent.click(hostBtn);

    await waitFor(() => {
      expect(ipc.driverNewCode).toHaveBeenCalled();
    });

    // After hosting, deliverable code is displayed
    const activeBanner = await screen.findByTestId('home-active-banner');
    expect(activeBanner).toBeInTheDocument();

    const copyBtn = screen.getByTestId('home-copy-code-btn');
    fireEvent.click(copyBtn);
    expect(writeTextSpy).toHaveBeenCalled();

    const enterBtn = screen.getByTestId('home-enter-session-btn');
    fireEvent.click(enterBtn);
    expect(onEnterSession).toHaveBeenCalled();
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
