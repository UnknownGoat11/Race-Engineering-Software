/**
 * Engineer Pitwall Screen (REDESIGN.md §3.2–§3.4 & §4 W3.2).
 * Professional race engineering pitwall workstation:
 * - Full-bleed pitwall layout (pitwall bar at y0, shell header hidden)
 * - Pitwall Bar (36px): stream state pill, driver/car/track metadata, FCY/SC, RTT, stream age, controls
 * - Strategy Strip (32px): session clock, completed laps, stint laps, fuel laps left, VE laps left, tyre stint, track/air temps, weather, flag
 * - Scoring comes from `useScoring` (adapter `toScoringView` over the real `core:scoring` JSON, audit U02)
 * - State Banner (28px): incompatible, ended, driver away, stale, FCY caution
 * - Primary 1920x1080 4-column ergonomic pitwall grid (content height 992px, zero tabs):
 *   - Column 1 (384px): Standings Tower
 *   - Column 2 (1fr): Alert strip (0-1x28px +N), Track Map (584x340) | Session log, Minisector row (24px, only
 *     with a value), Telemetry Graph (MoTeC bar only on drag-over / error / loaded file)
 *   - Column 3 (Right-A, 280px): Timing (240px), Fuel & Energy (352px), Driver Inputs (176px)
 *   - Column 4 (Right-B, 272px): Tyres (456px), Brakes (224px), Hybrid Energy (296px)
 * - Responsive 1366x768 3-column pitwall grid:
 *   - Column 1 (272px): Timing Tower
 *   - Column 2 (1fr): Full-width Track Map (240px), Telemetry Graph (>=300px plot); no minisector row
 *   - Column 3 (300px): RightColumnTabs with pinned mini-row (24px) + segmented tabs (28px)
 */
import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type JSX } from 'react';
import { PanelErrorBoundary } from '../components/ErrorBoundary';
import { EventsLog } from '../components/engineer/EventsLog';
import { MinisectorRibbon } from '../components/engineer/MinisectorRibbon';
import { PitwallBar } from '../components/engineer/PitwallBar';
import { RightColumnTabs, type RightTab } from '../components/engineer/RightColumnTabs';
import { StateBanner } from '../components/engineer/StateBanner';
import { StrategyStrip } from '../components/engineer/StrategyStrip';
import { Announcer } from '../components/ui/Announcer';
import { useAnnouncer } from '../hooks/useAnnouncer';
import { AlertsBanner } from '../components/telemetry/AlertsBanner';
import { BrakePanel } from '../components/telemetry/BrakePanel';
import { ChannelSelectorModal } from '../components/telemetry/ChannelSelectorModal';
import { DriverInputsPanel } from '../components/telemetry/DriverInputsPanel';
import { EnergyPanel } from '../components/telemetry/EnergyPanel';
import { FuelPanel } from '../components/telemetry/FuelPanel';
import { ScopeDqContext, type ScopeDq } from '../components/telemetry/presentation';
import { SuspensionPanel } from '../components/telemetry/SuspensionPanel';
import { TimingPanel } from '../components/telemetry/TimingPanel';
import { TimingTower } from '../components/telemetry/TimingTower';
import { TyrePanel } from '../components/telemetry/TyrePanel';
import { TrackMap } from '../components/trackmap/TrackMap';
import type { CompetitorMarker, DriverPosition } from '../components/trackmap/types';
import { useTrackGeometry } from '../components/trackmap/useTrackGeometry';
import { ch, FrequencyClass } from '../generated/channels';
import { LiveRingSource, type LiveRingSourceOptions } from '../graph/liveSource';
import { TelemetryGraph } from '../graph/TelemetryGraph';
import type { GraphSeries, XAxisMode } from '../graph/types';
import { useMotecDropZone } from '../motec/motecDropZone';
import { MotecAddButton, MotecImportBar } from '../motec/MotecImportBar';
import { useMotecReferences } from '../motec/useMotecStore';
import {
  getWorksheet,
  DEFAULT_WORKSHEET_ID,
  type Worksheet,
  type WorksheetId,
} from '../graph/worksheets';
import { useAlerts } from '../hooks/useAlerts';
import { useCoreStatus } from '../hooks/useCoreStatus';
import { useScoring } from '../hooks/useScoring';
import { parseVehicleClass } from '../lib/classes';
import { engineerDisconnect, engineerRequestSnapshot, type Origin } from '../lib/ipc';
import { COPY } from '../lib/labels';
import { deriveEngineerView, mayShowDriverIdentity } from '../lib/statusModel';
import { NO_VALUE } from '../lib/units';
import { connectTauriTelemetry } from '../telemetry/bridge';
import { TelemetryStoreContext } from '../telemetry/hooks';
import { TelemetryStore } from '../telemetry/store';
import { JoinSessionModal } from './JoinSessionModal';
import '../styles/common.css';
import '../styles/engineer.css';
import '../styles/modal.css';

