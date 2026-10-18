import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';
import type { AppConfig, AppInfo, ConfigSnapshot, CoreStatus, LoginStateDto, Mode } from './lib/ipc';
import * as ipc from './lib/ipc';

vi.mock('./lib/ipc', async (importOriginal) => {
  const actual = await importOriginal<typeof ipc>();
  return {
    ...actual,
    getAppInfo: vi.fn(),
    getConfig: vi.fn(),
    setMode: vi.fn(),
    getLoginState: vi.fn(),
    getCoreStatus: vi.fn(),
    logout: vi.fn(),
    login: vi.fn(),
  };
});

const INFO: AppInfo = {
  name: 'LMU Race Engineering',
  version: '0.1.0',
  identifier: 'com.lmurace.engineering',
  build_profile: 'debug',
  environment: 'development',
  log_dir: 'C:\\logs',
};

const DRIVER_ACCOUNT: ipc.AccountInfoDto = {
  account: '11112222333344445555666677778888',
  username: 'sam_driver',
  display_name: 'Sam Driver',
  role: 'driver',
};

const AUTH_DRIVER: LoginStateDto = {
  authenticated: true,
  role: 'driver',
  account: DRIVER_ACCOUNT,
};

const UNAUTH_DRIVER: LoginStateDto = {
  authenticated: false,
  role: 'driver',
  account: null,
};

function snapshot(mode: Mode | null): ConfigSnapshot {
  const config: AppConfig = {
    schema_version: 1,
    application: { mode, environment: 'development' },
    network: { relay_host: '', relay_port: 443, connect_timeout_ms: 5000, extra_ca_file: null },
    telemetry: { recording_enabled: true, recording_dir: null },
    fuel: { window_laps: 5, reserve_laps: 1 },
    ui: { scale: 1, max_refresh_hz: 60 },
    driver: { auto_stream: false },
    engines: { default_alerts_enabled: true },
    window: { x: null, y: null, width: null, height: null, maximized: false },
  };
  return { config, path: 'C:\\config\\config.json', issue: null, restart_required: false, core_issue: null };
}

function idleCoreStatus(): CoreStatus {
  return {
    mode: 'driver',
    origin: 'synthetic',
    lmu: { state: 'NOT_RUNNING', detail: null, game_version: null },
    session: { session_id: null, session_type: null, track: null, car: null, vehicle_class: null, started_at_unix_s: null },
    recording: { state: 'off', recording_id: null, bytes_written: 0, frames_written: 0, frames_lost: 0, disk_free_bytes: null, error: null },
    network: {
      state: 'DISCONNECTED',
      streaming: false,
      connect_code: null,
      code_expires_at_unix_s: null,
      engineers_connected: 0,
      engineers_max: 2,
      rtt_ms: null,
      relogin_required: false,
      last_error: null,
      failure: null,
      degraded_reason: null,
      engineers: [],
      driver_away: false,
      security_issue: false,
    },
    rates: { telemetry_read_hz: null, scoring_read_hz: null, frames_sent_hz: null, bytes_sent_per_s: null, frames_dropped_total: 0 },
    drops: { reader_overrun: 0, pipeline_overrun: 0, net_dropped_obsolete: 0, recorder_overrun: 0, net_events_dropped: 0, ui_dropped: 0 },
    engineer: null,
  };
}

