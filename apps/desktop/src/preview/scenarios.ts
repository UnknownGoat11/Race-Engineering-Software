/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). The scenario catalogue: every screen and state the
 * preview can show, selected by `?screen=<group>&state=<state>` (or `?scenario=<id>`). Each scenario defines the
 * fake backend's answers (config, CoreStatus over time, telemetry plan, alerts, join behaviour) and optional UI
 * steps (clicks / typing) that bring the real screens into the wanted state. All data is SYNTHETIC.
 */
import type { AlertDto, ConfigSnapshot, CoreStatus, IdentityCardDto, LoginStateDto, Mode } from '../lib/ipc';
import { CAR_PARAMS, type CarVariant } from './synthetic/car';
import type { FieldKind } from './synthetic/field';
import { SESSION_TIME_AT_LOAD_S, STREAM_START_S, formatLap, raceTimeline } from './synthetic/race';
import { TRACK_NAME } from './synthetic/track';
import {
  ENGINEER_SLOTS,
  NO_DROPS,
  driverStatus,
  engineerBlock,
  engineerStatus,
  lmu,
  network,
} from './status';

export type ScenarioGroup = 'driver' | 'engineer' | 'mode' | 'settings' | 'app';

export type Step =
  | { readonly kind: 'click'; readonly selector: string }
  | { readonly kind: 'clickText'; readonly text: string }
  | { readonly kind: 'type'; readonly selector: string; readonly text: string }
  | { readonly kind: 'wait'; readonly ms: number };

export interface TelemetryPlan {
  readonly variant: CarVariant;
  readonly field: FieldKind;
  readonly playerPlace: number;
  /** Class position (defaults to `playerPlace`). */
  readonly classPlace?: number;
  /** 'live': keep streaming at 100 Hz. 'stop': history + one live batch, then nothing (the stream goes stale). */
  readonly flow: 'live' | 'stop';
}

/** Backend facts a status builder may depend on. */
export interface StatusContext {
  /** Seconds since the page loaded. */
  readonly elapsedS: number;
  /** Engineer join phase held by the fake backend (`idle` | `awaiting_confirmation` | `monitoring` | ...). */
  readonly phase: string;
}

export interface Scenario {
  readonly id: string;
  readonly group: ScenarioGroup;
  readonly state: string;
  readonly title: string;
  readonly description: string;
  readonly mode: Mode | null;
  readonly snapshot?: Partial<ConfigSnapshot>;
  /** `get_app_info` / `get_config` fail (the app's "Backend unavailable" screen). */
  readonly backendDown?: boolean;
  readonly status: (ctx: StatusContext) => CoreStatus;
  readonly telemetry?: TelemetryPlan;
  /** Emit scoring (and telemetry) only while the engineer phase is `monitoring` (engineer scenarios). */
  readonly gatedByPhase?: boolean;
  readonly initialPhase?: string;
  readonly alerts?: readonly AlertDto[];
  readonly login?: LoginStateDto;
  /** `engineer_join` rejects with this error instead of returning the identity card. */
  readonly joinError?: { readonly code: string; readonly message: string };
  readonly steps?: readonly Step[];
  /** Extra wait after the steps before the page reports ready (ms). */
  readonly settleMs?: number;
}

const NOW_UNIX_S = 1_792_312_000;
const SESSION_ID = 7_340_219;
const CODE = 'K7Q2XM';
const DRIVER_NAME = 'Sam Driver';
const DRIVER_USERNAME = 'sam_driver';

const GT3 = CAR_PARAMS.gt3;
const HYPER = CAR_PARAMS.hypercar;

function liveSession(variant: CarVariant): CoreStatus['session'] {
  const car = CAR_PARAMS[variant];
  return {
    session_id: SESSION_ID,
    session_type: 'RACE',
    track: TRACK_NAME,
    car: car.name,
    vehicle_class: car.vehicleClass,
    started_at_unix_s: NOW_UNIX_S - SESSION_TIME_AT_LOAD_S,
  };
}

function recordingActive(elapsedS: number): CoreStatus['recording'] {
  const frames = 241_380 + Math.round(elapsedS * 100);
  return {
    state: 'active',
    recording_id: '2026-10-18T14-20-11Z_sebring_race',
    bytes_written: frames * 512,
    frames_written: frames,
    frames_lost: 0,
    disk_free_bytes: 482 * 1024 ** 3,
    error: null,
  };
}

