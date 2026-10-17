/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). Scenario-driven fake of the Tauri backend.
 *
 * It answers the app's typed commands (src/lib/ipc.ts `COMMAND_NAMES`), emits the JSON core events
 * (`core:status` 4 Hz, `core:scoring` 5 Hz, `core:engine_state` 2 Hz, `core:alert`) and streams raw UiBatch
 * buffers on the subscribed Channel exactly like `bridge_subscribe` (RESET + BACKFILL, then live batches at
 * 100 Hz rows). The Tauri shims in ./tauri route `invoke`, `listen` and `Channel` here; the vite alias in
 * preview/vite.config.ts swaps them in for `@tauri-apps/api/*`, so the app's own modules run unchanged.
 * Every CoreStatus and subscription reports `origin: 'synthetic'`.
 */
import type { EngineStateDto } from '../hooks/useEngineState';
import type { BridgeSubscribeResponse, CommandName, ConfigSnapshot, CoreEventName, CoreStatus, Mode } from '../lib/ipc';
import { identityCardFor, type Scenario } from './scenarios';
import { PREVIEW_APP_INFO, previewSnapshot } from './status';
import { SyntheticField } from './synthetic/field';
import { SyntheticTelemetryStream } from './synthetic/telemetry';
import { CAR_PARAMS, ROW_DT_S, type CarVariant } from './synthetic/car';
import {
  BACKFILL_S,
  CURRENT_STINT,
  RACE_LENGTH_S,
  STINT_LAPS_DONE,
  STREAM_START_S,
  raceTimeline,
} from './synthetic/race';

const LIVE_BATCH_INTERVAL_MS = 50;
/** Fuel margin (laps) the player starts the backfill with: comfortable for the GT3, tight for the Hypercar. */
const FUEL_MARGIN_LAPS: Readonly<Record<CarVariant, number>> = { gt3: 1.4, hypercar: 0.6 };

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

export class FakeBackend {
  #scenario: Scenario | null = null;
  #mode: Mode | null = null;
  #phase = 'idle';
  #startMs = 0;
  readonly #listeners = new Map<string, Set<Handler>>();
  readonly #subscriptions = new Map<number, { channel: ChannelLike; stream: SyntheticTelemetryStream | null; liveRows: number }>();
  #nextSubscription = 1;
  #field: SyntheticField | null = null;
  #timers: ReturnType<typeof setInterval>[] = [];
  #alertsSent = false;

  /** Selects the scenario and starts the event timers. */
  start(scenario: Scenario): void {
    this.#scenario = scenario;
    this.#mode = scenario.mode;
    this.#phase = scenario.initialPhase ?? 'idle';
    this.#startMs = performance.now();
    const plan = scenario.telemetry;
    if (plan !== undefined) {
      const probe = new SyntheticTelemetryStream(this.#streamOptions());
      this.#field = new SyntheticField({
        kind: plan.field,
        playerVehicle: probe.sim.params.name,
        playerClass: probe.sim.params.vehicleClass,
        playerDriver: 'Sam Driver',
        playerPlace: plan.playerPlace,
        playerProfile: probe.sim.profile,
        playerLapsDone: raceTimeline(probe.sim.variant).startLap,
        sessionType: 'RACE',
        raceLengthS: RACE_LENGTH_S,
        referenceSessionTimeS: STREAM_START_S,
      });
    }
    this.#timers.push(
      setInterval(() => {
        this.#emit('core:status', this.#status());
      }, 250),
      setInterval(() => {
        this.#tickScoring();
      }, 200),
      setInterval(() => {
        this.#tickEngineState();
      }, 500),
      setInterval(() => {
        this.#tickTelemetry();
      }, LIVE_BATCH_INTERVAL_MS),
    );
  }

  #elapsedS(): number {
    return (performance.now() - this.#startMs) / 1000;
  }

  #variant(): CarVariant {
    return this.#scenario?.telemetry?.variant ?? 'gt3';
  }

  #streamOptions(): ConstructorParameters<typeof SyntheticTelemetryStream>[0] {
    const plan = this.#scenario?.telemetry;
    const variant = this.#variant();
    const race = raceTimeline(variant);
    const car = CAR_PARAMS[variant];
    const lapsLeft = race.raceLaps - race.startLap;
    return {
      variant,
      startSessionTimeS: STREAM_START_S,
      startLap: race.startLap,
      startFuelL: (lapsLeft + FUEL_MARGIN_LAPS[variant]) * car.fuelPerLapL,
      raceLaps: race.raceLaps,
      bestLapS: race.bestLapS,
      position: plan?.playerPlace ?? 7,
      classPosition: plan?.classPlace ?? plan?.playerPlace ?? 7,
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
    return s.status({ elapsedS: this.#elapsedS(), phase: this.#phase });
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
    if (this.#field === null || !this.#telemetryActive()) return;
    const flow = this.#scenario?.telemetry?.flow;
    // A stale / away stream freezes the field at the last update the engineer received.
    const elapsed = flow === 'stop' ? 0 : this.#elapsedS();
    this.#emit('core:scoring', this.#field.snapshot(BACKFILL_S + elapsed));
    if (!this.#alertsSent && (this.#scenario?.alerts?.length ?? 0) > 0) {
      this.#alertsSent = true;
      for (const alert of this.#scenario?.alerts ?? []) this.#emit('core:alert', alert);
    }
  }

  #tickEngineState(): void {
    if (!this.#telemetryActive()) return;
    const race = raceTimeline(this.#variant());
    const stintLaps = STINT_LAPS_DONE + Math.floor((BACKFILL_S + this.#elapsedS()) / race.lapTimeS);
    const state: EngineStateDto = {
      stint: {
        current_stint: CURRENT_STINT,
        stint_laps: stintLaps,
        stint_start_time_s: race.stintStartS,
        tyre_age_laps: stintLaps,
      },
    };
    this.#emit('core:engine_state', state);
  }

  // --- telemetry bridge -------------------------------------------------------------------------------

  #startStream(id: number): void {
    const sub = this.#subscriptions.get(id);
    if (sub?.stream !== null || !this.#telemetryActive()) return;
    const stream = new SyntheticTelemetryStream(this.#streamOptions());
    sub.stream = stream;
    for (const batch of stream.backfill(BACKFILL_S)) sub.channel.onmessage(toArrayBuffer(batch));
    // One live batch confirms every channel, so the stream starts LIVE (and a 'stop' stream then ages).
    sub.channel.onmessage(toArrayBuffer(stream.live(1)));
    sub.liveRows = 1;
  }

  #tickTelemetry(): void {
    if (!this.#telemetryActive()) return;
    for (const [id, sub] of this.#subscriptions) {
      if (sub.stream === null) {
        this.#startStream(id);
        continue;
      }
      if (this.#scenario?.telemetry?.flow !== 'live') continue;
      const due = Math.floor(this.#elapsedS() / ROW_DT_S);
      const rows = Math.min(200, due - sub.liveRows);
      if (rows <= 0) continue;
      sub.channel.onmessage(toArrayBuffer(sub.stream.live(rows)));
      sub.liveRows += rows;
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
      case 'engineer_join':
        if (s.joinError !== undefined) throw new CommandError(s.joinError.code, s.joinError.message);
        this.#phase = 'awaiting_confirmation';
        return identityCardFor(s);
      case 'engineer_confirm':
        this.#phase = 'monitoring';
        this.#startMs = performance.now();
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
        const response: BridgeSubscribeResponse = { subscription_id: id, origin: 'synthetic' };
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
        return null;
    }
  }
}

/** The single fake backend instance of the preview page. */
export const fakeBackend = new FakeBackend();
