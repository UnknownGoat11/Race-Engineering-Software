import type { JSX } from 'react';
import { TrackMap } from '../../components/trackmap/TrackMap';
import { TimingTower } from '../../components/telemetry/TimingTower';
import { TyrePanel } from '../../components/telemetry/TyrePanel';
import { REAL_TRACKS } from '../../preview/synthetic/realTracks';

// --- Laptime Data Matrix ---
export function LaptimeDataView(): JSX.Element {
  const laps = Array.from({ length: 15 }).map((_, i) => ({
    lap: 15 - i,
    time: `1:${(34.42 + (Math.sin(i) * 0.4 + (i * 0.05))).toFixed(3)}`,
    s1: `29.${(140 + Math.sin(i) * 80).toFixed(0)}`,
    s2: `38.${(450 + Math.cos(i) * 120).toFixed(0)}`,
    s3: `26.${(522 + Math.sin(i * 2) * 60).toFixed(0)}`,
    speedTrap: (334 + Math.sin(i) * 6).toFixed(1),
    fuelUsed: (2.82 + Math.sin(i) * 0.04).toFixed(2),
    valid: i !== 3,
  }));

  return (
    <div className="view-card-container" data-testid="laptime-data-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 TIMING MATRIX</span>
        <h2 className="view-title">Laptime Data Matrix</h2>
        <p className="view-desc">Detailed lap-by-lap timing ledger including sector micro-splits, speed trap, and fuel burn per lap.</p>
      </div>
      <table className="stat-table">
        <thead>
          <tr>
            <th>Lap #</th>
            <th>Lap Time</th>
            <th>Sector 1</th>
            <th>Sector 2</th>
            <th>Sector 3</th>
            <th>Speed Trap</th>
            <th>Fuel Burn</th>
            <th>Validity</th>
          </tr>
        </thead>
        <tbody>
          {laps.map((l) => (
            <tr key={l.lap}>
              <td className="font-bold">L{l.lap}</td>
              <td className="font-mono text-cyan font-bold">{l.time}</td>
              <td className="font-mono">{l.s1}s</td>
              <td className="font-mono">{l.s2}s</td>
              <td className="font-mono">{l.s3}s</td>
              <td className="font-mono">{l.speedTrap} km/h</td>
              <td className="font-mono">{l.fuelUsed} L</td>
              <td>{l.valid ? <span className="status-badge status-badge-ok">VALID</span> : <span className="status-badge status-badge-warn">TRACK LIMITS</span>}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// --- Race Info & Weather ---
export function RaceInfoView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="race-info-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 RACE CONTROL</span>
        <h2 className="view-title">Race Info &amp; Weather Intelligence</h2>
        <p className="view-desc">Session status, track temperature timeline, rain radar forecasts, and track incident records.</p>
      </div>
      <div className="race-info-grid">
        <div className="race-stat-card">
          <span className="race-stat-label">SESSION PHASE</span>
          <div className="race-stat-val text-green">GREEN FLAG (RACING)</div>
          <span className="race-stat-sub">4 Hours of Spa • Lap 54 / 72</span>
        </div>
        <div className="race-stat-card">
          <span className="race-stat-label">TRACK / AMBIENT TEMP</span>
          <div className="race-stat-val num-cyan">28.4°C / 22.1°C</div>
          <span className="race-stat-sub">Track temp falling (-0.8°C / hour)</span>
        </div>
        <div className="race-stat-card">
          <span className="race-stat-label">RAIN PROBABILITY</span>
          <div className="race-stat-val text-amber">15% (DRY FORECAST)</div>
          <span className="race-stat-sub">Light clouds, zero precipitation</span>
        </div>
        <div className="race-stat-card">
          <span className="race-stat-label">SAFETY CAR STATUS</span>
          <div className="race-stat-val">CLEAR (NO CAUTION)</div>
          <span className="race-stat-sub">0 FCY / 0 Full SC periods in stint</span>
        </div>
      </div>
    </div>
  );
}

// --- Time Trial Hotlap Info ---
export function TimeTrialInfoView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="time-trial-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 HOTLAP PACING</span>
        <h2 className="view-title">Time Trial Info &amp; Ghost Delta</h2>
        <p className="view-desc">Micro-split targets against all-time track records, ghost vehicle delta, and delta rate derivative.</p>
      </div>
      <div className="tt-hero-cluster">
        <div className="tt-target-card">
          <span className="tt-target-label">TARGET LAP (TRACK RECORD)</span>
          <div className="tt-target-time text-purple">1:34.110</div>
          <span className="tt-target-meta">Kamui Kobayashi • Toyota GR010</span>
        </div>
        <div className="tt-target-card">
          <span className="tt-target-label">CURRENT DELTA (Δt)</span>
          <div className="tt-target-time text-green">-0.142s</div>
          <span className="tt-target-meta">Ahead of target ghost</span>
        </div>
        <div className="tt-target-card">
          <span className="tt-target-label">PROJECTED LAP TIME</span>
          <div className="tt-target-time text-cyan">1:33.968</div>
          <span className="tt-target-meta">On pace for class pole record</span>
        </div>
      </div>
    </div>
  );
}

// --- Multi-car Map (2D Circuit GPS) ---
export function MulticarMapView(): JSX.Element {
  const spaTrack = REAL_TRACKS.find((t) => t.id === 'spa');
  return (
    <div className="view-card-container" data-testid="multicar-map-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 2D GPS TRACK MAP</span>
        <h2 className="view-title">Multi-Car Circuit GPS Map</h2>
        <p className="view-desc">Authentic circuit geometry with live competitor GPS positions, class color-coding, and sector markers.</p>
      </div>
      <div className="multicar-map-wrapper">
        <TrackMap
          trackName={spaTrack?.name ?? 'Circuit de Spa-Francorchamps'}
          trackLengthM={spaTrack?.lengthM ?? 7004}
          driverPlace={1}
          driverSector={2}
          driverCarNumber="7"
        />
      </div>
    </div>
  );
}

// --- Multi-car Race Timing (Timing Tower) ---
export function MulticarRaceTimingView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-race-timing-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 PITWALL TOWER</span>
        <h2 className="view-title">Multi-Car Race Timing Tower</h2>
        <p className="view-desc">Standings, intervals, gaps to leader, pit counts, and tyre compounds across all multiclass vehicles.</p>
      </div>
      <div className="timing-tower-wrapper">
        <TimingTower scoring={null} />
      </div>
    </div>
  );
}

