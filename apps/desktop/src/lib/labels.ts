/**
 * The single copy table of the UI (REDESIGN.md §2.9, ux audit §4).
 *
 * Every backend enumeration that reaches the screen goes through a mapper here, so no raw enum text
 * (`NOT_RUNNING`, `normal_lap`, `healthy`, …) is ever rendered. Rules:
 * - `word` is the short state word for badges and driver tiles: UPPER CASE, spaces, no underscores.
 * - `label` is the sentence-case badge/line text; `helper` is an optional one-line explanation or next step.
 * - A value this build does not know maps to a neutral "Unknown" copy, never to the raw string.
 * - A live LMU state with a synthetic origin reads DEMO, never LIVE (owner rule OR-1).
 */
import type { BuildProfile, CommandErrorCode, DemoScenario, Environment, Mode, Origin } from './ipc';

export { vehicleClassCode } from './classes';

/** Display copy of one state. */
export interface StateCopy {
  /** Short badge / tile word: upper case, no underscores. */
  readonly word: string;
  /** Sentence-case label. */
  readonly label: string;
  /** One-line explanation or next step, or null. */
  readonly helper: string | null;
}

function copy(word: string, label: string, helper: string | null = null): StateCopy {
  return { word, label, helper };
}

/** Copy for a value this build does not know (the raw value is deliberately not shown). */
export const UNKNOWN_STATE: StateCopy = copy('UNKNOWN', 'Unknown', null);

// ---------------------------------------------------------------------------------------------------------
// Terminology (ux audit §4.2) — one word per concept, used everywhere
// ---------------------------------------------------------------------------------------------------------

export const COPY = {
  engineerCode: 'Engineer code',
  engineerCodeHint: 'Single use: one code per engineer.',
  newEngineerCode: 'New engineer code',
  copyCode: 'Copy code',
  codeCopied: 'Code copied',
  noCodeYet: 'Start streaming to get a code',
  joinDriver: 'Join driver',
  joinWithNewCode: 'Join with new code',
  confirmJoin: 'Confirm and start watching',
  notThisDriver: 'Not this driver',
  notThisDriverConsequence: "This code can't be reused. Ask the driver for a new one.",
  leaveSession: 'Leave session',
  leaveSessionConsequence: 'You need a new engineer code from the driver to rejoin.',
  endSession: 'End session',
  endSessionPrompt: 'End for everyone?',
  endSessionConsequence: 'Every engineer is disconnected and needs a new code to rejoin.',
  removeEngineer: 'Remove',
  removeEngineerConsequence: 'This engineer is disconnected and needs a new code to rejoin.',
  streaming: 'Streaming',
  startStreaming: 'Start streaming',
  stopStreaming: 'Stop streaming',
  startRecording: 'Start recording',
  stopRecording: 'Stop recording',
  addMarker: 'Add marker',
  refreshAllData: 'Refresh all data',
  chooseChannels: 'Choose channels',
  driverChip: 'DRIVER',
  leaderGap: 'LDR',
  status: 'Status',
  driver: 'Driver',
  driverSetup: 'Setup',
  pitBoard: 'Pit board',
  analysis: 'Analysis',
  dashboard: 'Dashboard',
  pitwall: 'Pitwall',
  settings: 'Settings',
  signIn: 'Sign in',
  signInAgain: 'Sign in again',
  logOut: 'Log out',
  updateRequired: 'Update required',
  notConnected: 'Not connected to a driver',
  notConnectedHint: 'Ask your driver for a one-time engineer code.',
  confirm: 'Confirm',
  cancel: 'Cancel',
  demoBand: 'DEMO — synthetic data, not recorded or streamed',
  demoDisplayOnly: 'Demo is display-only',
  syntheticData: 'SYNTHETIC DATA',
} as const;

// ---------------------------------------------------------------------------------------------------------
// App enumerations (kept from the original module)
// ---------------------------------------------------------------------------------------------------------

/** Upper-case display labels for enumerations shown in the UI. */
export function modeLabel(mode: Mode): string {
  switch (mode) {
    case 'driver':
      return 'DRIVER';
    case 'engineer':
      return 'ENGINEER';
  }
}

