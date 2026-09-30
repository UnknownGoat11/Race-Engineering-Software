/**
 * DISPLAY unit conversion and formatting — the single place in the UI where this happens
 * (docs/ARCHITECTURE.md §4 "Rendered" stage).
 *
 * Inputs are canonical internal units as defined by `telemetry-model::units` (SI):
 *   speed m/s · pressure Pa · temperature °C · fuel L · fuel per lap L/lap · time s.
 * All other conversions (raw LMU units → canonical) live in the Rust `lmu-normalizer` crate.
 *
 * Rules:
 * - Missing / invalid values (null, undefined, NaN, ±Infinity) render as NO_VALUE ("—"). A missing value is
 *   never shown as 0.
 * - Rounding is half away from zero and is performed on a decimal representation, so binary floating point
 *   artefacts (e.g. 1.005 → "1.00") do not leak into the display.
 * - A value that rounds to zero is never shown with a minus sign ("-0", "-0.00").
 * - Formatters return the value only; pass `{ withUnit: true }` to append the unit label.
 */

/** Placeholder rendered for a missing or invalid value (U+2014 EM DASH). */
export const NO_VALUE = '—';

/** Canonical → display unit labels. */
export const UNIT_LABEL = {
  speed: 'km/h',
  pressure: 'bar',
  temperature: '°C',
  fuel: 'L',
  fuelPerLap: 'L/lap',
  time: 's',
} as const;

export type MaybeNumber = number | null | undefined;

export interface FormatOptions {
  /** Append the display unit label, separated by a single space. */
  readonly withUnit?: boolean;
}

const MPS_TO_KMH = 3.6;
const PA_PER_BAR = 100_000;

/** Narrowing guard: true only for finite numbers. */
export function isFiniteNumber(value: MaybeNumber): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/** Metres per second → kilometres per hour (unrounded). */
export function mpsToKmh(mps: number): number {
  return mps * MPS_TO_KMH;
}

/** Pascal → bar (unrounded). */
export function paToBar(pa: number): number {
  return pa / PA_PER_BAR;
}

/**
 * Round |value| to `decimals` places, half away from zero, returning the magnitude as an integer count of
 * 10^-decimals units. `toPrecision(15)` removes binary representation error before rounding.
 */
function roundedMagnitude(value: number, decimals: number): number {
  const scaled = Math.abs(value) * 10 ** decimals;
  return Math.round(Number(scaled.toPrecision(15)));
}

/** Format an integer count of 10^-decimals units as a fixed-point string (no sign). */
function fixedFromUnits(units: number, decimals: number): string {
  if (decimals === 0) {
    return String(units);
  }
  const digits = String(units).padStart(decimals + 1, '0');
  return `${digits.slice(0, -decimals)}.${digits.slice(-decimals)}`;
}

type SignMode = 'negative-only' | 'always';

function formatFixed(value: number, decimals: number, sign: SignMode): string {
  const units = roundedMagnitude(value, decimals);
  const body = fixedFromUnits(units, decimals);
  if (units === 0) {
    return sign === 'always' ? `+${body}` : body;
  }
  if (value < 0) {
    return `-${body}`;
  }
  return sign === 'always' ? `+${body}` : body;
}

function withUnit(text: string, unit: string, options: FormatOptions | undefined): string {
  return options?.withUnit === true ? `${text} ${unit}` : text;
}

/** Speed: m/s → km/h, integer. `83.5` → `"301"`. */
export function formatSpeed(mps: MaybeNumber, options?: FormatOptions): string {
  if (!isFiniteNumber(mps)) return NO_VALUE;
  return withUnit(formatFixed(mpsToKmh(mps), 0, 'negative-only'), UNIT_LABEL.speed, options);
}

/** Pressure: Pa → bar, 2 decimals. `182_300` → `"1.82"`. */
export function formatPressure(pa: MaybeNumber, options?: FormatOptions): string {
  if (!isFiniteNumber(pa)) return NO_VALUE;
  return withUnit(formatFixed(paToBar(pa), 2, 'negative-only'), UNIT_LABEL.pressure, options);
}

/** Temperature: °C, integer. `87.5` → `"88"`. */
export function formatTemperature(celsius: MaybeNumber, options?: FormatOptions): string {
  if (!isFiniteNumber(celsius)) return NO_VALUE;
  return withUnit(formatFixed(celsius, 0, 'negative-only'), UNIT_LABEL.temperature, options);
}

/** Fuel quantity: litres, 1 decimal. `45.67` → `"45.7"`. */
export function formatFuel(litres: MaybeNumber, options?: FormatOptions): string {
  if (!isFiniteNumber(litres)) return NO_VALUE;
  return withUnit(formatFixed(litres, 1, 'negative-only'), UNIT_LABEL.fuel, options);
}

/** Fuel consumption: litres per lap, 2 decimals. `3.456` → `"3.46"`. */
export function formatFuelPerLap(litresPerLap: MaybeNumber, options?: FormatOptions): string {
  if (!isFiniteNumber(litresPerLap)) return NO_VALUE;
  return withUnit(formatFixed(litresPerLap, 2, 'negative-only'), UNIT_LABEL.fuelPerLap, options);
}

/** Lap-time delta: seconds, always signed, 3 decimals. `-0.183` → `"-0.183"`, `0.421` → `"+0.421"`. */
export function formatDelta(seconds: MaybeNumber, options?: FormatOptions): string {
  if (!isFiniteNumber(seconds)) return NO_VALUE;
  return withUnit(formatFixed(seconds, 3, 'always'), UNIT_LABEL.time, options);
}

/** Gap to another car: seconds, always signed, 3 decimals. `1.234` → `"+1.234"`. */
export function formatGap(seconds: MaybeNumber, options?: FormatOptions): string {
  if (!isFiniteNumber(seconds)) return NO_VALUE;
  return withUnit(formatFixed(seconds, 3, 'always'), UNIT_LABEL.time, options);
}

/**
 * Lap time: seconds → `m:ss.mmm`. `83.456` → `"1:23.456"`. Minutes are not capped (`62:03.500`).
 * A lap time that is not strictly positive is not a valid lap time and renders as NO_VALUE.
 */
export function formatLapTime(seconds: MaybeNumber): string {
  if (!isFiniteNumber(seconds) || seconds <= 0) return NO_VALUE;
  const totalMs = roundedMagnitude(seconds, 3);
  const minutes = Math.floor(totalMs / 60_000);
  const remainderMs = totalMs - minutes * 60_000;
  const secs = Math.floor(remainderMs / 1000);
  const ms = remainderMs - secs * 1000;
  return `${minutes}:${String(secs).padStart(2, '0')}.${String(ms).padStart(3, '0')}`;
}
