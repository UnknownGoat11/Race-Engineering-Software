/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). Scenario-driven fake of the Tauri backend.
 *
 * It answers the app's typed commands (src/lib/ipc.ts `COMMAND_NAMES`), emits the JSON core events
 * (`core:status` 4 Hz, `core:scoring` 5 Hz, `core:engine_state` 2 Hz, `core:alert`) and streams raw UiBatch
 * buffers on the subscribed Channel exactly like `bridge_subscribe` (RESET + BACKFILL, then live batches at
 * 100 Hz rows). The Tauri shims in ./tauri route `invoke`, `listen` and `Channel` here; the vite alias in
 * preview/vite.config.ts swaps them in for `@tauri-apps/api/*`, so the app's own modules run unchanged.
 *
 * Origin: every CoreStatus and subscription reports the scenario's `origin` (default `'synthetic'`). A scenario
 * with `origin: 'lmu'` simulates what the app shows for real LMU data; the data itself is still synthetic and the
 * page keeps its PREVIEW corner tag (src/preview/main.tsx), so it is never presented as real data.
 *
 * Determinism (for the capture pixel diff): everything the backend emits is a function of a virtual clock that
 * advances in fixed 50 ms ticks, never of wall time or `Math.random`. `freeze()` advances the clock to the
 * scenario's planned capture time, emits one final state and stops; the page then holds the clocks
 * (src/preview/clock.ts) so the app's own freshness timers see a fresh stream while the screenshot is taken.
 */
import type { EngineStateDto } from '../hooks/useEngineState';
import type {
  BridgeSubscribeResponse,
  CommandName,
  ConfigSnapshot,
  CoreEventName,
  CoreStatus,
  Mode,
  Origin,
} from '../lib/ipc';
import { identityCardFor, scenarioTimeline, type Scenario } from './scenarios';
import { PREVIEW_APP_INFO, previewSnapshot } from './status';
import { SyntheticField } from './synthetic/field';
import { SyntheticTelemetryStream } from './synthetic/telemetry';
import { CAR_PARAMS, ROW_DT_S, type CarVariant } from './synthetic/car';

/** One virtual tick (ms). Telemetry every tick, scoring every 4, status every 5, engine state every 10. */
export const TICK_MS = 50;
/** Fuel margin (laps) the player starts the backfill with: comfortable for the GT3, tight for the Hypercar. */
const FUEL_MARGIN_LAPS: Readonly<Record<CarVariant, number>> = { gt3: 1.4, hypercar: 0.6 };
/** Largest live batch (rows) the fake bridge sends at once. */
const MAX_ROWS_PER_BATCH = 200;

export interface ChannelLike {
  onmessage: (message: ArrayBuffer) => void;
}

type Handler = (payload: unknown) => void;

/** A failed command; `toIpcError` (src/lib/ipc.ts) reads its `code` and `message` like a Rust command error. */
class CommandError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(out).set(bytes);
  return out;
}

interface Subscription {
  readonly channel: ChannelLike;
  stream: SyntheticTelemetryStream | null;
  liveRows: number;
}

export class FakeBackend {
  #scenario: Scenario | null = null;
  #mode: Mode | null = null;
  #phase = 'idle';
  /** Virtual clock: ticks since start, and the tick the scenario's elapsed time counts from. */
  #tick = 0;
  #tickBase = 0;
  #frozen = false;
  /** Failed `engineer_join` attempts so far (for scenarios whose join fails only the first time). */
  #joinAttempts = 0;
  readonly #listeners = new Map<string, Set<Handler>>();
  readonly #subscriptions = new Map<number, Subscription>();
  #nextSubscription = 1;
  #field: SyntheticField | null = null;
  #timer: ReturnType<typeof setInterval> | null = null;
  #alertsSent = false;

  /** Origin the scenario reports (`synthetic` unless the scenario simulates LMU-origin data). */
  get origin(): Origin {
    return this.#scenario?.origin ?? 'synthetic';
  }