export function environmentLabel(environment: Environment): string {
  switch (environment) {
    case 'development':
      return 'DEVELOPMENT';
    case 'staging':
      return 'STAGING';
    case 'production':
      return 'PRODUCTION';
  }
}

export function buildProfileLabel(profile: BuildProfile): string {
  return profile === 'debug' ? 'DEBUG' : 'RELEASE';
}

// ---------------------------------------------------------------------------------------------------------
// Status DTO enumerations (crates/app-core/src/bridge/status.rs)
// ---------------------------------------------------------------------------------------------------------

/** LMU source state words (`LmuSourceStatus.state`). */
export const LMU_STATES = ['NOT_RUNNING', 'CONNECTING', 'LIVE', 'PAUSED', 'STALE', 'ERROR', 'UNKNOWN'] as const;

/**
 * LMU source state. `LIVE` with a synthetic origin is the demo and reads DEMO. An unknown origin (null) is not
 * proof of LMU data either, so `LIVE` then reads "Live (source unknown)" rather than "LMU live".
 */
export function lmuStateLabel(state: string | null | undefined, origin: Origin | null): StateCopy {
  switch (state) {
    case 'NOT_RUNNING':
      return copy('NOT RUNNING', 'LMU not running', 'Start Le Mans Ultimate. The app connects automatically.');
    case 'CONNECTING':
      return copy('CONNECTING', 'Connecting to LMU');
    case 'LIVE':
      if (origin === 'synthetic') {
        return copy('DEMO', 'Demo (synthetic)', 'Display only — not recorded or streamed.');
      }
      if (origin === 'lmu') return copy('LIVE', 'LMU live');
      return copy('UNVERIFIED', 'Live (source unknown)', 'The data source is not confirmed as LMU.');
    case 'PAUSED':
      return copy('PAUSED', 'LMU paused', 'Telemetry resumes when the game runs again.');
    case 'STALE':
      return copy('STALE', 'No new data', 'LMU sends no updates (paused, loading or in menus).');
    case 'ERROR':
      return copy('ERROR', 'LMU error');
    case 'UNKNOWN':
      return copy('CHECKING', 'Checking…');
    default:
      return UNKNOWN_STATE;
  }
}

/** Transport states (`NetworkStatusDto.state`). */
export const NETWORK_STATES = [
  'DISCONNECTED',
  'CONNECTING',
  'AUTHENTICATING',
  'CONNECTED',
  'DEGRADED',
  'STALE',
  'RECONNECTING',
  'FAILED',
  'NOT_CONFIGURED',
  'UNKNOWN',
] as const;

export interface NetworkLabelOptions {
  /** The display-only demo is running (streaming is never available then). */
  readonly demo?: boolean;
}

/** Streaming (relay link) state. "Relay" is an implementation detail and never appears in the copy. */
export function networkStateLabel(state: string | null | undefined, options?: NetworkLabelOptions): StateCopy {
  switch (state) {
    case 'DISCONNECTED':
      return copy('OFF', 'Not streaming', 'Press Start streaming to share with your engineer.');
    case 'NOT_CONFIGURED':
      return options?.demo === true
        ? copy('NOT AVAILABLE', 'Streaming not available', 'Not available in demo.')
        : copy('NOT SET UP', 'Streaming not set up', 'Streaming is unavailable. Recording still works.');
    case 'CONNECTING':
      return copy('CONNECTING', 'Connecting…');
    case 'AUTHENTICATING':
      return copy('SIGNING IN', 'Signing in…');
    case 'CONNECTED':
      return copy('ON', 'Streaming');
    case 'DEGRADED':
      return copy('DEGRADED', 'Streaming (degraded)');
    case 'RECONNECTING':
      return copy('RECONNECTING', 'Reconnecting…', 'Recording continues. Engineers keep their place.');
    case 'STALE':
      return copy('STALLED', 'Link stalled');
    case 'FAILED':
      return copy('FAILED', 'Streaming stopped');
    case 'UNKNOWN':
      return copy('CHECKING', 'Checking…');
    default:
      return UNKNOWN_STATE;
  }
}

