import { useMemo, type JSX } from 'react';
import { TrackMap } from '../../components/trackmap/TrackMap';
import type { CompetitorMarker } from '../../components/trackmap/types';
import { TimingTower } from '../../components/telemetry/TimingTower';
import { TyrePanel } from '../../components/telemetry/TyrePanel';
import { ch } from '../../generated/channels';
import type { CompetitorEntryDto, ScoringStateDto } from '../../hooks/useScoring';
import { parseVehicleClass } from '../../lib/classes';
import { formatClock, formatDelta, formatGap, formatLapGap, formatLapTime, formatTemperature, NO_VALUE } from '../../lib/units';
import { fracPct, useLiveReadings } from '../liveData';
import { LiveCell, NO_FIELD_DATA_REASON, NO_HISTORY_REASON, NOT_CONNECTED_REASON, ViewNoData } from './ViewNoData';

/** Props of the views fed by live scoring (core:scoring). */
export interface ScoringViewProps {
  /** Live scoring; null when none has arrived. */
  readonly scoring: ScoringStateDto | null;
}

function ViewBanner({ title, desc }: { readonly title: string; readonly desc: string }): JSX.Element {
  return (
    <div className="view-banner">
      <h2 className="view-title">{title}</h2>
      <p className="view-desc">{desc}</p>
    </div>
  );
}

function sortedByPlace(vehicles: readonly CompetitorEntryDto[]): CompetitorEntryDto[] {
  return [...vehicles].sort((a, b) => (a.place ?? Infinity) - (b.place ?? Infinity));
}

function placeText(place: number | null): string {
  return place !== null ? `P${String(place)}` : NO_VALUE;
}

function leaderGapText(v: CompetitorEntryDto): string {
  if (v.place === 1) return 'LEADER';
  if (v.laps_behind_leader !== null && v.laps_behind_leader > 0) return formatLapGap(v.laps_behind_leader);
  return formatGap(v.time_behind_leader_s);
}

// --- Laptime Data ---
const LAPTIME_CHANNELS: readonly number[] = [
  ch.LAP_NUMBER,
  ch.LAST_LAP_TIME,
  ch.LAST_SECTOR1,
  ch.LAST_SECTOR2,
  ch.LAST_SECTOR3,
  ch.FUEL_USED_LAST_LAP,
  ch.LAP_INVALID,
];

/** The last completed lap from live scoring; the lap-by-lap ledger needs history the app does not record. */
export function LaptimeDataView(): JSX.Element {
  const r = useLiveReadings(LAPTIME_CHANNELS);
  const fuel = r.num(ch.FUEL_USED_LAST_LAP);
  const invalid = r.bool(ch.LAP_INVALID);
  return (
    <div className="view-card-container" data-testid="laptime-data-view">
      <ViewBanner title="Laptime Data" desc="Last completed lap from live scoring. The lap ledger is not recorded yet." />
      {!r.connected && <ViewNoData title="Last lap" reason={NOT_CONNECTED_REASON} testId="laptime-not-connected" />}
      <div className="view-live-grid" data-testid="laptime-last-lap">
        <LiveCell label="LAST LAP" value={formatLapTime(r.num(ch.LAST_LAP_TIME))} />
        <LiveCell label="S1" value={formatLapTime(r.num(ch.LAST_SECTOR1))} />
        <LiveCell label="S2" value={formatLapTime(r.num(ch.LAST_SECTOR2))} />
        <LiveCell label="S3" value={formatLapTime(r.num(ch.LAST_SECTOR3))} />
        <LiveCell label="FUEL USED" value={fuel !== null ? `${fuel.toFixed(2)} L` : NO_VALUE} />
        <LiveCell label="CURRENT LAP INVALID" value={invalid === null ? NO_VALUE : invalid ? 'YES' : 'NO'} />
      </div>
      <div className="view-live-section">
        <span className="view-live-section-title">LAP LEDGER</span>
        <ViewNoData title="Lap history" reason={NO_HISTORY_REASON} testId="laptime-history-no-data" />
      </div>
    </div>
  );
}

