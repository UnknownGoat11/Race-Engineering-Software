/**
 * Typed bridge to the Rust backend (src-tauri/src/commands.rs).
 *
 * Only explicit, typed commands exist — there is no generic "execute" entry point. Payload types mirror the
 * Rust DTOs exactly; field names are snake_case on both sides (and in the on-disk config file), so there is
 * no renaming layer to drift out of sync.
 *
 * High-rate telemetry will NOT use these request/response commands; it arrives via a Tauri Channel in batches
 * (docs/DECISIONS.md D-012). This module is the Gate 0 placeholder.
 */
import { invoke, isTauri } from '@tauri-apps/api/core';

// ---------------------------------------------------------------------------------------------------------
// Payload types (mirror src-tauri/src/config.rs and src-tauri/src/app_info.rs)
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
  readonly environment: Environment;
}

export interface NetworkConfig {
  readonly relay_host: string;
  readonly relay_port: number;
  readonly connect_timeout_ms: number;
  /** Only honoured when the build environment is `development`. */
  readonly extra_ca_file: string | null;
}

export interface TelemetryConfig {
  readonly recording_enabled: boolean;
  readonly recording_dir: string | null;
}

export interface FuelConfig {
  readonly window_laps: number;
  readonly reserve_laps: number;
}

export interface UiConfig {
  readonly scale: number;
  readonly max_refresh_hz: number;
}

export interface AppConfig {
  readonly schema_version: number;
  readonly application: ApplicationConfig;
  readonly network: NetworkConfig;
  readonly telemetry: TelemetryConfig;
  readonly fuel: FuelConfig;
  readonly ui: UiConfig;
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
}

// ---------------------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------------------

export type CommandName = 'get_app_info' | 'get_config' | 'set_mode';

/** A failed backend command. `code` is a stable machine-readable identifier from the Rust side. */
export class IpcError extends Error {
  override readonly name = 'IpcError';

  constructor(
    readonly command: CommandName,
    readonly code: string,
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

// ---------------------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------------------

/** Application name, version, build profile and build environment. */
export function getAppInfo(): Promise<AppInfo> {
  return call<AppInfo>('get_app_info');
}

/** Current configuration, its file path, and any load issue (invalid file → safe defaults). */
export function getConfig(): Promise<ConfigSnapshot> {
  return call<ConfigSnapshot>('get_config');
}

/** Persist the operating mode. Resolves with the updated configuration. */
export function setMode(mode: Mode): Promise<ConfigSnapshot> {
  return call<ConfigSnapshot>('set_mode', { mode });
}