// --- Multi-car One-Lap Timing ---
export function MulticarOneLapTimingView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-one-lap-timing-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 ACTIVE LAP SPLITS</span>
        <h2 className="view-title">Multi-Car One-Lap Timing Matrix</h2>
        <p className="view-desc">Real-time split tracking for all cars currently traversing the circuit on their flying lap.</p>
      </div>
      <table className="stat-table">
        <thead>
          <tr>
            <th>Pos</th>
            <th>Car</th>
            <th>Driver</th>
            <th>Current Sector</th>
            <th>Sector 1 Split</th>
            <th>Sector 2 Split</th>
            <th>Projected Lap</th>
            <th>Delta to Leader</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td className="font-bold text-cyan">P1</td>
            <td>#7</td>
            <td>Player (Toyota GR010)</td>
            <td>Sector 2</td>
            <td className="text-purple font-mono">29.140s</td>
            <td className="text-purple font-mono">38.450s</td>
            <td className="font-mono font-bold text-cyan">1:34.112</td>
            <td>LEADER</td>
          </tr>
          <tr>
            <td className="font-bold">P2</td>
            <td>#51</td>
            <td>Ferrari 499P #51</td>
            <td>Sector 2</td>
            <td className="font-mono">29.210s</td>
            <td className="font-mono">38.580s</td>
            <td className="font-mono font-bold">1:34.380</td>
            <td className="text-red font-mono">+0.268s</td>
          </tr>
          <tr>
            <td className="font-bold">P3</td>
            <td>#6</td>
            <td>Porsche 963 #6</td>
            <td>Sector 1</td>
            <td className="font-mono">29.340s</td>
            <td className="font-mono">In Progress</td>
            <td className="font-mono font-bold">1:34.820</td>
            <td className="text-red font-mono">+0.708s</td>
          </tr>
        </tbody>
      </table>
    </div>
  );
}