/** Recording states (`RecordingStatusDto.state`). */
export const RECORDING_STATES = ['off', 'active', 'suspended', 'failed', 'disabled', 'unknown'] as const;

/**
 * Recording state. `suspended` is the recorder's disk-space protection (crates/recorder writer.rs: free space
 * below the critical threshold; records are dropped and counted until space recovers).
 */
export function recordingStateLabel(state: string | null | undefined): StateCopy {
  switch (state) {
    case 'active':
      return copy('REC', 'Recording');
    case 'off':
      return copy('OFF', 'Not recording');
    case 'suspended':
      return copy(
        'SUSPENDED',
        'Recording suspended',
        'Disk space is critically low. Data is not saved until space is freed.',
      );
    case 'failed':
      return copy('FAILED', 'Recording failed');
    case 'disabled':
      return copy('OFF IN SETTINGS', 'Recording off in settings', 'Turn recording on in Settings.');
    case 'unknown':
      return copy('CHECKING', 'Checking…');
    default:
      return UNKNOWN_STATE;
  }
}

/** Engineer slot link quality (`EngineerSlotDto.link`). */
export const ENGINEER_LINKS = ['healthy', 'degraded', 'reconnecting'] as const;

export function engineerLinkLabel(link: string | null | undefined): StateCopy {
  switch (link) {
    case 'healthy':
      return copy('CONNECTED', 'Connected');
    case 'degraded':
      return copy('WEAK LINK', 'Weak link');
    case 'reconnecting':
      return copy('RECONNECTING', 'Reconnecting');
    default:
      return UNKNOWN_STATE;
  }
}

/** Engineer-side stream health (`EngineerStatusDto.stream_health`). */
export const STREAM_HEALTHS = ['no_data', 'healthy', 'stale'] as const;

/**
 * Engineer-side stream health. `healthy` reads LIVE only for LMU data; synthetic data reads SYNTHETIC and data
 * of unknown origin UNVERIFIED (OR-1). `ageText` (already formatted) is appended to STALE.
 */
export function streamHealthLabel(
  health: string | null | undefined,
  origin: Origin | null,
  ageText?: string | null,
): StateCopy {
  switch (health) {
    case 'healthy':
      if (origin === 'synthetic') return copy('SYNTHETIC', 'Synthetic data', 'Not live LMU telemetry.');
      if (origin === 'lmu') return copy('LIVE', 'Live');
      return copy('UNVERIFIED', 'Source unknown', 'The data source is not confirmed as LMU.');
    case 'stale': {
      const suffix = ageText !== undefined && ageText !== null && ageText !== '' ? ` ${ageText}` : '';
      return copy(`STALE${suffix}`, `Stale${suffix}`, 'No new data from the driver.');
    }
    case 'no_data':
      return copy('NO DATA', 'Waiting for data');
    default:
      return UNKNOWN_STATE;
  }
}

/** Engineer connection phases (`EngineerStatusDto.phase`). */
export const ENGINEER_PHASES = [
  'idle',
  'joining',
  'awaiting_confirmation',
  'monitoring',
  'ended',
  'not_started',
] as const;

export function engineerPhaseLabel(phase: string | null | undefined): StateCopy {
  switch (phase) {
    case 'idle':
    case 'not_started':
      return copy('NOT CONNECTED', COPY.notConnected, COPY.notConnectedHint);
    case 'joining':
      return copy('JOINING', 'Joining…');
    case 'awaiting_confirmation':
      return copy('CONFIRM DRIVER', 'Confirm the driver', 'Only confirm if this is the driver you expect.');
    case 'monitoring':
      return copy('WATCHING', 'Watching the driver');
    case 'ended':
      return copy('ENDED', 'Session ended');
    default:
      return UNKNOWN_STATE;
  }
}

// ---------------------------------------------------------------------------------------------------------
// Ended reasons
// ---------------------------------------------------------------------------------------------------------

/** Why an engineer session ended (crates/transport/src/state.rs `EngineerEndReason`). */
export type EndedReasonKind =
  | 'ticket_expired'
  | 'kicked'
  | 'driver_ended_session'
  | 'session_expired'
  | 'replaced'
  | 'user_disconnected'
  | 'relogin_required'
  | 'incompatible'
  | 'configuration'
  | 'environment_mismatch'
  | 'not_an_engineer'
  | 'unknown';

