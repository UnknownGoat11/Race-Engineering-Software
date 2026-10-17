/**
 * Driver Application UI (spec §88–89, §2108–2150).
 * Lightweight, low-overhead interface for sim drivers:
 * - Real-time system status & session cards (LMU, Relay, Recording, Drops)
 * - Connection code display with 1-click copy & kick slot
 * - Stream, recording, session, and demo controls
 * - Optional high-contrast Driver Telemetry pit board tab
 */
import { useState, type JSX } from 'react';
import { PanelErrorBoundary } from '../components/ErrorBoundary';
import { DriverTelemetryTab } from '../components/telemetry/DriverTelemetryTab';
import { useCoreStatus } from '../hooks/useCoreStatus';
import {
  DEMO_SCENARIOS,
  demoStart,
  demoStop,
  driverAddMarker,
  driverEndSession,
  driverKickEngineer,
  driverNewCode,
  driverSetRecording,
  driverStartStreaming,
  driverStopStreaming,
  type DemoScenario,
} from '../lib/ipc';
import { NO_VALUE } from '../lib/units';
import '../styles/screens.css';

function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes)) return NO_VALUE;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

function lmuStateBadgeClass(state: string | undefined): string {
  switch (state) {
    case 'LIVE':
      return 'status-badge-live';
    case 'STALE':
      return 'status-badge-stale';
    case 'PAUSED':
    case 'CONNECTING':
      return 'status-badge-warn';
    case 'ERROR':
      return 'status-badge-error';
    default:
      return 'status-badge-neutral';
  }
}

function networkStateBadgeClass(state: string | undefined): string {
  switch (state) {
    case 'CONNECTED':
      return 'status-badge-live';
    case 'CONNECTING':
    case 'AUTHENTICATING':
    case 'RECONNECTING':
    case 'DEGRADED':
      return 'status-badge-warn';
    case 'FAILED':
      return 'status-badge-error';
    default:
      return 'status-badge-neutral';
  }
}

