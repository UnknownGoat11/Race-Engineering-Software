/**
 * Driver Application UI (spec §88–89, §2108–2150, REDESIGN.md §3.1 & §4 W3.1).
 * Lightweight, glanceable interface for sim drivers:
 * - 5 primary glance tiles (LMU, Recording, Streaming, Engineers, Code)
 * - Tile height 220px (1920), 168px (1366), 293px (2560)
 * - Tile state word 56px (1920), 44px (1366), 75px (2560)
 * - Worst-state escalation (critical tile full red border, warning amber)
 * - Dedicated action bar (64px) with primary Start Streaming (48px)
 * - Worst-first attention rows (48px each, max 3 + N)
 * - Collapsible Details disclosure table (4 columns at 1920, 2 columns at 1366)
 * - Collapsible Developer tools disclosure (visible only when environment !== 'production')
 * - Telemetry Dashboard pit board tab fed via TelemetryStoreContext (4 glance items per C7)
 */
import { useEffect, useMemo, useRef, useState, type JSX } from 'react';
import { PanelErrorBoundary } from '../components/ErrorBoundary';
import { AttentionRow } from '../components/driver/AttentionRow';
import { DeveloperTools } from '../components/driver/DeveloperTools';
import { DriverDetails } from '../components/driver/DriverDetails';
import { GlanceTile } from '../components/driver/GlanceTile';
import { DriverTelemetryTab } from '../components/telemetry/DriverTelemetryTab';
import { Announcer } from '../components/ui/Announcer';
import { ConfirmAction } from '../components/ui/ConfirmAction';
import { useAnnouncer } from '../hooks/useAnnouncer';
import { useCoreStatus } from '../hooks/useCoreStatus';
import {
  demoStart,
  demoStop,
  driverAddMarker,
  driverEndSession,
  driverKickEngineer,
  driverNewCode,
  driverSetRecording,
  driverStartStreaming,
  driverStopStreaming,
  getAppInfo,
  type DemoScenario,
  type EngineerSlotDto,
  type Environment,
} from '../lib/ipc';
import {
  EMPTY_HISTORY,
  deriveDriverTiles,
  recordStatus,
  type StatusHistory,
  type TileAction,
} from '../lib/statusModel';
import { NO_VALUE } from '../lib/units';
import { connectTauriTelemetry } from '../telemetry/bridge';
import { TelemetryStoreContext } from '../telemetry/hooks';
import { TelemetryStore } from '../telemetry/store';
import '../styles/common.css';
import '../styles/driver.css';

export interface DriverScreenProps {
  readonly onRequireLogin?: () => void;
  readonly environment?: Environment;
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
  const [fetchedEnv, setFetchedEnv] = useState<Environment | null>(null);
  const { status, error, refresh } = useCoreStatus();
  const [history, setHistory] = useState<StatusHistory>(EMPTY_HISTORY);
  const [nowMs, setNowMs] = useState(0);
  const [activeTab, setActiveTab] = useState<'status' | 'telemetry'>('status');
  const [copiedCode, setCopiedCode] = useState(false);
  const [markerText, setMarkerText] = useState('');
  const [selectedScenario, setSelectedScenario] = useState<DemoScenario>('demo');
  const [actionError, setActionError] = useState<string | null>(null);

  const { announce, politeMessage, assertiveMessage } = useAnnouncer();
  const prevLmuStateRef = useRef<string | undefined>(undefined);
  const prevStreamingRef = useRef<boolean | undefined>(undefined);
  const prevRecordingStateRef = useRef<string | undefined>(undefined);
  const prevEngineersRef = useRef<readonly EngineerSlotDto[] | undefined>(undefined);

