/**
 * MoTeC i2 Pro Logged Data (.ld) Format Types and Structures.
 *
 * MoTeC .ld files are the motorsport industry standard telemetry binary format.
 */

export type MotecDataType = 'f32' | 'f16' | 'i16' | 'i32' | 'f64' | 'unknown';

export interface MotecHeader {
  readonly driver: string;
  readonly vehicleId: string;
  readonly venue: string;
  readonly session: string;
  readonly shortComment: string;
  readonly dateTimeStr: string;
  readonly timestampMs: number;
  readonly numChannels: number;
}

export interface MotecChannelMeta {
  readonly id: number;
  readonly name: string;
  readonly shortName: string;
  readonly unit: string;
  readonly frequencyHz: number;
  readonly sampleCount: number;
  readonly scale: number;
  readonly shift: number;
  readonly mul: number;
  readonly decPlaces: number;
  readonly dataType: MotecDataType;
  readonly dataPtr: number;
}

export interface MotecChannelData {
  readonly meta: MotecChannelMeta;
  /** Decoded engineering unit values (e.g. km/h, %, rpm, °C). */
  readonly samples: Float64Array;
  /** Matching canonical application channel ID, if mapped. */
  readonly appChannelId?: number | undefined;
}

export interface MotecLapMarker {
  readonly lapNumber: number;
  readonly timeS: number;
  readonly distanceM: number;
}

export interface MotecLap {
  readonly lapNumber: number;
  readonly lapTimeS: number;
  readonly startTimeS: number;
  readonly endTimeS: number;
  readonly startDistM: number;
  readonly endDistM: number;
  readonly isValid: boolean;
}

export interface ParsedMotecFile {
  readonly filename: string;
  readonly header: MotecHeader;
  readonly channels: ReadonlyMap<string, MotecChannelData>;
  readonly channelsByAppId: ReadonlyMap<number, MotecChannelData>;
  /** Total session duration in seconds. */
  readonly durationS: number;
  /** Total lap / track distance in meters. */
  readonly maxDistanceM: number;
  /** Timeline of distance (m) sampled at the primary frequency. */
  readonly distanceTimelineM: Float64Array;
  /** Timeline of elapsed seconds (s) sampled at the primary frequency. */
  readonly timeTimelineS: Float64Array;
  readonly laps: readonly MotecLap[];
  readonly fastestLap?: MotecLap | undefined;
}

export interface MotecDeltaAtDistance {
  readonly distanceM: number;
  readonly motecSpeedKmh: number;
  readonly liveSpeedKmh: number;
  readonly deltaSpeedKmh: number;
  readonly motecThrottlePct: number;
  readonly liveThrottlePct: number;
  readonly motecBrakePct: number;
  readonly liveBrakePct: number;
  readonly deltaTimeS: number;
}
