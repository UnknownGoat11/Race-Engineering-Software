/**
 * Types for the Track Map component and pipeline (spec §23–24, §79, §155–157, §342).
 * Mirrors the definitions in `crates/race-engines/src/track.rs`.
 */

export type TrackMapSource = 'generated' | 'verified';

/** One point along the circuit geometry polyline. */
export interface MapPoint {
  readonly distM: number;
  readonly x: number;
  readonly z: number;
}

/** A marked location on the circuit (start/finish, pit entry, pit exit). */
export interface MapMarker {
  readonly distM: number;
  readonly x: number;
  readonly z: number;
}

/** Start of a sector (sector 0 starts at the start/finish line). */
export interface SectorMark {
  readonly index: number;
  readonly distM: number;
  readonly x: number;
  readonly z: number;
}

/** Axis-aligned world bounds (m) of the circuit. */
export interface MapBounds {
  readonly minX: number;
  readonly maxX: number;
  readonly minZ: number;
  readonly maxZ: number;
}

/** Versioned track map data structure (spec §156). */
export interface TrackMap {
  readonly trackId: string;
  readonly version: number;
  readonly source: TrackMapSource;
  readonly lengthM: number;
  readonly points: readonly MapPoint[];
  readonly sectors: readonly SectorMark[];
  readonly startFinish: MapMarker;
  readonly pitEntry?: MapMarker | null;
  readonly pitExit?: MapMarker | null;
  readonly generatedFromLaps?: number;
  readonly engineVersion?: number;
}

/**
 * World (x, z) to screen (u, v) viewport transform (A-LMU-025).
 * Screen right = +x, Screen up = +z, Screen v is downwards.
 */
export interface MapTransform {
  readonly scale: number;
  readonly rotationRad: number;
  readonly centreX: number;
  readonly centreZ: number;
  readonly offsetU: number;
  readonly offsetV: number;
}

/** Vehicle class category for styling on track maps. */
export type VehicleClass = 'hypercar' | 'lmp2' | 'gt3' | 'gte' | 'unknown';

/** Competitor vehicle marker displayed on the track map. */
export interface CompetitorMarker {
  readonly id: string | number;
  readonly carNumber: string;
  readonly driverName: string;
  readonly vehicleClass: VehicleClass;
  readonly x?: number | null;
  readonly z?: number | null;
  readonly lapDistM?: number | null;
  readonly isDriver?: boolean;
  readonly inPits?: boolean;
}

/** Driver vehicle live position and orientation. */
export interface DriverPosition {
  readonly x: number | null;
  readonly z: number | null;
  readonly headingRad?: number | null;
  readonly lapDistM?: number | null;
  readonly state?: 'live' | 'stale' | 'unavailable';
}

/**
 * Data-quality scope of the whole map (REDESIGN §2.2): stale keeps the last positions with hollow markers and a
 * STALE tag; ended and untrusted dim the map and say so. Other states render normally (the SYNTHETIC pill lives
 * in the pitwall bar, not on the map).
 */
export type TrackMapDq = 'live' | 'stale' | 'unavailable' | 'synthetic' | 'degraded' | 'untrusted' | 'ended';

/** One car on the lap-distance bar (the fallback when no circuit outline is available). */
export interface LapPositionMarker {
  readonly id: string | number;
  /** Car number without '#', or '' when unknown. */
  readonly carNumber: string;
  readonly vehicleClass: VehicleClass;
  /** Distance from the start/finish line (m). */
  readonly lapDistM: number;
  readonly isDriver?: boolean;
  readonly inPits?: boolean;
}

/** Where the track length came from: reported by LMU (TRACK_LENGTH / verified map) or inferred from lap wraps. */
export type TrackLengthSource = 'reported' | 'observed';
