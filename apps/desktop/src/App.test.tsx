import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';
import type { AppConfig, AppInfo, ConfigSnapshot, Mode } from './lib/ipc';
import * as ipc from './lib/ipc';

vi.mock('./lib/ipc', async (importOriginal) => {
  const actual = await importOriginal<typeof ipc>();
  return {
    ...actual,
    getAppInfo: vi.fn(),
    getConfig: vi.fn(),
    setMode: vi.fn(),
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

describe('App', () => {
  beforeEach(() => {
    vi.mocked(ipc.getAppInfo).mockResolvedValue(INFO);
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
});