describe('App', () => {
  beforeEach(() => {
    vi.mocked(ipc.getAppInfo).mockResolvedValue(INFO);
    vi.mocked(ipc.getLoginState).mockResolvedValue(AUTH_DRIVER);
    vi.mocked(ipc.getCoreStatus).mockResolvedValue(idleCoreStatus());
    vi.mocked(ipc.logout).mockResolvedValue(undefined);
  });

  it('asks for the operating mode on first launch and persists the choice', async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue(snapshot(null));
    vi.mocked(ipc.setMode).mockResolvedValue(snapshot('driver'));

    render(<App />);

    expect(await screen.findByText('LMU RACE ENGINEERING')).toBeInTheDocument();
    expect(screen.getByText('Select operating mode:')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'ENGINEER MODE' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'DRIVER MODE' }));

    expect(await screen.findByTestId('header-mode')).toHaveTextContent('DRIVER');
    expect(ipc.setMode).toHaveBeenCalledWith('driver');
    expect(screen.getByTestId('header-version')).toHaveTextContent('v0.1.0');
    expect(screen.getByTestId('header-environment')).toHaveTextContent('DEVELOPMENT');
  });

  it('shows the shell directly when a mode is already stored, and allows changing it in Settings', async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue(snapshot('engineer'));
    vi.mocked(ipc.setMode).mockResolvedValue(snapshot('driver'));

    render(<App />);

    expect(await screen.findByTestId('header-mode')).toHaveTextContent('ENGINEER');
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(screen.getByRole('button', { name: 'DRIVER MODE' }));
    fireEvent.click(screen.getByRole('button', { name: 'Apply mode change' }));

    expect(await screen.findByText('Current: DRIVER')).toBeInTheDocument();
    expect(screen.getByTestId('header-mode')).toHaveTextContent('DRIVER');
    expect(ipc.setMode).toHaveBeenCalledWith('driver');
  });

  it('keeps the mode selection screen and shows the error when saving fails', async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue(snapshot(null));
    vi.mocked(ipc.setMode).mockRejectedValue(new ipc.IpcError('set_mode', 'config_write_failed', 'disk full'));

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'ENGINEER MODE' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('disk full');
    expect(screen.getByRole('button', { name: 'ENGINEER MODE' })).toBeEnabled();
  });

  it('shows the config load issue banner', async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue({
      ...snapshot('driver'),
      issue: {
        kind: 'invalid',
        message: 'config.json failed validation',
        errors: [{ field: 'network.relay_port', message: 'must be between 1 and 65535' }],
        preserved_as: 'C:\\config\\config.invalid-20260930T120000Z.json',
      },
    });

    render(<App />);

    const banner = await screen.findByRole('alert');
    expect(banner).toHaveTextContent('Configuration file invalid');
    expect(banner).toHaveTextContent('network.relay_port');
    expect(banner).toHaveTextContent('config.invalid-20260930T120000Z.json');
  });

  it('shows a retryable error when the backend is unavailable', async () => {
    vi.mocked(ipc.getConfig).mockRejectedValue(new ipc.IpcError('get_config', 'backend_unavailable', 'no backend'));

    render(<App />);

    expect(await screen.findByText('Backend unavailable')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument();
  });

  it('routes to LoginScreen when mode is set but login state is unauthenticated (T11, B12)', async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue(snapshot('driver'));
    vi.mocked(ipc.getLoginState).mockResolvedValue(UNAUTH_DRIVER);

    render(<App />);

    expect(await screen.findByTestId('login-screen')).toBeInTheDocument();
    expect(screen.getByTestId('login-mode-text')).toHaveTextContent('DRIVER');
    expect(screen.getByTestId('login-tab-signin')).toBeInTheDocument();
  });

  it('transitions from LoginScreen to ShellScreen after successful login', async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue(snapshot('driver'));
    vi.mocked(ipc.getLoginState)
      .mockResolvedValueOnce(UNAUTH_DRIVER)
      .mockResolvedValue(AUTH_DRIVER);
    vi.mocked(ipc.login).mockResolvedValue(DRIVER_ACCOUNT);

    render(<App />);

    expect(await screen.findByTestId('login-screen')).toBeInTheDocument();

    fireEvent.change(screen.getByTestId('login-username-input'), { target: { value: 'sam_driver' } });
    fireEvent.change(screen.getByTestId('login-password-input'), { target: { value: 'secret' } });
    fireEvent.click(screen.getByTestId('login-submit-btn'));

    expect(await screen.findByTestId('header-mode')).toHaveTextContent('DRIVER');
    expect(screen.getByTestId('header-account')).toHaveTextContent('Sam Driver');
  });

  it('logs out from ShellScreen and routes to LoginScreen', async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue(snapshot('driver'));
    vi.mocked(ipc.getLoginState)
      .mockResolvedValueOnce(AUTH_DRIVER)
      .mockResolvedValue(UNAUTH_DRIVER);

    render(<App />);

    expect(await screen.findByTestId('header-logout')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('header-logout'));

    await waitFor(() => {
      expect(ipc.logout).toHaveBeenCalled();
    });

    expect(await screen.findByTestId('login-screen')).toBeInTheDocument();
  });

  it('requires ConfirmAction in Settings when streaming (OR-5)', async () => {
    vi.mocked(ipc.getConfig).mockResolvedValue(snapshot('driver'));
    vi.mocked(ipc.setMode).mockResolvedValue(snapshot('engineer'));
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...idleCoreStatus(),
      network: {
        ...idleCoreStatus().network,
        streaming: true,
      },
    });

    render(<App />);

    expect(await screen.findByTestId('header-mode')).toHaveTextContent('DRIVER');
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(screen.getByRole('button', { name: 'ENGINEER MODE' }));

    // Busy state uses ConfirmAction
    const applyTrigger = await screen.findByTestId('settings-apply-mode-trigger');
    expect(applyTrigger).toBeInTheDocument();
    fireEvent.click(applyTrigger);

    // Prompt and consequence shown
    expect(screen.getByText('Change operating mode?')).toBeInTheDocument();
    expect(screen.getByText(/Changing operating mode will stop active streaming/)).toBeInTheDocument();

    // Confirm click
    fireEvent.click(screen.getByTestId('settings-apply-mode-confirm'));

    await waitFor(() => {
      expect(ipc.setMode).toHaveBeenCalledWith('engineer');
    });
  });
});
