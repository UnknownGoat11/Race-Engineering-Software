/**
 * Typed bridge to the Rust backend (src-tauri/src/commands.rs).
 *
 * Only explicit, typed commands exist — there is no generic "execute" entry point. Payload types mirror the
 * Rust DTOs exactly; field names are snake_case on both sides (and in the on-disk config file), and every Rust
 * command is declared with `rename_all = "snake_case"`, so argument names are snake_case too. There is no
 * renaming layer to drift out of sync. `COMMAND_NAMES` is checked against the Rust command lists by a Rust test
 * (`commands::tests::command_lists_agree`).
 *
 * High-rate telemetry does NOT use request/response commands; it arrives as raw `UiBatch` buffers on a Tauri
 * Channel (D-012, D-023): see `bridgeSubscribe` and `src/telemetry/bridge.ts`.
 *
 * Secrets: passwords are only ever function arguments passed straight to `invoke`; nothing here stores them.
 * 64-bit integers (ids, byte counters, nanosecond timestamps) arrive as JS numbers: ids are relay counters (far
 * below 2^53); nanosecond timestamps lose sub-microsecond precision, which is irrelevant for display.
 */
import { invoke, isTauri, type Channel } from '@tauri-apps/api/core';

// ---------------------------------------------------------------------------------------------------------
// Configuration (mirror src-tauri/src/config.rs, src-tauri/src/state.rs and src-tauri/src/app_info.rs)
// ---------------------------------------------------------------------------------------------------------

export type Mode = 'driver' | 'engineer';
export type Environment = 'development' | 'staging' | 'production';
export type BuildProfile = 'debug' | 'release';

export interface AppInfo {
  readonly name: string;
  readonly version: string;
  readonly identifier: string;
  readonly build_profile: BuildProfile;
  readonly environment: Environment;
  readonly log_dir: string | null;
}

export interface ApplicationConfig {
  readonly mode: Mode | null;
  /** Informational: always the build environment. */
  readonly environment: Environment;
}

export interface NetworkConfig {
  /** Host name or IP address; empty = no relay configured. Changes take effect after a restart. */
  readonly relay_host: string;
  readonly relay_port: number;
  readonly connect_timeout_ms: number;
  /** Only honoured when the build environment is `development`. */
  readonly extra_ca_file: string | null;
}

export interface TelemetryConfig {
  /** Recording runs independent of login and network (owner decision Q9). */
  readonly recording_enabled: boolean;
  /** Absolute path; null = per-user app data directory. */
  readonly recording_dir: string | null;
}

export interface FuelConfig {
  /** Laps averaged for fuel per lap: 1, 2, 3, 5 or 10 (other values are rejected). */
  readonly window_laps: number;
  readonly reserve_laps: number;
}

export interface UiConfig {
  readonly scale: number;
  readonly max_refresh_hz: number;
}

export interface DriverConfig {
  /** Start streaming automatically once LMU telemetry is available and the user is logged in. Default false (Q7). */
  readonly auto_stream: boolean;
}

/** Race-engine options; applied (with `fuel`) to the driver pipeline at the next driver start. */
export interface EnginesConfig {
  readonly default_alerts_enabled: boolean;
}

/** Main-window geometry in physical pixels; null = default from tauri.conf.json. */
export interface WindowConfig {
  readonly x: number | null;
  readonly y: number | null;
  readonly width: number | null;
  readonly height: number | null;
  readonly maximized: boolean;
}

export interface AppConfig {
  readonly schema_version: number;
  readonly application: ApplicationConfig;
  readonly network: NetworkConfig;
  readonly telemetry: TelemetryConfig;
  readonly fuel: FuelConfig;
  readonly ui: UiConfig;
  readonly driver: DriverConfig;
  readonly engines: EnginesConfig;
  readonly window: WindowConfig;
}

export type ConfigIssueKind = 'invalid' | 'unsupported_version' | 'unreadable';

export interface ConfigFieldError {
  readonly field: string;
  readonly message: string;
}

/** Why the stored config could not be used. Safe defaults are active whenever this is present. */
export interface ConfigIssue {
  readonly kind: ConfigIssueKind;
  readonly message: string;
  readonly errors: readonly ConfigFieldError[];
  /** Where the rejected file was moved to (config.invalid-<timestamp>.json), if it was preserved. */
  readonly preserved_as: string | null;
}

