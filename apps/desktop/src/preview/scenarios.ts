/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). The scenario catalogue: every screen and state the
 * preview can show, selected by `?screen=<group>&state=<state>` (or `?scenario=<id>`). Each scenario defines the
 * fake backend's answers (config, CoreStatus over time, telemetry plan, alerts, join behaviour) and optional UI
 * steps (clicks / typing) that bring the real screens into the wanted state. All data is SYNTHETIC; a scenario
 * with `origin: 'lmu'` only simulates the provenance the app sees for real LMU data (the page still carries the
 * PREVIEW tag, see main.tsx).
 */
import type { AlertDto, ConfigSnapshot, CoreStatus, IdentityCardDto, LoginStateDto, Mode, Origin } from '../lib/ipc';
import { CAR_PARAMS, noise, type CarVariant } from './synthetic/car';
import type { FieldKind } from './synthetic/field';
import { formatLap, raceTimeline, streamTimeline, type StreamTimeline, type TimelineKind } from './synthetic/race';
import { TRACK_NAME } from './synthetic/track';
import {
  ENGINEER_SLOTS,
  NO_DROPS,
  PREVIEW_NOW_UNIX_S,
  driverStatus,
  engineerBlock,
  engineerStatus,
  lmu,
  network,
  previewConfig,
} from './status';

export type ScenarioGroup = 'driver' | 'engineer' | 'mode' | 'settings' | 'app';

export type Step =
  | { readonly kind: 'click'; readonly selector: string }
  /**
   * Clicks the enabled button / link / tab whose text is exactly `text`. `optional`: skip (after a short wait)
   * when it is not on the page, e.g. a right-column tab that only exists in the compact (< 1800 px) layout.
   */
  | { readonly kind: 'clickText'; readonly text: string; readonly optional?: boolean }
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
  /** Part of the session the stream shows (default `final-stint` of the 4 h race). */
  readonly timeline?: TimelineKind;
  /** Seconds of history the subscription backfills (default: the timeline's). */
  readonly backfillS?: number;
  /** Non-hybrid car that reports virtual energy (LMGT3 runs VE without ERS). */
  readonly virtualEnergy?: boolean;
  /** Full-course yellow just declared. */
  readonly fcy?: boolean;
}

/** Backend facts a status builder may depend on. */
export interface StatusContext {
  /** Seconds since the page loaded. */
  readonly elapsedS: number;
  /** Engineer join phase held by the fake backend (`idle` | `awaiting_confirmation` | `monitoring` | ...). */
  readonly phase: string;
  /** `engineer_join` calls so far. */
  readonly joinAttempts: number;
}

export interface Scenario {
  readonly id: string;
  readonly group: ScenarioGroup;
  readonly state: string;
  readonly title: string;
  readonly description: string;
  readonly mode: Mode | null;
  /**
   * Provenance the fake backend reports in every CoreStatus and subscription (default `'synthetic'`). `'lmu'`
   * simulates the LMU-origin (live-looking) path with synthetic data; the index and the page tag say so.
   */
  readonly origin?: Origin;
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
  /** Only the first N `engineer_join` calls fail with `joinError` (default: every call). */
  readonly joinErrorAttempts?: number;
  readonly steps?: readonly Step[];
  /** Extra wait after the steps before the page reports ready (ms). */
  readonly settleMs?: number;
  /**
   * Scenario time (s, virtual) at which the backend freezes for the screenshot. Default: settle + 1 s per step
   * + 2 s, which is always later than the real time the steps take, so every run captures the same frame.
   */
  readonly captureAtS?: number;
}

/** Scenario time (s) of the screenshot frame. */
export function captureTimeS(scenario: Scenario): number {
  if (scenario.captureAtS !== undefined) return scenario.captureAtS;
  return 2 + (scenario.settleMs ?? 800) / 1000 + (scenario.steps?.length ?? 0);
}

/** Text of the fixed corner tag: always PREVIEW, never mistakable for real LMU data. */
export function previewTagText(scenario: Scenario): string {
  return scenario.origin === 'lmu'
    ? `PREVIEW HARNESS · SIMULATED LMU ORIGIN · NOT REAL DATA · ${scenario.id}`
    : `PREVIEW HARNESS · SYNTHETIC DATA · ${scenario.id}`;
}

const timelines = new Map<string, StreamTimeline>();

