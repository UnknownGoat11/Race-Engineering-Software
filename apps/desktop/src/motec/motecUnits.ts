/**
 * MoTeC channel name -> registry channel mapping and unit conversion into registry (SI) units.
 *
 * The graph and the rest of the app expect registry units (crates/telemetry-model/src/registry.rs,
 * src/generated/channels.ts): SPEED m/s, pedals as fractions 0..1, pressures Pa, angles rad, temperatures °C.
 * MoTeC files store engineering units (km/h, %, kPa, deg, …) named by the channel's unit string. A mapping is
 * used only when that unit string converts to the registry unit; otherwise it is dropped (never guessed).
 */
import { CHANNEL_BY_ID, ch, type Unit } from '../generated/channels';

/**
 * MoTeC channel names (normalised: lower case, non-alphanumerics as single spaces) -> registry channel.
 *
 * Deliberately NOT mapped:
 * - lateral / longitudinal G: the sign convention of a MoTeC file is not stated in the file (registry G_LAT is
 *   positive to the LEFT), so a mapping could silently flip corners;
 * - steering as a fraction/percentage: sign and full-scale convention are not stated; steering *angles* map to
 *   STEERING_ANGLE (rad);
 * - brake pressure / force: a pressure is not a pedal position (BRAKE is a fraction);
 * - wheel speed: one wheel's speed is not vehicle speed;
 * - plain 'time' / 'distance': elapsed session time / odometer are not lap time / lap distance.
 */