const LIVE_RATES: CoreStatus['rates'] = {
  telemetry_read_hz: 100,
  scoring_read_hz: 5,
  frames_sent_hz: 100,
  bytes_sent_per_s: 61_400,
  frames_dropped_total: 0,
};

const IDLE_RATES: CoreStatus['rates'] = { ...LIVE_RATES, frames_sent_hz: null, bytes_sent_per_s: null };

const LOGGED_IN_DRIVER: LoginStateDto = {
  authenticated: true,
  role: 'driver',
  account: { account: '6f1c2a9b8e7d4c3b2a1f0e9d8c7b6a59', username: DRIVER_USERNAME, display_name: DRIVER_NAME, role: 'driver' },
};
const LOGGED_IN_ENGINEER: LoginStateDto = {
  authenticated: true,
  role: 'engineer',
  account: { account: '0a1b2c3d4e5f60718293a4b5c6d7e8f9', username: 'pitwall_ana', display_name: 'Ana (Race Eng.)', role: 'engineer' },
};

function streamingNetwork(engineers: number, overrides: Partial<CoreStatus['network']> = {}): CoreStatus['network'] {
  return network({
    state: 'CONNECTED',
    streaming: true,
    rtt_ms: 38,
    connect_code: CODE,
    code_expires_at_unix_s: NOW_UNIX_S + 540,
    engineers_connected: engineers,
    engineers: ENGINEER_SLOTS.slice(0, engineers),
    ...overrides,
  });
}

function liveDriver(overrides: Partial<CoreStatus> = {}, elapsedS = 0, variant: CarVariant = 'gt3'): CoreStatus {
  return driverStatus({
    lmu: lmu('LIVE'),
    session: liveSession(variant),
    recording: recordingActive(elapsedS),
    network: network(),
    rates: IDLE_RATES,
    ...overrides,
  });
}

// --- driver ---------------------------------------------------------------------------------------------

