/**
 * DISPLAY unit conversion and formatting — the single place in the UI where this happens
 * (docs/ARCHITECTURE.md §4 "Rendered" stage).
 *
 * Inputs are canonical internal units as defined by `telemetry-model::units` (SI):
 *   speed m/s · pressure Pa · temperature °C · fuel L · fuel per lap L/lap · time s · angle rad · …
 * All other conversions (raw LMU units → canonical) live in the Rust `lmu-normalizer` crate.
 *
 * Rules:
 * - Missing / invalid values (null, undefined, NaN, ±Infinity) render as NO_VALUE ("—"). A missing value is
 *   never shown as 0.
 * - Rounding is half away from zero and is performed on a decimal representation, so binary floating point
 *   artefacts (e.g. 1.005 → "1.00") do not leak into the display.
 * - A value that rounds to zero is never shown with a minus sign ("-0", "-0.00").
 * - Formatters return the value only; pass `{ withUnit: true }` to append the unit label.
 * - Precision is fixed per unit (or per channel display kind), so digits do not jump as values change
 *   (spec §161, §339); callers may override the decimals ("make format configurable").
 */

import { Unit, ValueType, type ChannelDef } from '../generated/channels';

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

export interface LapTimeOptions {
  /**
   * Accept 0 (`0:00.000`): a running lap timer reads 0 at the line. Off by default, because a completed lap
   * time of 0 is not a lap time.
   */
  readonly allowZero?: boolean;
}

/**
 * Lap time: seconds → `m:ss.mmm`. `83.456` → `"1:23.456"`. Minutes are not capped (`62:03.500`).
 * A negative value, or 0 unless `allowZero`, is not a valid lap time and renders as NO_VALUE.
 */
export function formatLapTime(seconds: MaybeNumber, options?: LapTimeOptions): string {
  if (!isFiniteNumber(seconds) || seconds < 0 || (seconds === 0 && options?.allowZero !== true)) return NO_VALUE;
  const totalMs = roundedMagnitude(seconds, 3);
  const minutes = Math.floor(totalMs / 60_000);
  const remainderMs = totalMs - minutes * 60_000;
  const secs = Math.floor(remainderMs / 1000);
  const ms = remainderMs - secs * 1000;
  return `${minutes}:${String(secs).padStart(2, '0')}.${String(ms).padStart(3, '0')}`;
}

/** Gap to a car one or more laps away (spec §341): `1` → `"+1 LAP"`, `-2` → `"-2 LAPS"`. Whole laps only. */
export function formatLapGap(laps: MaybeNumber): string {
  if (!isFiniteNumber(laps) || !Number.isInteger(laps) || laps === 0) return NO_VALUE;
  const magnitude = Math.abs(laps);
  return `${laps < 0 ? '-' : '+'}${magnitude} ${magnitude === 1 ? 'LAP' : 'LAPS'}`;
}

// ---------------------------------------------------------------------------------------------------------------
// Channel values by canonical unit (generated metadata, src/generated/channels.ts)
// ---------------------------------------------------------------------------------------------------------------

/** A display unit: the label shown next to a value, its spoken name, the conversion and the precision. */
export interface DisplayUnit {
  /** Label shown next to the value ('' for none). */
  readonly label: string;
  /** Spoken unit name for accessible labels ('' for none). */
  readonly name: string;
  /** Default decimals for floating-point values. */
  readonly decimals: number;
  /** Internal (SI) value → display value. */
  readonly toDisplay: (internal: number) => number;
}