const MOTEC_NAME_TO_CH_ID: Readonly<Record<string, number>> = {
  // Speed
  'ground speed': ch.SPEED,
  'speed': ch.SPEED,
  'car speed': ch.SPEED,
  'gps speed': ch.SPEED,
  'vcar': ch.SPEED,
  'v car': ch.SPEED,
  'corr speed': ch.SPEED,
  'vehicle speed': ch.SPEED,
  'speed kmh': ch.SPEED,
  'speed ms': ch.SPEED,
  'velocity': ch.SPEED,

  // Throttle (pedal position)
  'throttle pos': ch.THROTTLE,
  'throttle': ch.THROTTLE,
  'throt': ch.THROTTLE,
  'rthrottle': ch.THROTTLE,
  'athrottle': ch.THROTTLE,
  'accel pos': ch.THROTTLE,
  'accel pedal pos': ch.THROTTLE,
  'throttle filtered': ch.THROTTLE_FILTERED,

  // Brake (pedal position)
  'brake pos': ch.BRAKE,
  'brake': ch.BRAKE,
  'rbrake': ch.BRAKE,
  'abrake': ch.BRAKE,
  'brakepos': ch.BRAKE,
  'brake pedal pos': ch.BRAKE,
  'brake filtered': ch.BRAKE_FILTERED,

  // Clutch
  'clutch pos': ch.CLUTCH,
  'clutch': ch.CLUTCH,
  'rclutch': ch.CLUTCH,
  'aclutch': ch.CLUTCH,

  // Steering wheel angle
  'steering angle': ch.STEERING_ANGLE,
  'steer angle': ch.STEERING_ANGLE,
  'steered angle': ch.STEERING_ANGLE,
  'str angle': ch.STEERING_ANGLE,
  'steering wheel angle': ch.STEERING_ANGLE,
  'steering pos': ch.STEERING_ANGLE,
  'steering': ch.STEERING_ANGLE,
  'steer': ch.STEERING_ANGLE,
  'asteer': ch.STEERING_ANGLE,

  // Engine
  'engine rpm': ch.RPM,
  'rpm': ch.RPM,
  'revs': ch.RPM,
  'nengine': ch.RPM,
  'engine speed': ch.RPM,
  'eng rpm': ch.RPM,
  'engine torque': ch.ENGINE_TORQUE,
  'torque': ch.ENGINE_TORQUE,
  'gear': ch.GEAR,
  'ngear': ch.GEAR,
  'current gear': ch.GEAR,

  // Lap / track position
  'lap distance': ch.LAP_DISTANCE,
  'lap dist': ch.LAP_DISTANCE,
  'lapdist': ch.LAP_DISTANCE,
  'slap': ch.LAP_DISTANCE,
  'distance in lap': ch.LAP_DISTANCE,
  'lap time': ch.LAP_TIME,
  'laptime': ch.LAP_TIME,
  'lap number': ch.LAP_NUMBER,
  'lap': ch.LAP_NUMBER,
  'lap count': ch.LAP_NUMBER,
  'current lap': ch.LAP_NUMBER,
  'lapnum': ch.LAP_NUMBER,
  'mlap': ch.LAP_NUMBER,

  // Tyre pressures
  'tyre pressure fl': ch.TYRE_PRESSURE_FL,
  'tyrespressure fl': ch.TYRE_PRESSURE_FL,
  'tyre press fl': ch.TYRE_PRESSURE_FL,
  'tire press fl': ch.TYRE_PRESSURE_FL,
  'fl press': ch.TYRE_PRESSURE_FL,
  'tyre pressure fr': ch.TYRE_PRESSURE_FR,
  'tyrespressure fr': ch.TYRE_PRESSURE_FR,
  'tyre press fr': ch.TYRE_PRESSURE_FR,
  'tire press fr': ch.TYRE_PRESSURE_FR,
  'fr press': ch.TYRE_PRESSURE_FR,
  'tyre pressure rl': ch.TYRE_PRESSURE_RL,
  'tyrespressure rl': ch.TYRE_PRESSURE_RL,
  'tyre press rl': ch.TYRE_PRESSURE_RL,
  'tire press rl': ch.TYRE_PRESSURE_RL,
  'rl press': ch.TYRE_PRESSURE_RL,
  'tyre pressure rr': ch.TYRE_PRESSURE_RR,
  'tyrespressure rr': ch.TYRE_PRESSURE_RR,
  'tyre press rr': ch.TYRE_PRESSURE_RR,
  'tire press rr': ch.TYRE_PRESSURE_RR,
  'rr press': ch.TYRE_PRESSURE_RR,

  // Brake temperatures
  'brake temp fl': ch.BRAKE_TEMP_FL,
  'fl brake temp': ch.BRAKE_TEMP_FL,
  'disc temp fl': ch.BRAKE_TEMP_FL,
  'brake temp fr': ch.BRAKE_TEMP_FR,
  'fr brake temp': ch.BRAKE_TEMP_FR,
  'disc temp fr': ch.BRAKE_TEMP_FR,
  'brake temp rl': ch.BRAKE_TEMP_RL,
  'rl brake temp': ch.BRAKE_TEMP_RL,
  'disc temp rl': ch.BRAKE_TEMP_RL,
  'brake temp rr': ch.BRAKE_TEMP_RR,
  'rr brake temp': ch.BRAKE_TEMP_RR,
  'disc temp rr': ch.BRAKE_TEMP_RR,

  // Tyre temperatures inner / core / outer
  'tyre temp fl inner': ch.TYRE_TEMP_INNER_FL,
  'tyre temp fl core': ch.TYRE_TEMP_MIDDLE_FL,
  'tyre temp fl outer': ch.TYRE_TEMP_OUTER_FL,
  'tire temp fl inner': ch.TYRE_TEMP_INNER_FL,
  'tire temp fl core': ch.TYRE_TEMP_MIDDLE_FL,
  'tire temp fl outer': ch.TYRE_TEMP_OUTER_FL,
  'tyre temp fr inner': ch.TYRE_TEMP_INNER_FR,
  'tyre temp fr core': ch.TYRE_TEMP_MIDDLE_FR,
  'tyre temp fr outer': ch.TYRE_TEMP_OUTER_FR,
  'tire temp fr inner': ch.TYRE_TEMP_INNER_FR,
  'tire temp fr core': ch.TYRE_TEMP_MIDDLE_FR,
  'tire temp fr outer': ch.TYRE_TEMP_OUTER_FR,
  'tyre temp rl inner': ch.TYRE_TEMP_INNER_RL,
  'tyre temp rl core': ch.TYRE_TEMP_MIDDLE_RL,
  'tyre temp rl outer': ch.TYRE_TEMP_OUTER_RL,
  'tire temp rl inner': ch.TYRE_TEMP_INNER_RL,
  'tire temp rl core': ch.TYRE_TEMP_MIDDLE_RL,
  'tire temp rl outer': ch.TYRE_TEMP_OUTER_RL,
  'tyre temp rr inner': ch.TYRE_TEMP_INNER_RR,
  'tyre temp rr core': ch.TYRE_TEMP_MIDDLE_RR,
  'tyre temp rr outer': ch.TYRE_TEMP_OUTER_RR,
  'tire temp rr inner': ch.TYRE_TEMP_INNER_RR,
  'tire temp rr core': ch.TYRE_TEMP_MIDDLE_RR,
  'tire temp rr outer': ch.TYRE_TEMP_OUTER_RR,
};

