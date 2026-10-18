/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). Builders for the backend DTOs the fake backend
 * returns, typed against src/lib/ipc.ts so they always have exactly the real shapes. Every CoreStatus built here
 * carries `origin: 'synthetic'`; the fake backend replaces it with the scenario's `origin` (an `'lmu'` scenario
 * simulates the LMU-origin look, still with synthetic data and the PREVIEW corner tag on the page).
 */
import type {
  AppConfig,
  AppInfo,
  ConfigSnapshot,
  CoreStatus,
  DropsDto,
  EngineerSlotDto,
  EngineerStatusDto,
  LmuSourceStatus,
  Mode,
  NetworkStatusDto,
  RatesDto,
  RecordingStatusDto,
  SessionStatusDto,
} from '../lib/ipc';

/** Wall clock (Unix s) of the preview world at page load; `Date.now()` is anchored here (src/preview/clock.ts). */
export const PREVIEW_NOW_UNIX_S = 1_792_312_000;

export const PREVIEW_APP_INFO: AppInfo = {
  name: 'LMU Race Engineering',
  version: '0.1.0',
  identifier: 'com.lmu-race-engineering.desktop',
  build_profile: 'debug',
  environment: 'development',
  log_dir: 'D:\\RaceEngineeringData\\preview\\logs',
};

export function previewConfig(mode: Mode | null): AppConfig {
  return {
    schema_version: 1,
    application: { mode, environment: 'development' },
    network: { relay_host: 'relay.example.test', relay_port: 7443, connect_timeout_ms: 5000, extra_ca_file: null },
    telemetry: { recording_enabled: true, recording_dir: 'D:\\RaceEngineeringData\\preview\\recordings' },
    fuel: { window_laps: 3, reserve_laps: 1 },
    ui: { scale: 1, max_refresh_hz: 60 },
    driver: { auto_stream: false },
    engines: { default_alerts_enabled: true },
    window: { x: null, y: null, width: null, height: null, maximized: true },
  };
}

export function previewSnapshot(mode: Mode | null, overrides: Partial<ConfigSnapshot> = {}): ConfigSnapshot {
  return {
    config: previewConfig(mode),
    path: 'D:\\RaceEngineeringData\\preview\\config.json',
    issue: null,
    restart_required: false,
    core_issue: null,
    ...overrides,
  };
}

export const NO_SESSION: SessionStatusDto = {
  session_id: null,
  session_type: null,
  track: null,
  car: null,
  vehicle_class: null,
  started_at_unix_s: null,
};

export const NO_RATES: RatesDto = {
  telemetry_read_hz: null,
  scoring_read_hz: null,
  frames_sent_hz: null,
  bytes_sent_per_s: null,
  frames_dropped_total: 0,
};

export const NO_DROPS: DropsDto = {
  reader_overrun: 0,
  pipeline_overrun: 0,
  recorder_overrun: 0,
  net_dropped_obsolete: 0,
  net_events_dropped: 0,
  ui_dropped: 0,
};

export const RECORDING_OFF: RecordingStatusDto = {
  state: 'off',
  recording_id: null,
  bytes_written: 0,
  frames_written: 0,
  frames_lost: 0,
  disk_free_bytes: null,
  error: null,
};

export function network(overrides: Partial<NetworkStatusDto> = {}): NetworkStatusDto {
  return {
    state: 'DISCONNECTED',
    streaming: false,
    rtt_ms: null,
    connect_code: null,
    code_expires_at_unix_s: null,
    engineers_connected: 0,
    engineers_max: 2,
    degraded_reason: null,
    driver_away: false,
    last_error: null,
    security_issue: false,
    failure: null,
    relogin_required: false,
    engineers: [],
    ...overrides,
  };
}

export function lmu(state: string, detail: string | null = null, gameVersion: number | null = 1041): LmuSourceStatus {
  return { state, detail, game_version: gameVersion };
}

/** Invented engineer accounts (no real names). */
export const ENGINEER_SLOTS: readonly EngineerSlotDto[] = [
  { connection_id: 41, username: 'pitwall_ana', display_name: 'Ana (Race Eng.)', link: 'healthy', connected_since_unix_s: 1_792_310_400 },
  { connection_id: 57, username: 'strategy_kai', display_name: 'Kai (Strategy)', link: 'healthy', connected_since_unix_s: 1_792_311_100 },
];

export function driverStatus(overrides: Partial<CoreStatus> = {}): CoreStatus {
  return {
    mode: 'driver',
    origin: 'synthetic',
    lmu: lmu('UNKNOWN', null, null),
    session: NO_SESSION,
    recording: RECORDING_OFF,
    network: network(),
    rates: NO_RATES,
    drops: NO_DROPS,
    engineer: null,
    ...overrides,
  };
}

export function engineerBlock(overrides: Partial<EngineerStatusDto> = {}): EngineerStatusDto {
  return {
    phase: 'idle',
    driver_name: null,
    ended_reason: null,
    stream_health: 'no_data',
    stream_age_s: null,
    driver_away: false,
    incompatible_schema: false,
    ...overrides,
  };
}

export function engineerStatus(overrides: Partial<CoreStatus> = {}): CoreStatus {
  return {
    mode: 'engineer',
    origin: 'synthetic',
    lmu: lmu('UNKNOWN', null, null),
    session: NO_SESSION,
    recording: { ...RECORDING_OFF, state: 'unknown' },
    network: network({ engineers_max: 2 }),
    rates: NO_RATES,
    drops: NO_DROPS,
    engineer: engineerBlock(),
    ...overrides,
  };
}
