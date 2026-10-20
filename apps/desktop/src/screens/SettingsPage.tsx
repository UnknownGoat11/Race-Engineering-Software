import { useState, type JSX } from 'react';
import { ConfirmAction } from '../components/ui/ConfirmAction';
import { useCoreStatus } from '../hooks/useCoreStatus';
import {
  restartApp,
  testRelayConnection,
  updateSettings,
  type AppInfo,
  type ConfigSnapshot,
  type Mode,
} from '../lib/ipc';
import { buildProfileLabel, environmentLabel, modeLabel } from '../lib/labels';
import { NO_VALUE } from '../lib/units';

interface SettingsPageProps {
  readonly info: AppInfo;
  readonly snapshot: ConfigSnapshot;
  readonly mode: Mode;
  readonly onChangeMode: (mode: Mode) => Promise<void>;
}

const MODES: readonly Mode[] = ['driver', 'engineer'];

type SaveState = { readonly kind: 'idle' } | { readonly kind: 'saving' } | { readonly kind: 'error'; readonly message: string };

type TestState =
  | { readonly kind: 'idle' }
  | { readonly kind: 'testing' }
  | { readonly kind: 'success'; readonly message: string; readonly rtt: number | null }
  | { readonly kind: 'error'; readonly message: string };

type SettingsSaveState =
  | { readonly kind: 'idle' }
  | { readonly kind: 'saving' }
  | { readonly kind: 'success'; readonly restartRequired: boolean }
  | { readonly kind: 'error'; readonly message: string };

function textOrDash(value: string | null): string {
  return value === null || value === '' ? NO_VALUE : value;
}