export interface ConfigSnapshot {
  readonly config: AppConfig;
  readonly path: string;
  readonly issue: ConfigIssue | null;
  /** Saved relay settings differ from the running ones; they apply after an app restart. */
  readonly restart_required: boolean;
  /** The core could not use the saved relay settings and runs without a relay (reason). */
  readonly core_issue: string | null;
}

// ---------------------------------------------------------------------------------------------------------
// Core status and events (mirror crates/app-core/src/bridge/status.rs)
// ---------------------------------------------------------------------------------------------------------

export type AppMode = 'driver' | 'engineer';
/** Provenance of the telemetry. `synthetic` must always be labelled as such in the UI. */
export type Origin = 'lmu' | 'synthetic';

export interface LmuSourceStatus {
  /** `NOT_RUNNING` | `CONNECTING` | `LIVE` | `PAUSED` | `STALE` | `ERROR`, or `UNKNOWN` when not observed. */
  readonly state: string;
  readonly detail: string | null;
  readonly game_version: number | null;
}

export interface SessionStatusDto {
  readonly session_id: number | null;
  readonly session_type: string | null;
  readonly track: string | null;
  readonly car: string | null;
  readonly vehicle_class: string | null;
  readonly started_at_unix_s: number | null;
}

export interface RecordingStatusDto {
  /**
   * `off` | `active` | `suspended` | `failed`; `disabled` when no recorder exists; `unknown` when not
   * reported yet (engineer: the driver's recording state arrives over the relay).
   */
  readonly state: string;
  readonly recording_id: string | null;
  readonly bytes_written: number;
  readonly frames_written: number;
  readonly frames_lost: number;
  readonly disk_free_bytes: number | null;
  readonly error: string | null;
}

/** One engineer holding a slot in the driver's relay session. */
export interface EngineerSlotDto {
  /** Target of `driver_kick_engineer`. */
  readonly connection_id: number;
  readonly username: string;
  readonly display_name: string;
  /** `healthy` | `degraded` | `reconnecting`. */
  readonly link: string;
  /** Relay wall clock, seconds since the Unix epoch. */
  readonly connected_since_unix_s: number;
}

export interface NetworkStatusDto {
  /**
   * Transport state (`DISCONNECTED` | `CONNECTING` | `AUTHENTICATING` | `CONNECTED` | `DEGRADED` | `STALE` |
   * `RECONNECTING` | `FAILED`), `NOT_CONFIGURED` (no relay, or the display-only demo) or `UNKNOWN`.
   */
  readonly state: string;
  /** The driver is streaming live telemetry (started and not stopped). */
  readonly streaming: boolean;
  readonly rtt_ms: number | null;
  /** The engineer connection code: shown to the driver only, never logged or stored. */
  readonly connect_code: string | null;
  readonly code_expires_at_unix_s: number | null;
  readonly engineers_connected: number;
  readonly engineers_max: number;
  readonly degraded_reason: string | null;
  readonly driver_away: boolean;
  readonly last_error: string | null;
  readonly security_issue: boolean;
  /** Why the link is FAILED (a user action is needed). */
  readonly failure: string | null;
  /** The user must log in again. */
  readonly relogin_required: boolean;
  /** Engineers holding a slot (driver mode). */
  readonly engineers: readonly EngineerSlotDto[];
}

export interface RatesDto {
  readonly telemetry_read_hz: number | null;
  readonly scoring_read_hz: number | null;
  readonly frames_sent_hz: number | null;
  readonly bytes_sent_per_s: number | null;
  readonly frames_dropped_total: number;
}

export interface DropsDto {
  readonly reader_overrun: number;
  readonly pipeline_overrun: number;
  readonly recorder_overrun: number;
  readonly net_dropped_obsolete: number;
  /** Session events dropped by the bounded network publisher queue. */
  readonly net_events_dropped: number;
  readonly ui_dropped: number;
}

/** Engineer-mode connection and stream status (`null` in driver mode). */
export interface EngineerStatusDto {
  /** `idle` | `joining` | `awaiting_confirmation` | `monitoring` | `ended` | `not_started`. */
  readonly phase: string;
  readonly driver_name: string | null;
  readonly ended_reason: string | null;
  /** `no_data` | `healthy` | `stale`. */
  readonly stream_health: string;
  readonly stream_age_s: number | null;
  readonly driver_away: boolean;
  readonly incompatible_schema: boolean;
}

/** `get_core_status` result and `core:status` event payload (4 Hz). */
export interface CoreStatus {
  readonly mode: AppMode;
  readonly origin: Origin;
  readonly lmu: LmuSourceStatus;
  readonly session: SessionStatusDto;
  readonly recording: RecordingStatusDto;
  readonly network: NetworkStatusDto;
  readonly rates: RatesDto;
  readonly drops: DropsDto;
  readonly engineer: EngineerStatusDto | null;
}

