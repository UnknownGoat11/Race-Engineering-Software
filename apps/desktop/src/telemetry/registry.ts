/**
 * Lookups over the generated channel table (`src/generated/channels.ts`) that the generated file does not
 * provide: O(1) registry index by id and the `ChannelDef` helper predicates of `telemetry-model::channel`.
 */
import { Availability, CHANNELS, Quality, ValueType, type ChannelDef } from '../generated/channels';

/** Number of channels in this build's registry. */
export const CHANNEL_COUNT = CHANNELS.length;

/** Marks an id that is not in the registry in the dense index table. */
const NO_INDEX = -1;

/** Dense id → registry index table (ids are u16 and sparse, at most a few thousand today). */
const INDEX_BY_ID: Int32Array = (() => {
  let maxId = 0;
  for (const def of CHANNELS) {
    maxId = Math.max(maxId, def.id);
  }
  const table = new Int32Array(maxId + 1).fill(NO_INDEX);
  CHANNELS.forEach((def, index) => {
    table[def.id] = index;
  });
  return table;
})();

/** Registry index (position in `CHANNELS`) of `id`, or -1 if this build does not know the id. */
export function channelIndex(id: number): number {
  return INDEX_BY_ID[id] ?? NO_INDEX;
}

/** Definition of `id`, or undefined if this build does not know the id. */
export function channelDef(id: number): ChannelDef | undefined {
  return CHANNELS[channelIndex(id)];
}

/** `ChannelDef::is_numeric`: true if the value is numeric (graphable). Strings and NONE are not. */
export function isNumericChannel(def: ChannelDef): boolean {
  return def.valueType !== ValueType.Str && def.valueType !== ValueType.None;
}

/** `ChannelDef::is_float`: true if the value is a floating-point number. */
export function isFloatChannel(def: ChannelDef): boolean {
  return def.valueType === ValueType.F32 || def.valueType === ValueType.F64;
}

/**
 * `ChannelDef::default_quality`: the quality a fresh frame carries for a channel — NOT_SUPPORTED for
 * channels no source provides, otherwise UNAVAILABLE. Never a value.
 */
export function defaultQuality(def: ChannelDef): Quality {
  return def.availability === Availability.NotProvided ? Quality.NotSupported : Quality.Unavailable;
}