const driverScenarios: Scenario[] = [
  {
    id: 'driver-waiting-lmu',
    group: 'driver',
    state: 'waiting-lmu',
    title: 'Driver: waiting for LMU',
    description: 'App started, Le Mans Ultimate not running; nothing recorded or streamed.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: () =>
      driverStatus({
        lmu: lmu('NOT_RUNNING', 'Waiting for Le Mans Ultimate (shared memory not found)', null),
        network: network({ state: 'DISCONNECTED' }),
      }),
  },
  {
    id: 'driver-live-recording',
    group: 'driver',
    state: 'live-recording',
    title: 'Driver: live + recording, not streaming',
    description: 'LMU live at 100 Hz, local recording active, relay not started.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) => liveDriver({}, elapsedS),
  },
  {
    id: 'driver-streaming-1',
    group: 'driver',
    state: 'streaming-1',
    title: 'Driver: streaming, code shown, 1/2 engineers',
    description: 'Streaming to the relay with a fresh connect code; one engineer connected.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) => liveDriver({ network: streamingNetwork(1), rates: LIVE_RATES }, elapsedS),
  },
  {
    id: 'driver-streaming-2',
    group: 'driver',
    state: 'streaming-2',
    title: 'Driver: streaming, 2/2 engineers (session full)',
    description: 'Both engineer slots taken.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) => liveDriver({ network: streamingNetwork(2), rates: LIVE_RATES }, elapsedS),
  },
  {
    id: 'driver-relay-reconnecting',
    group: 'driver',
    state: 'relay-reconnecting',
    title: 'Driver: relay reconnecting',
    description: 'Network blip: link reconnecting, engineers keep their slots; recording unaffected.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) =>
      liveDriver(
        {
          network: streamingNetwork(1, {
            state: 'RECONNECTING',
            rtt_ms: null,
            degraded_reason: 'relay connection lost; reconnecting (attempt 3)',
            last_error: 'connection reset by peer',
            engineers: ENGINEER_SLOTS.slice(0, 1).map((e) => ({ ...e, link: 'reconnecting' })),
          }),
          rates: { ...LIVE_RATES, frames_sent_hz: 0, bytes_sent_per_s: 0 },
          drops: { ...NO_DROPS, net_dropped_obsolete: 412 },
        },
        elapsedS,
      ),
  },
  {
    id: 'driver-lmu-stale',
    group: 'driver',
    state: 'lmu-stale',
    title: 'Driver: LMU telemetry stale',
    description: 'LMU stopped refreshing shared memory (paused / loading); stream still connected.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) =>
      liveDriver(
        {
          lmu: lmu('STALE', `No new LMU telemetry for ${(4.2 + elapsedS).toFixed(1)} s`),
          network: streamingNetwork(1),
          rates: { ...LIVE_RATES, telemetry_read_hz: 0, frames_sent_hz: 0 },
          recording: { ...recordingActive(0), state: 'suspended' },
        },
        elapsedS,
      ),
  },
  {
    id: 'driver-recording-failed',
    group: 'driver',
    state: 'recording-failed',
    title: 'Driver: recording failed (disk full)',
    description: 'Local recording stopped because the disk is full; live and streaming continue.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) =>
      liveDriver(
        {
          network: streamingNetwork(1),
          rates: LIVE_RATES,
          recording: {
            state: 'failed',
            recording_id: '2026-10-18T14-20-11Z_sebring_race',
            bytes_written: 1_934_221_312,
            frames_written: 3_777_776,
            frames_lost: 1_208,
            disk_free_bytes: 212 * 1024 ** 2,
            error: 'Disk full: 212 MiB free on D:\\ (at least 1 GiB required). Recording stopped.',
          },
          drops: { ...NO_DROPS, recorder_overrun: 1_208 },
        },
        elapsedS,
      ),
  },
  {
    id: 'driver-relogin',
    group: 'driver',
    state: 'relogin',
    title: 'Driver: relogin required',
    description: 'The relay refused the refresh token; streaming stopped until the driver logs in again.',
    mode: 'driver',
    login: { authenticated: false, role: 'driver', account: null },
    status: ({ elapsedS }) =>
      liveDriver(
        {
          network: network({
            state: 'FAILED',
            failure: 'Login expired. Log in again to stream.',
            relogin_required: true,
            last_error: 'auth_refused: refresh token expired',
          }),
        },
        elapsedS,
      ),
  },
  {
    id: 'driver-pipeline-degraded',
    group: 'driver',
    state: 'pipeline-degraded',
    title: 'Driver: pipeline degraded (overruns)',
    description: 'Reader / pipeline overruns counted (CPU pressure); everything else live.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) =>
      liveDriver(
        {
          network: streamingNetwork(1),
          rates: { ...LIVE_RATES, telemetry_read_hz: 93 },
          drops: { ...NO_DROPS, reader_overrun: 37, pipeline_overrun: 12, net_dropped_obsolete: 88, ui_dropped: 4 },
        },
        elapsedS,
      ),
  },
  {
    id: 'driver-demo',
    group: 'driver',
    state: 'demo',
    title: 'Driver: synthetic demo running',
    description: 'Display-only demo (never recorded or streamed): no relay applies.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: () =>
      driverStatus({
        lmu: lmu('LIVE', 'Synthetic demo scenario: demo', null),
        session: { ...liveSession('gt3'), session_type: 'PRACTICE' },
        recording: { ...driverStatus().recording, state: 'disabled' },
        network: network({ state: 'NOT_CONFIGURED' }),
        rates: { ...IDLE_RATES, telemetry_read_hz: 100 },
      }),
  },
  {
    id: 'driver-telemetry-tab',
    group: 'driver',
    state: 'telemetry-tab',
    title: 'Driver: telemetry dashboard tab',
    description: 'Streaming state with the optional Telemetry Dashboard tab selected.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) => liveDriver({ network: streamingNetwork(1), rates: LIVE_RATES }, elapsedS),
    steps: [{ kind: 'clickText', text: 'Telemetry Dashboard' }],
  },
];

// --- engineer -------------------------------------------------------------------------------------------

function engineerAlerts(variant: CarVariant): readonly AlertDto[] {
  const race = raceTimeline(variant);
  const pbLap = race.startLap - 3;
  return [
    {
      id: 'brake_temp_fl',
      name: 'Brake temperature',
      level: 'warning',
      raised: true,
      message: 'Front-left brake peak above 650 °C for 3 consecutive braking zones.',
      timestamp_ns: (SESSION_TIME_AT_LOAD_S - 5) * 1e9,
    },
    {
      id: 'personal_best',
      name: 'Personal best',
      level: 'info',
      raised: true,
      message: `Lap ${pbLap}: new personal best ${formatLap(race.bestLapS)}.`,
      timestamp_ns: Math.round(STREAM_START_S - 3 * race.lapTimeS) * 1e9,
    },
  ];
}

