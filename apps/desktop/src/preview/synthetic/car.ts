/**
 * PREVIEW HARNESS ONLY (dev-only, never imported by the app). SYNTHETIC car model.
 *
 * A quasi-static lap simulation over the synthetic layout (track.ts): corner speed limit from lateral grip,
 * forward pass with power-limited acceleration, backward pass with braking deceleration. The resulting speed
 * profile drives throttle, brake, steering, gear and RPM, and a stateful thermal model integrates brake and tyre
 * temperatures, tyre pressures, wear, fuel and (hybrid variant) virtual energy / battery at 100 Hz.
 * Every value is invented for UI preview purposes and must only ever be shown labelled SYNTHETIC.
 */
import { TRACK_LENGTH_M, TRACK_SAMPLES, sectorAt } from './track';

export type CarVariant = 'gt3' | 'hypercar';

interface CarParams {
  readonly name: string;
  readonly vehicleClass: string;
  readonly vmax: number;
  readonly latG: number;
  readonly brakeG: number;
  readonly powerAccel: number;
  readonly fuelCapacityL: number;
  readonly fuelPerLapL: number;
  readonly rpmMax: number;
  readonly gearTopSpeeds: readonly number[];
  readonly brakeBiasFront: number;
  readonly hybrid: boolean;
}

export const CAR_PARAMS: Readonly<Record<CarVariant, CarParams>> = {
  gt3: {
    name: 'Porsche 911 GT3 R LMGT3',
    vehicleClass: 'GT3',
    vmax: 75,
    latG: 1.35,
    brakeG: 1.6,
    powerAccel: 4.0,
    fuelCapacityL: 100,
    fuelPerLapL: 3.62,
    rpmMax: 9400,
    gearTopSpeeds: [22, 33, 43, 53, 63, 76],
    brakeBiasFront: 0.565,
    hybrid: false,
  },
  hypercar: {
    name: 'Toyota GR010 Hybrid',
    vehicleClass: 'Hypercar',
    vmax: 92,
    latG: 1.95,
    brakeG: 2.4,
    powerAccel: 5.6,
    fuelCapacityL: 90,
    fuelPerLapL: 3.05,
    rpmMax: 9000,
    gearTopSpeeds: [24, 36, 48, 60, 74, 93],
    brakeBiasFront: 0.545,
    hybrid: true,
  },
};

const G = 9.81;
/** Simulation step (s): LMU's nominal 100 Hz telemetry rate. */
export const ROW_DT_S = 0.01;

export interface LapProfile {
  /** Speed (m/s) per metre of lap distance. */
  readonly speed: Float64Array;
  /** Longitudinal acceleration (m/s²) per metre. */
  readonly accel: Float64Array;
  /** Time (s) at which each metre is reached from the line. */
  readonly timeAt: Float64Array;
  readonly lapTimeS: number;
  /** Sector end times (s from the line) for sectors 1 and 2. */
  readonly sectorTimes: readonly [number, number];
}

/** Builds the reference lap of `variant` over the synthetic layout. */
export function buildLapProfile(variant: CarVariant): LapProfile {
  const p = CAR_PARAMS[variant];
  const n = TRACK_SAMPLES.length;
  const limit = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    const k = Math.abs(TRACK_SAMPLES[i]?.curvature ?? 0);
    limit[i] = k < 1e-5 ? p.vmax : Math.min(p.vmax, Math.sqrt((p.latG * G) / k));
  }
  const v = new Float64Array(n);
  // Two laps of passes so the lap wraps consistently.
  v.fill(p.vmax);
  for (let pass = 0; pass < 2; pass++) {
    for (let i = 0; i < 2 * n; i++) {
      const a = i % n;
      const prev = v[(a - 1 + n) % n] ?? 0;
      const aero = 1 - (prev / p.vmax) ** 2 * 0.85;
      const up = Math.sqrt(prev * prev + 2 * p.powerAccel * aero);
      v[a] = Math.min(limit[a] ?? p.vmax, up, v[a] ?? p.vmax);
    }
    for (let i = 2 * n; i >= 0; i--) {
      const a = i % n;
      const next = v[(a + 1) % n] ?? 0;
      const down = Math.sqrt(next * next + 2 * p.brakeG * G * (1 + (next / p.vmax) * 0.35));
      v[a] = Math.min(v[a] ?? p.vmax, down);
    }
  }
  const accel = new Float64Array(n);
  const timeAt = new Float64Array(n);
  let t = 0;
  for (let i = 0; i < n; i++) {
    const vi = v[i] ?? 1;
    const vn = v[(i + 1) % n] ?? 1;
    accel[i] = (vn * vn - vi * vi) / 2;
    timeAt[i] = t;
    t += 2 / (vi + vn);
  }
  return {
    speed: v,
    accel,
    timeAt,
    lapTimeS: t,
    sectorTimes: [timeAt[1985] ?? t / 3, timeAt[4130] ?? (2 * t) / 3],
  };
}

