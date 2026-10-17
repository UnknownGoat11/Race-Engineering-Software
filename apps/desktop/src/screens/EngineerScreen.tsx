/**
 * Engineer Pitwall Screen (spec §72–85, §164, §246, §1735–1755, §2108–2150).
 * Complete pitwall monitoring workstation:
 * - Join flow (code entry modal & identity card verification)
 * - Top Bar: LIVE/STALE pill, stream age, latency (RTT), packet rate, driver & session metadata, snapshot & disconnect controls
 * - 3-Column Ergonomic Grid:
 *   - Left: Timing Tower standings table (class filtering, gaps, pit flags)
 *   - Center: Track Map vector pipeline + Synchronized Telemetry Graph (presets: Inputs, Powertrain, Tyres, Brakes) + Alerts
 *   - Right: Telemetry calculation panels (Timing, Fuel, Tyres, Brakes, Energy, Driver Inputs)
 */
import { useEffect, useMemo, useState, type JSX } from 'react';
import { PanelErrorBoundary } from '../components/ErrorBoundary';
import { AlertsBanner } from '../components/telemetry/AlertsBanner';
import { BrakePanel } from '../components/telemetry/BrakePanel';
import { DriverInputsPanel } from '../components/telemetry/DriverInputsPanel';
import { EnergyPanel } from '../components/telemetry/EnergyPanel';
import { FuelPanel } from '../components/telemetry/FuelPanel';
import { TimingPanel } from '../components/telemetry/TimingPanel';
import { TimingTower } from '../components/telemetry/TimingTower';
import { TyrePanel } from '../components/telemetry/TyrePanel';
import { TrackMap } from '../components/trackmap/TrackMap';
import type { CompetitorMarker, DriverPosition, VehicleClass } from '../components/trackmap/types';
import { ch } from '../generated/channels';
import { TelemetryGraph } from '../graph/TelemetryGraph';
import { LiveRingSource } from '../graph/liveSource';
import type { GraphSeries } from '../graph/types';
import { useAlerts } from '../hooks/useAlerts';
import { useCoreStatus } from '../hooks/useCoreStatus';
import { useScoring } from '../hooks/useScoring';
import { engineerDisconnect, engineerRequestSnapshot } from '../lib/ipc';
import { NO_VALUE } from '../lib/units';
import { connectTauriTelemetry } from '../telemetry/bridge';
import { TelemetryStoreContext } from '../telemetry/hooks';
import { TelemetryStore } from '../telemetry/store';
import { JoinSessionModal } from './JoinSessionModal';
import '../styles/screens.css';

type GraphPreset = 'inputs' | 'powertrain' | 'tyres' | 'brakes';
type RightPanelTab = 'timing_fuel' | 'tyres_brakes' | 'inputs_energy';

const GRAPH_SERIES_PRESETS: Record<GraphPreset, readonly GraphSeries[]> = {
  inputs: [
    { channelId: ch.SPEED, color: 1 },
    { channelId: ch.THROTTLE, color: 2 },
    { channelId: ch.BRAKE, color: 3 },
    { channelId: ch.STEERING, color: 4 },
  ],
  powertrain: [
    { channelId: ch.RPM, color: 1 },
    { channelId: ch.SPEED, color: 2 },
    { channelId: ch.FUEL, color: 3 },
  ],
  tyres: [
    { channelId: ch.TYRE_TEMP_MIDDLE_FL, color: 1 },
    { channelId: ch.TYRE_TEMP_MIDDLE_FR, color: 2 },
    { channelId: ch.TYRE_TEMP_MIDDLE_RL, color: 3 },
    { channelId: ch.TYRE_TEMP_MIDDLE_RR, color: 4 },
  ],
  brakes: [
    { channelId: ch.BRAKE_TEMP_FL, color: 1 },
    { channelId: ch.BRAKE_TEMP_FR, color: 2 },
    { channelId: ch.BRAKE_TEMP_RL, color: 3 },
    { channelId: ch.BRAKE_TEMP_RR, color: 4 },
  ],
};

