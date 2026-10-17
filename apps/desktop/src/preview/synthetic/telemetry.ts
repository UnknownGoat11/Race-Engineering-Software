/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). SYNTHETIC telemetry stream.
 *
 * Turns the synthetic car model (car.ts) into real wire data: TLV blocks (one per 100 Hz row) wrapped in UiBatch
 * v1 envelopes, encoded with the same test encoder the unit tests check byte-for-byte against the Rust encoder
 * (src/test/tlvEncoder.ts). Value types come from the generated channel registry, so the real TelemetryStore
 * decodes them exactly as it decodes the Rust core's stream:
 * - the first block of a RESET carries every registry channel: values for the channels the synthetic car
 *   provides, the registry's default quality (UNAVAILABLE / NOT_SUPPORTED) for every other channel;
 * - later blocks are deltas (physics every row, scoring-class channels at 5 Hz, event channels on lap change).
 */
import { CHANNELS, Quality, ValueType, ch } from '../../generated/channels';
import { channelDef, defaultQuality } from '../../telemetry/registry';
import { UI_BATCH_FLAG_BACKFILL, UI_BATCH_FLAG_RESET } from '../../telemetry/uiBatch';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../../test/tlvEncoder';
import { TRACK_LENGTH_M } from './track';
import { CarSimulator, ROW_DT_S, type CarRow, type CarVariant } from './car';

const WHEEL_SUFFIXES = ['FL', 'FR', 'RL', 'RR'] as const;

/** A record with the registry's value type for `id` (numbers only; strings via `rec.str`). */
export function numberRecord(id: number, value: number): FixtureRecord {
  const def = channelDef(id);
  if (def === undefined) throw new Error(`channel 0x${id.toString(16)} is not in the registry`);
  if (!Number.isFinite(value)) return rec.quality(id, Quality.Unavailable);
  switch (def.valueType) {
    case ValueType.F32:
      return rec.f32(id, value);
    case ValueType.F64:
      return rec.f64(id, value);
    case ValueType.I32:
      return rec.i32(id, Math.round(value));
    case ValueType.U32:
      return rec.u32(id, Math.max(0, Math.round(value)));
    case ValueType.U8:
      return rec.u8(id, Math.max(0, Math.min(255, Math.round(value))));
    case ValueType.I8:
      return rec.i8(id, Math.max(-128, Math.min(127, Math.round(value))));
    case ValueType.Bool:
      return rec.bool(id, value !== 0);
    case ValueType.U64:
      return rec.u64(id, BigInt(Math.max(0, Math.round(value))));
    default:
      throw new Error(`channel ${def.constName} is not numeric`);
  }
}

function wheelId(prefix: string, w: number): number {
  const name = `${prefix}_${WHEEL_SUFFIXES[w] ?? 'FL'}`;
  const id = (ch as Record<string, number | undefined>)[name];
  if (id === undefined) throw new Error(`no channel ${name}`);
  return id;
}

export interface StreamOptions {
  readonly variant: CarVariant;
  /** Session time (s) of the first backfilled row. */
  readonly startSessionTimeS: number;
  /** Laps completed before the first row. */
  readonly startLap: number;
  readonly startFuelL: number;
  /** Race length in laps (for fuel to finish). */
  readonly raceLaps: number;
  /** Player's personal best lap (s). */
  readonly bestLapS: number;
  /** Overall and class position of the player. */
  readonly position: number;
  readonly classPosition: number;
}

/** Stateful generator of UiBatch messages for one subscription. */
export class SyntheticTelemetryStream {
  readonly sim: CarSimulator;
  readonly #opts: StreamOptions;
  #seq = 0;
  #rowsEmitted = 0;
  #lastLap = -1;
  #bestLapS: number;
  #lastLapS: number;

  constructor(opts: StreamOptions) {
    this.#opts = opts;
    this.sim = new CarSimulator(opts.variant, opts.startSessionTimeS, opts.startLap, opts.startFuelL);
    this.#bestLapS = opts.bestLapS;
    this.#lastLapS = this.sim.profile.lapTimeS + 0.387;
  }

  /** Session time (s) of the next row. */
  get nextTimeS(): number {
    return this.sim.nextTimeS;
  }

