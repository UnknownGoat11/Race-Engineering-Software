/**
 * V1.1 Engineering View Suite Type Definitions.
 *
 * Defines the complete taxonomy of 27 motorsport engineering views matching
 * the user's catalogue, grouped into cohesive functional categories.
 */

export type ViewCategory =
  | 'performance' // Driver DNA, Driving, Lap Comparison, Multilap, Histogram, Scatter
  | 'timing' // Laptime Data, Stints, Statistics, RED/BLUE/SES, Race Info, Time Trial
  | 'multicar' // Multi-car Map, Circle Radar, One-lap Timing, Race Timing, Minisectors, Opponent Stats, Field Spread, Race History, Session Data, Multi-car Telemetry
  | 'engineering'; // Telemetry, Workspace, Raw Data, Minimal UI

export type ViewId =
  // Category: Performance & Driver DNA
  | 'driver-dna'
  | 'driving'
  | 'traction-circle'
  | 'lap-comparison'
  | 'multilap-telemetry'
  | 'multihistogram'
  | 'multiplotter'
  // Category: Timing, Stints & Statistics
  | 'laptime-data'
  | 'stints-data'
  | 'tyre-degradation'
  | 'statistics-table'
  | 'statistics-table-graphs'
  | 'statistics-red-blue'
  | 'race-info'
  | 'time-trial'
  // Category: Field Intelligence & Multi-Car
  | 'multicar-map'
  | 'multicar-circle'
  | 'multicar-one-lap-timing'
  | 'multicar-race-timing'
  | 'multicar-minisector-timing'
  | 'multicar-opponent-stats'
  | 'multicar-race-field-spread'
  | 'multicar-race-history'
  | 'multicar-session-data'
  | 'multicar-telemetry'
  // Category: Engineering Workspaces & Diagnostics
  | 'workspace'
  | 'telemetry'
  | 'hypercar-hybrid'
  | 'raw-data'
  | 'minimal-ui';

export interface ViewDefinition {
  readonly id: ViewId;
  /** Display title matching the catalogue item. */
  readonly title: string;
  /** Functional category for grouping and navigation. */
  readonly category: ViewCategory;
  /** Short summary of the engineering value provided. */
  readonly description: string;
  /** Visual glyph/icon for the view. */
  readonly glyph: string;
  /** Optional badge text (e.g. "NEW", "V1.1", "HOT"). */
  readonly badge?: string;
  /** Section header separator text if applicable. */
  readonly isDivider?: boolean;
}

export type TyreTempZone = 'cold' | 'slight-cold' | 'optimal' | 'getting-warm' | 'warm' | 'hot';

/**
 * Live readings of one wheel for the tyre/brake views, in registry units (°C, Pa, raw wear fraction).
 * Every field is null when the channel has no live value: views render NO_VALUE, never a placeholder.
 */
export interface WheelReadings {
  /** Surface temperature, left edge (car frame), °C (TYRE_TEMP_LEFT_*). */
  readonly leftC: number | null;
  /** Surface temperature, middle, °C (TYRE_TEMP_MIDDLE_*). */
  readonly middleC: number | null;
  /** Surface temperature, right edge (car frame), °C (TYRE_TEMP_RIGHT_*). */
  readonly rightC: number | null;
  /** Mean surface temperature, °C (TYRE_TEMP_AVG_*). */
  readonly surfaceAvgC: number | null;
  /** Carcass temperature, °C (TYRE_CARCASS_TEMP_*). */
  readonly carcassC: number | null;
  /** Tyre pressure, Pa (TYRE_PRESSURE_*). */
  readonly pressurePa: number | null;
  /** Tyre wear as reported by LMU (raw fraction, TYRE_WEAR_*). */
  readonly wearRaw: number | null;
  /** Brake disc temperature, °C (BRAKE_TEMP_*). */
  readonly brakeC: number | null;
  /** Brake disc peak temperature, °C (BRAKE_TEMP_PEAK_*). */
  readonly brakePeakC: number | null;
}

export interface WheelSet {
  readonly fl: WheelReadings;
  readonly fr: WheelReadings;
  readonly rl: WheelReadings;
  readonly rr: WheelReadings;
}