export type AlertLevel = 'info' | 'warning' | 'critical';

/** `core:alert` payload. */
export interface AlertDto {
  readonly id: string;
  readonly name: string;
  readonly level: AlertLevel;
  readonly raised: boolean;
  readonly message: string;
  readonly timestamp_ns: number;
}

/** `core:error` payload. */
export interface ErrorDto {
  readonly category: string;
  readonly message: string;
  readonly fatal: boolean;
}

/** `core:engineer` payload. */
export interface EngineerDto {
  readonly phase: string;
  readonly driver_name: string | null;
  readonly car: string | null;
  readonly vehicle_class: string | null;
  readonly track: string | null;
  readonly session_type: string | null;
  readonly ended_reason: string | null;
}

/** JSON events emitted by the backend (listen with `@tauri-apps/api/event`). */
export const CORE_EVENTS = [
  'core:status',
  'core:scoring',
  'core:session_event',
  'core:alert',
  'core:capabilities',
  'core:engine_state',
  'core:engineer',
  'core:error',
] as const;
export type CoreEventName = (typeof CORE_EVENTS)[number];

// ---------------------------------------------------------------------------------------------------------
// Command DTOs (mirror src-tauri/src/commands.rs and src-tauri/src/core_bridge.rs)
// ---------------------------------------------------------------------------------------------------------

export type Role = 'driver' | 'engineer';

export interface AccountInfoDto {
  /** 32 lowercase hex digits. */
  readonly account: string;
  readonly username: string;
  readonly display_name: string;
  readonly role: Role;
}

export interface LoginStateDto {
  /** A refresh token exists for `role` (the active mode's role). */
  readonly authenticated: boolean;
  readonly role: Role;
  /** Known after a login in this run; not restored from the credential store. */
  readonly account: AccountInfoDto | null;
}

/** The driver's identity card shown before the engineer confirms (D-022). */
export interface IdentityCardDto {
  readonly session_id: number;
  readonly driver_display_name: string;
  readonly driver_username: string;
  readonly car: string | null;
  readonly vehicle_class: string | null;
  readonly track: string | null;
  readonly session_type: string | null;
  readonly started_at_ns: number;
  readonly schema_version: number;
}

export type StageId = 'dns' | 'tcp' | 'tls' | 'hello' | 'auth' | 'session_service' | 'rtt';
export type StageOutcome = 'passed' | 'failed' | 'skipped';

export interface StageResultDto {
  readonly stage: StageId;
  readonly stage_label: string;
  readonly outcome: StageOutcome;
  readonly detail: string | null;
  readonly error: string | null;
  readonly duration_ms: number;
}

export interface RttSummaryDto {
  readonly samples: number;
  readonly min_ms: number;
  readonly avg_ms: number;
  readonly max_ms: number;
  readonly clock_offset_ns: number | null;
  readonly clock_uncertainty_ns: number | null;
}

export interface ConnectivityReportDto {
  readonly endpoint: string;
  readonly passed: boolean;
  readonly stages: readonly StageResultDto[];
  readonly rtt: RttSummaryDto | null;
}

/** A frontend error report for the app log. Never put secrets or connection codes in it. */
export interface UiErrorEntry {
  readonly message: string;
  readonly stack: string | null;
  readonly component: string | null;
}

/** Result of `log_ui_error`. */
export interface UiErrorLogged {
  /** False when the rate limit dropped this entry. */
  readonly logged: boolean;
  /** Entries dropped by the rate limit before this one (or pending, when this one was dropped). */
  readonly suppressed: number;
}

export interface BridgeSubscribeResponse {
  readonly subscription_id: number;
  /** Provenance of the subscribed stream at subscribe time (`core:status.origin` tracks later changes). */
  readonly origin: Origin;
}

export const DEMO_SCENARIOS = [
  'demo',
  'normal_lap',
  'pit_lap',
  'rain',
  'hybrid',
  'non_hybrid',
  'safety_car',
] as const;
export type DemoScenario = (typeof DEMO_SCENARIOS)[number];

// ---------------------------------------------------------------------------------------------------------
// Commands and errors
// ---------------------------------------------------------------------------------------------------------