// --- Race Info & Weather ---
const RACE_INFO_CHANNELS: readonly number[] = [
  ch.SESSION_TIME_REMAINING,
  ch.LAP_NUMBER,
  ch.SESSION_MAX_LAPS,
  ch.TRACK_TEMP,
  ch.AIR_TEMP,
  ch.RAIN,
  ch.TRACK_WETNESS,
  ch.FULL_COURSE_YELLOW,
  ch.TRACK_NAME,
];

export function RaceInfoView({ scoring }: ScoringViewProps): JSX.Element {
  const r = useLiveReadings(RACE_INFO_CHANNELS);
  const remaining = r.num(ch.SESSION_TIME_REMAINING);
  const lap = r.num(ch.LAP_NUMBER);
  const maxLaps = r.num(ch.SESSION_MAX_LAPS);
  const fcy = r.bool(ch.FULL_COURSE_YELLOW);
  const trackName = scoring?.session.track_name ?? r.text(ch.TRACK_NAME);
  return (
    <div className="view-card-container" data-testid="race-info-view">
      <ViewBanner title="Race Info & Weather" desc="Session state and current weather from live scoring. No forecasts." />
      {!r.connected && <ViewNoData title="Race info" reason={NOT_CONNECTED_REASON} testId="race-info-not-connected" />}
      <div className="view-live-grid" data-testid="race-info-live">
        <LiveCell label="TRACK" value={trackName ?? NO_VALUE} />
        <LiveCell label="SESSION" value={scoring?.session.session_type ?? NO_VALUE} />
        <LiveCell label="TIME REMAINING" value={remaining !== null ? formatClock(remaining) : NO_VALUE} />
        <LiveCell
          label="LAP"
          value={`${lap !== null ? String(Math.round(lap)) : NO_VALUE}${maxLaps !== null && maxLaps > 0 ? ` / ${String(Math.round(maxLaps))}` : ''}`}
        />
        <LiveCell label="TRACK TEMP" value={formatTemperature(r.num(ch.TRACK_TEMP), { withUnit: true })} />
        <LiveCell label="AIR TEMP" value={formatTemperature(r.num(ch.AIR_TEMP), { withUnit: true })} />
        <LiveCell label="RAIN (CURRENT)" value={fracPct(r.num(ch.RAIN), 0)} />
        <LiveCell label="TRACK WETNESS (AVG)" value={fracPct(r.num(ch.TRACK_WETNESS), 0)} />
        <LiveCell label="FCY / SAFETY CAR" value={fcy === null ? NO_VALUE : fcy ? 'ACTIVE' : 'CLEAR'} />
      </div>
    </div>
  );
}

// --- Time Trial ---
const TIME_TRIAL_CHANNELS: readonly number[] = [ch.BEST_LAP_TIME, ch.DELTA_BEST, ch.PREDICTED_LAP_TIME];

export function TimeTrialInfoView(): JSX.Element {
  const r = useLiveReadings(TIME_TRIAL_CHANNELS);
  const delta = r.num(ch.DELTA_BEST);
  return (
    <div className="view-card-container" data-testid="time-trial-view">
      <ViewBanner title="Time Trial Info" desc="Pace against your own best lap from live timing." />
      {!r.connected && <ViewNoData title="Time trial" reason={NOT_CONNECTED_REASON} testId="time-trial-not-connected" />}
      <div className="tt-hero-cluster">
        <div className="tt-target-card">
          <span className="tt-target-label">BEST LAP (TARGET)</span>
          <div className="tt-target-time text-purple" data-testid="tt-best">
            {formatLapTime(r.num(ch.BEST_LAP_TIME))}
          </div>
        </div>
        <div className="tt-target-card">
          <span className="tt-target-label">CURRENT DELTA (Δt)</span>
          <div className={`tt-target-time ${delta !== null && delta < 0 ? 'text-green' : ''}`} data-testid="tt-delta">
            {delta !== null ? `${formatDelta(delta)}s` : NO_VALUE}
          </div>
        </div>
        <div className="tt-target-card">
          <span className="tt-target-label">PREDICTED LAP TIME (ESTIMATED)</span>
          <div className="tt-target-time text-cyan" data-testid="tt-predicted">
            {formatLapTime(r.num(ch.PREDICTED_LAP_TIME))}
          </div>
        </div>
      </div>
      <ViewNoData
        title="Track record / ghost"
        reason="No track-record or ghost-lap source is available."
        testId="tt-record-no-data"
      />
    </div>
  );
}

