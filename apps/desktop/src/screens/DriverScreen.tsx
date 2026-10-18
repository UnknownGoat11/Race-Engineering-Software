/**
 * Driver Application UI (spec §88–89, §2108–2150).
 * Lightweight, low-overhead interface for sim drivers:
 * - Real-time glanceable status tiles derived via deriveDriverTiles (LMU, Recording, Streaming, Engineers, Code)
 * - Attention strip for pipeline overruns, recording failures, or relogin needs
 * - Connection code display with countdown, single-use, and session full guard
 * - Safe actions with inline ConfirmAction for destructive actions (End session, Remove engineer)
 * - Collapsible diagnostics details & developer demo tools
 * - Telemetry Dashboard pit board tab fed via TelemetryStoreContext
 */
import { useEffect, useMemo, useState, type JSX } from 'react';
import { PanelErrorBoundary } from '../components/ErrorBoundary';
import { ConfirmAction } from '../components/ui/ConfirmAction';
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
import { demoScenarioLabel } from '../lib/labels';
import {
  EMPTY_HISTORY,
  deriveDriverTiles,
  recordStatus,
  type Level,
  type StatusHistory,
} from '../lib/statusModel';
import { NO_VALUE, formatByteSize } from '../lib/units';
import { connectTauriTelemetry } from '../telemetry/bridge';
import { TelemetryStoreContext } from '../telemetry/hooks';
import { TelemetryStore } from '../telemetry/store';
import '../styles/common.css';
import '../styles/driver.css';

export interface DriverScreenProps {
  readonly onRequireLogin?: () => void;
}

function levelGlyph(level: Level): string {
  switch (level) {
    case 'ok':
      return '●';
    case 'warning':
      return '◆';
    case 'critical':
      return '■';
    case 'synthetic':
      return '▨';
    case 'off':
    default:
      return '□';
  }
}

