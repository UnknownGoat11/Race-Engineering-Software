/**
 * X axis modes (spec §62): time, lap time, lap distance and normalized lap distance. Each mode fixes the axis
 * title, the display conversion (lib/units.ts) and how the LIVE view behaves; the x values themselves come from
 * the caller's data source.
 */
import { Unit } from '../generated/channels';
import { formatAxisTick, unitAxisDisplay, type AxisDisplay } from '../lib/units';
import { niceClockStep, niceStep, ticksForStep, type Ticks } from './scale';
import type { XAxisMode, XRange } from './types';

/**
 * How a LIVE view of the axis behaves: 'scroll' pins the right edge to the newest x (time grows forever);
 * 'domain' shows the whole lap domain (x restarts every lap).
 */
export type XViewKind = 'scroll' | 'domain';

/** Static description of one x axis mode. */
export interface XAxisSpec {
  readonly mode: XAxisMode;
  /** Axis title shown with the unit label. */
  readonly title: string;
  readonly display: AxisDisplay;
  readonly viewKind: XViewKind;
  /** Smallest visible span (x units) zooming may reach. */
  readonly minSpan: number;
  /** Fixed domain of the mode (normalized distance: 0…1), or null when the caller/data decide it. */
  readonly domain: XRange | null;
  /** Decimals of the cursor position label (display units). */
  readonly cursorDecimals: number;
}

/** Smallest time span: ten samples at the nominal 100 Hz. */
const MIN_TIME_SPAN_S = 0.1;
const MIN_LAP_DISTANCE_SPAN_M = 5;
const MIN_LAP_FRACTION_SPAN = 0.001;
const TIME_CURSOR_DECIMALS = 2;
const DISTANCE_CURSOR_DECIMALS = 0;
const FRACTION_CURSOR_DECIMALS = 1;
const LAP_FRACTION_DOMAIN: XRange = { min: 0, max: 1 };

/** Clock axes ("12:41") carry no unit label: the clock format is the unit. */
const CLOCK_AXIS: AxisDisplay = { ...unitAxisDisplay(Unit.Second, 'clock'), label: '', name: '' };

/** Every x axis mode's description. */
export const X_AXIS_SPECS: Readonly<Record<XAxisMode, XAxisSpec>> = {
  time: {
    mode: 'time',
    title: 'Session time',
    display: CLOCK_AXIS,
    viewKind: 'scroll',
    minSpan: MIN_TIME_SPAN_S,
    domain: null,
    cursorDecimals: TIME_CURSOR_DECIMALS,
  },
  lapTime: {
    mode: 'lapTime',
    title: 'Lap time',
    display: CLOCK_AXIS,
    viewKind: 'domain',
    minSpan: MIN_TIME_SPAN_S,
    domain: null,
    cursorDecimals: TIME_CURSOR_DECIMALS,
  },
  lapDistance: {
    mode: 'lapDistance',
    title: 'Lap distance',
    display: unitAxisDisplay(Unit.Metre),
    viewKind: 'domain',
    minSpan: MIN_LAP_DISTANCE_SPAN_M,
    domain: null,
    cursorDecimals: DISTANCE_CURSOR_DECIMALS,
  },
  lapFraction: {
    mode: 'lapFraction',
    title: 'Lap distance (normalized)',
    display: unitAxisDisplay(Unit.Fraction),
    viewKind: 'domain',
    minSpan: MIN_LAP_FRACTION_SPAN,
    domain: LAP_FRACTION_DOMAIN,
    cursorDecimals: FRACTION_CURSOR_DECIMALS,
  },
};

/** Axis title with its unit label, e.g. "Lap distance (m)"; clock axes carry no unit label. */
export function xAxisTitle(spec: XAxisSpec): string {
  return spec.display.label === '' ? spec.title : `${spec.title} (${spec.display.label})`;
}

/** Labelled ticks of an x range: values in x units (not display units) with their label text. */
export interface LabelledTicks {
  readonly values: readonly number[];
  readonly labels: readonly string[];
}

/** Ticks for an x range at about `targetCount` intervals, labelled in display units. */
export function xTicks(spec: XAxisSpec, range: XRange, targetCount: number): LabelledTicks {
  const f = spec.display.factor;
  const dMin = range.min * f;
  const dMax = range.max * f;
  const span = dMax - dMin;
  const step = spec.display.tickStyle === 'clock' ? niceClockStep(span, targetCount) : niceStep(span, targetCount);
  const ticks: Ticks = ticksForStep(dMin, dMax, step);
  return {
    values: ticks.values.map((d) => d / f),
    labels: ticks.values.map((d) => formatAxisTick(d, ticks.decimals, spec.display.tickStyle)),
  };
}

/** Text of an x position (cursor label), e.g. "12:41.25" or "1250 m". */
export function formatX(spec: XAxisSpec, x: number): string {
  const text = formatAxisTick(x * spec.display.factor, spec.cursorDecimals, spec.display.tickStyle);
  return spec.display.label === '' ? text : `${text} ${spec.display.label}`;
}