  // Announce state transitions to screen readers (REDESIGN.md §4 W4.2, A02, A11)
  useEffect(() => {
    if (!status) return;

    // LMU state transition
    const currentLmu = status.lmu.state;
    if (prevLmuStateRef.current !== undefined && prevLmuStateRef.current !== currentLmu) {
      if (currentLmu === 'LIVE') {
        announce('LMU connected', 'polite');
      } else if (currentLmu === 'STALE') {
        announce('LMU data is stale', 'polite');
      } else if (currentLmu === 'ERROR' || currentLmu === 'NOT_RUNNING') {
        announce('LMU not running', 'polite');
      }
    }
    prevLmuStateRef.current = currentLmu;

    // Streaming transition
    const currentStreaming = status.network.streaming;
    if (prevStreamingRef.current !== undefined && prevStreamingRef.current !== currentStreaming) {
      if (currentStreaming) {
        announce('Streaming started', 'polite');
      } else {
        announce('Streaming stopped', 'polite');
      }
    }
    prevStreamingRef.current = currentStreaming;

    // Recording transition
    const currentRec = status.recording.state;
    if (prevRecordingStateRef.current !== undefined && prevRecordingStateRef.current !== currentRec) {
      if (currentRec === 'active') {
        announce('Recording started', 'polite');
      } else if (currentRec === 'paused') {
        announce('Recording paused', 'polite');
      } else if (currentRec === 'failed') {
        announce('Recording failed', 'assertive');
      }
    }
    prevRecordingStateRef.current = currentRec;

    // Engineer joined / left
    const currentEngineers = status.network.engineers;
    if (prevEngineersRef.current !== undefined) {
      const prevIds = new Set(prevEngineersRef.current.map((e) => e.connection_id));
      const currIds = new Set(currentEngineers.map((e) => e.connection_id));
      for (const eng of currentEngineers) {
        if (!prevIds.has(eng.connection_id)) {
          const name = eng.display_name.trim() !== '' ? eng.display_name : eng.username;
          announce(`Engineer ${name} joined`, 'polite');
        }
      }
      for (const eng of prevEngineersRef.current) {
        if (!currIds.has(eng.connection_id)) {
          const name = eng.display_name.trim() !== '' ? eng.display_name : eng.username;
          announce(`Engineer ${name} left`, 'polite');
        }
      }
    }
    prevEngineersRef.current = currentEngineers;
  }, [status, announce]);

  useEffect(() => {
    if (actionError) {
      announce(`Error: ${actionError}`, 'assertive');
    }
  }, [actionError, announce]);

  // Details disclosure state & active section highlighted
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [activeSection, setActiveSection] = useState<
    'lmu' | 'recording' | 'streaming' | 'engineers' | 'code' | 'pipeline' | null
  >(null);