/** Stream timeline of a scenario (the shared final stint unless its telemetry plan says otherwise). */
export function scenarioTimeline(scenario: Scenario): StreamTimeline {
  const plan = scenario.telemetry;
  const key = `${plan?.variant ?? 'gt3'}|${plan?.timeline ?? 'final-stint'}|${String(plan?.backfillS)}`;
  let t = timelines.get(key);
  if (t === undefined) {
    t = streamTimeline(plan?.variant ?? 'gt3', plan?.timeline ?? 'final-stint', plan?.backfillS);
    timelines.set(key, t);
  }
  return t;
}

const NOW_UNIX_S = PREVIEW_NOW_UNIX_S;
const SESSION_ID = 7_340_219;
const CODE = 'K7Q2XM';
const DRIVER_NAME = 'Sam Driver';
const DRIVER_USERNAME = 'sam_driver';

const GT3 = CAR_PARAMS.gt3;
const HYPER = CAR_PARAMS.hypercar;

function liveSession(variant: CarVariant, timeline: StreamTimeline = streamTimeline(variant)): CoreStatus['session'] {
  const car = CAR_PARAMS[variant];
  return {
    session_id: SESSION_ID,
    session_type: timeline.sessionType,
    track: TRACK_NAME,
    car: car.name,
    vehicle_class: car.vehicleClass,
    started_at_unix_s: NOW_UNIX_S - timeline.sessionTimeAtLoadS,
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
    disk_free_bytes: DISK_FREE_OK_BYTES,
    error: null,
  };
}

const DISK_FREE_OK_BYTES = 482 * 1024 ** 3;

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
    origin: 'lmu',
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
    origin: 'lmu',
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
    origin: 'lmu',
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
    origin: 'lmu',
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
    origin: 'lmu',
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
    origin: 'lmu',
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
    origin: 'lmu',
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
    origin: 'lmu',
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
    origin: 'lmu',
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
        session: { ...liveSession('gt3'), session_type: 'Practice 1' },
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
    origin: 'lmu',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) => liveDriver({ network: streamingNetwork(1), rates: LIVE_RATES }, elapsedS),
    steps: [{ kind: 'click', selector: '[data-testid="tab-telemetry"]' }],
  },
];

// --- engineer -------------------------------------------------------------------------------------------

function engineerAlerts(variant: CarVariant): readonly AlertDto[] {
  const race = raceTimeline(variant);
  const timeline = streamTimeline(variant);
  const pbLap = race.startLap - 3;
  return [
    {
      id: 'brake_temp_fl',
      name: 'Brake temperature',
      level: 'warning',
      raised: true,
      message: 'Front-left brake peak above 650 °C for 3 consecutive braking zones.',
      timestamp_ns: (timeline.sessionTimeAtLoadS - 5) * 1e9,
    },
    {
      id: 'personal_best',
      name: 'Personal best',
      level: 'info',
      raised: true,
      message: `Lap ${pbLap}: new personal best ${formatLap(race.bestLapS)}.`,
      timestamp_ns: Math.round(timeline.streamStartS - 3 * race.lapTimeS) * 1e9,
    },
  ];
}