export function SettingsPage({ info, snapshot, mode, onChangeMode }: SettingsPageProps): JSX.Element {
  const [selected, setSelected] = useState<Mode>(mode);
  const [save, setSave] = useState<SaveState>({ kind: 'idle' });
  const { config } = snapshot;
  const { status } = useCoreStatus();
  const extraCaHonoured = info.environment === 'development';

  // Network & Relay server state
  const [relayHost, setRelayHost] = useState(config.network.relay_host);
  const [relayPort, setRelayPort] = useState(config.network.relay_port);
  const [connectTimeout, setConnectTimeout] = useState(config.network.connect_timeout_ms);
  const [recordingEnabled, setRecordingEnabled] = useState(config.telemetry.recording_enabled);
  const [fuelWindow, setFuelWindow] = useState(config.fuel.window_laps);
  const [fuelReserve, setFuelReserve] = useState(config.fuel.reserve_laps);
  const [uiRefresh, setUiRefresh] = useState(config.ui.max_refresh_hz);

  const [testState, setTestState] = useState<TestState>({ kind: 'idle' });
  const [settingsSave, setSettingsSave] = useState<SettingsSaveState>({ kind: 'idle' });
  const [restarting, setRestarting] = useState(false);

  const isStreaming = status?.network.streaming === true;
  const isRecording = status?.recording.state === 'active';
  const hasEngineers = (status?.network.engineers_connected ?? 0) > 0;
  const isEngineerConnected =
    status?.engineer?.phase === 'monitoring' ||
    status?.engineer?.phase === 'awaiting_confirmation';

  const isBusy = isStreaming || isRecording || hasEngineers || isEngineerConnected;

  const applyMode = async (): Promise<void> => {
    setSave({ kind: 'saving' });
    try {
      await onChangeMode(selected);
      setSave({ kind: 'idle' });
    } catch (reason: unknown) {
      setSave({ kind: 'error', message: reason instanceof Error ? reason.message : String(reason) });
    }
  };

  const handleTestConnection = async (): Promise<void> => {
    const hostToTest = relayHost.trim();
    if (!hostToTest) {
      setTestState({
        kind: 'error',
        message: 'Please enter a Relay Server Host / IP before testing.',
      });
      return;
    }
    setTestState({ kind: 'testing' });
    try {
      const report = await testRelayConnection({ host: hostToTest, port: relayPort });
      const rtt = report.rtt?.avg_ms ?? null;
      setTestState({
        kind: 'success',
        rtt,
        message: rtt !== null
          ? `Server reached successfully! Latency: ${rtt.toFixed(1)} ms. TLS Handshake & Authentication verified.`
          : 'Server reached successfully! TLS Handshake verified.',
      });
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setTestState({
        kind: 'error',
        message: `Connection failed: ${msg}. Check host address and verify the server is running.`,
      });
    }
  };

  const handleSaveSettings = async (): Promise<void> => {
    setSettingsSave({ kind: 'saving' });
    try {
      const newConfig = {
        ...config,
        network: {
          ...config.network,
          relay_host: relayHost.trim(),
          relay_port: Number(relayPort),
          connect_timeout_ms: Number(connectTimeout),
        },
        telemetry: {
          ...config.telemetry,
          recording_enabled: recordingEnabled,
        },
        fuel: {
          ...config.fuel,
          window_laps: Number(fuelWindow),
          reserve_laps: Number(fuelReserve),
        },
        ui: {
          ...config.ui,
          max_refresh_hz: Number(uiRefresh),
        },
      };

      const result = await updateSettings(newConfig);
      setSettingsSave({ kind: 'success', restartRequired: result.restart_required });
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setSettingsSave({ kind: 'error', message: msg });
    }
  };

  const handleRestart = async (): Promise<void> => {
    setRestarting(true);
    try {
      await restartApp();
    } catch (err) {
      console.error('Failed to restart app:', err);
      setRestarting(false);
    }
  };

  return (
    <section className="page settings-page">
      <h2 className="page-title">Settings</h2>

      {/* Operating Mode Panel */}
      <div className="panel">
        <div className="panel-header">
          <span id="settings-mode-label">Operating mode</span>
          <span>Current: {modeLabel(mode)}</span>
        </div>
        <div className="panel-body">
          <div className="button-row" role="group" aria-labelledby="settings-mode-label">
            {MODES.map((option) => (
              <button
                key={option}
                type="button"
                className="btn"
                aria-pressed={selected === option}
                disabled={save.kind === 'saving'}
                onClick={() => {
                  setSelected(option);
                }}
              >
                {modeLabel(option)} MODE
              </button>
            ))}
            {isBusy ? (
              <ConfirmAction
                label="Apply mode change"
                prompt="Change operating mode?"
                consequence="Changing operating mode will stop active streaming, recording, and disconnect all connected sessions."
                confirmLabel="Confirm change"
                onConfirm={applyMode}
                disabled={selected === mode || save.kind === 'saving'}
                variant="danger"
                testId="settings-apply-mode"
              />
            ) : (
              <button
                type="button"
                className="btn btn-primary"
                disabled={selected === mode || save.kind === 'saving'}
                onClick={() => {
                  void applyMode();
                }}
                data-testid="settings-apply-mode"
              >
                Apply mode change
              </button>
            )}
            {save.kind === 'saving' && <span className="form-status">Saving…</span>}
          </div>
          {save.kind === 'error' && (
            <div className="alert alert-error" role="alert">
              <span className="alert-title">Could not save the operating mode</span>
              <span>{save.message}</span>
            </div>
          )}
        </div>
      </div>

      {/* Relay Server & Global Connection Panel */}
      <div className="panel" data-testid="settings-relay-panel">
        <div className="panel-header">
          <span>Relay Server Connection (Global & Online)</span>
          <span className="badge badge-info">
            {config.network.relay_host ? `Active: ${config.network.relay_host}:${config.network.relay_port}` : 'Not Connected'}
          </span>
        </div>
        <div className="panel-body">
          <p className="panel-note" style={{ marginBottom: '14px' }}>
            The relay server allows drivers and race engineers anywhere in the world to connect over the internet
            with outbound TLS connections (no port forwarding or VPN needed).
          </p>

          {/* Quick Presets */}
          <div style={{ marginBottom: '16px' }}>
            <span style={{ fontSize: '12px', fontWeight: 600, display: 'block', marginBottom: '6px' }}>
              Connection Presets:
            </span>
            <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
              <button
                type="button"
                className={`btn btn-xs ${relayHost === 'relay.lmuracing.com' ? 'btn-primary' : ''}`}
                onClick={() => {
                  setRelayHost('relay.lmuracing.com');
                  setRelayPort(443);
                }}
                title="Connect via official global cloud server for worldwide low-latency telemetry"
              >
                🌐 Global Cloud Relay (relay.lmuracing.com)
              </button>
              <button
                type="button"
                className={`btn btn-xs ${relayHost === '127.0.0.1' ? 'btn-primary' : ''}`}
                onClick={() => {
                  setRelayHost('127.0.0.1');
                  setRelayPort(443);
                }}
                title="Connect via localhost or local machine"
              >
                💻 Localhost (127.0.0.1:443)
              </button>
              <button
                type="button"
                className="btn btn-xs"
                onClick={() => {
                  setRelayHost('');
                }}
                title="Clear host to type a custom domain, public IP, or dynamic DNS"
              >
                🌍 Custom Team Server / Direct IP
              </button>
            </div>
          </div>

          {/* Editable Fields Grid */}
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(220px, 1fr))', gap: '14px', marginBottom: '16px' }}>
            <div>
              <label htmlFor="relay-host-input" style={{ display: 'block', fontSize: '12px', fontWeight: 600, marginBottom: '4px' }}>
                Relay Server Host / Domain / IP
              </label>
              <input
                id="relay-host-input"
                type="text"
                className="input"
                placeholder="e.g. relay.lmuracing.com or 192.168.1.100"
                value={relayHost}
                onChange={(e) => { setRelayHost(e.target.value); }}
                style={{ width: '100%', fontFamily: 'monospace' }}
                data-testid="relay-host-input"
              />
            </div>

            <div>
              <label htmlFor="relay-port-input" style={{ display: 'block', fontSize: '12px', fontWeight: 600, marginBottom: '4px' }}>
                Relay Port
              </label>
              <input
                id="relay-port-input"
                type="number"
                className="input"
                placeholder="443"
                value={relayPort}
                onChange={(e) => { setRelayPort(Number(e.target.value) || 443); }}
                style={{ width: '100%', fontFamily: 'monospace' }}
                data-testid="relay-port-input"
              />
            </div>

            <div>
              <label htmlFor="relay-timeout-input" style={{ display: 'block', fontSize: '12px', fontWeight: 600, marginBottom: '4px' }}>
                Connect Timeout (ms)
              </label>
              <input
                id="relay-timeout-input"
                type="number"
                className="input"
                placeholder="5000"
                value={connectTimeout}
                onChange={(e) => { setConnectTimeout(Number(e.target.value) || 5000); }}
                style={{ width: '100%', fontFamily: 'monospace' }}
              />
            </div>
          </div>

          {/* Action Buttons: Test Connection & Save */}
          <div style={{ display: 'flex', gap: '10px', alignItems: 'center', flexWrap: 'wrap', marginBottom: '14px' }}>
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => { void handleTestConnection(); }}
              disabled={testState.kind === 'testing' || restarting}
              data-testid="test-relay-btn"
            >
              {testState.kind === 'testing' ? '⏳ Testing Connection…' : '⚡ Test Connection'}
            </button>

            <button
              type="button"
              className="btn btn-primary"
              onClick={() => { void handleSaveSettings(); }}
              disabled={settingsSave.kind === 'saving' || restarting}
              data-testid="save-relay-btn"
            >
              {settingsSave.kind === 'saving' ? 'Saving…' : '💾 Save Server Settings'}
            </button>
          </div>

          {/* Test Feedback Banner */}
          {testState.kind === 'testing' && (
            <div className="alert alert-info" style={{ marginBottom: '12px' }}>
              <span>Testing 7-stage connectivity to <strong>{relayHost}:{relayPort}</strong>…</span>
            </div>
          )}

          {testState.kind === 'success' && (
            <div className="alert alert-success" style={{ marginBottom: '12px', borderColor: 'var(--color-success)' }}>
              <strong>✅ Connection Successful!</strong>
              <div style={{ fontSize: '12px', marginTop: '2px' }}>{testState.message}</div>
            </div>
          )}

          {testState.kind === 'error' && (
            <div className="alert alert-error" style={{ marginBottom: '12px' }}>
              <strong>❌ Connectivity Test Warning</strong>
              <div style={{ fontSize: '12px', marginTop: '2px' }}>{testState.message}</div>
            </div>
          )}

          {/* Save Status & Restart Required Banner */}
          {settingsSave.kind === 'success' && (
            <div
              className={`alert ${settingsSave.restartRequired ? 'alert-warning' : 'alert-success'}`}
              style={{ marginBottom: '12px' }}
            >
              <strong>
                {settingsSave.restartRequired ? '⚠️ Settings Saved — Restart Required' : '✅ Settings Saved Successfully'}
              </strong>
              <div style={{ fontSize: '12px', marginTop: '4px' }}>
                {settingsSave.restartRequired
                  ? 'The relay server connection was modified. Restart the application now to establish connections using the new server.'
                  : 'Your configuration updates were saved and applied immediately.'}
              </div>
              {settingsSave.restartRequired && (
                <div style={{ marginTop: '10px' }}>
                  <button
                    type="button"
                    className="btn btn-primary"
                    onClick={() => { void handleRestart(); }}
                    disabled={restarting}
                    data-testid="restart-app-btn"
                  >
                    {restarting ? 'Restarting…' : '🔄 Restart Application Now'}
                  </button>
                </div>
              )}
            </div>
          )}

          {settingsSave.kind === 'error' && (
            <div className="alert alert-error" style={{ marginBottom: '12px' }}>
              <span className="alert-title">Could not save settings</span>
              <span>{settingsSave.message}</span>
            </div>
          )}
        </div>
      </div>

      {/* Telemetry & Engines Configuration */}
      <div className="panel">
        <div className="panel-header">
          <span>Telemetry & Engines</span>
          <span className="num">Schema v{config.schema_version}</span>
        </div>
        <div className="panel-body">
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(200px, 1fr))', gap: '14px', marginBottom: '14px' }}>
            <div>
              <label htmlFor="recording-toggle" style={{ display: 'block', fontSize: '12px', fontWeight: 600, marginBottom: '4px' }}>
                Local Recording
              </label>
              <select
                id="recording-toggle"
                className="input"
                value={recordingEnabled ? 'true' : 'false'}
                onChange={(e) => { setRecordingEnabled(e.target.value === 'true'); }}
                style={{ width: '100%' }}
              >
                <option value="true">Enabled (Record laps)</option>
                <option value="false">Disabled</option>
              </select>
            </div>

            <div>
              <label htmlFor="fuel-window-input" style={{ display: 'block', fontSize: '12px', fontWeight: 600, marginBottom: '4px' }}>
                Fuel Window (laps)
              </label>
              <select
                id="fuel-window-input"
                className="input"
                value={fuelWindow}
                onChange={(e) => { setFuelWindow(Number(e.target.value) || 5); }}
                style={{ width: '100%' }}
              >
                <option value="1">1 Lap</option>
                <option value="2">2 Laps</option>
                <option value="3">3 Laps</option>
                <option value="5">5 Laps (Default)</option>
                <option value="10">10 Laps</option>
              </select>
            </div>

            <div>
              <label htmlFor="fuel-reserve-input" style={{ display: 'block', fontSize: '12px', fontWeight: 600, marginBottom: '4px' }}>
                Fuel Reserve (laps)
              </label>
              <input
                id="fuel-reserve-input"
                type="number"
                step="0.5"
                min="0"
                max="10"
                className="input"
                value={fuelReserve}
                onChange={(e) => { setFuelReserve(Number(e.target.value) || 1.0); }}
                style={{ width: '100%' }}
              />
            </div>

            <div>
              <label htmlFor="ui-refresh-input" style={{ display: 'block', fontSize: '12px', fontWeight: 600, marginBottom: '4px' }}>
                UI Max Refresh (Hz)
              </label>
              <input
                id="ui-refresh-input"
                type="number"
                min="10"
                max="60"
                className="input"
                value={uiRefresh}
                onChange={(e) => { setUiRefresh(Number(e.target.value) || 60); }}
                style={{ width: '100%' }}
              />
            </div>
          </div>

          <div style={{ marginTop: '12px' }}>
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => { void handleSaveSettings(); }}
              disabled={settingsSave.kind === 'saving' || restarting}
            >
              Save Engine & Telemetry Settings
            </button>
          </div>
        </div>
      </div>

      {/* Application Diagnostics Panel */}
      <div className="panel">
        <div className="panel-header">
          <span>Application Diagnostics</span>
        </div>
        <div className="panel-body">
          <dl className="kv">
            <dt>Version</dt>
            <dd className="num">{info.version}</dd>
            <dt>Build profile</dt>
            <dd>{buildProfileLabel(info.build_profile)}</dd>
            <dt>Environment</dt>
            <dd>{environmentLabel(info.environment)}</dd>
            <dt>Identifier</dt>
            <dd>
              <code>{info.identifier}</code>
            </dd>
            <dt>Configuration file</dt>
            <dd>
              <code>{snapshot.path}</code>
            </dd>
            <dt>Log directory</dt>
            <dd>
              {info.log_dir !== null ? <code>{info.log_dir}</code> : textOrDash(null)}
            </dd>
            <dt>Extra CA file</dt>
            <dd>
              {config.network.extra_ca_file !== null ? (
                <code>{config.network.extra_ca_file}</code>
              ) : (
                textOrDash(null)
              )}
              {config.network.extra_ca_file !== null && !extraCaHonoured && ' (ignored outside development builds)'}
            </dd>
            <dt>Recording directory</dt>
            <dd>
              <code>{config.telemetry.recording_dir ?? 'Default (per-user app data)'}</code>
            </dd>
          </dl>
        </div>
      </div>
    </section>
  );
}