/** Every backend command, in the order of `COMMAND_NAMES` in src-tauri/src/commands.rs. */
export const COMMAND_NAMES = [
  'get_app_info',
  'get_config',
  'set_mode',
  'update_settings',
  'get_core_status',
  'test_relay_connection',
  'log_ui_error',
  'login',
  'register_account',
  'logout',
  'get_login_state',
  'driver_start_streaming',
  'driver_stop_streaming',
  'driver_end_session',
  'driver_new_code',
  'driver_kick_engineer',
  'driver_set_recording',
  'driver_add_marker',
  'driver_ack_alert',
  'engineer_join',
  'engineer_confirm',
  'engineer_cancel',
  'engineer_disconnect',
  'engineer_request_snapshot',
  'demo_start',
  'demo_stop',
  'bridge_subscribe',
  'bridge_unsubscribe',
] as const;
export type CommandName = (typeof COMMAND_NAMES)[number];

/** Stable error codes (src-tauri/src/commands.rs `codes`), plus the two produced on this side. */
export type CommandErrorCode =
  | 'config_invalid'
  | 'config_serialize_failed'
  | 'config_write_failed'
  | 'invalid_input'
  | 'not_implemented'
  | 'relay_not_configured'
  | 'restart_required'
  | 'credential_store_failed'
  | 'bridge_closed'
  | 'demo_active'
  | 'unknown_scenario'
  | 'core_shutdown'
  | 'invalid_mode'
  | 'driver_not_running'
  | 'driver_already_running'
  | 'engineer_not_running'
  | 'engineer_already_running'
  | 'demo_error'
  | 'settings_error'
  | 'pipeline_error'
  | 'runtime_error'
  | 'io_error'
  | 'recording_unavailable'
  | 'network_unavailable'
  | 'not_streaming'
  | 'not_recording'
  | 'recording_failed'
  | 'unknown_alert'
  | 'network_busy'
  | 'network_error'
  | 'code_malformed'
  | 'code_invalid'
  | 'code_expired'
  | 'session_full'
  | 'session_closed'
  | 'not_an_engineer'
  | 'rate_limited'
  | 'relogin_required'
  | 'relay_unreachable'
  | 'relay_incompatible'
  | 'environment_mismatch'
  | 'join_refused'
  | 'join_busy'
  | 'join_cancelled'
  | 'client_closed'
  | 'connect_failed'
  | 'invalid_credentials'
  | 'auth_refused'
  | 'role_mismatch'
  | 'handshake_failed'
  | 'backend_unavailable'
  | 'backend_error';

/** A failed backend command. `code` is a stable machine-readable identifier from the Rust side. */
export class IpcError extends Error {
  override readonly name = 'IpcError';