const SECONDS_PER_MINUTE = 60;
const SECONDS_PER_HOUR = 3_600;
const SECONDS_PER_DAY = 86_400;
const MS_PER_S = 1_000;
const MM_PER_M = 1_000;
const PERCENT = 100;
const DEG_PER_RAD = 180 / Math.PI;
const W_PER_KW = 1_000;
const J_PER_MJ = 1_000_000;
const BIT_PER_KBIT = 1_000;
const BYTES_PER_KIB = 1_024;
const BYTE_UNITS: readonly (readonly [label: string, name: string])[] = [
  ['B', 'bytes'],
  ['KiB', 'kibibytes'],
  ['MiB', 'mebibytes'],
  ['GiB', 'gibibytes'],
  ['TiB', 'tebibytes'],
];
const BYTE_SCALED_DECIMALS = 1;
const MILLISECOND_DECIMALS = 1;
const MILLIMETRE_DECIMALS = 1;
const AGE_SECONDS_DECIMALS = 1;
const AGE_TENTHS_PER_S = 10 ** AGE_SECONDS_DECIMALS;
/** Significant digits kept before truncating an age, so binary artefacts (3.42 × 10 = 34.1999…) vanish. */
const AGE_TRUNCATION_DIGITS = 15;
const CLOCK_FIELD_DIGITS = 2;

const identity = (v: number): number => v;

/** Default display of every canonical unit (spec §105 display conversions, §339 precision). */
export const DISPLAY_UNITS: Readonly<Record<Unit, DisplayUnit>> = {
  None: { label: '', name: '', decimals: 2, toDisplay: identity },
  Metre: { label: 'm', name: 'metres', decimals: 1, toDisplay: identity },
  MetrePerSecond: { label: UNIT_LABEL.speed, name: 'kilometres per hour', decimals: 0, toDisplay: mpsToKmh },
  MetrePerSecond2: { label: 'm/s²', name: 'metres per second squared', decimals: 1, toDisplay: identity },
  Pascal: { label: UNIT_LABEL.pressure, name: 'bar', decimals: 2, toDisplay: paToBar },
  Celsius: { label: UNIT_LABEL.temperature, name: 'degrees Celsius', decimals: 0, toDisplay: identity },
  Radian: { label: '°', name: 'degrees', decimals: 1, toDisplay: (v) => v * DEG_PER_RAD },
  RadianPerSecond: { label: '°/s', name: 'degrees per second', decimals: 1, toDisplay: (v) => v * DEG_PER_RAD },
  Litre: { label: UNIT_LABEL.fuel, name: 'litres', decimals: 1, toDisplay: identity },
  LitrePerLap: { label: UNIT_LABEL.fuelPerLap, name: 'litres per lap', decimals: 2, toDisplay: identity },
  Second: { label: UNIT_LABEL.time, name: 'seconds', decimals: 3, toDisplay: identity },
  Newton: { label: 'N', name: 'newtons', decimals: 0, toDisplay: identity },
  NewtonMetre: { label: 'N·m', name: 'newton metres', decimals: 0, toDisplay: identity },
  Watt: { label: 'kW', name: 'kilowatts', decimals: 1, toDisplay: (v) => v / W_PER_KW },
  Joule: { label: 'MJ', name: 'megajoules', decimals: 2, toDisplay: (v) => v / J_PER_MJ },
  Rpm: { label: 'rpm', name: 'revolutions per minute', decimals: 0, toDisplay: identity },
  Fraction: { label: '%', name: 'percent', decimals: 0, toDisplay: (v) => v * PERCENT },
  G: { label: 'g', name: 'g', decimals: 2, toDisplay: identity },
  Lap: { label: 'laps', name: 'laps', decimals: 1, toDisplay: identity },
  Count: { label: '', name: '', decimals: 0, toDisplay: identity },
  Enum: { label: '', name: '', decimals: 0, toDisplay: identity },
  Text: { label: '', name: '', decimals: 0, toDisplay: identity },
  Nanosecond: { label: 'ns', name: 'nanoseconds', decimals: 0, toDisplay: identity },
  BitsPerSecond: { label: 'kbit/s', name: 'kilobits per second', decimals: 1, toDisplay: (v) => v / BIT_PER_KBIT },
  Hertz: { label: 'Hz', name: 'hertz', decimals: 1, toDisplay: identity },
  RadianPerSecond2: {
    label: '°/s²',
    name: 'degrees per second squared',
    decimals: 0,
    toDisplay: (v) => v * DEG_PER_RAD,
  },
  FractionPerLap: { label: '%/lap', name: 'percent per lap', decimals: 2, toDisplay: (v) => v * PERCENT },
  // Bytes are scaled by magnitude (B … TiB, see formatBytes); this entry is the unscaled form.
  Byte: { label: 'B', name: 'bytes', decimals: 0, toDisplay: identity },
  KilogramPerCubicMetre: { label: 'kg/m³', name: 'kilograms per cubic metre', decimals: 3, toDisplay: identity },
};