// --- Multi-car Opponent Stats ---
export function MulticarOpponentStatsView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-opponent-stats-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 RIVAL INTELLIGENCE</span>
        <h2 className="view-title">Multi-Car Opponent Stats &amp; Strategy</h2>
        <p className="view-desc">Scouting data on key rivals: straight line top speeds, cornering strengths, and pit strategy deltas.</p>
      </div>
      <div className="opponents-grid">
        <div className="opponent-card">
          <div className="opp-header">
            <span className="car-badge">#51</span>
            <span className="opp-name">Ferrari 499P (P2, +2.1s)</span>
          </div>
          <div className="opp-metrics">
            <div><span>Top Speed:</span> <strong>342.0 km/h</strong> (+3.6 km/h on straights)</div>
            <div><span>Apex Speeds:</span> <strong>Slower in T7 &amp; T12</strong> (-2.1 km/h)</div>
            <div><span>Braking Stability:</span> <strong>High</strong> (Brake peak: 580°C)</div>
            <div><span>Estimated Pit Lap:</span> <strong>Lap 18</strong> (2 laps after you)</div>
          </div>
        </div>

        <div className="opponent-card">
          <div className="opp-header">
            <span className="car-badge">#6</span>
            <span className="opp-name">Porsche 963 (P3, +4.8s)</span>
          </div>
          <div className="opp-metrics">
            <div><span>Top Speed:</span> <strong>334.8 km/h</strong> (High downforce wing)</div>
            <div><span>Apex Speeds:</span> <strong>Faster in Pouhon</strong> (+3.2 km/h)</div>
            <div><span>Tyre Wear:</span> <strong>Accelerated on FR</strong> (0.095% / lap)</div>
            <div><span>Estimated Pit Lap:</span> <strong>Lap 17</strong></div>
          </div>
        </div>
      </div>
    </div>
  );
}

// --- Multi-car Race History (Position Bump Chart) ---
export function MulticarRaceHistoryView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-race-history-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 POSITION BUMP CHART</span>
        <h2 className="view-title">Multi-Car Race History (Lap-by-Lap Position Shifts)</h2>
        <p className="view-desc">Tracks position changes, on-track overtakes, and pit stop shuffles from start to finish.</p>
      </div>
      <div className="bump-chart-card">
        <div className="bump-summary-text">
          <span>Started: <strong>P3</strong></span> • <span>Current: <strong className="text-cyan">P1</strong></span> • <span>Overtakes Made: <strong>4</strong></span> • <span>Times Passed: <strong>1</strong></span>
        </div>
      </div>
    </div>
  );
}