  constructor(
    readonly command: CommandName,
    readonly code: CommandErrorCode | (string & {}),
    message: string,
  ) {
    super(message);
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function toIpcError(command: CommandName, error: unknown): IpcError {
  if (error instanceof IpcError) return error;
  if (isRecord(error) && typeof error['code'] === 'string' && typeof error['message'] === 'string') {
    return new IpcError(command, error['code'], error['message']);
  }
  if (typeof error === 'string') return new IpcError(command, 'backend_error', error);
  if (error instanceof Error) return new IpcError(command, 'backend_error', error.message);
  return new IpcError(command, 'backend_error', 'Unknown backend error');
}

async function call<T>(command: CommandName, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new IpcError(
      command,
      'backend_unavailable',
      'The desktop backend is not available. Start the app with `pnpm tauri dev`.',
    );
  }
  try {
    return await invoke<T>(command, args);
  } catch (error: unknown) {
    throw toIpcError(command, error);
  }
}

/** A command whose Rust result is `()` (serialized as `null`). */
async function callUnit(command: CommandName, args?: Record<string, unknown>): Promise<void> {
  await call<unknown>(command, args);
}

// --- general ----------------------------------------------------------------------------------------------

/** Application name, version, build profile and build environment. */
export function getAppInfo(): Promise<AppInfo> {
  return call<AppInfo>('get_app_info');
}

/** Current configuration, its file path, and any load issue (invalid file → safe defaults). */
export function getConfig(): Promise<ConfigSnapshot> {
  return call<ConfigSnapshot>('get_config');
}

/**
 * Persist the operating mode and start its services. The choice is saved even when the services fail to start;
 * that failure is the rejection (then `getConfig` already shows the new mode).
 */
export function setMode(mode: Mode): Promise<ConfigSnapshot> {
  return call<ConfigSnapshot>('set_mode', { mode });
}

/**
 * Validate, save and apply a full configuration. `application.mode` must be unchanged (use `setMode`). Relay
 * changes set `restart_required`.
 */
export function updateSettings(patch: AppConfig): Promise<ConfigSnapshot> {
  return call<ConfigSnapshot>('update_settings', { patch });
}

/** Composite status (same shape as the `core:status` event). */
export function getCoreStatus(): Promise<CoreStatus> {
  return call<CoreStatus>('get_core_status');
}

/** Seven-stage connectivity test against the configured relay. */
export function testRelayConnection(): Promise<ConnectivityReportDto> {
  return call<ConnectivityReportDto>('test_relay_connection');
}

/** Write a frontend error to the app log (rate-limited, bounded, redacted on the Rust side). */
export function logUiError(entry: UiErrorEntry): Promise<UiErrorLogged> {
  return call<UiErrorLogged>('log_ui_error', { entry });
}

// --- auth -------------------------------------------------------------------------------------------------

/** Log in for the active mode's role. The password is passed through once and never stored here. */
export function login(username: string, password: string): Promise<AccountInfoDto> {
  return call<AccountInfoDto>('login', { username, password });
}

/** Create an account on the relay. The password is passed through once and never stored here. */
export function registerAccount(
  username: string,
  displayName: string,
  role: Role,
  password: string,
): Promise<AccountInfoDto> {
  return call<AccountInfoDto>('register_account', { username, display_name: displayName, role, password });
}

/** Forget the tokens and delete the stored refresh tokens. */
export function logout(): Promise<void> {
  return callUnit('logout');
}

export function getLoginState(): Promise<LoginStateDto> {
  return call<LoginStateDto>('get_login_state');
}

// --- driver -----------------------------------------------------------------------------------------------

export function driverStartStreaming(): Promise<void> {
  return callUnit('driver_start_streaming');
}

export function driverStopStreaming(): Promise<void> {
  return callUnit('driver_stop_streaming');
}

export function driverEndSession(): Promise<void> {
  return callUnit('driver_end_session');
}

/** Request a new one-time connection code; it appears in `core:status.network.connect_code`. */
export function driverNewCode(): Promise<void> {
  return callUnit('driver_new_code');
}

export function driverKickEngineer(connectionId: number): Promise<void> {
  return callUnit('driver_kick_engineer', { connection_id: connectionId });
}

export function driverSetRecording(enabled: boolean): Promise<void> {
  return callUnit('driver_set_recording', { enabled });
}

/** Add a marker (1–200 characters) to the active recording. */
export function driverAddMarker(text: string): Promise<void> {
  return callUnit('driver_add_marker', { text });
}

/** Acknowledges an active alert by the `id` of its `core:alert` event (`unknown_alert` if not active). */
export function driverAckAlert(id: string): Promise<void> {
  return callUnit('driver_ack_alert', { id });
}

// --- engineer ---------------------------------------------------------------------------------------------

/**
 * Redeem a connection code typed by the user and return the driver's identity card. Nothing is streamed until
 * `engineerConfirm`; `engineerCancel` declines (the code stays used). The only way to join a driver.
 */
export function engineerJoin(code: string): Promise<IdentityCardDto> {
  return call<IdentityCardDto>('engineer_join', { code });
}

export function engineerConfirm(): Promise<void> {
  return callUnit('engineer_confirm');
}

export function engineerCancel(): Promise<void> {
  return callUnit('engineer_cancel');
}

export function engineerDisconnect(): Promise<void> {
  return callUnit('engineer_disconnect');
}

export function engineerRequestSnapshot(): Promise<void> {
  return callUnit('engineer_request_snapshot');
}

// --- demo -------------------------------------------------------------------------------------------------

/** Start a synthetic, display-only demo (never recorded or streamed). Default scenario: `demo`. */
export function demoStart(scenario?: DemoScenario): Promise<void> {
  return callUnit('demo_start', { scenario: scenario ?? null });
}

/** Stop the demo; the live LMU pipeline restarts. */
export function demoStop(): Promise<void> {
  return callUnit('demo_stop');
}

// --- bridge -----------------------------------------------------------------------------------------------

/** Subscribe `channel` to the raw `UiBatch` stream. Prefer `createBridgeSubscribe` (src/telemetry/bridge.ts). */
export function bridgeSubscribe(channel: Channel<ArrayBuffer>): Promise<BridgeSubscribeResponse> {
  return call<BridgeSubscribeResponse>('bridge_subscribe', { on_batch: channel });
}

/** End a batch subscription (idempotent). */
export function bridgeUnsubscribe(subscriptionId: number): Promise<void> {
  return callUnit('bridge_unsubscribe', { subscription_id: subscriptionId });
}
