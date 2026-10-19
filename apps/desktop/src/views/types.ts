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
  | 'lap-comparison'
  | 'multilap-telemetry'
  | 'multihistogram'
  | 'multiplotter'
  // Category: Timing, Stints & Statistics
  | 'laptime-data'
  | 'stints-data'
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

export interface TyreSectionTemperatures {
  readonly outerLeftC: number;
  readonly coreC: number;
  readonly outerRightC: number;
  readonly surfaceC: number;
  readonly carcassC: number;
  readonly pressureKpa: number;
  readonly wearFraction: number; // 0.0 to 1.0 (1.0 = brand new)
  readonly brakeDiscTempC: number;
  readonly brakePeakTempC: number;
  readonly slipPercentage: number;
  readonly lockupActive: boolean;
}

export interface VehicleDynamicsState {
  readonly brakeFrac: number;
  readonly throttleFrac: number;
  readonly rpm: number;
  readonly rpmMax: number;
  readonly torqueNm: number;
  readonly gear: number; // 0 = N, -1 = R, 1-7
  readonly speedKmh: number;
  readonly clutchFrac: number;
  readonly fuelL: number;
  readonly fuelCapacityL: number;
  readonly batterySocFrac: number;
  readonly regenKw: number;
  readonly latG: number;
  readonly longG: number;
  readonly steerAngleDeg: number;
  readonly engineWaterTempC: number;
  readonly engineOilTempC: number;
  readonly clutchRpm: number;
  readonly tcLevel: number;
  readonly tcCut: number;
  readonly tcSlip: number;
  readonly absLevel: number;
  readonly brakeBiasFrontFrac: number;
  readonly pitLimiterActive: boolean;
  readonly ignitionActive: boolean;
  readonly eBoostState: 'READY' | 'DEPLOYING' | 'DEPLETED' | 'DISABLED';
  readonly engineMap: number;
  readonly wheels: {
    readonly fl: TyreSectionTemperatures;
    readonly fr: TyreSectionTemperatures;
    readonly rl: TyreSectionTemperatures;
    readonly rr: TyreSectionTemperatures;
  };
}
