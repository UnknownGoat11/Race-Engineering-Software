/**
 * Mathematical coordinate transforms for track map rendering (spec §23–24, §155).
 * Directly matches `crates/race-engines/src/track.rs`.
 */
import type { MapBounds, MapPoint, MapTransform } from './types';

/**
 * Computes axis-aligned bounds [minX, maxX, minZ, maxZ] of points.
 * Returns null if points array is empty or contains no finite coordinates.
 */
export function computeBounds(points: readonly { readonly x: number; readonly z: number }[]): MapBounds | null {
  if (points.length === 0) {
    return null;
  }
  let minX = Infinity;
  let maxX = -Infinity;
  let minZ = Infinity;
  let maxZ = -Infinity;
  let hasValid = false;

  for (const p of points) {
    if (Number.isFinite(p.x) && Number.isFinite(p.z)) {
      if (p.x < minX) minX = p.x;
      if (p.x > maxX) maxX = p.x;
      if (p.z < minZ) minZ = p.z;
      if (p.z > maxZ) maxZ = p.z;
      hasValid = true;
    }
  }

  if (!hasValid) {
    return null;
  }

  return { minX, maxX, minZ, maxZ };
}

/**
 * Fits circuit geometry into a viewport of `width x height` with `padding`.
 * Supports counter-clockwise rotation in radians.
 */
export function fitMapTransform(
  bounds: MapBounds,
  points: readonly MapPoint[],
  width: number,
  height: number,
  padding = 24,
  rotationRad = 0,
): MapTransform | null {
  const w = width - 2 * padding;
  const h = height - 2 * padding;
  if (w <= 0 || h <= 0 || !Number.isFinite(rotationRad)) {
    return null;
  }

  const cx = 0.5 * (bounds.minX + bounds.maxX);
  const cz = 0.5 * (bounds.minZ + bounds.maxZ);
  const cos = Math.cos(rotationRad);
  const sin = Math.sin(rotationRad);

  let maxU = 0;
  let maxV = 0;

  for (const p of points) {
    if (Number.isFinite(p.x) && Number.isFinite(p.z)) {
      const dx = p.x - cx;
      const dz = p.z - cz;
      const ru = Math.abs(dx * cos - dz * sin);
      const rv = Math.abs(dx * sin + dz * cos);
      if (ru > maxU) maxU = ru;
      if (rv > maxV) maxV = rv;
    }
  }

  if (maxU <= 0 || maxV <= 0) {
    return null;
  }

  const scale = Math.min((0.5 * w) / maxU, (0.5 * h) / maxV);
  if (!Number.isFinite(scale) || scale <= 0) {
    return null;
  }

  return {
    scale,
    rotationRad,
    centreX: cx,
    centreZ: cz,
    offsetU: 0.5 * width,
    offsetV: 0.5 * height,
  };
}

/**
 * Projects world coordinates (x, z) to screen pixel coordinates (u, v).
 * Screen right = +x, Screen up = +z, Screen v is downwards.
 */
export function applyTransform(transform: MapTransform, x: number, z: number): [number, number] {
  const cos = Math.cos(transform.rotationRad);
  const sin = Math.sin(transform.rotationRad);
  const dx = x - transform.centreX;
  const dz = z - transform.centreZ;
  const ru = dx * cos - dz * sin;
  const rv = dx * sin + dz * cos;
  return [
    transform.offsetU + transform.scale * ru,
    transform.offsetV - transform.scale * rv,
  ];
}

/**
 * Inverts screen pixel coordinates (u, v) back to world coordinates (x, z).
 */
export function inverseTransform(transform: MapTransform, u: number, v: number): [number, number] {
  const cos = Math.cos(transform.rotationRad);
  const sin = Math.sin(transform.rotationRad);
  const ru = (u - transform.offsetU) / transform.scale;
  const rv = (transform.offsetV - v) / transform.scale;
  const dx = ru * cos + rv * sin;
  const dz = -ru * sin + rv * cos;
  return [transform.centreX + dx, transform.centreZ + dz];
}