function lmuStateBadgeClass(state: string | undefined, isSynthetic: boolean): string {
  if (isSynthetic) return 'status-badge-warn';
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

export function DriverScreen(props?: DriverScreenProps): JSX.Element {
  const onRequireLogin = props?.onRequireLogin;
  const { status, error, refresh } = useCoreStatus();
  const [history, setHistory] = useState<StatusHistory>(EMPTY_HISTORY);
  const [nowMs, setNowMs] = useState(0);
  const [activeTab, setActiveTab] = useState<'status' | 'telemetry'>('status');
  const [copiedCode, setCopiedCode] = useState(false);
  const [markerText, setMarkerText] = useState('');
  const [selectedScenario, setSelectedScenario] = useState<DemoScenario>('demo');
  const [actionError, setActionError] = useState<string | null>(null);

  // Live TelemetryStore & Bridge connection (fixes B06)
  const store = useMemo(() => new TelemetryStore(), []);
  useEffect(() => {
    const conn = connectTauriTelemetry(store);
    return () => {
      conn.disconnect();
    };
  }, [store]);

  // Keep rolling history of status drops and engineer links
  useEffect(() => {
    if (status) {
      const now = Date.now();
      queueMicrotask(() => {
        setNowMs(now);
        setHistory((h) => recordStatus(h, status, now));
      });
    }
  }, [status]);

  // Truthful tiles and attention items derived from status and history
  const tiles = useMemo(() => {
    return deriveDriverTiles(status, history, { nowMs });
  }, [status, history, nowMs]);

  const lmu = status?.lmu;
  const session = status?.session;
  const network = status?.network;
  const recording = status?.recording;
  const rates = status?.rates;
  const drops = status?.drops;
  const isSynthetic = status?.origin === 'synthetic';

  const isStreaming = network?.streaming === true;
  const isRecordingActive = recording?.state === 'active';
  const currentCode = tiles.code.code;

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

  return (
    <TelemetryStoreContext value={store}>
      <div className="screen-container" data-testid="driver-screen">
        {/* System Status Header Banner */}
        <div className="screen-header-banner">
          <div className="screen-header-left">
            <span
              className={`status-badge ${lmuStateBadgeClass(lmu?.state, isSynthetic)}`}
              data-testid="driver-lmu-state"
            >
              LMU: {status ? (isSynthetic ? 'DEMO' : lmu?.state ?? 'UNKNOWN') : 'UNKNOWN'}
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
                data-testid="tab-status"
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
                data-testid="tab-telemetry"
              >
                Telemetry Dashboard
              </button>
            </nav>
          </div>
        </div>

        {/* Demo banner per §3.1 */}
        {isSynthetic && (
          <div className="driver-demo-band" data-testid="driver-demo-band">
            ▨ DEMO — synthetic data, not recorded or streamed ▨
          </div>
        )}

        {actionError && (
          <div
            className="screen-header-banner"
            style={{ backgroundColor: 'var(--color-error-bg)', borderColor: 'var(--color-error)' }}
          >
            <span className="status-badge status-badge-error">ERROR</span>
            <span className="card-row-value">{actionError}</span>
          </div>
        )}

        {error && (
          <div
            className="screen-header-banner"
            style={{ backgroundColor: 'var(--color-error-bg)', borderColor: 'var(--color-error)' }}
          >
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
            {/* Glanceable Status Strip (5 Tiles per §3.1) */}
            <div className="driver-status-strip" data-testid="driver-status-strip">
              {/* LMU Tile */}
              <div
                className={`glance-tile glance-tile-${tiles.lmu.level}`}
                data-testid="tile-lmu"
              >
                <div className="glance-header">
                  <span className="glance-label">LMU</span>
                  <span className="glance-glyph" aria-hidden="true">
                    {levelGlyph(tiles.lmu.level)}
                  </span>
                </div>
                <div className="glance-body">
                  <div className="glance-word">{tiles.lmu.word}</div>
                </div>
                <div className="glance-footer">
                  {tiles.lmu.detail && <span className="glance-detail">{tiles.lmu.detail}</span>}
                  {tiles.lmu.reason && <span className="glance-reason">{tiles.lmu.reason}</span>}
                </div>
              </div>

              {/* Recording Tile */}
              <div
                className={`glance-tile glance-tile-${tiles.recording.level}`}
                data-testid="tile-recording"
              >
                <div className="glance-header">
                  <span className="glance-label">RECORDING</span>
                  <span className="glance-glyph" aria-hidden="true">
                    {levelGlyph(tiles.recording.level)}
                  </span>
                </div>
                <div className="glance-body">
                  <div className="glance-word">{tiles.recording.word}</div>
                </div>
                <div className="glance-footer">
                  {tiles.recording.detail && (
                    <span className="glance-detail">{tiles.recording.detail}</span>
                  )}
                  {tiles.recording.reason && (
                    <span className="glance-reason">{tiles.recording.reason}</span>
                  )}
                </div>
              </div>

              {/* Streaming Tile */}
              <div
                className={`glance-tile glance-tile-${tiles.streaming.level}`}
                data-testid="tile-streaming"
              >
                <div className="glance-header">
                  <span className="glance-label">STREAMING</span>
                  <span className="glance-glyph" aria-hidden="true">
                    {levelGlyph(tiles.streaming.level)}
                  </span>
                </div>
                <div className="glance-body">
                  <div className="glance-word">{tiles.streaming.word}</div>
                </div>
                <div className="glance-footer">
                  {tiles.streaming.detail && (
                    <span className="glance-detail">{tiles.streaming.detail}</span>
                  )}
                  {tiles.streaming.reason && (
                    <span className="glance-reason">{tiles.streaming.reason}</span>
                  )}
                </div>
              </div>

              {/* Engineers Tile */}
              <div
                className={`glance-tile glance-tile-${tiles.engineers.level}`}
                data-testid="tile-engineers"
              >
                <div className="glance-header">
                  <span className="glance-label">ENGINEERS</span>
                  <span className="glance-glyph" aria-hidden="true">
                    {levelGlyph(tiles.engineers.level)}
                  </span>
                </div>
                <div className="glance-body">
                  <div className="glance-word" data-testid="engineers-count">
                    {tiles.engineers.word}
                  </div>
                </div>
                <div className="glance-footer">
                  {tiles.engineers.detail && (
                    <span className="glance-detail">{tiles.engineers.detail}</span>
                  )}
                  {tiles.engineers.reason && (
                    <span className="glance-reason">{tiles.engineers.reason}</span>
                  )}
                  {network?.engineers && network.engineers.length > 0 && (
                    <div className="glance-engineers-list">
                      {network.engineers.map((eng) => (
                        <div key={eng.connection_id} className="glance-engineer-item">
                          <span>
                            {eng.display_name || eng.username} &bull; {eng.link}
                          </span>
                          <ConfirmAction
                            label="Remove"
                            prompt={`Remove ${eng.display_name || eng.username}?`}
                            consequence={`Disconnects ${eng.display_name || eng.username}. They will need a new code to reconnect.`}
                            onConfirm={() => handleKickEngineer(eng.connection_id)}
                            variant="danger"
                            size="sm"
                            testId={`kick-engineer-${eng.connection_id}`}
                          />
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </div>

              {/* Engineer Code Tile */}
              <div
                className={`glance-tile glance-tile-${tiles.code.level}`}
                data-testid="tile-code"
              >
                <div className="glance-header">
                  <span className="glance-label">ENGINEER CODE</span>
                  <span className="glance-glyph" aria-hidden="true">
                    {levelGlyph(tiles.code.level)}
                  </span>
                </div>
                <div className="glance-body">
                  {currentCode ? (
                    <span className="glance-code-text" data-testid="driver-connect-code">
                      {currentCode}
                    </span>
                  ) : tiles.code.sessionFull ? (
                    <span className="glance-code-full" data-testid="driver-connect-code">
                      SESSION FULL
                    </span>
                  ) : (
                    <span className="glance-code-slots" data-testid="driver-connect-code">
                      {tiles.code.word === NO_VALUE ? '——————' : tiles.code.word}
                    </span>
                  )}
                </div>
                <div className="glance-footer">
                  {tiles.code.reason && <span className="glance-reason">{tiles.code.reason}</span>}
                  <div className="glance-code-actions">
                    {currentCode && (
                      <button
                        type="button"
                        className="btn btn-sm"
                        onClick={() => {
                          void handleCopyCode(currentCode);
                        }}
                        data-testid="copy-code-btn"
                      >
                        {copiedCode ? 'Copied!' : 'Copy Code'}
                      </button>
                    )}
                    <button
                      type="button"
                      className="btn btn-sm"
                      onClick={() => {
                        void handleNewCode();
                      }}
                      disabled={!isStreaming || tiles.code.sessionFull || isSynthetic}
                      title={
                        isSynthetic
                          ? 'Not available in demo'
                          : tiles.code.sessionFull
                            ? 'Session full (2/2). Remove an engineer to add another.'
                            : !isStreaming
                              ? 'Start streaming to get a code'
                              : undefined
                      }
                      data-testid="new-code-btn"
                    >
                      New Code
                    </button>
                  </div>
                </div>
              </div>
            </div>

            {/* Attention Strip (Worst-first issues per §2.7, §3.1) */}
            {tiles.attention.length > 0 && (
              <div className="driver-attention-strip" data-testid="driver-attention-strip">
                {tiles.attention.slice(0, 3).map((item) => (
                  <div
                    key={item.id}
                    className={`attention-row attention-row-${item.level}`}
                    data-testid={`attention-${item.id}`}
                  >
                    <span className="attention-glyph" aria-hidden="true">
                      {item.level === 'critical' ? '■' : '◆'}
                    </span>
                    <div className="attention-body">
                      <strong className="attention-title">{item.title}</strong>
                      {item.detail && <span className="attention-detail"> — {item.detail}</span>}
                    </div>
                    {item.action === 'sign_in' && (
                      <button
                        type="button"
                        className="btn btn-sm btn-primary"
                        onClick={() => {
                          if (onRequireLogin) {
                            onRequireLogin();
                          } else {
                            setActionError('Please sign in to continue streaming.');
                          }
                        }}
                        data-testid="attention-action-sign-in"
                      >
                        Sign in again
                      </button>
                    )}
                    {item.action === 'new_code' && (
                      <button
                        type="button"
                        className="btn btn-sm"
                        onClick={() => {
                          void handleNewCode();
                        }}
                        data-testid="attention-action-new-code"
                      >
                        New code
                      </button>
                    )}
                  </div>
                ))}
                {tiles.attention.length > 3 && (
                  <div className="attention-overflow">
                    +{tiles.attention.length - 3} more issues
                  </div>
                )}
              </div>
            )}

            {/* Action Controls Bar */}
            <div className="action-bar" data-testid="driver-action-bar">
              <div className="action-bar-group">
                <button
                  type="button"
                  className={isStreaming ? 'btn' : 'btn btn-primary'}
                  disabled={isSynthetic}
                  title={isSynthetic ? 'Demo is display-only' : undefined}
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
                    void handleToggleRecording();
                  }}
                  disabled={isSynthetic || recording?.state === 'failed'}
                  title={
                    isSynthetic
                      ? 'Demo is not recorded'
                      : recording?.state === 'failed'
                        ? (recording.error ?? 'Recording failed')
                        : undefined
                  }
                  data-testid="toggle-recording-btn"
                >
                  {isRecordingActive ? 'Pause Recording' : 'Resume Recording'}
                </button>
                <ConfirmAction
                  label="End Session"
                  prompt="End session for everyone?"
                  consequence="Ends the session for you and all connected engineers."
                  onConfirm={handleEndSession}
                  disabled={!session?.session_id}
                  disabledReason="No active session to end."
                  variant="danger"
                  testId="end-session-btn"
                />
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
                  disabled={!isRecordingActive}
                  data-testid="marker-input"
                />
                <button
                  type="button"
                  className="btn"
                  onClick={() => {
                    void handleAddMarker();
                  }}
                  disabled={!isRecordingActive || !markerText.trim()}
                  data-testid="add-marker-btn"
                >
                  Add Marker
                </button>
              </div>
            </div>

            {/* Diagnostics Details Table (Collapsed by default per §3.1) */}
            <details className="driver-details" data-testid="driver-details">
              <summary className="driver-details-summary">▸ Details (Diagnostics & Pipeline)</summary>
              <div className="driver-details-grid">
                {/* Source Column */}
                <div className="driver-details-col">
                  <h4 className="driver-details-title">LMU Telemetry Source</h4>
                  <div className="driver-details-row">
                    <span>State</span>
                    <span>{lmu?.state ?? NO_VALUE}</span>
                  </div>
                  <div className="driver-details-row">
                    <span>Game Version</span>
                    <span>{lmu?.game_version !== null && lmu?.game_version !== undefined ? String(lmu.game_version) : NO_VALUE}</span>
                  </div>
                  <div className="driver-details-row">
                    <span>Read Rate</span>
                    <span>
                      {rates?.telemetry_read_hz !== null && rates?.telemetry_read_hz !== undefined
                        ? `${Math.round(rates.telemetry_read_hz)} Hz`
                        : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>Detail</span>
                    <span>{lmu?.detail ?? NO_VALUE}</span>
                  </div>
                </div>

                {/* Streaming Column */}
                <div className="driver-details-col">
                  <h4 className="driver-details-title">Relay Streaming</h4>
                  <div className="driver-details-row">
                    <span>State</span>
                    <span>{network?.state ?? NO_VALUE}</span>
                  </div>
                  <div className="driver-details-row">
                    <span>Send Rate</span>
                    <span>
                      {rates?.frames_sent_hz !== null && rates?.frames_sent_hz !== undefined
                        ? `${Math.round(rates.frames_sent_hz)} Hz`
                        : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>Relay RTT</span>
                    <span>
                      {network?.rtt_ms !== null && network?.rtt_ms !== undefined
                        ? `${Math.round(network.rtt_ms)} ms`
                        : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>Degraded Reason</span>
                    <span>{network?.degraded_reason ?? NO_VALUE}</span>
                  </div>
                  <div className="driver-details-row">
                    <span>Last Error</span>
                    <span>{network?.last_error ?? NO_VALUE}</span>
                  </div>
                </div>

                {/* Recording Column */}
                <div className="driver-details-col">
                  <h4 className="driver-details-title">Local Recording</h4>
                  <div className="driver-details-row">
                    <span>State</span>
                    <span>{recording?.state ?? NO_VALUE}</span>
                  </div>
                  <div className="driver-details-row">
                    <span>Frames Written</span>
                    <span>
                      {recording ? recording.frames_written.toLocaleString() : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>Bytes Written</span>
                    <span>
                      {recording ? formatByteSize(recording.bytes_written) : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>Disk Free</span>
                    <span>
                      {recording?.disk_free_bytes !== null && recording?.disk_free_bytes !== undefined
                        ? formatByteSize(recording.disk_free_bytes)
                        : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>Error</span>
                    <span>{recording?.error ?? NO_VALUE}</span>
                  </div>
                </div>

                {/* Pipeline Health Column */}
                <div className="driver-details-col">
                  <h4 className="driver-details-title">Pipeline Health</h4>
                  <div className="driver-details-row">
                    <span>Overall Health</span>
                    <span>{tiles.pipeline.word} ({tiles.pipeline.label})</span>
                  </div>
                  <div className="driver-details-row">
                    <span>Reader Overrun</span>
                    <span>
                      {drops ? drops.reader_overrun.toLocaleString() : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>Pipeline Overrun</span>
                    <span>
                      {drops ? drops.pipeline_overrun.toLocaleString() : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>Recorder Drops</span>
                    <span>
                      {drops ? drops.recorder_overrun.toLocaleString() : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>Net Dropped</span>
                    <span>
                      {drops ? drops.net_dropped_obsolete.toLocaleString() : NO_VALUE}
                    </span>
                  </div>
                  <div className="driver-details-row">
                    <span>UI Dropped</span>
                    <span>
                      {drops ? drops.ui_dropped.toLocaleString() : NO_VALUE}
                    </span>
                  </div>
                </div>
              </div>
            </details>

            {/* Developer / Demo Controls (Collapsible) */}
            <details className="driver-developer" data-testid="driver-developer-tools">
              <summary className="driver-developer-summary">▸ Developer Tools (Synthetic Demo)</summary>
              <div
                className="action-bar"
                style={{ marginTop: '8px', backgroundColor: 'var(--color-surface-raised)' }}
              >
                <div className="action-bar-group">
                  <span className="card-row-label">Demo Scenario:</span>
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
                        {demoScenarioLabel(sc)} ({sc})
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    className="btn"
                    onClick={() => {
                      void handleStartDemo();
                    }}
                    disabled={isStreaming || isRecordingActive}
                    title={
                      isStreaming || isRecordingActive
                        ? 'Cannot start demo while streaming or recording'
                        : undefined
                    }
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
                    disabled={!isSynthetic}
                    title={!isSynthetic ? 'Demo is not active' : undefined}
                    data-testid="stop-demo-btn"
                  >
                    Stop Demo
                  </button>
                </div>
              </div>
            </details>
          </>
        )}
      </div>
    </TelemetryStoreContext>
  );
}