/**
 * `EngineerStatusDto.ended_reason` carries `EngineerEndReason::message()` (a sentence, not a code). These are
 * those exact sentences; the snake_case names are accepted too, should the DTO switch to codes.
 */
const ENDED_REASON_MESSAGES: Readonly<Record<string, EndedReasonKind>> = {
  'Connection ended. Enter a new code from the driver.': 'ticket_expired',
  'The driver disconnected you.': 'kicked',
  'The driver ended the session.': 'driver_ended_session',
  "The driver's session expired.": 'session_expired',
  'Your account connected to this driver from elsewhere.': 'replaced',
  'Disconnected.': 'user_disconnected',
  'Your login is no longer valid. Log in, then enter a new code from the driver.': 'relogin_required',
  'This app version is not compatible with the relay. Update the app, then enter a new code from the driver.':
    'incompatible',
  'Configuration error. Please check your network and TLS settings.': 'configuration',
  'The relay environment does not match this app version.': 'environment_mismatch',
  'This account is not authorized as an engineer.': 'not_an_engineer',
};

const ENDED_REASON_KINDS: readonly EndedReasonKind[] = [
  'ticket_expired',
  'kicked',
  'driver_ended_session',
  'session_expired',
  'replaced',
  'user_disconnected',
  'relogin_required',
  'incompatible',
  'configuration',
  'environment_mismatch',
  'not_an_engineer',
];

export function endedReasonKind(reason: string | null | undefined): EndedReasonKind {
  if (reason === null || reason === undefined) return 'unknown';
  const trimmed = reason.trim();
  const byMessage = ENDED_REASON_MESSAGES[trimmed];
  if (byMessage !== undefined) return byMessage;
  const asKind = ENDED_REASON_KINDS.find((kind) => kind === trimmed);
  return asKind ?? 'unknown';
}

const REJOIN_HINT = 'To watch again, ask the driver for a new code.';

/** Ended-session banner copy. `driverName` is used only once the identity was confirmed earlier. */
export function endedReasonLabel(reason: string | null | undefined, driverName?: string | null): StateCopy {
  const who = driverName !== undefined && driverName !== null && driverName.trim() !== '' ? driverName : 'the driver';
  switch (endedReasonKind(reason)) {
    case 'driver_ended_session':
      return copy('ENDED', `Session ended by ${who}`, REJOIN_HINT);
    case 'kicked':
      return copy('REMOVED', `Removed by ${who}`, REJOIN_HINT);
    case 'ticket_expired':
      return copy('DISCONNECTED', 'Connection lost and could not resume', REJOIN_HINT);
    case 'session_expired':
      return copy('ENDED', "The driver's session expired", REJOIN_HINT);
    case 'replaced':
      return copy('REPLACED', 'Your account joined this driver from another place', null);
    case 'user_disconnected':
      return copy('LEFT', 'You left the session', REJOIN_HINT);
    case 'relogin_required':
      return copy('SIGN IN', 'Your login expired', 'Sign in again, then enter a new code from the driver.');
    case 'incompatible':
      return copy('UPDATE', COPY.updateRequired, 'Update the app, then enter a new code from the driver.');
    case 'configuration':
      return copy('ERROR', 'Connection settings error', 'Check the network and TLS settings.');
    case 'environment_mismatch':
      return copy('ERROR', 'Wrong server environment', 'This app version cannot use this relay.');
    case 'not_an_engineer':
      return copy('NOT ALLOWED', 'This account is not an engineer account', null);
    case 'unknown':
      return copy('ENDED', 'Session ended', REJOIN_HINT);
  }
}

// ---------------------------------------------------------------------------------------------------------
// Demo, join errors, pipeline, schema
// ---------------------------------------------------------------------------------------------------------