/**
 * How a channel is displayed when its unit alone does not say it (seconds can be a lap time, a delta, a gap,
 * a session clock, a time of day or a latency; metres can be a ride height). Unlisted channels use their unit.
 */
export type DisplayKind =
  | 'lapTime'
  | 'delta'
  | 'gap'
  | 'clock'
  | 'timeOfDay'
  | 'milliseconds'
  | 'millimetres'
  | 'signed';

const WHEELS = ['fl', 'fr', 'rl', 'rr'] as const;

function perWheel(pattern: (wheel: string) => string, kind: DisplayKind): [string, DisplayKind][] {
  return WHEELS.map((wheel) => [pattern(wheel), kind]);
}

/** Display kind by canonical channel name (each name is checked against the generated registry in tests). */
export const CHANNEL_DISPLAY_KINDS: ReadonlyMap<string, DisplayKind> = new Map<string, DisplayKind>([
  // m:ss.mmm (spec §339)
  ['timing.best_lap_s', 'lapTime'],
  ['timing.last_lap_s', 'lapTime'],
  ['timing.estimated_lap_s', 'lapTime'],
  ['timing.predicted_lap_s', 'lapTime'],
  ['lap.current_time_s', 'lapTime'],
  // Always-signed deltas (spec §340)
  ['timing.delta_best_s', 'delta'],
  ['timing.delta_personal_best_s', 'delta'],
  // Racing gap format (spec §341)
  ['timing.gap_car_ahead_s', 'gap'],
  ['timing.gap_car_behind_s', 'gap'],
  ['timing.gap_place_ahead_s', 'gap'],
  ['timing.gap_place_behind_s', 'gap'],
  ['race.time_behind_leader_s', 'gap'],
  ['race.time_behind_next_s', 'gap'],
  // Durations as m:ss / h:mm:ss
  ['session.elapsed_s', 'clock'],
  ['session.end_s', 'clock'],
  ['session.time_remaining_s', 'clock'],
  ['session.time_remaining_calc_s', 'clock'],
  ['stint.time_s', 'clock'],
  ['fuel.time_remaining_s', 'clock'],
  ['rec.duration_s', 'clock'],
  // Seconds since midnight as hh:mm:ss
  ['session.time_of_day', 'timeOfDay'],
  ['session.start_time_of_day_s', 'timeOfDay'],
  // Short intervals in milliseconds
  ['time.delta_s', 'milliseconds'],
  ['app.pipeline_latency_s', 'milliseconds'],
  ['source.read_duration_s', 'milliseconds'],
  ['source.telemetry_age_s', 'milliseconds'],
  ['source.scoring_age_s', 'milliseconds'],
  ['net.rtt_s', 'milliseconds'],
  ['net.latency_s', 'milliseconds'],
  ['net.jitter_s', 'milliseconds'],
  ['net.stream_age_s', 'milliseconds'],
  // Millimetre-scale lengths
  ['chassis.front_ride_height_m', 'millimetres'],
  ['chassis.rear_ride_height_m', 'millimetres'],
  ['chassis.front_3rd_deflection_m', 'millimetres'],
  ['chassis.rear_3rd_deflection_m', 'millimetres'],
  ['aero.front_wing_height_m', 'millimetres'],
  ...perWheel((w) => `suspension.${w}.ride_height_m`, 'millimetres'),
  ...perWheel((w) => `suspension.${w}.deflection_m`, 'millimetres'),
  ...perWheel((w) => `brakes.${w}.thickness_m`, 'millimetres'),
  ...perWheel((w) => `tyres.${w}.vertical_deflection_m`, 'millimetres'),
  ...perWheel((w) => `tyres.${w}.static_radius_m`, 'millimetres'),
  ...perWheel((w) => `wheels.${w}.y_location_m`, 'millimetres'),
  // Signed margins: the sign is the information
  ['fuel.margin_laps', 'signed'],
]);