/** Registry channel for a MoTeC channel name, or undefined. Units are checked separately (`motecUnitConverter`). */
export function matchChannelNameToAppId(name: string): number | undefined {
  const norm = name.trim().toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
  const direct = MOTEC_NAME_TO_CH_ID[norm];
  if (direct !== undefined) return direct;
  return MOTEC_NAME_TO_CH_ID[norm.replace(/\s+/g, '')];
}

/**
 * Normalises a unit string: lower case, degree sign -> "deg", superscript two -> "2", spaces and other
 * punctuation removed (keeps "/", "%", "."). Bytes are read one per character, so a UTF-8 "°" arrives as "Â°".
 */
export function normaliseUnit(unit: string): string {
  return unit
    .replace(/Â(?=[°²])/g, '')
    .replace(/°/g, 'deg')
    .replace(/²/g, '2')
    .toLowerCase()
    .replace(/[^a-z0-9/%.]+/g, '');
}

type Converter = (value: number) => number;

const identity: Converter = (v) => v;
const scaleBy =
  (factor: number): Converter =>
  (v) =>
    v * factor;

/** Accepted source units per registry unit. A unit not listed is incompatible (mapping dropped). */
const CONVERSIONS: Partial<Record<Unit, Readonly<Record<string, Converter>>>> = {
  MetrePerSecond: {
    'm/s': identity,
    mps: identity,
    'km/h': scaleBy(1 / 3.6),
    kmh: scaleBy(1 / 3.6),
    kph: scaleBy(1 / 3.6),
    mph: scaleBy(0.44704),
  },
  Fraction: {
    '%': scaleBy(0.01),
    frac: identity,
    fraction: identity,
  },
  Pascal: {
    pa: identity,
    kpa: scaleBy(1000),
    mbar: scaleBy(100),
    bar: scaleBy(100_000),
    psi: scaleBy(6894.757293168),
  },
  Celsius: {
    c: identity,
    degc: identity,
    degreesc: identity,
    f: (v) => ((v - 32) * 5) / 9,
    degf: (v) => ((v - 32) * 5) / 9,
    k: (v) => v - 273.15,
  },
  Radian: {
    rad: identity,
    deg: scaleBy(Math.PI / 180),
    degrees: scaleBy(Math.PI / 180),
  },
  Rpm: {
    rpm: identity,
    'r/min': identity,
    '1/min': identity,
  },
  NewtonMetre: {
    nm: identity,
    'n.m': identity,
    lbft: scaleBy(1.3558179483314),
    ftlb: scaleBy(1.3558179483314),
  },
  Enum: { '': identity, gear: identity },
  Count: { '': identity, lap: identity, laps: identity },
  Metre: { m: identity, km: scaleBy(1000) },
  Second: { s: identity, sec: identity, ms: scaleBy(0.001) },
};

/** Registry unit of a channel id (e.g. 'MetrePerSecond'), or undefined for an unknown id. */
export function registryUnit(appChannelId: number): Unit | undefined {
  return CHANNEL_BY_ID.get(appChannelId)?.unit;
}

/** Registry unit symbol of a channel id ('' when it has none). */
export function registryUnitSymbol(appChannelId: number): string {
  return CHANNEL_BY_ID.get(appChannelId)?.unitSymbol ?? '';
}

/**
 * Converter from a MoTeC unit string to the registry unit of `appChannelId`, or null when the unit is not a
 * known compatible unit (the mapping must then be dropped).
 */
export function motecUnitConverter(appChannelId: number, motecUnit: string): Converter | null {
  const target = registryUnit(appChannelId);
  if (target === undefined) return null;
  const table = CONVERSIONS[target];
  if (table === undefined) return null;
  return table[normaliseUnit(motecUnit)] ?? null;
}
