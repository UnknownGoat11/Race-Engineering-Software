/**
 * Vehicle class helpers (REDESIGN.md C1, owner approval 2026-10-18).
 *
 * Class hues come from bands no status colour uses: Hypercar cyan, LMP2 blue, LMGT3 / LMGTE lime, unknown grey.
 * Red and amber stay reserved for alerts. Class colour appears only as a row stripe, a map dot and a 3-char
 * chip, never as text colour or a fill behind values. The colours themselves live in tokens.css
 * (`--class-*`); this module only names the token.
 */

/** Vehicle classes the UI distinguishes. Structurally identical to `components/trackmap/types` VehicleClass. */
export type VehicleClass = 'hypercar' | 'lmp2' | 'gt3' | 'gte' | 'unknown';

export const VEHICLE_CLASSES: readonly VehicleClass[] = ['hypercar', 'lmp2', 'gt3', 'gte', 'unknown'];

/**
 * Parses an LMU class name (`mVehicleClass`, e.g. "Hypercar", "Hyper", "Hypercar H1", "LMP2", "LMP2_ELMS",
 * "GT3", "LMGT3", "GT3 G3", "GTE", "LMGTE"). Case, spaces, dashes and underscores are ignored. Anything else,
 * including LMP3 (no approved colour), is 'unknown'.
 */
export function parseVehicleClass(name: string | null | undefined): VehicleClass {
  if (name === null || name === undefined) return 'unknown';
  const key = name.toUpperCase().replace(/[^A-Z0-9]/g, '');
  if (key === '') return 'unknown';
  if (key.startsWith('HYPER') || key === 'LMH' || key === 'LMDH') return 'hypercar';
  if (key.startsWith('LMP2')) return 'lmp2';
  if (key.startsWith('LMGT3') || key.startsWith('GT3')) return 'gt3';
  if (key.startsWith('LMGTE') || key.startsWith('GTE')) return 'gte';
  return 'unknown';
}

const CLASS_CODES: Readonly<Record<VehicleClass, string>> = {
  hypercar: 'HY',
  lmp2: 'P2',
  gt3: 'GT3',
  gte: 'GTE',
  unknown: '—',
};

const CLASS_NAMES: Readonly<Record<VehicleClass, string>> = {
  hypercar: 'Hypercar',
  lmp2: 'LMP2',
  gt3: 'LMGT3',
  gte: 'LMGTE',
  unknown: 'Unknown class',
};

/** Accepts a parsed class or a raw LMU class name. */
function toClass(value: string | null | undefined): VehicleClass {
  return VEHICLE_CLASSES.find((c) => c === value) ?? parseVehicleClass(value);
}

/** 3-char chip text: HY / P2 / GT3 / GTE, or "—" when unknown. */
export function vehicleClassCode(value: string | null | undefined): string {
  return CLASS_CODES[toClass(value)];
}

/** Full class name for tooltips and accessible names. */
export function vehicleClassName(value: string | null | undefined): string {
  return CLASS_NAMES[toClass(value)];
}

/** CSS custom property name of the class colour (defined in tokens.css). */
export function classColorToken(value: string | null | undefined): `--class-${VehicleClass}` {
  return `--class-${toClass(value)}`;
}

/** `var(--class-…)` for inline SVG fills / CSS custom properties. */
export function classColorVar(value: string | null | undefined): string {
  return `var(${classColorToken(value)})`;
}