/** A channel value as the telemetry store exposes it. */
export type DisplayValue = number | bigint | boolean | string;

/** A formatted value with the pieces a readout needs. */
export interface FormattedValue {
  /** The value text, or NO_VALUE. */
  readonly text: string;
  /** Unit label shown next to the value ('' for none). */
  readonly unitLabel: string;
  /** The value read aloud with its unit name, for aria labels ('' for a missing value). */
  readonly spoken: string;
}

export interface ValueFormatOptions {
  /** Overrides the default decimals of the channel's display (spec §339: configurable). */
  readonly decimals?: number;
}

const MISSING: FormattedValue = { text: NO_VALUE, unitLabel: '', spoken: '' };

function formatted(text: string, unitLabel: string, unitName: string): FormattedValue {
  return { text, unitLabel, spoken: unitName === '' ? text : `${text} ${unitName}` };
}

/** True for value types that carry integers. */
function isIntegerType(valueType: ValueType): boolean {
  return (
    valueType === ValueType.I32 ||
    valueType === ValueType.U32 ||
    valueType === ValueType.U8 ||
    valueType === ValueType.I8 ||
    valueType === ValueType.U64
  );
}

/** Display kind of a channel, if its unit alone does not decide the format. */
export function displayKindOf(def: ChannelDef): DisplayKind | null {
  return CHANNEL_DISPLAY_KINDS.get(def.name) ?? null;
}

/** Display unit label and spoken unit name of a channel (what a header or graph axis shows). */
export function channelUnitLabel(def: ChannelDef): { readonly label: string; readonly name: string } {
  switch (displayKindOf(def)) {
    case 'lapTime':
    case 'clock':
    case 'timeOfDay':
      return { label: '', name: '' };
    case 'milliseconds':
      return { label: 'ms', name: 'milliseconds' };
    case 'millimetres':
      return { label: 'mm', name: 'millimetres' };
    case 'delta':
    case 'gap':
    case 'signed':
    case null:
      return { label: DISPLAY_UNITS[def.unit].label, name: DISPLAY_UNITS[def.unit].name };
  }
}

function pad2(n: number): string {
  return String(n).padStart(CLOCK_FIELD_DIGITS, '0');
}