function engineerLive(
  variant: CarVariant,
  ctx: StatusContext,
  block: Partial<CoreStatus['engineer'] & object> = {},
  overrides: Partial<CoreStatus> = {},
): CoreStatus {
  const monitoring = ctx.phase === 'monitoring';
  return engineerStatus({
    lmu: monitoring ? lmu('LIVE') : lmu('UNKNOWN', null, null),
    session: monitoring ? liveSession(variant) : engineerStatus().session,
    recording: monitoring ? { ...recordingActive(ctx.elapsedS), state: 'active' } : engineerStatus().recording,
    network: network({ state: monitoring ? 'CONNECTED' : 'DISCONNECTED', rtt_ms: monitoring ? 34 : null, engineers_max: 2 }),
    rates: monitoring ? { ...LIVE_RATES, telemetry_read_hz: null, scoring_read_hz: null } : engineerStatus().rates,
    engineer: engineerBlock({
      phase: ctx.phase,
      driver_name: ctx.phase === 'idle' ? null : DRIVER_NAME,
      stream_health: monitoring ? 'healthy' : 'no_data',
      stream_age_s: monitoring ? 0.04 + 0.03 * Math.random() : null,
      ...block,
    }),
    ...overrides,
  });
}

function engineerScenario(
  partial: Omit<Scenario, 'group' | 'mode' | 'status' | 'gatedByPhase' | 'login'> & {
    readonly status?: Scenario['status'];
  },
  variant: CarVariant = 'gt3',
): Scenario {
  return {
    group: 'engineer',
    mode: 'engineer',
    gatedByPhase: true,
    login: LOGGED_IN_ENGINEER,
    telemetry: { variant, field: variant === 'gt3' ? 'gt3' : 'mixed', playerPlace: variant === 'gt3' ? 7 : 4, flow: 'live' },
    alerts: engineerAlerts(variant),
    status: (ctx) => engineerLive(variant, ctx),
    ...partial,
  };
}

const LIVE_SETTLE_MS = 2500;