  // Resolve environment for developer tools gating (T27)
  useEffect(() => {
    if (props?.environment) return;
    let active = true;
    getAppInfo()
      .then((info) => {
        if (active) {
          setFetchedEnv(info.environment);
        }
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [props?.environment]);

  const env: Environment = props?.environment ?? fetchedEnv ?? 'production';

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
  const isSynthetic = status?.origin === 'synthetic';

  const isStreaming = network?.streaming === true;
  const isRecordingActive = recording?.state === 'active';
  const currentCode = tiles.code.code;

  const handleCopyCode = async (code: string) => {
    try {
      await navigator.clipboard.writeText(code);
      setCopiedCode(true);
      announce('Engineer code copied to clipboard', 'polite');
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

  const handleTileClick = (section: 'lmu' | 'recording' | 'streaming' | 'engineers' | 'code') => {
    setDetailsOpen(true);
    setActiveSection(section);
  };

  const handleAttentionAction = (action: TileAction) => {
    switch (action) {
      case 'sign_in':
        if (onRequireLogin) {
          onRequireLogin();
        } else {
          setActionError('Please sign in to continue streaming.');
        }
        break;
      case 'new_code':
        void handleNewCode();
        break;
      case 'open_recordings':
        // Highlight local recording section in details
        setDetailsOpen(true);
        setActiveSection('recording');
        break;
      default:
        break;
    }
  };

  return (
    <TelemetryStoreContext value={store}>
      <div className="screen-container driver-screen-container" data-testid="driver-screen">
        {/* System Status Header Banner (C6 session context in app bar) */}
        <div className="screen-header-banner driver-header-banner">
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
            <nav className="shell-nav" role="tablist" aria-label="Driver views">
              <button
                type="button"
                role="tab"
                className="btn btn-sm"
                aria-selected={activeTab === 'status'}
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
                role="tab"
                className="btn btn-sm"
                aria-selected={activeTab === 'telemetry'}
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

        {/* Live region announcer per A02, A11 */}
        <Announcer politeMessage={politeMessage} assertiveMessage={assertiveMessage} />

        {/* Demo banner per §3.1 */}
        {isSynthetic && (
          <div className="driver-demo-band" data-testid="driver-demo-band">
            ▨ DEMO — synthetic data, not recorded or streamed ▨
          </div>
        )}

        {actionError && (
          <div className="driver-error-banner" role="alert">
            <span className="status-badge status-badge-error">ERROR</span>
            <span className="driver-error-message">{actionError}</span>
            <button
              type="button"
              className="btn btn-xs driver-error-dismiss"
              onClick={() => {
                setActionError(null);
              }}
              aria-label="Dismiss error"
              data-testid="driver-dismiss-error-btn"
            >
              Dismiss
            </button>
          </div>
        )}

        {error && (
          <div className="driver-error-banner" role="alert">
            <span className="status-badge status-badge-error">CORE STATUS ERROR</span>
            <span className="driver-error-message">{error}</span>
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
              {/* Tile 1: LMU */}
              <GlanceTile
                id="lmu"
                label="LMU"
                level={tiles.lmu.level}
                word={tiles.lmu.word}
                detail={tiles.lmu.detail}
                reason={tiles.lmu.reason}
                onClick={() => {
                  handleTileClick('lmu');
                }}
              />

              {/* Tile 2: Recording */}
              <GlanceTile
                id="recording"
                label="RECORDING"
                level={tiles.recording.level}
                word={tiles.recording.word}
                detail={tiles.recording.detail}
                reason={tiles.recording.reason}
                onClick={() => {
                  handleTileClick('recording');
                }}
              />

              {/* Tile 3: Streaming */}
              <GlanceTile
                id="streaming"
                label="STREAMING"
                level={tiles.streaming.level}
                word={tiles.streaming.word}
                detail={tiles.streaming.detail}
                reason={tiles.streaming.reason}
                onClick={() => {
                  handleTileClick('streaming');
                }}
              />

              {/* Tile 4: Engineers */}
              <GlanceTile
                id="engineers"
                label="ENGINEERS"
                level={tiles.engineers.level}
                word={tiles.engineers.word}
                detail={tiles.engineers.detail}
                reason={tiles.engineers.reason}
                onClick={() => {
                  handleTileClick('engineers');
                }}
              >
                {network?.engineers && network.engineers.length > 0 && (
                  <div className="glance-engineers-list">
                    {network.engineers.map((eng) => {
                      const engineerName = eng.display_name || eng.username;
                      const handle = eng.username ? `@${eng.username}` : '';
                      const removeAriaLabel = handle
                        ? `Remove ${engineerName} (${handle})`
                        : `Remove ${engineerName}`;

                      return (
                        <div key={eng.connection_id} className="glance-engineer-item">
                          <div className="glance-engineer-info">
                            <span
                              className={`glance-eng-dot glance-eng-dot--${eng.link}`}
                              title={`Link: ${eng.link}`}
                              aria-label={`Link: ${eng.link}`}
                            />
                            <span className="glance-engineer-name" title={engineerName}>
                              {engineerName}
                            </span>
                          </div>
                          <ConfirmAction
                            label="Remove"
                            ariaLabel={removeAriaLabel}
                            prompt={`Remove ${engineerName}?`}
                            consequence={`Disconnects ${engineerName}. They will need a new code to reconnect.`}
                            onConfirm={() => handleKickEngineer(eng.connection_id)}
                            variant="danger"
                            size="sm"
                            testId={`kick-engineer-${eng.connection_id}`}
                          />
                        </div>
                      );
                    })}
                  </div>
                )}
              </GlanceTile>

              {/* Tile 5: Engineer Code */}
              <GlanceTile
                id="code"
                label="ENGINEER CODE"
                level={tiles.code.level}
                word={currentCode ?? (tiles.code.sessionFull ? 'SESSION FULL' : tiles.code.word)}
                detail={tiles.code.detail}
                reason={tiles.code.reason}
                isCode={true}
                isSessionFull={tiles.code.sessionFull}
                onClick={() => {
                  handleTileClick('code');
                }}
              >
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
              </GlanceTile>
            </div>

            {/* Action Bar (64px, §3.1) */}
            <div className="action-bar driver-action-bar" data-testid="driver-action-bar">
              <div className="action-bar-group">
                <button
                  type="button"
                  className={isStreaming ? 'btn btn-driver-action' : 'btn btn-primary btn-driver-action'}
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
                  className="btn btn-driver-action"
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
                <label htmlFor="driver-marker-input" className="sr-only">
                  Log marker note
                </label>
                <input
                  id="driver-marker-input"
                  type="text"
                  className="input driver-marker-input"
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
                  className="btn btn-driver-action"
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

            {/* Attention Strip (Worst-first issues per §2.7, §3.1) */}
            {tiles.attention.length > 0 && (
              <div className="driver-attention-strip" data-testid="driver-attention-strip">
                {tiles.attention.slice(0, 3).map((item) => (
                  <AttentionRow
                    key={item.id}
                    item={item}
                    onAction={handleAttentionAction}
                  />
                ))}
                {tiles.attention.length > 3 && (
                  <div className="attention-overflow">
                    +{tiles.attention.length - 3} more issues
                  </div>
                )}
              </div>
            )}

            {/* Diagnostics Details Table (Collapsed by default per §3.1) */}
            <DriverDetails
              status={status}
              tiles={tiles}
              nowMs={nowMs}
              open={detailsOpen}
              onToggle={setDetailsOpen}
              activeSection={activeSection}
            />

            {/* Developer / Demo Controls (Collapsible, hidden in production per §3.1) */}
            <DeveloperTools
              environment={env}
              isSynthetic={isSynthetic}
              isStreaming={isStreaming}
              isRecordingActive={isRecordingActive}
              selectedScenario={selectedScenario}
              onSelectScenario={setSelectedScenario}
              onStartDemo={handleStartDemo}
              onStopDemo={handleStopDemo}
            />
          </>
        )}
      </div>
    </TelemetryStoreContext>
  );
}