// --- Multi-car Map ---
const MAP_CHANNELS: readonly number[] = [ch.TRACK_NAME, ch.TRACK_LENGTH];

export function MulticarMapView({ scoring }: ScoringViewProps): JSX.Element {
  const r = useLiveReadings(MAP_CHANNELS);
  const competitors = useMemo((): readonly CompetitorMarker[] => {
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
          vehicleClass: parseVehicleClass(v.class_name),
          isDriver: false,
          inPits: v.in_pits ?? false,
          lapDistM: v.lap_dist_m ?? null,
        });
      }
    }
    return list;
  }, [scoring]);
  const player = scoring?.vehicles.find((v) => v.is_player) ?? null;

  return (
    <div className="view-card-container" data-testid="multicar-map-view">
      <ViewBanner title="Multi-Car Circuit Map" desc="Track map with live competitor positions from scoring." />
      <div className="multicar-map-wrapper">
        <TrackMap
          trackName={scoring?.session.track_name ?? r.text(ch.TRACK_NAME)}
          trackLengthM={r.num(ch.TRACK_LENGTH)}
          competitors={competitors}
          driverPlace={player?.place ?? null}
          driverSector={player?.sector ?? null}
        />
      </div>
    </div>
  );
}

// --- Multi-car Race Timing ---
export function MulticarRaceTimingView({ scoring }: ScoringViewProps): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-race-timing-view">
      <ViewBanner title="Multi-Car Race Timing Tower" desc="Standings and gaps from live scoring." />
      <div className="timing-tower-wrapper">
        <TimingTower scoring={scoring} />
      </div>
    </div>
  );
}

// --- Multi-car One-Lap Timing ---
function sector2(v: CompetitorEntryDto): number | null {
  return v.last_sector12_s !== null && v.last_sector1_s !== null ? v.last_sector12_s - v.last_sector1_s : null;
}