  #physicsRecords(r: CarRow): FixtureRecord[] {
    const p = this.sim.params;
    const out: FixtureRecord[] = [
      numberRecord(ch.SPEED, r.speed),
      numberRecord(ch.THROTTLE, r.throttle),
      numberRecord(ch.BRAKE, r.brake),
      numberRecord(ch.CLUTCH, r.clutch),
      numberRecord(ch.STEERING, r.steering),
      numberRecord(ch.GEAR, r.gear),
      numberRecord(ch.RPM, r.rpm),
      numberRecord(ch.TC_ACTIVE, r.tcActive ? 1 : 0),
      numberRecord(ch.SPEED_LIMITER, 0),
      numberRecord(ch.LAP_NUMBER, r.lap),
      numberRecord(ch.LAP_TIME, r.lapTimeS),
      numberRecord(ch.SECTOR, r.sector),
      numberRecord(ch.DELTA_BEST, r.deltaBestS),
      numberRecord(ch.DELTA_PERSONAL_BEST, r.deltaBestS + 0.041),
      numberRecord(ch.PREDICTED_LAP_TIME, this.#bestLapS + r.deltaBestS + 0.041),
      numberRecord(ch.FUEL, r.fuelL),
      numberRecord(ch.BRAKE_BIAS_FRONT, p.brakeBiasFront),
      numberRecord(ch.BRAKE_BIAS_REAR, 1 - p.brakeBiasFront),
      numberRecord(ch.SRC_TELEMETRY_AGE, 0.008 + 0.004 * Math.random()),
    ];
    const fuelPerLap = p.fuelPerLapL;
    const lapsLeftInRace = Math.max(0, this.#opts.raceLaps - r.lap + 1 - r.lapDistM / TRACK_LENGTH_M);
    const fuelLaps = r.fuelL / fuelPerLap;
    const toFinish = lapsLeftInRace * fuelPerLap;
    out.push(
      numberRecord(ch.FUEL_LAPS_REMAINING, fuelLaps),
      numberRecord(ch.FUEL_TO_FINISH, toFinish),
      numberRecord(ch.FUEL_TO_ADD, Math.min(p.fuelCapacityL, Math.max(0, toFinish + fuelPerLap - r.fuelL))),
      numberRecord(ch.FUEL_MARGIN_LAPS, fuelLaps - lapsLeftInRace),
    );
    let front = 0;
    let rear = 0;
    let left = 0;
    let right = 0;
    let brakeFront = 0;
    let brakeRear = 0;
    r.wheels.forEach((w, i) => {
      out.push(
        numberRecord(wheelId('TYRE_TEMP_INNER', i), w.inner),
        numberRecord(wheelId('TYRE_TEMP_MIDDLE', i), w.surface),
        numberRecord(wheelId('TYRE_TEMP_OUTER', i), w.outer),
        numberRecord(wheelId('TYRE_CARCASS_TEMP', i), w.carcass),
        numberRecord(wheelId('TYRE_PRESSURE', i), w.pressurePa),
        numberRecord(wheelId('TYRE_WEAR', i), w.wear),
        numberRecord(wheelId('BRAKE_TEMP', i), w.brake),
        numberRecord(wheelId('BRAKE_TEMP_PEAK', i), w.brakePeak),
      );
      if (i < 2) {
        front += w.surface / 2;
        brakeFront += w.brake / 2;
      } else {
        rear += w.surface / 2;
        brakeRear += w.brake / 2;
      }
      if (i % 2 === 0) left += w.surface / 2;
      else right += w.surface / 2;
    });
    out.push(
      numberRecord(ch.TYRE_TEMP_BALANCE_FRONT_REAR, front - rear),
      numberRecord(ch.TYRE_TEMP_BALANCE_LEFT_RIGHT, left - right),
      numberRecord(ch.BRAKE_TEMP_FRONT_REAR, brakeFront - brakeRear),
    );
    if (p.hybrid) {
      out.push(
        numberRecord(ch.VIRTUAL_ENERGY, r.virtualEnergy ?? Number.NaN),
        numberRecord(ch.BATTERY_SOC, r.batterySoc ?? Number.NaN),
        numberRecord(ch.BATTERY_CHARGE, r.batterySoc ?? Number.NaN),
        numberRecord(ch.REGEN_POWER, r.regenPowerW ?? Number.NaN),
        numberRecord(ch.HYBRID_MOTOR_TORQUE, r.motorTorqueNm ?? Number.NaN),
        numberRecord(ch.HYBRID_MOTOR_RPM, r.motorRpm ?? Number.NaN),
      );
    }
    return out;
  }

  #scoringRecords(r: CarRow): FixtureRecord[] {
    const best = this.#bestLapS;
    const s = this.sim.profile.sectorTimes;
    return [
      numberRecord(ch.LAST_LAP_TIME, this.#lastLapS),
      numberRecord(ch.BEST_LAP_TIME, best),
      numberRecord(ch.BEST_SECTOR1, s[0] - 0.08),
      numberRecord(ch.BEST_SECTOR2, s[1] - 0.15),
      numberRecord(ch.LAST_SECTOR1, s[0] + 0.12),
      numberRecord(ch.LAST_SECTOR2, s[1] - s[0] + 0.19),
      numberRecord(ch.LAST_SECTOR3, this.#lastLapS - s[1] - 0.31),
      r.sector >= 1 ? numberRecord(ch.CUR_SECTOR1, s[0] + 0.05) : rec.quality(ch.CUR_SECTOR1, Quality.Unavailable),
      numberRecord(ch.POSITION, this.#opts.position),
      numberRecord(ch.CLASS_POSITION, this.#opts.classPosition),
      numberRecord(ch.YELLOW_FLAG_STATE, 0),
      numberRecord(ch.BLUE_FLAG, 0),
      numberRecord(ch.SRC_SCORING_AGE, 0.05 + 0.1 * Math.random()),
    ];
  }

  /** Every registry channel: the synthetic car's values, the registry default quality for the rest. */
  #fullStateRecords(r: CarRow): FixtureRecord[] {
    const p = this.sim.params;
    const provided = new Map<number, FixtureRecord>();
    const put = (records: readonly FixtureRecord[]): void => {
      for (const record of records) provided.set(record.channel, record);
    };
    put(this.#physicsRecords(r));
    put(this.#scoringRecords(r));
    put([
      numberRecord(ch.FUEL_CAPACITY, p.fuelCapacityL),
      numberRecord(ch.RPM_MAX, p.rpmMax),
      rec.str(ch.VEHICLE_NAME, p.name),
      numberRecord(ch.FUEL_PER_LAP, p.fuelPerLapL),
      numberRecord(ch.FUEL_USED_LAST_LAP, p.fuelPerLapL + 0.04),
    ]);
    return CHANNELS.map((def) => provided.get(def.id) ?? rec.quality(def.id, defaultQuality(def)));
  }

  #block(full: boolean): Uint8Array {
    const r = this.sim.step();
    let records: FixtureRecord[];
    if (full) {
      records = this.#fullStateRecords(r);
    } else {
      records = this.#physicsRecords(r);
      // Scoring-class channels at 5 Hz.
      if (this.#rowsEmitted % 20 === 0) records.push(...this.#scoringRecords(r));
      if (r.lap !== this.#lastLap && this.#lastLap >= 0) {
        this.#lastLapS = this.sim.profile.lapTimeS + 0.1 + 0.4 * Math.random();
        records.push(
          numberRecord(ch.FUEL_PER_LAP, this.sim.params.fuelPerLapL),
          numberRecord(ch.FUEL_USED_LAST_LAP, this.sim.params.fuelPerLapL + 0.06 * (Math.random() - 0.5)),
          ...this.#scoringRecords(r),
        );
      }
    }
    this.#lastLap = r.lap;
    this.#rowsEmitted++;
    return encodeTlvBlock({ sourceEtS: r.sessionTimeS }, records);
  }

  /**
   * The RESET + BACKFILL burst a fresh subscription starts with: `seconds` of history, split into batches of at
   * most `rowsPerBatch` rows (the bridge splits large backfills the same way).
   */
  backfill(seconds: number, rowsPerBatch = 1000): Uint8Array[] {
    const rows = Math.round(seconds / ROW_DT_S);
    const batches: Uint8Array[] = [];
    let blocks: Uint8Array[] = [];
    let first = true;
    for (let i = 0; i < rows; i++) {
      blocks.push(this.#block(i === 0));
      if (blocks.length >= rowsPerBatch || i === rows - 1) {
        const flags = UI_BATCH_FLAG_BACKFILL | (first ? UI_BATCH_FLAG_RESET : 0);
        batches.push(encodeUiBatch(flags, this.#seq++, blocks));
        blocks = [];
        first = false;
      }
    }
    return batches;
  }

  /** One live batch with `rows` rows. */
  live(rows: number): Uint8Array {
    const blocks: Uint8Array[] = [];
    for (let i = 0; i < rows; i++) blocks.push(this.#block(this.#rowsEmitted === 0 && i === 0));
    return encodeUiBatch(0, this.#seq++, blocks);
  }
}