const engineerScenarios: Scenario[] = [
  engineerScenario({
    id: 'engineer-idle',
    state: 'idle',
    title: 'Engineer: not joined',
    description: 'Engineer mode before joining a driver: no stream, empty pitwall.',
    initialPhase: 'idle',
    alerts: [],
  }),
  engineerScenario({
    id: 'engineer-join-modal',
    state: 'join-modal',
    title: 'Engineer: join modal (code entry)',
    description: 'Join dialog open, waiting for the 6-character code.',
    initialPhase: 'idle',
    alerts: [],
    steps: [{ kind: 'click', selector: '[data-testid="btn-open-join"]' }],
  }),
  engineerScenario({
    id: 'engineer-join-code-typed',
    state: 'join-code-typed',
    title: 'Engineer: join modal with code typed',
    description: 'Code entered, Join button enabled.',
    initialPhase: 'idle',
    alerts: [],
    steps: [
      { kind: 'click', selector: '[data-testid="btn-open-join"]' },
      { kind: 'type', selector: '[data-testid="join-code-input"]', text: CODE },
    ],
  }),
  engineerScenario({
    id: 'engineer-join-error',
    state: 'join-error',
    title: 'Engineer: join refused (expired code)',
    description: 'The relay rejected the code; error shown in the dialog.',
    initialPhase: 'idle',
    alerts: [],
    joinError: { code: 'code_expired', message: 'This connection code has expired. Ask the driver for a new code.' },
    steps: [
      { kind: 'click', selector: '[data-testid="btn-open-join"]' },
      { kind: 'type', selector: '[data-testid="join-code-input"]', text: 'H3WZ9P' },
      { kind: 'click', selector: '[data-testid="btn-join-session"]' },
      { kind: 'wait', ms: 300 },
    ],
  }),
  engineerScenario({
    id: 'engineer-identity-card',
    state: 'identity-card',
    title: 'Engineer: identity card confirm',
    description: 'Code redeemed; the driver identity card waits for confirmation (D-022).',
    initialPhase: 'idle',
    alerts: [],
    steps: [
      { kind: 'click', selector: '[data-testid="btn-open-join"]' },
      { kind: 'type', selector: '[data-testid="join-code-input"]', text: CODE },
      { kind: 'click', selector: '[data-testid="btn-join-session"]' },
      { kind: 'wait', ms: 300 },
    ],
  }),
  engineerScenario({
    id: 'engineer-live',
    state: 'live',
    title: 'Engineer: live pitwall (GT3 race, 25 cars)',
    description: 'Monitoring a GT3 driver in a 25-car race; timing & fuel panels.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
  }),
  engineerScenario({
    id: 'engineer-live-tyres',
    state: 'live-tyres',
    title: 'Engineer: live pitwall, tyres & brakes',
    description: 'Right column on Tyres & Brakes; graph on the Tyres preset.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    steps: [
      { kind: 'clickText', text: 'Tyres & Brakes' },
      { kind: 'clickText', text: 'Tyres' },
    ],
  }),
  engineerScenario({
    id: 'engineer-live-inputs',
    state: 'live-inputs',
    title: 'Engineer: live pitwall, inputs & energy (non-hybrid)',
    description: 'Right column on Inputs & Energy for a GT3 (no ERS / virtual energy).',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    steps: [{ kind: 'clickText', text: 'Inputs & Energy' }],
  }),
  engineerScenario({
    id: 'engineer-live-channels',
    state: 'live-channels',
    title: 'Engineer: channel selector open',
    description: 'MoTeC-style channel selector dialog over the live pitwall.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    steps: [{ kind: 'click', selector: '[data-testid="open-channel-selector-btn"]' }],
  }),
  engineerScenario({
    id: 'engineer-driver-away',
    state: 'driver-away',
    title: 'Engineer: driver away',
    description: 'Driver left the car / LMU; the relay link is up but the driver reports away.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    telemetry: { variant: 'gt3', field: 'gt3', playerPlace: 7, flow: 'stop' },
    status: (ctx) =>
      engineerLive(
        'gt3',
        ctx,
        { driver_away: true, stream_health: 'stale', stream_age_s: 12.4 + ctx.elapsedS },
        { lmu: lmu('NOT_RUNNING', 'Driver reports LMU closed', null) },
      ),
  }),
  engineerScenario({
    id: 'engineer-stream-stale',
    state: 'stream-stale',
    title: 'Engineer: stream stale',
    description: 'No telemetry received for several seconds (relay or driver network problem).',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    telemetry: { variant: 'gt3', field: 'gt3', playerPlace: 7, flow: 'stop' },
    status: (ctx) =>
      engineerLive(
        'gt3',
        ctx,
        { stream_health: 'stale', stream_age_s: 3.2 + ctx.elapsedS },
        { network: network({ state: 'STALE', rtt_ms: null, degraded_reason: 'no frames from the driver for 3 s' }) },
      ),
  }),
  engineerScenario(
    {
      id: 'engineer-hybrid',
      state: 'hybrid',
      title: 'Engineer: hybrid Hypercar, inputs & energy',
      description: 'Monitoring a Hypercar in a mixed-class field; virtual energy and hybrid panel populated.',
      initialPhase: 'monitoring',
      settleMs: LIVE_SETTLE_MS,
      steps: [{ kind: 'clickText', text: 'Inputs & Energy' }],
    },
    'hypercar',
  ),
  engineerScenario(
    {
      id: 'engineer-hybrid-overview',
      state: 'hybrid-overview',
      title: 'Engineer: hybrid Hypercar, timing & fuel',
      description: 'Mixed-class field standings (class filter) with a Hypercar player.',
      initialPhase: 'monitoring',
      settleMs: LIVE_SETTLE_MS,
    },
    'hypercar',
  ),
  engineerScenario({
    id: 'engineer-incompatible',
    state: 'incompatible',
    title: 'Engineer: incompatible schema',
    description: "The driver's app sends a newer telemetry schema.",
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    status: (ctx) => engineerLive('gt3', ctx, { incompatible_schema: true }),
  }),
  engineerScenario({
    id: 'engineer-ended',
    state: 'ended',
    title: 'Engineer: session ended by driver',
    description: 'The driver ended the session; nothing streams any more.',
    initialPhase: 'ended',
    alerts: [],
    telemetry: { variant: 'gt3', field: 'gt3', playerPlace: 7, flow: 'stop' },
    status: (ctx) =>
      engineerLive('gt3', ctx, {
        driver_name: DRIVER_NAME,
        ended_reason: 'driver_ended_session',
        stream_health: 'no_data',
      }),
  }),
];