export function EngineerScreen(): JSX.Element {
  const { status, error: coreError } = useCoreStatus();
  const scoring = useScoring();
  const { alerts, dismissAlert, clearAllAlerts } = useAlerts();

  const [selectedSlotId, setSelectedSlotId] = useState<number | null>(null);
  const [graphPreset, setGraphPreset] = useState<GraphPreset>('inputs');
  const [rightTab, setRightTab] = useState<RightPanelTab>('timing_fuel');
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [showJoinModal, setShowJoinModal] = useState<boolean>(false);

  // Live TelemetryStore & Bridge connection
  const store = useMemo(() => new TelemetryStore(), []);

  useEffect(() => {
    const conn = connectTauriTelemetry(store);
    return () => {
      conn.disconnect();
    };
  }, [store]);

  const dataKind = status?.origin === 'synthetic' ? 'synthetic' : 'lmu';
  const liveSource = useMemo(() => new LiveRingSource(store, { xMode: 'time', dataKind }), [store, dataKind]);

  const engineerStatus = status?.engineer;
  const isJoined = engineerStatus?.phase === 'monitoring';
  const isAwaitingJoin =
    engineerStatus?.phase === 'awaiting_confirmation' || engineerStatus?.phase === 'joining';
  const isModalVisible = showJoinModal || isAwaitingJoin;

  const handleRequestSnapshot = async () => {
    setActionError(null);
    try {
      await engineerRequestSnapshot();
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
      await engineerDisconnect();
      setShowJoinModal(true);
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
          carNumber: v.vehicle_name ?? '',
          driverName: v.driver ?? '',
          vehicleClass: (v.class_name?.toLowerCase() as VehicleClass) || 'unknown',
          isDriver: false,
        });
      }
    }
    return list;
  }, [scoring]);

  const playerVehicle = useMemo(() => scoring?.vehicles.find((v) => v.is_player), [scoring]);
  const driverPos: DriverPosition | null = useMemo(() => {
    if (!playerVehicle) return null;
    if (playerVehicle.pos_x_m == null || playerVehicle.pos_z_m == null) {
      return null;
    }
    return {
      x: playerVehicle.pos_x_m,
      z: playerVehicle.pos_z_m,
      lapDistanceM: playerVehicle.lap_dist_m ?? undefined,
    };
  }, [playerVehicle]);

  const streamHealth = engineerStatus?.stream_health ?? 'no_data';
  const streamBadgeClass =
    streamHealth === 'healthy'
      ? 'status-badge-live'
      : streamHealth === 'stale'
        ? 'status-badge-stale'
        : 'status-badge-neutral';

  const streamHealthLabel =
    streamHealth === 'healthy'
      ? 'LIVE STREAM'
      : streamHealth === 'stale'
        ? 'STALE STREAM'
        : 'NO STREAM DATA';

  const driverDisplayName = engineerStatus?.driver_name ?? playerVehicle?.driver ?? 'Driver';
  const carName = status?.session.car ?? playerVehicle?.vehicle_name ?? NO_VALUE;
  const trackName = scoring?.session.track_name ?? status?.session.track ?? NO_VALUE;
  const vehicleClass = playerVehicle?.class_name ?? status?.session.vehicle_class ?? NO_VALUE;

  return (
    <TelemetryStoreContext value={store}>
      <div className="screen-container" data-testid="engineer-screen">
        {/* Top Pitwall Header Bar */}
        <div className="screen-header-banner" data-testid="engineer-header">
          <div className="screen-header-left">
            <span className={`status-badge ${streamBadgeClass}`} data-testid="stream-health-badge">
              {streamHealthLabel}
            </span>

            {engineerStatus?.driver_away && (
              <span className="status-badge status-badge-warn" data-testid="driver-away-badge">
                DRIVER AWAY
              </span>
            )}

            {engineerStatus?.incompatible_schema && (
              <span className="status-badge status-badge-error">
                INCOMPATIBLE SCHEMA
              </span>
            )}

            <div className="meta-chip">
              <span className="meta-chip-label">Driver</span>
              <span className="meta-chip-value" style={{ fontWeight: 700 }} data-testid="engineer-driver-name">
                {driverDisplayName}
              </span>
            </div>

            <div className="meta-chip">
              <span className="meta-chip-label">Vehicle</span>
              <span className="meta-chip-value" data-testid="engineer-car">
                {carName} ({vehicleClass})
              </span>
            </div>

            <div className="meta-chip">
              <span className="meta-chip-label">Track</span>
              <span className="meta-chip-value" data-testid="engineer-track">
                {trackName}
              </span>
            </div>

            <div className="meta-chip">
              <span className="meta-chip-label">Latency (RTT)</span>
              <span className="meta-chip-value" data-testid="engineer-rtt">
                {status?.network.rtt_ms != null ? `${status.network.rtt_ms.toFixed(0)} ms` : NO_VALUE}
              </span>
            </div>

            <div className="meta-chip">
              <span className="meta-chip-label">Stream Age</span>
              <span className="meta-chip-value" data-testid="engineer-stream-age">
                {engineerStatus?.stream_age_s != null
                  ? `${engineerStatus.stream_age_s.toFixed(1)}s`
                  : NO_VALUE}
              </span>
            </div>
          </div>

          <div className="screen-header-right">
            {!isJoined ? (
              <button
                type="button"
                className="btn btn-primary btn-sm"
                onClick={() => {
                  setShowJoinModal(true);
                }}
                data-testid="btn-open-join"
              >
                Join Driver Session
              </button>
            ) : (
              <>
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={() => { void handleRequestSnapshot(); }}
                  title="Request full state snapshot from driver"
                  data-testid="btn-request-snapshot"
                >
                  Snapshot
                </button>
                <button
                  type="button"
                  className="btn btn-sm btn-danger"
                  onClick={() => { void handleDisconnect(); }}
                  data-testid="btn-engineer-disconnect"
                >
                  Disconnect
                </button>
              </>
            )}
          </div>
        </div>

        {/* Action notices or errors */}
        {actionNotice && (
          <div className="status-badge status-badge-info" style={{ padding: '6px 12px' }}>
            {actionNotice}
          </div>
        )}
        {actionError && (
          <div className="status-badge status-badge-error" style={{ padding: '6px 12px' }}>
            {actionError}
          </div>
        )}
        {coreError && (
          <div className="status-badge status-badge-error" style={{ padding: '6px 12px' }}>
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
          />
        )}

        {/* Pitwall 3-Column Ergonomic Workspace */}
        <div className="engineer-workspace" data-testid="engineer-workspace">
          {/* Left Column: Standings Tower */}
          <div className="engineer-col" style={{ width: '280px' }}>
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

          {/* Center Column: Track Map & Telemetry Graph */}
          <div className="engineer-col" style={{ flex: 1, minWidth: 0 }}>
            {/* Active alerts banner */}
            <AlertsBanner
              alerts={alerts}
              onDismiss={dismissAlert}
              onClearAll={clearAllAlerts}
            />

            {/* Track Map */}
            <div className="card" style={{ height: '320px', minHeight: '300px', display: 'flex', flexDirection: 'column' }}>
              <div className="card-header" style={{ padding: '6px 12px' }}>
                <span className="card-header-title">LIVE VECTOR TRACK MAP</span>
                <span className="card-row-label">
                  {competitors.length + (playerVehicle ? 1 : 0)} CARS ON TRACK
                </span>
              </div>
              <div style={{ flex: 1, position: 'relative', overflow: 'hidden' }}>
                <TrackMap
                  driverPosition={driverPos}
                  competitors={competitors}
                  trackName={typeof trackName === 'string' ? trackName : null}
                />
              </div>
            </div>

            {/* Synchronized Telemetry Graph */}
            <div className="card" style={{ flex: 1, minHeight: '320px', display: 'flex', flexDirection: 'column' }}>
              <div className="card-header" style={{ padding: '6px 12px' }}>
                <span className="card-header-title">SYNCHRONIZED TELEMETRY GRAPH</span>
                <div style={{ display: 'flex', gap: '4px' }}>
                  <button
                    type="button"
                    className={`btn btn-xs ${graphPreset === 'inputs' ? 'btn-primary' : ''}`}
                    onClick={() => {
                      setGraphPreset('inputs');
                    }}
                  >
                    Inputs
                  </button>
                  <button
                    type="button"
                    className={`btn btn-xs ${graphPreset === 'powertrain' ? 'btn-primary' : ''}`}
                    onClick={() => {
                      setGraphPreset('powertrain');
                    }}
                  >
                    Powertrain
                  </button>
                  <button
                    type="button"
                    className={`btn btn-xs ${graphPreset === 'tyres' ? 'btn-primary' : ''}`}
                    onClick={() => {
                      setGraphPreset('tyres');
                    }}
                  >
                    Tyres
                  </button>
                  <button
                    type="button"
                    className={`btn btn-xs ${graphPreset === 'brakes' ? 'btn-primary' : ''}`}
                    onClick={() => {
                      setGraphPreset('brakes');
                    }}
                  >
                    Brakes
                  </button>
                </div>
              </div>
              <div style={{ flex: 1, minHeight: '260px', padding: '8px' }}>
                <PanelErrorBoundary name="Telemetry Graph">
                  <TelemetryGraph
                    source={liveSource}
                    xMode="time"
                    series={GRAPH_SERIES_PRESETS[graphPreset]}
                  />
                </PanelErrorBoundary>
              </div>
            </div>
          </div>

          {/* Right Column: Telemetry Calculation Panels */}
          <div className="engineer-col" style={{ width: '340px' }}>
            <div className="card" style={{ padding: '4px 8px', marginBottom: '-4px' }}>
              <div style={{ display: 'flex', gap: '4px', width: '100%' }}>
                <button
                  type="button"
                  className={`btn btn-xs ${rightTab === 'timing_fuel' ? 'btn-primary' : ''}`}
                  style={{ flex: 1 }}
                  onClick={() => {
                    setRightTab('timing_fuel');
                  }}
                >
                  Timing & Fuel
                </button>
                <button
                  type="button"
                  className={`btn btn-xs ${rightTab === 'tyres_brakes' ? 'btn-primary' : ''}`}
                  style={{ flex: 1 }}
                  onClick={() => {
                    setRightTab('tyres_brakes');
                  }}
                >
                  Tyres & Brakes
                </button>
                <button
                  type="button"
                  className={`btn btn-xs ${rightTab === 'inputs_energy' ? 'btn-primary' : ''}`}
                  style={{ flex: 1 }}
                  onClick={() => {
                    setRightTab('inputs_energy');
                  }}
                >
                  Inputs & Energy
                </button>
              </div>
            </div>

            {rightTab === 'timing_fuel' && (
              <>
                <PanelErrorBoundary name="Timing Panel">
                  <TimingPanel />
                </PanelErrorBoundary>
                <PanelErrorBoundary name="Fuel Panel">
                  <FuelPanel />
                </PanelErrorBoundary>
              </>
            )}

            {rightTab === 'tyres_brakes' && (
              <>
                <PanelErrorBoundary name="Tyre Panel">
                  <TyrePanel />
                </PanelErrorBoundary>
                <PanelErrorBoundary name="Brake Panel">
                  <BrakePanel />
                </PanelErrorBoundary>
              </>
            )}

            {rightTab === 'inputs_energy' && (
              <>
                <PanelErrorBoundary name="Driver Inputs Panel">
                  <DriverInputsPanel />
                </PanelErrorBoundary>
                <PanelErrorBoundary name="Energy Panel">
                  <EnergyPanel />
                </PanelErrorBoundary>
              </>
            )}
          </div>
        </div>
      </div>
    </TelemetryStoreContext>
  );
}
