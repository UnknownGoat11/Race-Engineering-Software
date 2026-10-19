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
  readonly flatspotPercent?: number; // 0..100%
  readonly flatspotSeverity?: 'NONE' | 'MILD' | 'MODERATE' | 'SEVERE';
  readonly lastLockupCorner?: string;
}

export interface LockupEvent {
  readonly id: string;
  readonly timestampMs: number;
  readonly lapNumber: number;
  readonly cornerName: string;
  readonly wheel: 'FL' | 'FR' | 'RL' | 'RR';
  readonly durationMs: number;
  readonly peakSlipPct: number;
  readonly severity: 'MILD' | 'MODERATE' | 'SEVERE';
  readonly flatspotDamagePct: number;
  readonly estimatedVibrationIndex: number; // 0..10
}

export interface TyreDegradationPoint {
  readonly lapNumber: number;
  readonly wearFl: number; // % remaining (e.g. 94.2)
  readonly wearFr: number;
  readonly wearRl: number;
  readonly wearRr: number;
  readonly gripIndex: number; // 1.0 down to ~0.7
  readonly lapTimeS: number;
}

export interface HypercarVetState {
  /** Maximum Virtual Energy allocated per stint by WEC BoP (e.g. 900.0 MJ). */
  readonly vetCapacityMj: number;
  /** Current Virtual Energy remaining in the stint (MJ). */
  readonly vetRemainingMj: number;
  /** Stint target energy consumption per lap (e.g. 26.50 MJ/lap). */
  readonly targetPerLapMj: number;
  /** Actual energy consumed on the most recent completed lap (MJ). */
  readonly actualLastLapMj: number;
  /** Delta vs target: negative = under target (saving energy / push allowed), positive = over target. */
  readonly deltaLastLapMj: number;
  /** Projected laps remaining until VET depletion. */
  readonly stintLapsRemaining: number;
  /** Instantaneous MGU-K motor deploy power in kW (0 to 200 kW). */
  readonly mgukDeployKw: number;
  /** Instantaneous regenerative braking power in kW (-150 to 0 kW). */
  readonly mgukRegenKw: number;
  /** Regulatory minimum speed for front-axle MGU-K deployment (e.g. 190 km/h in dry). */
  readonly mgukGateSpeedKmh: number;
  /** True when current ground speed satisfies the activation speed gate. */
  readonly mgukGateActive: boolean;
  /** High-voltage traction battery State of Charge fraction (0.0 to 1.0). */
  readonly batterySocFrac: number;
  /** Battery cell core temperature (°C). */
  readonly batteryTempC: number;
  /** Total regenerative energy harvested during the current lap (MJ). */
  readonly regenHarvestedLapMj: number;
  /** Active hybrid deployment mode. */
  readonly deployMode: 'BUILD' | 'BALANCED' | 'ATTACK' | 'DEFEND' | 'QUAL';
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
  readonly hypercarVet?: HypercarVetState;
  readonly lockupEvents?: readonly LockupEvent[];
  readonly tyreDegradationHistory?: readonly TyreDegradationPoint[];
}