function useIsWide(breakpoint = 1800): boolean {
  return useSyncExternalStore(
    (callback) => {
      window.addEventListener('resize', callback);
      return () => {
        window.removeEventListener('resize', callback);
      };
    },
    () => (typeof window !== 'undefined' ? window.innerWidth >= breakpoint : true),
    () => true,
  );
}

export interface EngineerScreenProps {
  readonly onOpenSettings?: (() => void) | undefined;
  readonly onOpenAnalysis?: (() => void) | undefined;
}

export function EngineerScreen({ onOpenSettings, onOpenAnalysis }: EngineerScreenProps): JSX.Element {
  const isWide = useIsWide();
  const { status, error: coreError } = useCoreStatus();
  const scoring = useScoring();
  const { alerts, log: alertLog, dismissAlert, clearAllAlerts } = useAlerts();
  const motecDrop = useMotecDropZone();

  // Selection & presets
  const [selectedSlotId, setSelectedSlotId] = useState<number | null>(null);
  const [worksheetId, setWorksheetId] = useState<WorksheetId>(DEFAULT_WORKSHEET_ID);
  const [xMode, setXMode] = useState<XAxisMode>('lapDistance');
  const [customSeries, setCustomSeries] = useState<readonly GraphSeries[]>([
    { channelId: ch.SPEED, color: 1 },
  ]);
  const [rightTab, setRightTab] = useState<RightTab>('timing_fuel');
  const [showJoinModal, setShowJoinModal] = useState<boolean>(false);
  const [showChannelModal, setShowChannelModal] = useState<boolean>(false);
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [awaySinceMs, setAwaySinceMs] = useState<number | null>(null);

  const { announce, politeMessage, assertiveMessage } = useAnnouncer();
  const prevViewKindRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    if (actionError) {
      announce(`Error: ${actionError}`, 'assertive');
    }
  }, [actionError, announce]);

  // Track when driver away starts (T08)
  const isDriverAway = (status?.engineer?.driver_away ?? false) || (status?.network.driver_away ?? false);
  useEffect(() => {
    const now = Date.now();
    queueMicrotask(() => {
      setAwaySinceMs((prev) => {
        if (isDriverAway) {
          return prev ?? now;
        }
        return null;
      });
    });
  }, [isDriverAway]);

  const currentWorksheet: Worksheet = useMemo(() => {
    if (worksheetId === 'custom') {
      return {
        id: 'custom',
        label: 'Custom',
        panes: [
          {
            id: 'custom-main',
            title: 'Custom Channels',
            proportion: 100,
            series: customSeries,
          },
        ],
      };
    }
    return getWorksheet(worksheetId);
  }, [worksheetId, customSeries]);

  const activeSeries = useMemo(() => {
    const list: GraphSeries[] = [];
    for (const p of currentWorksheet.panes) {
      list.push(...p.series);
    }
    return list;
  }, [currentWorksheet]);

  const activeChannelIds = useMemo(() => activeSeries.map((s) => s.channelId), [activeSeries]);
  // The MoTeC reference is aligned by lap distance or lap time only; other x-axes get no overlay.
  const motecReferences = useMotecReferences(
    xMode === 'lapDistance' || xMode === 'lapTime' ? activeChannelIds : [],
    xMode === 'lapTime' ? 'time' : 'distance',
  );

  const handleApplyCustomChannels = (selectedIds: readonly number[]) => {
    const newSeries: GraphSeries[] = selectedIds.map((channelId, index) => ({
      channelId,
      color: ((index % 8) + 1) as 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8,
      enabled: true,
    }));
    setCustomSeries(newSeries);
    setWorksheetId('custom');
  };

  // Live TelemetryStore & Bridge connection
  const store = useMemo(() => new TelemetryStore(), []);

  useEffect(() => {
    const conn = connectTauriTelemetry(store);
    return () => {
      conn.disconnect();
    };
  }, [store]);

  const dataKind = status?.origin === 'synthetic' ? 'synthetic' : 'lmu';

  const liveSource = useMemo(() => {
    const opts: LiveRingSourceOptions =
      xMode === 'time'
        ? { xMode: 'time', dataKind }
        : {
            xMode,
            xChannel:
              xMode === 'lapDistance'
                ? ch.LAP_DISTANCE
                : xMode === 'lapTime'
                  ? ch.LAP_TIME
                  : ch.LAP_PROGRESS,
            dataKind,
          };
    return new LiveRingSource(store, opts);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, dataKind]);

  useEffect(() => {
    if (xMode === 'time') {
      liveSource.setXMode('time');
    } else {
      const channel =
        xMode === 'lapDistance'
          ? ch.LAP_DISTANCE
          : xMode === 'lapTime'
            ? ch.LAP_TIME
            : ch.LAP_PROGRESS;
      liveSource.setXMode(xMode, channel);
    }
  }, [liveSource, xMode]);

  // Derived Engineer View (statusModel)
  const storeStatus = store.getStatus();
  const effectiveOrigin: Origin | null =
    status?.origin === 'synthetic'
      ? 'synthetic'
      : (storeStatus.origin ?? status?.origin ?? null);

  const storeFreshness =
    storeStatus.batchesApplied > 0 ? store.getStreamState(FrequencyClass.Physics) : null;

  const view = useMemo(
    () =>
      deriveEngineerView({
        status,
        origin: effectiveOrigin,
        storeFreshness,
        store: storeStatus,
        awaySinceMs,
      }),
    [status, effectiveOrigin, storeStatus, storeFreshness, awaySinceMs],
  );

  // Announce view state transitions to screen readers (REDESIGN.md §4 W4.2, A02, A11)
  useEffect(() => {
    if (prevViewKindRef.current !== undefined && prevViewKindRef.current !== view.kind) {
      switch (view.kind) {
        case 'live':
          announce('Connected to driver live stream', 'polite');
          break;
        case 'synthetic':
          announce('Connected to demo stream', 'polite');
          break;
        case 'stale':
          announce('Driver stream is stale', 'polite');
          break;
        case 'away':
          announce('Driver is away from session', 'polite');
          break;
        case 'incompatible':
          announce('Stream incompatible: update required', 'assertive');
          break;
        case 'ended':
          announce('Session ended', 'polite');
          break;
        case 'notConnected':
        case 'waiting':
          announce('Disconnected from driver session', 'polite');
          break;
        default:
          break;
      }
    }
    prevViewKindRef.current = view.kind;
  }, [view.kind, announce]);

  // Scope DQ for calculation panels, tower, and track map (T05, T06, T07)
  const scopeDq: ScopeDq = useMemo(() => {
    switch (view.kind) {
      case 'incompatible':
        return 'untrusted';
      case 'ended':
        return 'ended';
      case 'stale':
      case 'away':
        return 'stale';
      case 'live':
        return view.degraded ? 'degraded' : 'live';
      case 'synthetic':
        return 'live';
      case 'waiting':
        return 'unavailable';
      default:
        return 'unavailable';
    }
  }, [view]);

  // Clear active alerts on session end (T06); the session log keeps its history (U11)
  useEffect(() => {
    if (view.kind === 'ended') {
      clearAllAlerts({ log: false });
    }
  }, [view.kind, clearAllAlerts]);

  const engineerStatus = status?.engineer;
  const isJoined = engineerStatus?.phase === 'monitoring';
  const isWatchingOrEnded = isJoined || engineerStatus?.phase === 'ended';
  const isAwaitingJoin =
    engineerStatus?.phase === 'awaiting_confirmation' || engineerStatus?.phase === 'joining';
  const isModalVisible = showJoinModal || isAwaitingJoin;
  const hideHeader = isAwaitingJoin || view.kind === 'awaitingConfirmation';
  const showIdentity = mayShowDriverIdentity(view);

  const handleRequestSnapshot = async () => {
    setActionError(null);
    try {
      await engineerRequestSnapshot();
      announce('Resync requested', 'polite');
      setActionNotice('Snapshot requested from driver.');
      setTimeout(() => {
        setActionNotice(null);
      }, 3000);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setActionError(`Snapshot request failed: ${msg}`);
    }
  };

  const handleDisconnect = async () => {
    setActionError(null);
    try {
      // The "Not connected" panel follows; the engineer opens Join when ready (audit E05).
      await engineerDisconnect();
      setShowJoinModal(false);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setActionError(`Disconnect failed: ${msg}`);
    }
  };

  // Derive competitors and driver position for TrackMap
  const competitors: readonly CompetitorMarker[] = useMemo(() => {
    if (!scoring) return [];
    const list: CompetitorMarker[] = [];
    for (const v of scoring.vehicles) {
      if (v.pos_x_m != null && v.pos_z_m != null && !v.is_player) {
        list.push({
          id: v.slot_id,
          x: v.pos_x_m,
          z: v.pos_z_m,
          // The track map draws only the car number from this name and keeps the full name for the tooltip
          // (trackmap/carNumber.ts carMarkerLabel, audit U10).
          carNumber: v.vehicle_name ?? '',
          driverName: v.driver ?? '',
          vehicleClass: parseVehicleClass(v.class_name),
          isDriver: false,
          inPits: v.in_pits ?? false,
          lapDistM: v.lap_dist_m ?? null,
        });
      }
    }
    return list;
  }, [scoring]);

  const playerVehicle = useMemo(() => scoring?.vehicles.find((v: { readonly is_player: boolean }) => v.is_player), [scoring]);
  const driverPos: DriverPosition | null = useMemo(() => {
    if (!playerVehicle) return null;
    if (playerVehicle.pos_x_m == null || playerVehicle.pos_z_m == null) {
      return null;
    }
    return {
      x: playerVehicle.pos_x_m,
      z: playerVehicle.pos_z_m,
      lapDistM: playerVehicle.lap_dist_m ?? null,
    };
  }, [playerVehicle]);

  // Online track geometry rebuilding (B05)
  const trackName = showIdentity ? (scoring?.session.track_name ?? status?.session.track ?? NO_VALUE) : NO_VALUE;
  const { map } = useTrackGeometry(store, scoring, null, typeof trackName === 'string' && trackName !== NO_VALUE ? trackName : null);

  // Session caution from the raw LMU yellow flag state / game phase (useScoring.cautionOf, audit U02)
  const caution = scoring?.session.caution ?? null;
  // The yellow-state label is caution detail only for an actual yellow state (1-7); a caution from the game phase
  // alone (state 0 "No caution" or -1 "Invalid") is described by the caution itself.
  const yellowRaw = scoring?.session.yellow_flag_raw ?? null;
  const yellowDetail =
    yellowRaw !== null && yellowRaw >= 1 && yellowRaw <= 7 ? (scoring?.session.yellow_flag_state ?? null) : null;
  const cautionText = caution === 'red' ? 'Red flag' : caution === 'fcy' ? 'FCY / SC' : null;
  const isEnded = view.kind === 'ended';
  const carCount = competitors.length + (playerVehicle ? 1 : 0);

  // Driver, vehicle, track identity metadata (T09, OR-3: never show fake placeholders before confirm)
  const driverDisplayName = showIdentity
    ? (engineerStatus?.driver_name ?? playerVehicle?.driver ?? NO_VALUE)
    : NO_VALUE;
  const carName = showIdentity
    ? (status?.session.car ?? playerVehicle?.vehicle_name ?? NO_VALUE)
    : NO_VALUE;
  const rawClass = playerVehicle?.class_name ?? status?.session.vehicle_class ?? null;
  const vehicleClass = showIdentity ? (rawClass ?? NO_VALUE) : NO_VALUE;

  return (
    <TelemetryStoreContext.Provider value={store}>
      {/* Live region announcer per A02, A11, EN14: outside the pitwall layout, so it never takes a row (V01) */}
      <Announcer politeMessage={politeMessage} assertiveMessage={assertiveMessage} />
      <div className="pitwall">

        {/* Top Pitwall Bar (y0, 36px), Strategy Strip (y36, 32px), State Banner (y68, 28px) */}
        {!hideHeader && isWatchingOrEnded && (
          <>
            <PitwallBar
              view={view}
              origin={effectiveOrigin}
              driverName={driverDisplayName !== NO_VALUE ? driverDisplayName : null}
              carName={carName !== NO_VALUE ? carName : null}
              vehicleClass={vehicleClass !== NO_VALUE ? vehicleClass : null}
              trackName={trackName !== NO_VALUE ? trackName : null}
              sessionType={scoring?.session.session_type ?? status?.session.session_type ?? null}
              currentLap={playerVehicle?.laps != null ? playerVehicle.laps + 1 : null}
              currentSector={playerVehicle?.sector ?? null}
              sessionTimeRemainingS={scoring?.session.session_time_remaining_s ?? null}
              caution={caution}
              rttMs={status?.network.rtt_ms ?? null}
              streamAgeS={engineerStatus.stream_age_s ?? null}
              isConnected={isWatchingOrEnded}
              ended={isEnded}
              onClose={() => { void handleDisconnect(); }}
              onRefreshAllData={() => { void handleRequestSnapshot(); }}
              onLeaveSession={() => { void handleDisconnect(); }}
              onOpenSettings={onOpenSettings}
              onOpenAnalysis={onOpenAnalysis}
            />
            <StrategyStrip
              sessionClockS={scoring?.session.session_time_remaining_s ?? null}
              completedLaps={playerVehicle?.laps ?? null}
              trackTempC={scoring?.session.track_temp_c ?? null}
              airTempC={scoring?.session.ambient_temp_c ?? null}
              rainFrac={scoring?.session.raining ?? null}
              flagText={
                cautionText !== null
                  ? yellowDetail !== null
                    ? `${cautionText} · ${yellowDetail}`
                    : cautionText
                  : (scoring?.session.yellow_flag_state ?? null)
              }
              flagLevel={caution === 'red' ? 'critical' : caution === 'fcy' ? 'warn' : null}
            />
            <StateBanner
              view={view}
              driverName={driverDisplayName !== NO_VALUE ? driverDisplayName : (engineerStatus.driver_name ?? null)}
              caution={caution}
              cautionDetail={yellowDetail}
              onRejoin={() => { setShowJoinModal(true); }}
            />
          </>
        )}

        {/* Action notices or errors */}
        {actionNotice && (
          <div className="badge badge-subtle pitwall-action-banner" role="status">
            {actionNotice}
          </div>
        )}
        {actionError && (
          <div className="badge badge-status-critical pitwall-action-banner" role="alert">
            <span className="pitwall-action-banner-text">{actionError}</span>
            <button
              type="button"
              className="btn btn-xs pitwall-error-dismiss"
              onClick={() => {
                setActionError(null);
              }}
              aria-label="Dismiss error"
              data-testid="pitwall-dismiss-error-btn"
            >
              Dismiss
            </button>
          </div>
        )}
        {coreError && (
          <div className="badge badge-status-critical pitwall-action-banner" role="alert">
            CORE STATUS ERROR: {coreError}
          </div>
        )}

        {/* Join Session Modal */}
        {isModalVisible && (
          <JoinSessionModal
            onJoined={() => {
              setShowJoinModal(false);
            }}
            onCancel={() => {
              setShowJoinModal(false);
            }}
            onOpenSettings={
              onOpenSettings
                ? () => {
                    setShowJoinModal(false);
                    onOpenSettings();
                  }
                : undefined
            }
          />
        )}

        {/* Channel Selector Modal (MoTeC i2 Pro style) */}
        {showChannelModal && (
          <ChannelSelectorModal
            isOpen={showChannelModal}
            activeChannelIds={activeSeries.map((s) => s.channelId)}
            onClose={() => {
              setShowChannelModal(false);
            }}
            onApply={handleApplyCustomChannels}
          />
        )}

        {/* Main Pitwall Workspace: Centred Not-Connected panel OR 4-Column Grid */}
        {!isWatchingOrEnded ? (
          <div className="not-connected-container" data-testid="not-connected-panel">
            <div className="not-connected-card">
              <h2 className="not-connected-title">{COPY.notConnected}</h2>
              <p className="not-connected-hint">{COPY.notConnectedHint}</p>
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => {
                  setShowJoinModal(true);
                }}
                data-testid="btn-open-join"
              >
                {COPY.joinDriver}
              </button>
            </div>
          </div>
        ) : (
          <ScopeDqContext.Provider value={scopeDq}>
            <div className="pitwall-grid" data-testid="engineer-workspace">
              {/* Column 1: Standings Tower (384px at 1920, 272px at 1366) */}
              <div className="pitwall-tower-col">
                <PanelErrorBoundary name="Standings Tower">
                  <TimingTower
                    scoring={scoring}
                    selectedSlotId={selectedSlotId}
                    onSelectSlot={(slotId) => {
                      setSelectedSlotId(slotId);
                    }}
                  />
                </PanelErrorBoundary>
              </div>

              {/* Column 2: Centre Column (Alerts Strip, Track Map + Events Log, Telemetry Graph) */}
              <div className="pitwall-centre">
                {/* Active Alerts Strip (0-2 x 28px + N badge) */}
                <AlertsBanner
                  alerts={alerts}
                  onDismiss={dismissAlert}
                  onClearAll={(shownIds) => {
                    for (const id of shownIds) dismissAlert(id);
                  }}
                />

                {/* Track Map & Events Log in Centre Top (340px) */}
                <div className="pitwall-centre-top">
                  <div className="card pitwall-map-panel">
                    <div className="card-header pitwall-card-header">
                      <span className="card-header-title">Track map</span>
                      <span
                        className={`card-row-label pitwall-car-count ${isEnded ? 'pitwall-car-count-ended' : ''}`}
                        data-testid="pitwall-car-count"
                      >
                        <span className="num">{carCount}</span> {carCount === 1 ? 'car' : 'cars'}
                        {isEnded ? '' : ' on track'}
                      </span>
                    </div>
                    <div className="pitwall-map-body">
                      <TrackMap
                        map={map}
                        dq={scopeDq}
                        driverPosition={driverPos}
                        competitors={competitors}
                        trackName={typeof trackName === 'string' && trackName !== NO_VALUE ? trackName : null}
                      />
                    </div>
                  </div>
                  <EventsLog log={alertLog} />
                </div>

                {/* 12-minisector live delta row: wide layout only, and only once it has a value (V03) */}
                {isWide && (
                  <PanelErrorBoundary name="Minisector Ribbon">
                    <MinisectorRibbon />
                  </PanelErrorBoundary>
                )}

                {/* Synchronized Telemetry Graph */}
                <div className="card pitwall-graph-panel" {...motecDrop.dropZoneProps}>
                  <div className="card-header pitwall-card-header pitwall-graph-header">
                    <div className="pitwall-graph-header-left">
                      <span className="card-header-title">Telemetry</span>
                      {worksheetId === 'custom' && (
                        <span
                          className="badge pitwall-custom-badge"
                          data-testid="preset-custom-badge"
                        >
                          Custom · <span className="num">{customSeries.length}</span> ch
                        </span>
                      )}
                    </div>
                    <div className="pitwall-graph-header-actions">
                      <div className="pitwall-graph-mode-actions" role="group" aria-label="X-axis mode">
                        <button
                          type="button"
                          aria-pressed={xMode === 'lapDistance'}
                          className={`btn btn-xs ${xMode === 'lapDistance' ? 'btn-primary' : ''}`}
                          onClick={() => {
                            setXMode('lapDistance');
                          }}
                          data-testid="xaxis-mode-lapDistance"
                        >
                          Distance
                        </button>
                        <button
                          type="button"
                          aria-pressed={xMode === 'lapTime'}
                          className={`btn btn-xs ${xMode === 'lapTime' ? 'btn-primary' : ''}`}
                          onClick={() => {
                            setXMode('lapTime');
                          }}
                          data-testid="xaxis-mode-lapTime"
                        >
                          Lap time
                        </button>
                        <button
                          type="button"
                          aria-pressed={xMode === 'lapFraction'}
                          className={`btn btn-xs ${xMode === 'lapFraction' ? 'btn-primary' : ''}`}
                          onClick={() => {
                            setXMode('lapFraction');
                          }}
                          data-testid="xaxis-mode-lapFraction"
                        >
                          Lap %
                        </button>
                        <button
                          type="button"
                          aria-pressed={xMode === 'time'}
                          className={`btn btn-xs ${xMode === 'time' ? 'btn-primary' : ''}`}
                          onClick={() => {
                            setXMode('time');
                          }}
                          data-testid="xaxis-mode-time"
                        >
                          Time
                        </button>
                      </div>

                      <span className="pitwall-graph-separator" aria-hidden="true" />

                      <div className="pitwall-graph-worksheet-actions" role="group" aria-label="Graph presets">
                        <button
                          type="button"
                          aria-pressed={worksheetId === 'driver'}
                          className={`btn btn-xs ${worksheetId === 'driver' ? 'btn-primary' : ''}`}
                          onClick={() => {
                            setWorksheetId('driver');
                          }}
                          data-testid="worksheet-driver-btn"
                        >
                          Driver
                        </button>
                        <button
                          type="button"
                          aria-pressed={worksheetId === 'tyres'}
                          className={`btn btn-xs ${worksheetId === 'tyres' ? 'btn-primary' : ''}`}
                          onClick={() => {
                            setWorksheetId('tyres');
                          }}
                          data-testid="worksheet-tyres-btn"
                        >
                          Tyres
                        </button>
                        <button
                          type="button"
                          aria-pressed={worksheetId === 'brakes'}
                          className={`btn btn-xs ${worksheetId === 'brakes' ? 'btn-primary' : ''}`}
                          onClick={() => {
                            setWorksheetId('brakes');
                          }}
                          data-testid="worksheet-brakes-btn"
                        >
                          Brakes
                        </button>
                        <button
                          type="button"
                          aria-pressed={worksheetId === 'energy'}
                          className={`btn btn-xs ${worksheetId === 'energy' ? 'btn-primary' : ''}`}
                          onClick={() => {
                            setWorksheetId('energy');
                          }}
                          data-testid="worksheet-energy-btn"
                        >
                          Energy
                        </button>
                        <button
                          type="button"
                          aria-pressed={worksheetId === 'powertrain'}
                          className={`btn btn-xs ${worksheetId === 'powertrain' ? 'btn-primary' : ''}`}
                          onClick={() => {
                            setWorksheetId('powertrain');
                          }}
                          data-testid="worksheet-powertrain-btn"
                        >
                          Powertrain
                        </button>
                        {worksheetId === 'custom' && (
                          <button
                            type="button"
                            aria-pressed={true}
                            className="btn btn-xs btn-primary"
                            onClick={() => {
                              setWorksheetId('custom');
                            }}
                            data-testid="preset-custom-btn"
                          >
                            Custom
                          </button>
                        )}
                        <MotecAddButton />
                        <button
                          type="button"
                          className="btn btn-xs pitwall-channel-select-btn"
                          onClick={() => {
                            setShowChannelModal(true);
                          }}
                          title="Choose the channels to plot"
                          data-testid="open-channel-selector-btn"
                        >
                          Channels…
                        </button>
                      </div>
                    </div>
                  </div>
                  <div className="pitwall-graph-body">
                    <MotecImportBar compact dragActive={motecDrop.dragActive} />
                    <PanelErrorBoundary name="Telemetry Graph">
                      <TelemetryGraph
                        source={liveSource}
                        xMode={xMode}
                        worksheet={currentWorksheet}
                        references={motecReferences}
                      />
                    </PanelErrorBoundary>
                  </div>
                </div>
              </div>

              {isWide ? (
                <>
                  {/* Column 3: Right-A (280px at >= 1800px): Timing, Fuel & Energy, Inputs */}
                  <div className="pitwall-right-a">
                    <PanelErrorBoundary name="Timing">
                      <TimingPanel />
                    </PanelErrorBoundary>
                    <PanelErrorBoundary name="Fuel">
                      <FuelPanel />
                    </PanelErrorBoundary>
                    <PanelErrorBoundary name="Driver Inputs">
                      <DriverInputsPanel />
                    </PanelErrorBoundary>
                  </div>

                  {/* Column 4: Right-B (272px at >= 1800px): Tyres, Brakes, Suspension, Energy */}
                  <div className="pitwall-right-b">
                    <PanelErrorBoundary name="Tyres">
                      <TyrePanel />
                    </PanelErrorBoundary>
                    <PanelErrorBoundary name="Brakes">
                      <BrakePanel />
                    </PanelErrorBoundary>
                    <PanelErrorBoundary name="Suspension">
                      <SuspensionPanel />
                    </PanelErrorBoundary>
                    <PanelErrorBoundary name="Energy">
                      <EnergyPanel />
                    </PanelErrorBoundary>
                  </div>

                </>
              ) : (
                /* Compact Right Column (< 1800px, e.g. 1366x768): Pinned Mini-Row + Segmented Tabs */
                <div className="pitwall-right-compact">
                  <RightColumnTabs initialTab={rightTab} onTabChange={setRightTab} />
                </div>
              )}
            </div>
          </ScopeDqContext.Provider>
        )}
      </div>
    </TelemetryStoreContext.Provider>
  );
}