  /** Selects the scenario and starts the virtual clock. */
  start(scenario: Scenario): void {
    this.#scenario = scenario;
    this.#mode = scenario.mode;
    this.#phase = scenario.initialPhase ?? 'idle';
    this.#tick = 0;
    this.#tickBase = 0;
    const plan = scenario.telemetry;
    if (plan !== undefined) {
      const timeline = scenarioTimeline(scenario);
      const probe = new SyntheticTelemetryStream(this.#streamOptions());
      this.#field = new SyntheticField({
        kind: plan.field,
        playerVehicle: probe.sim.params.name,
        playerClass: probe.sim.params.vehicleClass,
        playerDriver: 'Sam Driver',
        playerPlace: plan.playerPlace,
        playerProfile: probe.sim.profile,
        playerLapsDone: timeline.startLap,
        playerLapOffsetS: timeline.lapOffsetS,
        sessionType: timeline.sessionType,
        raceLengthS: timeline.sessionLengthS,
        referenceSessionTimeS: timeline.streamStartS,
        phase: timeline.kind === 'race-start' ? 'start' : 'final-stint',
        qualifying: timeline.kind === 'qualifying',
        fcy: plan.fcy === true,
      });
    }
    this.#timer = setInterval(() => {
      this.#onTick();
    }, TICK_MS);
  }

  #onTick(): void {
    if (this.#frozen) return;
    this.#tick++;
    this.#emitTick(this.#tick);
  }

  /** Everything due at virtual tick `t`. */
  #emitTick(t: number): void {
    this.#tickTelemetry();
    if (t % 4 === 0) this.#tickScoring();
    if (t % 5 === 0) this.#emit('core:status', this.#status());
    if (t % 10 === 0) this.#tickEngineState();
  }

  /**
   * Advances the virtual clock to the scenario's planned capture time (`elapsedS` since the elapsed base, at
   * least the current time), emits the complete final state and stops all emission. Returns true when the
   * planned time had already passed (the page reports that, since the frame is then not exactly reproducible).
   */
  freeze(captureAtS: number): boolean {
    if (this.#timer !== null) clearInterval(this.#timer);
    this.#timer = null;
    const target = this.#tickBase + Math.round((captureAtS * 1000) / TICK_MS);
    const late = this.#tick > target;
    // Advance without per-tick event emission (the UI only needs the final state), but stream every row.
    this.#tick = Math.max(this.#tick, target);
    this.#tickTelemetry();
    this.#tickScoring();
    this.#emit('core:status', this.#status());
    this.#tickEngineState();
    this.#frozen = true;
    return late;
  }

  /** Scenario elapsed time (s): virtual, from the elapsed base (page start, or the engineer's confirm). */
  #elapsedS(): number {
    return ((this.#tick - this.#tickBase) * TICK_MS) / 1000;
  }

  #variant(): CarVariant {
    return this.#scenario?.telemetry?.variant ?? 'gt3';
  }

  #streamOptions(): ConstructorParameters<typeof SyntheticTelemetryStream>[0] {
    const s = this.#scenario;
    const plan = s?.telemetry;
    const variant = this.#variant();
    const timeline = s === null ? null : scenarioTimeline(s);
    const car = CAR_PARAMS[variant];
    if (timeline === null) throw new Error('no preview scenario selected');
    let startFuelL: number;
    if (timeline.kind === 'final-stint') {
      startFuelL = (timeline.raceLaps - timeline.startLap + FUEL_MARGIN_LAPS[variant]) * car.fuelPerLapL;
    } else if (timeline.kind === 'race-start') {
      startFuelL = car.fuelCapacityL * 0.97;
    } else {
      startFuelL = 6.5 * car.fuelPerLapL;
    }
    return {
      variant,
      startSessionTimeS: timeline.streamStartS,
      startLap: timeline.startLap,
      startFuelL,
      raceLaps: timeline.raceLaps,
      bestLapS: timeline.bestLapS,
      position: plan?.playerPlace ?? 7,
      classPosition: plan?.classPlace ?? plan?.playerPlace ?? 7,
      lapOffsetS: timeline.lapOffsetS,
      virtualEnergy: plan?.virtualEnergy === true,
      session: timeline.kind === 'qualifying' ? 'qualifying' : 'race',
      fcy: plan?.fcy === true,
    };
  }

  #telemetryActive(): boolean {
    const s = this.#scenario;
    if (s?.telemetry === undefined) return false;
    return s.gatedByPhase !== true || this.#phase === 'monitoring' || this.#phase === 'ended';
  }

  #status(): CoreStatus {
    const s = this.#scenario;
    if (s === null) throw new Error('no preview scenario selected');
    // The scenario's origin always wins: builders never decide provenance on their own.
    return { ...s.status({ elapsedS: this.#elapsedS(), phase: this.#phase, joinAttempts: this.#joinAttempts }), origin: this.origin };
  }

  #snapshot(): ConfigSnapshot {
    return previewSnapshot(this.#mode, this.#scenario?.snapshot ?? {});
  }

  // --- events -----------------------------------------------------------------------------------------

  listen(event: string, handler: Handler): () => void {
    let set = this.#listeners.get(event);
    if (set === undefined) {
      set = new Set();
      this.#listeners.set(event, set);
    }
    set.add(handler);
    // Late listeners still see the alerts the scenario raised.
    if (event === 'core:alert' && this.#alertsSent) {
      for (const alert of this.#scenario?.alerts ?? []) handler(alert);
    }
    return () => {
      set.delete(handler);
    };
  }

  #emit(event: CoreEventName, payload: unknown): void {
    for (const handler of this.#listeners.get(event) ?? []) handler(payload);
  }

  #tickScoring(): void {
    if (this.#field === null || !this.#telemetryActive() || this.#scenario === null) return;
    const flow = this.#scenario.telemetry?.flow;
    const timeline = scenarioTimeline(this.#scenario);
    // A stale / away stream freezes the field at the last update the engineer received.
    const elapsed = flow === 'stop' ? 0 : this.#elapsedS();
    this.#emit('core:scoring', this.#field.snapshot(timeline.backfillS + elapsed));
    if (!this.#alertsSent && (this.#scenario.alerts?.length ?? 0) > 0) {
      this.#alertsSent = true;
      for (const alert of this.#scenario.alerts ?? []) this.#emit('core:alert', alert);
    }
  }

  #tickEngineState(): void {
    if (!this.#telemetryActive() || this.#scenario === null) return;
    const timeline = scenarioTimeline(this.#scenario);
    const elapsed = this.#scenario.telemetry?.flow === 'stop' ? 0 : this.#elapsedS();
    const driven = timeline.lapOffsetS + timeline.backfillS + elapsed;
    const stintLaps = timeline.stintLapsDone + Math.floor(driven / timeline.lapTimeS);
    const state: EngineStateDto = {
      stint: {
        current_stint: timeline.stint,
        stint_laps: stintLaps,
        stint_start_time_s: timeline.stintStartS,
        tyre_age_laps: stintLaps,
      },
    };
    this.#emit('core:engine_state', state);
  }

  // --- telemetry bridge -------------------------------------------------------------------------------

  #startStream(sub: Subscription): void {
    if (sub.stream !== null || !this.#telemetryActive() || this.#scenario === null) return;
    const stream = new SyntheticTelemetryStream(this.#streamOptions());
    sub.stream = stream;
    for (const batch of stream.backfill(scenarioTimeline(this.#scenario).backfillS)) {
      sub.channel.onmessage(toArrayBuffer(batch));
    }
    // One live batch confirms every channel, so the stream starts LIVE (and a 'stop' stream then ages).
    sub.channel.onmessage(toArrayBuffer(stream.live(1)));
    sub.liveRows = 1;
  }

  #tickTelemetry(): void {
    if (!this.#telemetryActive()) return;
    for (const sub of this.#subscriptions.values()) {
      if (sub.stream === null) this.#startStream(sub);
      if (sub.stream === null || this.#scenario?.telemetry?.flow !== 'live') continue;
      const stream = sub.stream;
      const due = Math.floor(this.#elapsedS() / ROW_DT_S);
      while (due > sub.liveRows) {
        const rows = Math.min(MAX_ROWS_PER_BATCH, due - sub.liveRows);
        sub.channel.onmessage(toArrayBuffer(stream.live(rows)));
        sub.liveRows += rows;
      }
    }
  }

  // --- commands ---------------------------------------------------------------------------------------

  invoke(command: string, args: Record<string, unknown>): Promise<unknown> {
    try {
      return Promise.resolve(this.#command(command as CommandName, args));
    } catch (error: unknown) {
      return Promise.reject(error instanceof Error ? error : new Error(String(error)));
    }
  }

  #command(command: CommandName, args: Record<string, unknown>): unknown {
    const s = this.#scenario;
    if (s === null) throw new CommandError('backend_unavailable', 'No preview scenario selected.');
    if (s.backendDown === true && (command === 'get_app_info' || command === 'get_config')) {
      throw new CommandError('core_shutdown', 'The core runtime failed to start: relay TLS configuration unreadable (preview).');
    }
    switch (command) {
      case 'get_app_info':
        return PREVIEW_APP_INFO;
      case 'get_config':
        return this.#snapshot();
      case 'set_mode': {
        const mode = args['mode'];
        if (mode !== 'driver' && mode !== 'engineer') throw new CommandError('invalid_mode', 'Unknown mode.');
        this.#mode = mode;
        return this.#snapshot();
      }
      case 'update_settings':
        return this.#snapshot();
      case 'get_core_status':
        return this.#status();
      case 'test_relay_connection':
        throw new CommandError('relay_unreachable', 'Preview harness: no relay.');
      case 'log_ui_error':
        console.warn('[preview] log_ui_error', args['entry']);
        return { logged: true, suppressed: 0 };
      case 'get_login_state':
        return s.login ?? { authenticated: false, role: this.#mode ?? 'driver', account: null };
      case 'login':
      case 'register_account':
        throw new CommandError('not_implemented', 'Preview harness: authentication is not simulated.');
      case 'engineer_join': {
        const attempt = this.#joinAttempts++;
        const failures = s.joinErrorAttempts ?? Number.POSITIVE_INFINITY;
        if (s.joinError !== undefined && attempt < failures) {
          throw new CommandError(s.joinError.code, s.joinError.message);
        }
        this.#phase = 'awaiting_confirmation';
        return identityCardFor(s);
      }
      case 'engineer_confirm':
        this.#phase = 'monitoring';
        this.#tickBase = this.#tick;
        return null;
      case 'engineer_cancel':
      case 'engineer_disconnect':
        this.#phase = 'idle';
        return null;
      case 'bridge_subscribe': {
        const channel = args['on_batch'] as ChannelLike | undefined;
        if (channel === undefined) throw new CommandError('invalid_input', 'on_batch channel missing');
        const id = this.#nextSubscription++;
        this.#subscriptions.set(id, { channel, stream: null, liveRows: 0 });
        // The stream starts on the next telemetry tick, as the bridge's RESET + BACKFILL burst.
        const response: BridgeSubscribeResponse = { subscription_id: id, origin: this.origin };
        return response;
      }
      case 'bridge_unsubscribe':
        this.#subscriptions.delete(Number(args['subscription_id']));
        return null;
      // Driver / demo controls: accepted, the scenario's status stays as designed (the preview shows states,
      // it does not simulate transitions).
      case 'logout':
      case 'driver_start_streaming':
      case 'driver_stop_streaming':
      case 'driver_end_session':
      case 'driver_new_code':
      case 'driver_kick_engineer':
      case 'driver_set_recording':
      case 'driver_add_marker':
      case 'driver_ack_alert':
      case 'engineer_request_snapshot':
      case 'demo_start':
      case 'demo_stop':
      case 'restart_app':
      case 'pick_motec_file':
        return null;
    }
  }
}

/** The single fake backend instance of the preview page. */
export const fakeBackend = new FakeBackend();