export function DriverScreen(): JSX.Element {
  const { status, error, refresh } = useCoreStatus();
  const [activeTab, setActiveTab] = useState<'status' | 'telemetry'>('status');
  const [copiedCode, setCopiedCode] = useState(false);
  const [markerText, setMarkerText] = useState('');
  const [selectedScenario, setSelectedScenario] = useState<DemoScenario>('demo');
  const [actionError, setActionError] = useState<string | null>(null);

  const lmu = status?.lmu;
  const session = status?.session;
  const network = status?.network;
  const recording = status?.recording;
  const rates = status?.rates;
  const drops = status?.drops;
  const isSynthetic = status?.origin === 'synthetic';

  const isStreaming = network?.streaming === true;
  const isRecordingActive = recording?.state === 'active';

  const handleCopyCode = async (code: string) => {
    try {
      await navigator.clipboard.writeText(code);
      setCopiedCode(true);
      setTimeout(() => {
        setCopiedCode(false);
      }, 2000);
    } catch {
      setActionError('Failed to copy code to clipboard.');
    }
  };

  const handleToggleStreaming = async () => {
    try {
      setActionError(null);
      if (isStreaming) {
        await driverStopStreaming();
      } else {
        await driverStartStreaming();
      }
      await refresh();
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleNewCode = async () => {
    try {
      setActionError(null);
      await driverNewCode();
      await refresh();
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleKickEngineer = async (connId: number) => {
    try {
      setActionError(null);
      await driverKickEngineer(connId);
      await refresh();
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleToggleRecording = async () => {
    try {
      setActionError(null);
      await driverSetRecording(!isRecordingActive);
      await refresh();
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleAddMarker = async () => {
    if (!markerText.trim()) return;
    try {
      setActionError(null);
      await driverAddMarker(markerText.trim());
      setMarkerText('');
      await refresh();
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleEndSession = async () => {
    try {
      setActionError(null);
      await driverEndSession();
      await refresh();
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleStartDemo = async () => {
    try {
      setActionError(null);
      await demoStart(selectedScenario);
      await refresh();
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleStopDemo = async () => {
    try {
      setActionError(null);
      await demoStop();
      await refresh();
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  };

  const connectCode = network?.connect_code;

  return (
    <div className="screen-container" data-testid="driver-screen">
      {/* System Status Header Banner */}
      <div className="screen-header-banner">
        <div className="screen-header-left">
          <span className={`status-badge ${lmuStateBadgeClass(lmu?.state)}`} data-testid="driver-lmu-state">
            LMU: {lmu?.state ?? 'UNKNOWN'}
          </span>
          {isSynthetic && (
            <span className="status-badge status-badge-warn" data-testid="synthetic-badge">
              SYNTHETIC DATA (DISPLAY ONLY)
            </span>
          )}
          <div className="meta-chip">
            <span className="meta-chip-label">Vehicle</span>
            <span className="meta-chip-value" data-testid="driver-car">
              {session?.car ?? 'No car detected'}
            </span>
          </div>
          <div className="meta-chip">
            <span className="meta-chip-label">Class</span>
            <span className="meta-chip-value" data-testid="driver-class">
              {session?.vehicle_class ?? NO_VALUE}
            </span>
          </div>
          <div className="meta-chip">
            <span className="meta-chip-label">Track</span>
            <span className="meta-chip-value" data-testid="driver-track">
              {session?.track ?? 'No track loaded'}
            </span>
          </div>
          <div className="meta-chip">
            <span className="meta-chip-label">Session</span>
            <span className="meta-chip-value" data-testid="driver-session-type">
              {session?.session_type ?? NO_VALUE}
            </span>
          </div>
        </div>

        <div className="screen-header-right">
          <nav className="shell-nav" aria-label="Driver views">
            <button
              type="button"
              className="btn btn-sm"
              aria-current={activeTab === 'status' ? 'page' : undefined}
              onClick={() => {
                setActiveTab('status');
              }}
            >
              Status Hub
            </button>
            <button
              type="button"
              className="btn btn-sm"
              aria-current={activeTab === 'telemetry' ? 'page' : undefined}
              onClick={() => {
                setActiveTab('telemetry');
              }}
            >
              Telemetry Dashboard
            </button>
          </nav>
        </div>
      </div>

      {actionError && (
        <div className="screen-header-banner" style={{ backgroundColor: 'var(--color-error-bg)', borderColor: 'var(--color-error)' }}>
          <span className="status-badge status-badge-error">ERROR</span>
          <span className="card-row-value">{actionError}</span>
        </div>
      )}

      {error && (
        <div className="screen-header-banner" style={{ backgroundColor: 'var(--color-error-bg)', borderColor: 'var(--color-error)' }}>
          <span className="status-badge status-badge-error">CORE STATUS ERROR</span>
          <span className="card-row-value">{error}</span>
        </div>
      )}

      {activeTab === 'telemetry' ? (
        <PanelErrorBoundary name="Driver Telemetry Dashboard">
          <DriverTelemetryTab />
        </PanelErrorBoundary>
      ) : (
        <>
          {/* Status Cards (2x2 Grid) */}
          <div className="cards-grid">
            {/* LMU Source & Session Card */}
            <div className="card" data-testid="card-lmu-session">
              <div className="card-header">
                <span>LMU Telemetry Source</span>
                <span className={`status-badge ${lmuStateBadgeClass(lmu?.state)}`}>{lmu?.state ?? 'UNKNOWN'}</span>
              </div>
              <div className="card-body">
                <div className="card-row">
                  <span className="card-row-label">Game Version</span>
                  <span className="card-row-value">{lmu?.game_version ?? NO_VALUE}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Session ID</span>
                  <span className="card-row-value">{session?.session_id ?? NO_VALUE}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Read Rate</span>
                  <span className="card-row-value">
                    {rates?.telemetry_read_hz !== null && rates?.telemetry_read_hz !== undefined
                      ? `${rates.telemetry_read_hz.toFixed(0)} Hz`
                      : NO_VALUE}
                  </span>
                </div>
                {lmu?.detail && (
                  <div className="card-row">
                    <span className="card-row-label">Source Detail</span>
                    <span className="card-row-value" style={{ fontSize: '11px' }}>
                      {lmu.detail}
                    </span>
                  </div>
                )}
              </div>
            </div>

            {/* Relay Streaming & Connection Code Card */}
            <div className="card" data-testid="card-relay-stream">
              <div className="card-header">
                <span>Relay Stream</span>
                <span className={`status-badge ${networkStateBadgeClass(network?.state)}`}>
                  {network?.state ?? 'DISCONNECTED'}
                </span>
              </div>
              <div className="card-body">
                <div className="card-row">
                  <span className="card-row-label">Streaming State</span>
                  <span className="card-row-value">{isStreaming ? 'ACTIVE (100 Hz)' : 'OFFLINE'}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Relay Latency (RTT)</span>
                  <span className="card-row-value">
                    {network?.rtt_ms !== null && network?.rtt_ms !== undefined
                      ? `${network.rtt_ms.toFixed(0)} ms`
                      : NO_VALUE}
                  </span>
                </div>

                {/* Connect Code Box */}
                <div className="connect-code-box">
                  <div>
                    <div className="card-row-label">ENGINEER SESSION CODE</div>
                    <div className="connect-code-display" data-testid="driver-connect-code">
                      {connectCode ?? 'NO CODE'}
                    </div>
                  </div>
                  {connectCode && (
                    <button
                      type="button"
                      className="btn btn-sm"
                      onClick={() => {
                        void handleCopyCode(connectCode);
                      }}
                      data-testid="copy-code-btn"
                    >
                      {copiedCode ? 'Copied!' : 'Copy Code'}
                    </button>
                  )}
                </div>

                <div className="card-row">
                  <span className="card-row-label">Engineers Connected</span>
                  <span className="card-row-value" data-testid="engineers-count">
                    {network?.engineers_connected ?? 0} / {network?.engineers_max ?? 2}
                  </span>
                </div>

                {/* Active Engineers List */}
                {network?.engineers && network.engineers.length > 0 && (
                  <div className="alerts-list">
                    {network.engineers.map((eng) => (
                      <div key={eng.connection_id} className="alert-item alert-item-info">
                        <span>
                          {eng.display_name} (@{eng.username}) &bull; {eng.link}
                        </span>
                        <button
                          type="button"
                          className="btn btn-sm"
                          style={{ padding: '2px 6px', fontSize: '11px' }}
                          onClick={() => {
                            void handleKickEngineer(eng.connection_id);
                          }}
                          data-testid={`kick-engineer-${eng.connection_id}`}
                        >
                          Kick
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>

            {/* Local Recording Card */}
            <div className="card" data-testid="card-local-recording">
              <div className="card-header">
                <span>Local Recording</span>
                <span
                  className={`status-badge ${
                    recording?.state === 'active'
                      ? 'status-badge-live'
                      : recording?.state === 'failed'
                        ? 'status-badge-error'
                        : 'status-badge-neutral'
                  }`}
                >
                  {recording ? recording.state.toUpperCase() : 'OFF'}
                </span>
              </div>
              <div className="card-body">
                <div className="card-row">
                  <span className="card-row-label">Frames Written</span>
                  <span className="card-row-value">{recording?.frames_written.toLocaleString() ?? '0'}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Bytes Written</span>
                  <span className="card-row-value">{formatBytes(recording?.bytes_written)}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Disk Free</span>
                  <span className="card-row-value">{formatBytes(recording?.disk_free_bytes)}</span>
                </div>
                {recording?.error && (
                  <div className="card-row" style={{ color: 'var(--color-error)' }}>
                    <span className="card-row-label">Error</span>
                    <span className="card-row-value">{recording.error}</span>
                  </div>
                )}
              </div>
            </div>

            {/* Performance & Drops Card */}
            <div className="card" data-testid="card-metrics-drops">
              <div className="card-header">
                <span>Pipeline Health & Drops</span>
                <span
                  className={`status-badge ${
                    (drops?.reader_overrun ?? 0) > 0 || (drops?.pipeline_overrun ?? 0) > 0
                      ? 'status-badge-warn'
                      : 'status-badge-live'
                  }`}
                >
                  {(drops?.reader_overrun ?? 0) > 0 ? 'DEGRADED' : 'HEALTHY'}
                </span>
              </div>
              <div className="card-body">
                <div className="card-row">
                  <span className="card-row-label">Network Send Rate</span>
                  <span className="card-row-value">
                    {rates?.frames_sent_hz !== null && rates?.frames_sent_hz !== undefined
                      ? `${rates.frames_sent_hz.toFixed(0)} Hz`
                      : NO_VALUE}
                  </span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Overruns (Reader / Pipeline)</span>
                  <span className="card-row-value">
                    {drops?.reader_overrun ?? 0} / {drops?.pipeline_overrun ?? 0}
                  </span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Recorder Drops</span>
                  <span className="card-row-value">{drops?.recorder_overrun ?? 0}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Net Drops (Obsolete / Events)</span>
                  <span className="card-row-value">
                    {drops?.net_dropped_obsolete ?? 0} / {drops?.net_events_dropped ?? 0}
                  </span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">UI Buffer Overruns</span>
                  <span className="card-row-value">{drops?.ui_dropped ?? 0}</span>
                </div>
              </div>
            </div>
          </div>

          {/* Action Controls Bar */}
          <div className="action-bar" data-testid="driver-action-bar">
            <div className="action-bar-group">
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => {
                  void handleToggleStreaming();
                }}
                data-testid="toggle-streaming-btn"
              >
                {isStreaming ? 'Stop Streaming' : 'Start Streaming'}
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => {
                  void handleNewCode();
                }}
                data-testid="new-code-btn"
              >
                Generate New Code
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => {
                  void handleToggleRecording();
                }}
                data-testid="toggle-recording-btn"
              >
                {isRecordingActive ? 'Pause Recording' : 'Resume Recording'}
              </button>
              <button
                type="button"
                className="btn btn-danger"
                onClick={() => {
                  void handleEndSession();
                }}
                data-testid="end-session-btn"
              >
                End Session
              </button>
            </div>

            <div className="action-bar-group">
              <input
                type="text"
                className="input"
                style={{ width: '180px' }}
                placeholder="Log marker note..."
                value={markerText}
                onChange={(e) => {
                  setMarkerText(e.target.value);
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void handleAddMarker();
                }}
                data-testid="marker-input"
              />
              <button
                type="button"
                className="btn"
                onClick={() => {
                  void handleAddMarker();
                }}
                disabled={!markerText.trim()}
                data-testid="add-marker-btn"
              >
                Add Marker
              </button>
            </div>
          </div>

          {/* Demo Controls Bar */}
          <div className="action-bar" style={{ backgroundColor: 'var(--color-surface-raised)' }}>
            <div className="action-bar-group">
              <span className="card-row-label">Synthetic Demo Scenario:</span>
              <select
                className="select"
                value={selectedScenario}
                onChange={(e) => {
                  setSelectedScenario(e.target.value as DemoScenario);
                }}
                data-testid="demo-scenario-select"
              >
                {DEMO_SCENARIOS.map((sc) => (
                  <option key={sc} value={sc}>
                    {sc}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className="btn"
                onClick={() => {
                  void handleStartDemo();
                }}
                data-testid="start-demo-btn"
              >
                Start Demo
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => {
                  void handleStopDemo();
                }}
                data-testid="stop-demo-btn"
              >
                Stop Demo
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