/** Whole-second clock text: `m:ss`, or `h:mm:ss` from one hour (truncated, as a clock shows). */
function clockText(seconds: number): string {
  const total = Math.floor(Math.abs(seconds));
  const hours = Math.floor(total / SECONDS_PER_HOUR);
  const minutes = Math.floor((total % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE);
  const secs = total % SECONDS_PER_MINUTE;
  const body = hours > 0 ? `${hours}:${pad2(minutes)}:${pad2(secs)}` : `${minutes}:${pad2(secs)}`;
  return seconds < 0 && total > 0 ? `-${body}` : body;
}

/** A duration as `m:ss` or `h:mm:ss`, truncated to whole seconds and signed when negative. */
export function formatClock(seconds: MaybeNumber): string {
  if (!isFiniteNumber(seconds)) return NO_VALUE;
  return clockText(seconds);
}

/**
 * Seconds since midnight as `hh:mm:ss` (truncated); NO_VALUE outside one day. The end of the day, `86400`
 * (the registry's inclusive upper bound), reads `24:00:00`.
 */
export function formatTimeOfDay(seconds: MaybeNumber): string {
  if (!isFiniteNumber(seconds) || seconds < 0 || seconds > SECONDS_PER_DAY) return NO_VALUE;
  const total = Math.floor(seconds);
  const hours = Math.floor(total / SECONDS_PER_HOUR);
  const minutes = Math.floor((total % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE);
  return `${pad2(hours)}:${pad2(minutes)}:${pad2(total % SECONDS_PER_MINUTE)}`;
}

/** Byte counts scaled by magnitude (B, KiB, MiB, GiB, TiB, 1 decimal); plain bytes are integers. */
export function formatBytes(bytes: number | bigint | null | undefined): FormattedValue {
  if (bytes === null || bytes === undefined) return MISSING;
  if (typeof bytes === 'bigint' && bytes < BigInt(BYTES_PER_KIB) && bytes > -BigInt(BYTES_PER_KIB)) {
    return formatted(bytes.toString(), 'B', 'bytes');
  }
  const value = Number(bytes);
  if (!Number.isFinite(value)) return MISSING;
  let scaled = Math.abs(value);
  let step = 0;
  while (scaled >= BYTES_PER_KIB && step < BYTE_UNITS.length - 1) {
    scaled /= BYTES_PER_KIB;
    step += 1;
  }
  const [label, name] = BYTE_UNITS[step] ?? ['B', 'bytes'];
  const text = formatFixed(value < 0 ? -scaled : scaled, step === 0 ? 0 : BYTE_SCALED_DECIMALS, 'negative-only');
  return formatted(text, label, name);
}

/**
 * Whether a lap-time channel accepts 0: its registry plausibility range starts at or below 0 (a running lap
 * timer such as `lap.current_time_s`), unlike completed lap times whose range starts above 0.
 */
function lapTimeAllowsZero(def: ChannelDef): boolean {
  const range = def.validRange;
  return range !== null && range[0] <= 0;
}

/** Spoken form of a signed duration ("minus 1 hour 2 minutes 5 seconds"), truncated as `formatClock` shows it. */
function spokenClock(seconds: number): string {
  const total = Math.floor(Math.abs(seconds));
  return `${seconds < 0 && total > 0 ? 'minus ' : ''}${spokenDuration(total)}`;
}

/** Spoken form of `formatTimeOfDay` ("14 hours 2 minutes 5 seconds"; hours always read). */
function spokenTimeOfDay(seconds: number): string {
  return spokenDuration(Math.floor(seconds), true);
}

/** Spoken form of an `m:ss.mmm` lap time ("1 minute 23.456 seconds"). */
function spokenLapTime(text: string): string {
  const [minutes, seconds] = text.split(':');
  if (minutes === undefined || seconds === undefined) return text;
  return `${minutes} ${minutes === '1' ? 'minute' : 'minutes'} ${seconds} seconds`;
}

/**
 * Formats a channel value for display, choosing conversion and precision from the channel's unit (and display
 * kind where the unit is ambiguous). Missing values — null, undefined, non-finite numbers — render as
 * NO_VALUE, never as 0. BOOL values read YES / NO; text is shown as received.
 */
export function formatChannelValue(
  def: ChannelDef,
  value: DisplayValue | null | undefined,
  options?: ValueFormatOptions,
): FormattedValue {
  if (value === null || value === undefined) return MISSING;
  if (typeof value === 'boolean') return formatted(value ? 'YES' : 'NO', '', '');
  if (typeof value === 'string') return formatted(value, '', '');
  if (def.unit === Unit.Byte) return formatBytes(value);
  const unit = DISPLAY_UNITS[def.unit];
  if (typeof value === 'bigint') {
    // A U64 above 2^53 stays exact as long as the unit does not convert it.
    if (unit.toDisplay === identity && displayKindOf(def) === null) {
      return formatted(value.toString(), unit.label, unit.name);
    }
    return formatChannelValue(def, Number(value), options);
  }
  if (!Number.isFinite(value)) return MISSING;
  const decimals = options?.decimals ?? (isIntegerType(def.valueType) ? 0 : unit.decimals);
  switch (displayKindOf(def)) {
    case 'lapTime': {
      const text = formatLapTime(value, { allowZero: lapTimeAllowsZero(def) });
      return text === NO_VALUE ? MISSING : { text, unitLabel: '', spoken: spokenLapTime(text) };
    }
    case 'delta':
      return formatted(formatDelta(value), UNIT_LABEL.time, 'seconds');
    case 'gap':
      return formatted(formatGap(value), UNIT_LABEL.time, 'seconds');
    case 'clock':
      return { text: formatClock(value), unitLabel: '', spoken: spokenClock(value) };
    case 'timeOfDay': {
      const text = formatTimeOfDay(value);
      return text === NO_VALUE ? MISSING : { text, unitLabel: '', spoken: spokenTimeOfDay(value) };
    }
    case 'milliseconds':
      return formatted(
        formatFixed(value * MS_PER_S, options?.decimals ?? MILLISECOND_DECIMALS, 'negative-only'),
        'ms',
        'milliseconds',
      );
    case 'millimetres':
      return formatted(
        formatFixed(value * MM_PER_M, options?.decimals ?? MILLIMETRE_DECIMALS, 'negative-only'),
        'mm',
        'millimetres',
      );
    case 'signed':
      return formatted(formatFixed(unit.toDisplay(value), decimals, 'always'), unit.label, unit.name);
    case null:
      return formatted(formatFixed(unit.toDisplay(value), decimals, 'negative-only'), unit.label, unit.name);
  }
}

/**
 * A data age for quality indicators: `820` → "820 ms", `3_420` → "3.4 s", `125_000` → "2:05",
 * `3_725_000` → "1:02:05". Negative or missing ages render as NO_VALUE.
 *
 * Ages are truncated, like a clock, in every band: an age that counts up never shows a band's upper limit
 * ("1000 ms", "60.0 s") before switching to the next band, and never jumps back down when it does.
 */
export function formatAge(ms: MaybeNumber): FormattedValue {
  if (!isFiniteNumber(ms) || ms < 0) return MISSING;
  if (ms < MS_PER_S) return formatted(String(Math.floor(ms)), 'ms', 'milliseconds');
  const seconds = ms / MS_PER_S;
  if (seconds < SECONDS_PER_MINUTE) {
    const tenths = Math.floor(Number((seconds * AGE_TENTHS_PER_S).toPrecision(AGE_TRUNCATION_DIGITS)));
    return formatted(fixedFromUnits(tenths, AGE_SECONDS_DECIMALS), UNIT_LABEL.time, 'seconds');
  }
  return { text: clockText(seconds), unitLabel: '', spoken: spokenDuration(Math.floor(seconds)) };
}

/** "1 hour 2 minutes 5 seconds" (zero hours omitted unless `alwaysHours`). */
function spokenDuration(totalSeconds: number, alwaysHours = false): string {
  const hours = Math.floor(totalSeconds / SECONDS_PER_HOUR);
  const minutes = Math.floor((totalSeconds % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE);
  const seconds = totalSeconds % SECONDS_PER_MINUTE;
  const part = (n: number, unit: string): string => `${n} ${unit}${n === 1 ? '' : 's'}`;
  const parts = [part(minutes, 'minute'), part(seconds, 'second')];
  return hours > 0 || alwaysHours ? [part(hours, 'hour'), ...parts].join(' ') : parts.join(' ');
}

// ---------------------------------------------------------------------------------------------------------------
// Graph axes (src/graph): proportional display scales and tick labels
// ---------------------------------------------------------------------------------------------------------------

/** How an axis writes its tick values. */
export type AxisTickStyle = 'number' | 'signed' | 'clock' | 'timeOfDay';

/**
 * How a graph axis shows values of one canonical unit: display value = internal value × `factor`. Every display
 * conversion in this module is proportional (none has an offset), so an axis can compute nice ticks in display
 * units and map them back by dividing by `factor`.
 */
export interface AxisDisplay {
  /** Display value per internal unit (km/h per m/s = 3.6, % per fraction = 100, …). */
  readonly factor: number;
  /** Unit label for the axis title ('' for none). */
  readonly label: string;
  /** Spoken unit name ('' for none). */
  readonly name: string;
  readonly tickStyle: AxisTickStyle;
}

/** Axis display of a canonical unit (its DISPLAY_UNITS entry), written as plain numbers unless told otherwise. */
export function unitAxisDisplay(unit: Unit, tickStyle: AxisTickStyle = 'number'): AxisDisplay {
  const display = DISPLAY_UNITS[unit];
  return { factor: display.toDisplay(1), label: display.label, name: display.name, tickStyle };
}

/** Axis display of a channel: its unit, or its display kind where the unit alone does not decide the format. */
export function channelAxisDisplay(def: ChannelDef): AxisDisplay {
  const unit = channelUnitLabel(def);
  switch (displayKindOf(def)) {
    case 'lapTime':
    case 'clock':
      return { factor: 1, label: unit.label, name: unit.name, tickStyle: 'clock' };
    case 'timeOfDay':
      return { factor: 1, label: unit.label, name: unit.name, tickStyle: 'timeOfDay' };
    case 'milliseconds':
      return { factor: MS_PER_S, label: unit.label, name: unit.name, tickStyle: 'number' };
    case 'millimetres':
      return { factor: MM_PER_M, label: unit.label, name: unit.name, tickStyle: 'number' };
    case 'delta':
    case 'gap':
    case 'signed':
      return { ...unitAxisDisplay(def.unit, 'signed'), label: unit.label, name: unit.name };
    case null:
      return { ...unitAxisDisplay(def.unit), label: unit.label, name: unit.name };
  }
}

/** `m:ss(.d…)` or `h:mm:ss(.d…)` (`hh:mm:ss` for a time of day), rounded to `decimals` second decimals. */
function clockTickText(seconds: number, decimals: number, timeOfDay: boolean): string {
  const scale = 10 ** decimals;
  const units = roundedMagnitude(seconds, decimals);
  const whole = Math.floor(units / scale);
  const fraction = units - whole * scale;
  const hours = Math.floor(whole / SECONDS_PER_HOUR);
  const minutes = Math.floor((whole % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE);
  const secs = whole % SECONDS_PER_MINUTE;
  const secText = decimals > 0 ? `${pad2(secs)}.${String(fraction).padStart(decimals, '0')}` : pad2(secs);
  const body = timeOfDay
    ? `${pad2(hours)}:${pad2(minutes)}:${secText}`
    : hours > 0
      ? `${hours}:${pad2(minutes)}:${secText}`
      : `${minutes}:${secText}`;
  return seconds < 0 && units > 0 ? `-${body}` : body;
}

/**
 * Text of one axis tick or cursor position, given in DISPLAY units (already multiplied by the axis factor), with
 * `decimals` fixed decimals (seconds decimals for the clock styles). Non-finite values render as NO_VALUE. A
 * signed tick at zero reads "0", not "+0".
 */
export function formatAxisTick(display: number, decimals: number, style: AxisTickStyle): string {
  if (!Number.isFinite(display)) return NO_VALUE;
  switch (style) {
    case 'number':
      return formatFixed(display, decimals, 'negative-only');
    case 'signed':
      return roundedMagnitude(display, decimals) === 0
        ? fixedFromUnits(0, decimals)
        : formatFixed(display, decimals, 'always');
    case 'clock':
      return clockTickText(display, decimals, false);
    case 'timeOfDay':
      return clockTickText(display, decimals, true);
  }
}