export function MulticarOneLapTimingView({ scoring }: ScoringViewProps): JSX.Element {
  const vehicles = sortedByPlace(scoring?.vehicles ?? []);
  return (
    <div className="view-card-container" data-testid="multicar-one-lap-timing-view">
      <ViewBanner title="Multi-Car One-Lap Timing" desc="Current sector and last-lap splits per car from live scoring." />
      {vehicles.length === 0 ? (
        <ViewNoData title="Field timing" reason="No live scoring received." testId="one-lap-no-data" />
      ) : (
        <table className="stat-table" data-testid="one-lap-table">
          <thead>
            <tr>
              <th>Pos</th>
              <th>Vehicle</th>
              <th>Class</th>
              <th>Current Sector</th>
              <th>Last S1</th>
              <th>Last S2</th>
              <th>Last Lap</th>
              <th>Best Lap</th>
              <th>Gap to Leader</th>
            </tr>
          </thead>
          <tbody>
            {vehicles.map((v) => (
              <tr key={v.slot_id} className={v.is_player ? 'row-highlighted' : ''}>
                <td className="font-bold">{placeText(v.place)}</td>
                <td>{v.vehicle_name ?? NO_VALUE}</td>
                <td>{v.class_name ?? NO_VALUE}</td>
                <td>{v.sector !== null ? `S${String(v.sector)}` : NO_VALUE}</td>
                <td className="font-mono">{formatLapTime(v.last_sector1_s)}</td>
                <td className="font-mono">{formatLapTime(sector2(v))}</td>
                <td className="font-mono">{formatLapTime(v.last_lap_s)}</td>
                <td className="font-mono">{formatLapTime(v.best_lap_s)}</td>
                <td className="font-mono">{leaderGapText(v)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

// --- Multi-car Opponent Stats ---
export function MulticarOpponentStatsView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-opponent-stats-view">
      <ViewBanner title="Multi-Car Opponent Stats" desc="Rival speed traps, corner speeds and pit strategy." />
      <ViewNoData title="Opponent statistics" reason={NO_FIELD_DATA_REASON} testId="opponent-stats-no-data" />
    </div>
  );
}

// --- Multi-car Race History ---
export function MulticarRaceHistoryView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-race-history-view">
      <ViewBanner title="Multi-Car Race History" desc="Lap-by-lap position changes." />
      <ViewNoData title="Position history" reason={NO_HISTORY_REASON} testId="race-history-no-data" />
    </div>
  );
}

// --- Multi-car Session Data ---
export function MulticarSessionDataView({ scoring }: ScoringViewProps): JSX.Element {
  const vehicles = sortedByPlace(scoring?.vehicles ?? []);
  return (
    <div className="view-card-container" data-testid="multicar-session-data-view">
      <ViewBanner title="Multi-Car Session Data" desc="Entry list from live scoring." />
      {vehicles.length === 0 ? (
        <ViewNoData title="Entry list" reason="No live scoring received." testId="session-data-no-data" />
      ) : (
        <table className="stat-table" data-testid="session-data-table">
          <thead>
            <tr>
              <th>Pos</th>
              <th>Driver</th>
              <th>Vehicle</th>
              <th>Class</th>
              <th>Laps</th>
              <th>Pit Stops</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {vehicles.map((v) => (
              <tr key={v.slot_id} className={v.is_player ? 'row-highlighted' : ''}>
                <td className="font-bold">{placeText(v.place)}</td>
                <td>{v.driver ?? NO_VALUE}</td>
                <td>{v.vehicle_name ?? NO_VALUE}</td>
                <td>{v.class_name ?? NO_VALUE}</td>
                <td>{v.laps !== null ? String(v.laps) : NO_VALUE}</td>
                <td>{v.num_pitstops !== null ? String(v.num_pitstops) : NO_VALUE}</td>
                <td>{v.in_garage_stall === true ? 'GARAGE' : v.in_pits === true ? 'PITS' : v.in_pits === false ? 'ON TRACK' : NO_VALUE}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

// --- Multi-car Telemetry ---
export function MulticarTelemetryView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-telemetry-view">
      <ViewBanner title="Multi-Car Telemetry Overlay" desc="Speed, throttle and braking compared with an opponent." />
      <ViewNoData
        title="Opponent telemetry"
        reason="LMU provides full telemetry only for the player car; opponent traces are not available."
        testId="multicar-telemetry-no-data"
      />
    </div>
  );
}

// --- Multilap Telemetry ---
export function MultilapTelemetryView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multilap-telemetry-view">
      <ViewBanner title="Multilap Telemetry Overlay" desc="Several laps overlaid on a distance axis." />
      <ViewNoData title="Lap traces" reason={NO_HISTORY_REASON} testId="multilap-no-data" />
    </div>
  );
}

// --- Workspace ---
export function WorkspaceView({ scoring }: ScoringViewProps): JSX.Element {
  return (
    <div className="view-card-container" data-testid="workspace-view">
      <ViewBanner title="Engineering Workspace" desc="Live tyre panel and timing tower." />
      <div className="workspace-demo-grid">
        <div className="workspace-tile">
          <TyrePanel />
        </div>
        <div className="workspace-tile">
          <TimingTower scoring={scoring} />
        </div>
      </div>
    </div>
  );
}
