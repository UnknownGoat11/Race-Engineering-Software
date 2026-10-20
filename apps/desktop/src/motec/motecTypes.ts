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
  readonly startSampleIdx?: number;
  readonly endSampleIdx?: number;
  readonly isValid: boolean;
}

export interface ParsedMotecFile {
  readonly filename: string;
  readonly header: MotecHeader;
  /** Sliced channel data scoped to the quickest lap for comparison. */
  readonly channels: ReadonlyMap<string, MotecChannelData>;
  /** Sliced channels mapped by canonical app ID for the quickest lap. */
  readonly channelsByAppId: ReadonlyMap<number, MotecChannelData>;
  /** Duration of the quickest lap in seconds (used for comparison). */
  readonly durationS: number;
  /** Distance of the quickest lap in meters (used for comparison). */
  readonly maxDistanceM: number;
  /** Timeline of distance (m) for the quickest lap (starts at 0m). */
  readonly distanceTimelineM: Float64Array;
  /** Timeline of elapsed seconds (s) for the quickest lap (starts at 0s). */
  readonly timeTimelineS: Float64Array;
  /** All laps detected in the session. */
  readonly laps: readonly MotecLap[];
  /** The quickest lap recognized and used for comparison. */
  readonly fastestLap?: MotecLap | undefined;
  /** Total raw session duration across all laps in seconds. */
  readonly rawSessionDurationS?: number;
  /** Total raw session distance across all laps in meters. */
  readonly rawSessionDistanceM?: number;
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
