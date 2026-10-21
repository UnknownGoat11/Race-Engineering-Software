/** Per-wheel live channel sets for the tyre / brake views (registry ids from generated/channels.ts). */
import { ch } from '../generated/channels';
import type { LiveReadings } from './liveData';
import type { WheelReadings, WheelSet } from './types';

interface WheelChannelIds {
  readonly left: number;
  readonly middle: number;
  readonly right: number;
  readonly avg: number;
  readonly carcass: number;
  readonly pressure: number;
  readonly wear: number;
  readonly brake: number;
  readonly brakePeak: number;
}

const FL: WheelChannelIds = {
  left: ch.TYRE_TEMP_LEFT_FL,
  middle: ch.TYRE_TEMP_MIDDLE_FL,
  right: ch.TYRE_TEMP_RIGHT_FL,
  avg: ch.TYRE_TEMP_AVG_FL,
  carcass: ch.TYRE_CARCASS_TEMP_FL,
  pressure: ch.TYRE_PRESSURE_FL,
  wear: ch.TYRE_WEAR_FL,
  brake: ch.BRAKE_TEMP_FL,
  brakePeak: ch.BRAKE_TEMP_PEAK_FL,
};
const FR: WheelChannelIds = {
  left: ch.TYRE_TEMP_LEFT_FR,
  middle: ch.TYRE_TEMP_MIDDLE_FR,
  right: ch.TYRE_TEMP_RIGHT_FR,
  avg: ch.TYRE_TEMP_AVG_FR,
  carcass: ch.TYRE_CARCASS_TEMP_FR,
  pressure: ch.TYRE_PRESSURE_FR,
  wear: ch.TYRE_WEAR_FR,
  brake: ch.BRAKE_TEMP_FR,
  brakePeak: ch.BRAKE_TEMP_PEAK_FR,
};
const RL: WheelChannelIds = {
  left: ch.TYRE_TEMP_LEFT_RL,
  middle: ch.TYRE_TEMP_MIDDLE_RL,
  right: ch.TYRE_TEMP_RIGHT_RL,
  avg: ch.TYRE_TEMP_AVG_RL,
  carcass: ch.TYRE_CARCASS_TEMP_RL,
  pressure: ch.TYRE_PRESSURE_RL,
  wear: ch.TYRE_WEAR_RL,
  brake: ch.BRAKE_TEMP_RL,
  brakePeak: ch.BRAKE_TEMP_PEAK_RL,
};
const RR: WheelChannelIds = {
  left: ch.TYRE_TEMP_LEFT_RR,
  middle: ch.TYRE_TEMP_MIDDLE_RR,
  right: ch.TYRE_TEMP_RIGHT_RR,
  avg: ch.TYRE_TEMP_AVG_RR,
  carcass: ch.TYRE_CARCASS_TEMP_RR,
  pressure: ch.TYRE_PRESSURE_RR,
  wear: ch.TYRE_WEAR_RR,
  brake: ch.BRAKE_TEMP_RR,
  brakePeak: ch.BRAKE_TEMP_PEAK_RR,
};

/** Every channel id `readWheels` needs. */
export const WHEEL_CHANNELS: readonly number[] = [FL, FR, RL, RR].flatMap((w) => Object.values(w) as number[]);

function readWheel(r: LiveReadings, ids: WheelChannelIds): WheelReadings {
  return {
    leftC: r.num(ids.left),
    middleC: r.num(ids.middle),
    rightC: r.num(ids.right),
    surfaceAvgC: r.num(ids.avg),
    carcassC: r.num(ids.carcass),
    pressurePa: r.num(ids.pressure),
    wearRaw: r.num(ids.wear),
    brakeC: r.num(ids.brake),
    brakePeakC: r.num(ids.brakePeak),
  };
}

/** The four wheels' live readings (every field null when its channel has no live value). */
export function readWheels(r: LiveReadings): WheelSet {
  return { fl: readWheel(r, FL), fr: readWheel(r, FR), rl: readWheel(r, RL), rr: readWheel(r, RR) };
}