/** Lap distance (m) reached `tInLap` seconds after crossing the line. */
export function distanceAtTime(profile: LapProfile, tInLap: number): number {
  const { timeAt } = profile;
  let lo = 0;
  let hi = timeAt.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >>> 1;
    if ((timeAt[mid] ?? 0) <= tInLap) lo = mid;
    else hi = mid - 1;
  }
  const t0 = timeAt[lo] ?? 0;
  const t1 = lo + 1 < timeAt.length ? (timeAt[lo + 1] ?? t0) : profile.lapTimeS;
  return lo + (t1 > t0 ? (tInLap - t0) / (t1 - t0) : 0);
}

/** Deterministic noise in [-0.5, 0.5). */
export function noise(seed: number, i: number): number {
  const s = Math.sin(i * 12.9898 + seed * 78.233) * 43758.5453;
  return s - Math.floor(s) - 0.5;
}

export interface WheelState {
  surface: number;
  inner: number;
  outer: number;
  carcass: number;
  brake: number;
  brakePeak: number;
  pressurePa: number;
  wear: number;
}

/** One 100 Hz row of the synthetic car. */
export interface CarRow {
  readonly sessionTimeS: number;
  readonly lap: number;
  readonly lapTimeS: number;
  readonly lapDistM: number;
  readonly sector: number;
  readonly speed: number;
  readonly throttle: number;
  readonly brake: number;
  readonly clutch: number;
  readonly steering: number;
  readonly gear: number;
  readonly rpm: number;
  readonly tcActive: boolean;
  readonly fuelL: number;
  readonly deltaBestS: number;
  readonly wheels: readonly WheelState[];
  readonly virtualEnergy: number | null;
  readonly batterySoc: number | null;
  readonly regenPowerW: number | null;
  readonly motorTorqueNm: number | null;
  readonly motorRpm: number | null;
}

const AMBIENT_C = 29;
const TRACK_C = 41;

/** Stateful 100 Hz simulator. `startLap` laps are already completed at `startSessionTimeS`. */
export class CarSimulator {
  readonly params: CarParams;
  readonly profile: LapProfile;
  readonly variant: CarVariant;
  #row = 0;
  readonly #t0: number;
  readonly #startLap: number;
  readonly #wheels: WheelState[];
  #fuel: number;
  #ve: number;
  #soc: number;