const DEMO_SCENARIO_LABELS: Readonly<Record<DemoScenario, string>> = {
  demo: 'Demo mix',
  normal_lap: 'Normal lap',
  pit_lap: 'Pit stop lap',
  rain: 'Rain',
  hybrid: 'Hybrid',
  non_hybrid: 'Non-hybrid car',
  safety_car: 'Safety car',
};

export function demoScenarioLabel(scenario: DemoScenario): string {
  return DEMO_SCENARIO_LABELS[scenario];
}

/** Join error copy. `clearField`: the typed code cannot succeed again and should be cleared/selected. */
export interface JoinErrorCopy {
  readonly title: string;
  readonly helper: string;
  readonly clearField: boolean;
}

function joinError(title: string, helper: string, clearField: boolean): JoinErrorCopy {
  return { title, helper, clearField };
}

/** Maps an `IpcError.code` from `engineer_join`/`engineer_confirm` to sentence-case copy and a next step. */
export function joinErrorCopy(code: CommandErrorCode | (string & {}) | null | undefined): JoinErrorCopy {
  switch (code) {
    case 'code_malformed':
      return joinError('That is not an engineer code', 'An engineer code has 6 letters and digits.', true);
    case 'code_invalid':
      return joinError('That code is not valid', 'Check the code with the driver, or ask for a new one.', true);
    case 'code_expired':
      return joinError('That code has expired or was already used', 'Ask the driver for a new code.', true);
    case 'session_full':
      return joinError('The driver already has 2 engineers', 'The driver must remove an engineer first.', true);
    case 'session_closed':
      return joinError('The driver ended that session', 'Ask the driver for a new code.', true);
    case 'rate_limited':
      return joinError('Too many attempts', 'Wait a minute, then try again.', false);
    case 'role_mismatch':
    case 'not_an_engineer':
      return joinError('This account is not an engineer account', 'Sign in with an engineer account.', false);
    case 'relogin_required':
    case 'invalid_credentials':
    case 'auth_refused':
      return joinError('Your login expired', 'Sign in again, then enter the code.', false);
    case 'relay_not_configured':
      return joinError('No server is set up', 'Set the server address in Settings.', false);
    case 'relay_unreachable':
    case 'network_unavailable':
    case 'network_error':
    case 'connect_failed':
    case 'handshake_failed':
      return joinError('Cannot reach the server', 'Check your internet connection and try again.', false);
    case 'relay_incompatible':
      return joinError(COPY.updateRequired, 'This app version is not compatible with the server.', false);
    case 'environment_mismatch':
      return joinError('Wrong server environment', 'This app version cannot use this server.', false);
    case 'join_refused':
      return joinError('The join was refused', 'Ask the driver for a new code.', true);
    case 'join_busy':
    case 'engineer_already_running':
      return joinError('A join is already in progress', 'Wait for it to finish, or cancel it.', false);
    case 'join_cancelled':
      return joinError('The join was cancelled', 'Ask the driver for a new code to try again.', true);
    case 'backend_unavailable':
      return joinError('The app service is not running', 'Restart the app.', false);
    default:
      return joinError('Could not join the driver', 'Try again. If it keeps failing, ask for a new code.', false);
  }
}

/** Pipeline health states produced by `statusModel.pipelineHealth`. */
export type PipelineHealthState = 'healthy' | 'dropping' | 'dropping_sustained' | 'not_running' | 'unavailable';

export function pipelineHealthLabel(state: PipelineHealthState): StateCopy {
  switch (state) {
    case 'healthy':
      return copy('OK', 'No dropped data');
    case 'dropping':
      return copy('DROPS', 'Data dropped recently');
    case 'dropping_sustained':
      return copy('DROPPING', 'Data is being dropped');
    case 'not_running':
      return copy('NOT RUNNING', 'Not running');
    case 'unavailable':
      return copy('—', 'No status');
  }
}

/** Schema-incompatible copy (versions as reported by the store's schema-mismatch rejection, when known). */
export function incompatibleCopy(found: number | null, expected: number | null): StateCopy {
  const helper =
    found !== null && expected !== null
      ? `The driver's app sends data format v${found}; this app reads v${expected}.`
      : "The driver's app uses a different data format.";
  return copy('UPDATE REQUIRED', COPY.updateRequired, helper);
}