function engineerLive(
  variant: CarVariant,
  ctx: StatusContext,
  block: Partial<CoreStatus['engineer'] & object> = {},
  overrides: Partial<CoreStatus> = {},
  timeline: StreamTimeline = streamTimeline(variant),
): CoreStatus {
  const monitoring = ctx.phase === 'monitoring';
  return engineerStatus({
    lmu: monitoring ? lmu('LIVE') : lmu('UNKNOWN', null, null),
    session: monitoring ? liveSession(variant, timeline) : engineerStatus().session,
    recording: monitoring ? { ...recordingActive(ctx.elapsedS), state: 'active' } : engineerStatus().recording,
    network: network({ state: monitoring ? 'CONNECTED' : 'DISCONNECTED', rtt_ms: monitoring ? 34 : null, engineers_max: 2 }),
    rates: monitoring ? { ...LIVE_RATES, telemetry_read_hz: null, scoring_read_hz: null } : engineerStatus().rates,
    engineer: engineerBlock({
      phase: ctx.phase,
      driver_name: ctx.phase === 'idle' ? null : DRIVER_NAME,
      stream_health: monitoring ? 'healthy' : 'no_data',
      // Deterministic jitter (the capture diff needs identical frames).
      stream_age_s: monitoring ? 0.04 + 0.03 * (noise(31, Math.round(ctx.elapsedS * 4)) + 0.5) : null,
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
      { kind: 'clickText', text: 'Tyres+Brakes', optional: true },
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
    steps: [{ kind: 'clickText', text: 'Power', optional: true }],
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
      steps: [{ kind: 'clickText', text: 'Power', optional: true }],
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
        // The core reports EngineerEndReason::message() (crates/transport/src/state.rs).
        ended_reason: 'The driver ended the session.',
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
      { kind: 'click', selector: '[data-testid="settings-mode-driver"]' },
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

// --- W0.1 scenarios (REDESIGN H02): states later waves render and test ------------------------------------

const NOT_LOGGED_IN_DRIVER: LoginStateDto = { authenticated: false, role: 'driver', account: null };
const NOT_LOGGED_IN_ENGINEER: LoginStateDto = { authenticated: false, role: 'engineer', account: null };
/** Disk free below the 20 GB warning level (but above the 1 GiB recording stop). */
const DISK_FREE_LOW_BYTES = Math.round(14.2 * 1024 ** 3);

const driverW0Scenarios: Scenario[] = [
  {
    id: 'driver-not-logged-in',
    group: 'driver',
    state: 'not-logged-in',
    title: 'Driver: not logged in',
    description: 'No stored login: LMU live and recording locally; streaming needs a login first.',
    mode: 'driver',
    origin: 'lmu',
    login: NOT_LOGGED_IN_DRIVER,
    status: ({ elapsedS }) => liveDriver({ network: network({ state: 'DISCONNECTED' }) }, elapsedS),
  },
  {
    id: 'driver-live-lmu-origin',
    group: 'driver',
    state: 'live-lmu-origin',
    title: 'Driver: live LMU origin, streaming 1/2',
    description: 'Simulated LMU origin (origin = lmu): the live look the app shows for real LMU data.',
    mode: 'driver',
    origin: 'lmu',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) => liveDriver({ network: streamingNetwork(1), rates: LIVE_RATES }, elapsedS),
  },
  {
    id: 'driver-streaming-full',
    group: 'driver',
    state: 'streaming-full',
    title: 'Driver: streaming, 2/2, no code left',
    description: 'Both slots taken and the last single-use code consumed: no new code can be issued (OR-4).',
    mode: 'driver',
    origin: 'lmu',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) =>
      liveDriver(
        { network: streamingNetwork(2, { connect_code: null, code_expires_at_unix_s: null }), rates: LIVE_RATES },
        elapsedS,
      ),
  },
  {
    id: 'driver-code-expiring',
    group: 'driver',
    state: 'code-expiring',
    title: 'Driver: connect code expiring (< 60 s)',
    description: 'Streaming, no engineer yet; the code expires in under a minute.',
    mode: 'driver',
    origin: 'lmu',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) =>
      liveDriver(
        {
          network: streamingNetwork(0, { code_expires_at_unix_s: NOW_UNIX_S + 50 }),
          rates: LIVE_RATES,
        },
        elapsedS,
      ),
  },
  {
    id: 'driver-reconnecting-long',
    group: 'driver',
    state: 'reconnecting-long',
    title: 'Driver: relay reconnecting for over 30 s',
    description: 'Long outage: reconnecting for more than 30 s; recording unaffected, engineers held.',
    mode: 'driver',
    origin: 'lmu',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) =>
      liveDriver(
        {
          network: streamingNetwork(1, {
            state: 'RECONNECTING',
            rtt_ms: null,
            degraded_reason: `relay connection lost ${(34 + elapsedS).toFixed(0)} s ago; reconnecting (attempt 9)`,
            last_error: 'connection timed out',
            engineers: ENGINEER_SLOTS.slice(0, 1).map((e) => ({ ...e, link: 'reconnecting' })),
          }),
          rates: { ...LIVE_RATES, frames_sent_hz: 0, bytes_sent_per_s: 0 },
          drops: { ...NO_DROPS, net_dropped_obsolete: 3_412 },
        },
        elapsedS,
      ),
  },
  {
    id: 'driver-disk-low',
    group: 'driver',
    state: 'disk-low',
    title: 'Driver: disk space low (< 20 GB)',
    description: 'Recording active with 14.2 GiB free on the recording drive.',
    mode: 'driver',
    origin: 'lmu',
    login: LOGGED_IN_DRIVER,
    status: ({ elapsedS }) =>
      liveDriver(
        {
          network: streamingNetwork(1),
          rates: LIVE_RATES,
          recording: { ...recordingActive(elapsedS), disk_free_bytes: DISK_FREE_LOW_BYTES },
        },
        elapsedS,
      ),
  },
  {
    id: 'driver-dashboard-live',
    group: 'driver',
    state: 'dashboard-live',
    title: 'Driver: telemetry dashboard with live store',
    description: 'Dashboard tab with the telemetry stream flowing (bridge subscription fed at 100 Hz).',
    mode: 'driver',
    origin: 'lmu',
    login: LOGGED_IN_DRIVER,
    telemetry: { variant: 'gt3', field: 'gt3', playerPlace: 7, flow: 'live' },
    status: ({ elapsedS }) => liveDriver({ network: streamingNetwork(1), rates: LIVE_RATES }, elapsedS),
    steps: [{ kind: 'click', selector: '[data-testid="tab-telemetry"]' }],
    settleMs: LIVE_SETTLE_MS,
  },
];

const QUALI_TIMELINE = streamTimeline('gt3', 'qualifying');
const START_TIMELINE = streamTimeline('gt3', 'race-start');
/** Race start with just over one lap of history: enough for a lap-distance track outline. */
const GEOMETRY_BACKFILL_S = Math.ceil(raceTimeline('gt3').lapTimeS + 20);
const GEOMETRY_TIMELINE = streamTimeline('gt3', 'race-start', GEOMETRY_BACKFILL_S);

const joinSteps = (code: string): Step[] => [
  { kind: 'click', selector: '[data-testid="btn-open-join"]' },
  { kind: 'type', selector: '[data-testid="join-code-input"]', text: code },
  { kind: 'click', selector: '[data-testid="btn-join-session"]' },
  { kind: 'wait', ms: 300 },
];

const engineerW0Scenarios: Scenario[] = [
  {
    ...engineerScenario({
      id: 'engineer-not-logged-in',
      state: 'not-logged-in',
      title: 'Engineer: not logged in',
      description: 'Engineer mode without a stored login: joining needs a login first.',
      initialPhase: 'idle',
      alerts: [],
    }),
    login: NOT_LOGGED_IN_ENGINEER,
  },
  engineerScenario({
    id: 'engineer-live-lmu-origin',
    state: 'live-lmu-origin',
    title: 'Engineer: live pitwall, LMU origin',
    description: 'Simulated LMU origin (origin = lmu): the live look the app shows for real LMU data.',
    origin: 'lmu',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
  }),
  engineerScenario({
    id: 'engineer-kicked',
    state: 'kicked',
    title: 'Engineer: disconnected by the driver',
    description: 'The driver removed this engineer; a fresh code is needed to connect again (OR-3).',
    initialPhase: 'ended',
    alerts: [],
    telemetry: { variant: 'gt3', field: 'gt3', playerPlace: 7, flow: 'stop' },
    status: (ctx) =>
      engineerLive('gt3', ctx, {
        driver_name: DRIVER_NAME,
        ended_reason: 'The driver disconnected you.',
        stream_health: 'no_data',
      }),
  }),
  engineerScenario({
    id: 'engineer-join-session-full',
    state: 'join-session-full',
    title: 'Engineer: join refused (session full)',
    description: 'The driver already has 2 engineers connected; the relay refuses a third (OR-4).',
    initialPhase: 'idle',
    alerts: [],
    joinError: { code: 'session_full', message: 'Driver already has 2 engineers connected' },
    steps: joinSteps('P4HN7W'),
  }),
  engineerScenario({
    id: 'engineer-join-expired-retry',
    state: 'join-expired-retry',
    title: 'Engineer: expired code (a fresh code would be accepted)',
    description:
      'The redeem fails with code_expired and the dialog shows the expired-code error; only the first attempt ' +
      'fails, so the fresh code from the driver is accepted on retry (the identity card is engineer-identity-card).',
    initialPhase: 'idle',
    alerts: [],
    joinError: { code: 'code_expired', message: 'This code has expired. Ask the driver for a new code.' },
    joinErrorAttempts: 1,
    // The capture is the failed redeem: typing the fresh code clears the error, and the successful retry looks
    // exactly like engineer-identity-card (audit H02).
    steps: joinSteps('H3WZ9P'),
  }),
  engineerScenario({
    id: 'engineer-quali',
    state: 'quali',
    title: 'Engineer: qualifying session',
    description: 'GT3 qualifying, mid-session: order by best lap, best-lap gaps, no fuel-to-finish.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    alerts: [],
    telemetry: { variant: 'gt3', field: 'gt3', playerPlace: 5, flow: 'live', timeline: 'qualifying' },
    status: (ctx) => engineerLive('gt3', ctx, {}, {}, QUALI_TIMELINE),
  }),
  engineerScenario({
    id: 'engineer-class-filter',
    state: 'class-filter',
    title: 'Engineer: mixed field filtered to GT3',
    description: 'GT3 player (P18, class P5) in a mixed field; the tower filtered to the GT3 class.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    telemetry: { variant: 'gt3', field: 'mixed', playerPlace: 18, classPlace: 5, flow: 'live' },
    steps: [{ kind: 'clickText', text: 'GT3' }],
  }),
  engineerScenario({
    id: 'engineer-fcy',
    state: 'fcy',
    title: 'Engineer: full-course yellow declared',
    description: 'FCY just declared (yellow state pending): flags in telemetry and scoring.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    telemetry: { variant: 'gt3', field: 'gt3', playerPlace: 7, flow: 'live', fcy: true },
  }),
  engineerScenario({
    id: 'engineer-gt3-ve',
    state: 'gt3-ve',
    title: 'Engineer: GT3 with virtual energy (non-hybrid)',
    description: 'LMGT3 reports virtual energy but has no hybrid system: VE populated, ERS channels absent.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    telemetry: { variant: 'gt3', field: 'gt3', playerPlace: 7, flow: 'live', virtualEnergy: true },
    steps: [{ kind: 'clickText', text: 'Power', optional: true }],
  }),
  engineerScenario({
    id: 'engineer-first-lap-no-geometry',
    state: 'first-lap-no-geometry',
    title: 'Engineer: first lap, no track geometry yet',
    description: 'Joined at the race start: under one lap of history, no completed lap, no lap-based estimates.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    alerts: [],
    telemetry: { variant: 'gt3', field: 'gt3', playerPlace: 7, flow: 'live', timeline: 'race-start' },
    status: (ctx) => engineerLive('gt3', ctx, {}, {}, START_TIMELINE),
  }),
  engineerScenario({
    id: 'engineer-geometry-built',
    state: 'geometry-built',
    title: 'Engineer: one lap of history (geometry available)',
    description: 'Race start plus one full lap of position history, enough to build the track outline.',
    initialPhase: 'monitoring',
    settleMs: LIVE_SETTLE_MS,
    alerts: [],
    telemetry: {
      variant: 'gt3',
      field: 'gt3',
      playerPlace: 7,
      flow: 'live',
      timeline: 'race-start',
      backfillS: GEOMETRY_BACKFILL_S,
    },
    status: (ctx) => engineerLive('gt3', ctx, {}, {}, GEOMETRY_TIMELINE),
  }),
];

const settingsW0Scenarios: Scenario[] = [
  {
    id: 'settings-ui-scale-125',
    group: 'settings',
    state: 'ui-scale-125',
    title: 'Settings: UI scale 125 %',
    description: 'Driver mode with the stored UI scale at 1.25 (settings page open).',
    mode: 'driver',
    login: LOGGED_IN_DRIVER,
    snapshot: { config: { ...previewConfig('driver'), ui: { scale: 1.25, max_refresh_hz: 60 } } },
    status: ({ elapsedS }) => liveDriver({}, elapsedS),
    steps: [{ kind: 'clickText', text: 'Settings' }],
  },
];

export const SCENARIOS: readonly Scenario[] = [
  ...driverScenarios,
  ...driverW0Scenarios,
  ...engineerScenarios,
  ...engineerW0Scenarios,
  ...appScenarios,
  ...settingsW0Scenarios,
];

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
  const timeline = scenarioTimeline(scenario);
  return {
    session_id: SESSION_ID,
    driver_display_name: DRIVER_NAME,
    driver_username: DRIVER_USERNAME,
    car: car.name,
    vehicle_class: car.vehicleClass,
    track: TRACK_NAME,
    session_type: timeline.sessionType,
    started_at_ns: (NOW_UNIX_S - timeline.sessionTimeAtLoadS) * 1e9,
    schema_version: 1,
  };
}