// --- Multi-car Session Data ---
export function MulticarSessionDataView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-session-data-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 SESSION DIRECTORY</span>
        <h2 className="view-title">Multi-Car Session Data &amp; Entries</h2>
        <p className="view-desc">Full entry list: teams, drivers, chassis specs, BoP weights, and technical inspection status.</p>
      </div>
      <table className="stat-table">
        <thead>
          <tr>
            <th>#</th>
            <th>Team / Entrant</th>
            <th>Vehicle Model</th>
            <th>Class</th>
            <th>Tyre Compound</th>
            <th>Weight / Power BoP</th>
            <th>Status</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td className="font-bold">#7</td>
            <td>Toyota Gazoo Racing</td>
            <td>Toyota GR010 Hybrid</td>
            <td><span className="badge badge-subtle">Hypercar</span></td>
            <td>Medium (Set 3)</td>
            <td>1040 kg / 510 kW</td>
            <td><span className="status-badge status-badge-ok">ON TRACK</span></td>
          </tr>
          <tr>
            <td className="font-bold">#51</td>
            <td>Ferrari AF Corse</td>
            <td>Ferrari 499P</td>
            <td><span className="badge badge-subtle">Hypercar</span></td>
            <td>Medium (Set 2)</td>
            <td>1042 kg / 508 kW</td>
            <td><span className="status-badge status-badge-ok">ON TRACK</span></td>
          </tr>
          <tr>
            <td className="font-bold">#6</td>
            <td>Porsche Penske Motorsport</td>
            <td>Porsche 963</td>
            <td><span className="badge badge-subtle">Hypercar</span></td>
            <td>Hard (Set 1)</td>
            <td>1046 kg / 512 kW</td>
            <td><span className="status-badge status-badge-ok">ON TRACK</span></td>
          </tr>
        </tbody>
      </table>
    </div>
  );
}

// --- Multi-car Telemetry (Head to Head) ---
export function MulticarTelemetryView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multicar-telemetry-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 HEAD-TO-HEAD OVERLAY</span>
        <h2 className="view-title">Multi-Car Telemetry Overlay</h2>
        <p className="view-desc">Direct synchronous telemetry comparison between Player vehicle and selected opponent.</p>
      </div>
      <div className="telemetry-channels-grid">
        <div className="channel-box">
          <span className="channel-title">Speed Delta (vs #51 Ferrari 499P)</span>
          <div className="channel-val text-green">+2.4 km/h (Exit Speed Advantage)</div>
        </div>
        <div className="channel-box">
          <span className="channel-title">Braking Threshold Delta</span>
          <div className="channel-val text-cyan">-4.2m Later Braking</div>
        </div>
        <div className="channel-box">
          <span className="channel-title">Throttle Application Apex Delta</span>
          <div className="channel-val text-purple">+0.14s Earlier Throttle Pickup</div>
        </div>
      </div>
    </div>
  );
}

// --- Multilap Telemetry ---
export function MultilapTelemetryView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="multilap-telemetry-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 MULTI-LAP TRACES</span>
        <h2 className="view-title">Multilap Telemetry Overlay (5 Consecutive Laps)</h2>
        <p className="view-desc">Traces from Laps 10, 11, 12, 13, 14 overlaid on distance axis showing braking point drift.</p>
      </div>
      <div className="multilap-lines-list">
        <div className="multilap-row"><span className="legend-chip" style={{ color: '#22d3ee' }}>● Lap 14 (Current Best - 1:34.420)</span></div>
        <div className="multilap-row"><span className="legend-chip" style={{ color: '#38bdf8' }}>● Lap 13 (1:34.612)</span></div>
        <div className="multilap-row"><span className="legend-chip" style={{ color: '#60a5fa' }}>● Lap 12 (1:34.780)</span></div>
        <div className="multilap-row"><span className="legend-chip" style={{ color: '#818cf8' }}>● Lap 11 (1:35.012)</span></div>
        <div className="multilap-row"><span className="legend-chip" style={{ color: '#a78bfa' }}>● Lap 10 (1:35.240)</span></div>
      </div>
    </div>
  );
}

// --- Modular Engineering Workspace ---
export function WorkspaceView(): JSX.Element {
  return (
    <div className="view-card-container" data-testid="workspace-view">
      <div className="view-banner">
        <span className="badge badge-subtle">V1.1 MODULAR PITWALL</span>
        <h2 className="view-title">Custom Engineering Workspace</h2>
        <p className="view-desc">Modular 4-column layout combining multi-channel waveforms, live circuit map, and tyre thermals.</p>
      </div>
      <div className="workspace-demo-grid">
        <div className="workspace-tile"><TyrePanel /></div>
        <div className="workspace-tile"><TimingTower scoring={null} /></div>
      </div>
    </div>
  );
}