// --- mode select, settings, app states ------------------------------------------------------------------

const appScenarios: Scenario[] = [
  {
    id: 'mode-select',
    group: 'mode',
    state: 'first-launch',
    title: 'Mode select (first launch)',
    description: 'No mode saved yet: Driver or Engineer must be chosen.',
    mode: null,
    status: () => driverStatus(),
  },
  {
    id: 'settings-driver',
    group: 'settings',
    state: 'driver',
    title: 'Settings (driver mode)',
    description: 'Settings page while in driver mode.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) => liveDriver({}, elapsedS),
    steps: [{ kind: 'clickText', text: 'Settings' }],
  },
  {
    id: 'settings-engineer',
    group: 'settings',
    state: 'engineer',
    title: 'Settings (engineer mode)',
    description: 'Settings page while in engineer mode, mode change pending.',
    mode: 'engineer',
    login: LOGGED_IN_ENGINEER,
    status: (ctx) => engineerLive('gt3', ctx),
    initialPhase: 'idle',
    steps: [
      { kind: 'clickText', text: 'Settings' },
      { kind: 'clickText', text: 'DRIVER MODE' },
    ],
  },
  {
    id: 'settings-config-issue',
    group: 'settings',
    state: 'config-issue',
    title: 'Settings with config issue banner',
    description: 'Stored config was invalid: safe defaults in use, banner explains and names the preserved file.',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    snapshot: {
      issue: {
        kind: 'invalid',
        message: 'The configuration file failed validation.',
        errors: [
          { field: 'fuel.window_laps', message: 'must be one of 1, 2, 3, 5, 10 (found 4)' },
          { field: 'network.relay_port', message: 'must be between 1 and 65535 (found 0)' },
        ],
        preserved_as: 'D:\\RaceEngineeringData\\preview\\config.invalid-20261018T142011Z.json',
      },
      restart_required: true,
    },
    status: ({ elapsedS }) => liveDriver({}, elapsedS),
    steps: [{ kind: 'clickText', text: 'Settings' }],
  },
  {
    id: 'app-backend-unavailable',
    group: 'app',
    state: 'backend-unavailable',
    title: 'App: backend unavailable',
    description: 'The initial get_app_info / get_config failed (fatal start-up error screen).',
    mode: null,
    backendDown: true,
    status: () => driverStatus(),
  },
];

export const SCENARIOS: readonly Scenario[] = [...driverScenarios, ...engineerScenarios, ...appScenarios];

/** Scenario for the current URL (`?scenario=<id>` or `?screen=<group>&state=<state>`), or null for the index. */
export function scenarioFromQuery(search: string): Scenario | null {
  const q = new URLSearchParams(search);
  const id = q.get('scenario');
  if (id !== null && id !== '') return SCENARIOS.find((s) => s.id === id) ?? null;
  const screen = q.get('screen');
  if (screen === null) return null;
  const state = q.get('state');
  const inGroup = SCENARIOS.filter((s) => s.group === screen);
  if (state === null || state === '') return inGroup[0] ?? null;
  return inGroup.find((s) => s.state === state) ?? null;
}

/** The identity card the fake relay returns for a scenario. */
export function identityCardFor(scenario: Scenario): IdentityCardDto {
  const variant = scenario.telemetry?.variant ?? 'gt3';
  const car = variant === 'gt3' ? GT3 : HYPER;
  return {
    session_id: SESSION_ID,
    driver_display_name: DRIVER_NAME,
    driver_username: DRIVER_USERNAME,
    car: car.name,
    vehicle_class: car.vehicleClass,
    track: TRACK_NAME,
    session_type: 'RACE',
    started_at_ns: (NOW_UNIX_S - SESSION_TIME_AT_LOAD_S) * 1e9,
    schema_version: 1,
  };
}
