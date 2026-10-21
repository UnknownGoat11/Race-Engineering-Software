/**
 * MoTeC i2 Pro Logged Data (.ld) Format Types and Structures.
 *
 * MoTeC .ld files are the motorsport industry standard telemetry binary format.
 *
 * Owner rule: a value the file does not contain is `null` (or NaN inside a sample column), never an invented
 * default. The UI renders those as "no data".
 */

/** Sample encodings supported by the parser. Any other (dtype_a, dtype) pair is rejected. */
export type MotecDataType = 'f16' | 'f32' | 'f64' | 'i16' | 'i32';

export interface MotecHeader {
  /** Header strings; null when the field is empty in the file. */
  readonly driver: string | null;
  readonly vehicleId: string | null;
  readonly venue: string | null;
  readonly session: string | null;
  readonly shortComment: string | null;
  /** Date and time text exactly as stored, or null when both are empty. */
  readonly dateTimeStr: string | null;
  /** Local logging time parsed from `dd/mm/yyyy hh:mm:ss`, or null when absent or unparseable. */
  readonly timestampMs: number | null;
  /** Channel count declared in the header. */
  readonly numChannels: number;
}

export interface MotecChannelMeta {
  readonly id: number;
  readonly name: string;
  readonly shortName: string;
  /** Unit text as stored in the file ('' when the file has none). */
  readonly unit: string;
  readonly frequencyHz: number;
  /** Samples actually decoded from the file (may be less than declared when the file is truncated). */
  readonly sampleCount: number;
  /** Sample count declared in the channel header. */
  readonly declaredSampleCount: number;
  readonly scale: number;
  readonly shift: number;
  readonly mul: number;
  readonly decPlaces: number;
  readonly dataType: MotecDataType;
  readonly dataPtr: number;
}

export interface MotecChannelData {
  readonly meta: MotecChannelMeta;
  /**
   * In `ParsedMotecFile.channels`: every decoded sample of the whole file at the channel's own rate, in the
   * engineering unit of the file (`meta.unit`). In `channelsByAppId`: resampled by time onto the view's timeline
   * and converted to the registry (SI) unit of `appChannelId`. NaN = no valid value.
   */
  readonly samples: Float64Array;
  /** Matching canonical application channel ID, if mapped (only set when the unit could be converted). */
  readonly appChannelId?: number | undefined;
}

export interface MotecLap {
  readonly lapNumber: number;
  readonly lapTimeS: number;
  readonly startTimeS: number;
  readonly endTimeS: number;
  /** Distance at the lap start / end, or null when the file has no usable distance or speed channel. */
  readonly startDistM: number | null;
  readonly endDistM: number | null;
  /** Inclusive sample range on the file's primary timeline. */
  readonly startSampleIdx: number;
  readonly endSampleIdx: number;
  /** True only when both ends are real lap boundaries (not the start or end of the file). */
  readonly isComplete: boolean;
  /** Complete, at least 25 s and (when distance is known) within 5 % of the median lap distance. */
  readonly isValid: boolean;
  /** Why the lap is not valid, or null. */
  readonly invalidReason: string | null;
}

/** One lap (or the whole file) on a common timeline, ready for graphs and comparison. */
export interface MotecLapView {
  /** The lap shown, or null when the view covers the whole file. */
  readonly lap: MotecLap | null;
  /** Mapped channels resampled onto `timeTimelineS`, in registry (SI) units. */
  readonly channelsByAppId: ReadonlyMap<number, MotecChannelData>;
  /** Duration in seconds. */
  readonly durationS: number;
  /** Distance covered in metres, or null when distance is unavailable. */
  readonly maxDistanceM: number | null;
  /** Distance (m) per sample starting at 0, or null when the file has no lap distance or speed channel. */
  readonly distanceTimelineM: Float64Array | null;
  /** Elapsed seconds per sample, starting at 0. */
  readonly timeTimelineS: Float64Array;
}

/** Raw whole-file data kept so any lap can be extracted later (see `extractMotecLap`). */
export interface MotecSessionData {
  /** Highest channel rate in the file; the common timeline runs at this rate. */
  readonly primaryFrequencyHz: number;
  /** Samples on the common timeline. */
  readonly sampleCount: number;
  /** Mapped channels: raw file-rate samples already converted to registry units. */
  readonly appChannels: ReadonlyMap<number, MotecChannelData>;
  /** Whole-file distance on the common timeline (m), or null when unavailable. */
  readonly distanceTimelineM: Float64Array | null;
  /** Where the distance comes from. */
  readonly distanceSource: 'lap-distance' | 'integrated-speed' | null;
}

export interface ParsedMotecFile extends MotecLapView {
  readonly filename: string;
  readonly header: MotecHeader;
  /** Every decoded channel by lower-case name: whole file, own rate, file units. */
  readonly channels: ReadonlyMap<string, MotecChannelData>;
  /** 'fastest-lap' when a valid lap was found, otherwise 'whole-file'. */
  readonly scope: 'fastest-lap' | 'whole-file';
  /** All lap segments found in the file, including partial out/in laps (flagged `isComplete: false`). */
  readonly laps: readonly MotecLap[];
  /** The quickest valid lap, or undefined when no valid lap exists. */
  readonly fastestLap?: MotecLap | undefined;
  /** How lap boundaries were found, or null when none were found. */
  readonly lapSource: 'ldx' | 'lap-number' | 'lap-distance' | 'beacon' | null;
  /** Highest channel sample rate in the file (Hz). */
  readonly primaryFrequencyHz: number;
  /** Total raw file duration in seconds. */
  readonly rawSessionDurationS: number;
  /** Total raw file distance in metres, or null when unavailable. */
  readonly rawSessionDistanceM: number | null;
  /** Non-fatal problems found while reading (truncated channels, dropped mappings, …). */
  readonly warnings: readonly string[];
  readonly session: MotecSessionData;
}