  constructor(variant: CarVariant, startSessionTimeS: number, startLap: number, fuelL: number) {
    this.variant = variant;
    this.params = CAR_PARAMS[variant];
    this.profile = buildLapProfile(variant);
    this.#t0 = startSessionTimeS;
    this.#startLap = startLap;
    this.#fuel = fuelL;
    this.#ve = 0.71;
    this.#soc = 0.55;
    this.#wheels = [0, 1, 2, 3].map((w) => ({
      surface: w < 2 ? 88 : 84,
      inner: w < 2 ? 93 : 88,
      outer: w < 2 ? 82 : 79,
      carcass: w < 2 ? 86 : 83,
      brake: w < 2 ? 420 : 340,
      brakePeak: w < 2 ? 690 : 540,
      pressurePa: w < 2 ? 178_000 : 174_000,
      wear: 0.9 - 0.012 * w,
    }));
    // Warm the thermal state up over one lap without emitting it.
    const warm = Math.round(this.profile.lapTimeS / ROW_DT_S);
    for (let i = 0; i < warm; i++) this.#integrate(i, false);
  }

  /** Session time (s) of the next row. */
  get nextTimeS(): number {
    return this.#t0 + this.#row * ROW_DT_S;
  }

  /** Advances one row (10 ms) and returns it. */
  step(): CarRow {
    const row = this.#integrate(this.#row, true);
    this.#row++;
    return row;
  }

  #integrate(rowIndex: number, advanceConsumables: boolean): CarRow {
    const p = this.params;
    const prof = this.profile;
    const elapsed = rowIndex * ROW_DT_S;
    // Small lap-to-lap pace variation.
    const lapTime = prof.lapTimeS;
    const lapsDone = Math.floor(elapsed / lapTime);
    const tInLap = elapsed - lapsDone * lapTime;
    const dist = distanceAtTime(prof, tInLap);
    const i = Math.min(TRACK_SAMPLES.length - 1, Math.floor(dist));
    const speed = Math.max(0, (prof.speed[i] ?? 0) * (1 + 0.004 * noise(3, rowIndex)));
    const a = prof.accel[i] ?? 0;
    const k = TRACK_SAMPLES[i]?.curvature ?? 0;

    let throttle: number;
    let brake: number;
    if (a < -1.5) {
      brake = Math.min(1, -a / (p.brakeG * G) + 0.08);
      throttle = 0;
    } else if (a > 0.4) {
      brake = 0;
      throttle = Math.min(1, 0.55 + a / p.powerAccel);
    } else {
      brake = 0;
      throttle = speed >= p.vmax * 0.98 ? 1 : 0.42 + 0.2 * Math.abs(noise(7, Math.floor(rowIndex / 10)));
    }
    throttle = Math.max(0, Math.min(1, throttle + 0.01 * noise(11, rowIndex)));
    const steering = Math.max(-1, Math.min(1, k * 2.7 * 6.5 + 0.004 * noise(13, rowIndex)));

    let gear = p.gearTopSpeeds.findIndex((top) => speed <= top * 0.97) + 1;
    if (gear === 0) gear = p.gearTopSpeeds.length;
    const top = p.gearTopSpeeds[gear - 1] ?? p.vmax;
    const bottom = gear > 1 ? (p.gearTopSpeeds[gear - 2] ?? 0) : 0;
    const rpm = Math.min(
      p.rpmMax,
      Math.max(3200, 5600 + ((speed - bottom) / Math.max(1, top - bottom)) * (p.rpmMax - 5800)),
    );

    // Thermal model (first order): brake heat from brake pressure × speed, tyre heat from lateral and
    // longitudinal load; cooling grows with speed.
    const lat = Math.abs(k) * speed * speed;
    const leftCorner = k > 0;
    for (let w = 0; w < 4; w++) {
      const wheel = this.#wheels[w];
      if (wheel === undefined) continue;
      const front = w < 2;
      const outerSide = leftCorner ? w === 1 || w === 3 : w === 0 || w === 2;
      const bias = front ? p.brakeBiasFront : 1 - p.brakeBiasFront;
      const brakeHeat = brake * speed * bias * (front ? 3.5 : 3.0);
      const brakeCool = (wheel.brake - AMBIENT_C) * (0.0002 + speed * 0.000008) * 100;
      wheel.brake += (brakeHeat - brakeCool) * ROW_DT_S;
      wheel.brakePeak = Math.max(wheel.brakePeak * 0.99995, wheel.brake);
      const tyreHeat = lat * (outerSide ? 0.5 : 0.32) * (front ? 1.08 : 1) + brake * speed * (front ? 0.2 : 0.12);
      const tyreCool = (wheel.surface - TRACK_C) * (0.05 + speed * 0.0013);
      wheel.surface += (tyreHeat - tyreCool) * ROW_DT_S * 4.5;
      wheel.inner += (wheel.surface + 6 - wheel.inner) * ROW_DT_S * 0.9;
      wheel.outer += (wheel.surface - 6 - wheel.outer) * ROW_DT_S * 0.9;
      wheel.carcass += (wheel.surface - 1 - wheel.carcass) * ROW_DT_S * 0.08;
      const cold = front ? 155_000 : 152_000;
      wheel.pressurePa = cold * ((wheel.carcass + 273.15) / (25 + 273.15));
      if (advanceConsumables) wheel.wear = Math.max(0.05, wheel.wear - (lat * 1.5e-9 + brake * 1e-8));
    }

    let ve: number | null = null;
    let soc: number | null = null;
    let regen: number | null = null;
    let torque: number | null = null;
    let motorRpm: number | null = null;
    if (advanceConsumables) this.#fuel = Math.max(0, this.#fuel - (p.fuelPerLapL / lapTime) * ROW_DT_S * (0.35 + throttle));
    if (p.hybrid) {
      const deploy = throttle > 0.9 && speed > 33 ? 1 : 0;
      regen = brake > 0.05 ? -brake * 200_000 : deploy * 150_000;
      torque = brake > 0.05 ? -brake * 180 : deploy * 210;
      motorRpm = speed * 95;
      if (advanceConsumables) {
        this.#soc = Math.min(0.95, Math.max(0.05, this.#soc + (brake * 0.0012 - deploy * 0.0002)));
        this.#ve = Math.max(0, this.#ve - (0.0285 / lapTime) * ROW_DT_S * (0.35 + throttle));
      }
      ve = this.#ve;
      soc = this.#soc;
    }

    return {
      sessionTimeS: this.#t0 + elapsed,
      lap: this.#startLap + lapsDone + 1,
      lapTimeS: tInLap,
      lapDistM: dist,
      sector: sectorAt(dist),
      speed,
      throttle,
      brake,
      clutch: speed < 4 ? 0.8 : 0,
      steering,
      gear: speed < 0.5 ? 0 : gear,
      rpm,
      tcActive: throttle > 0.85 && Math.abs(k) > 0.004,
      fuelL: this.#fuel,
      deltaBestS: 0.18 * Math.sin((2 * Math.PI * dist) / TRACK_LENGTH_M) - 0.12 + 0.05 * Math.sin(dist / 410),
      wheels: this.#wheels.map((w) => ({ ...w })),
      virtualEnergy: ve,
      batterySoc: soc,
      regenPowerW: regen,
      motorTorqueNm: torque,
      motorRpm,
    };
  }
}
