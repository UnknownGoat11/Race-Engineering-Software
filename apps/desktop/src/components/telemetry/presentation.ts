/**
 * Pure presentation rules for the telemetry primitives: state labels (spec §87, §107, §126), the text a readout
 * shows for each sample state, and accessible labels with spoken unit names (spec §161). No value is ever
 * invented: a sample without a value shows NO_VALUE ("—") together with the reason.
 */
import type { Provenance } from '../../generated/channels';
import { NO_VALUE, channelUnitLabel, formatAge, formatChannelValue, type ValueFormatOptions } from '../../lib/units';
import type { FreshnessBasis } from '../../telemetry/freshness';
import type { ChannelSample, SampleState } from '../../telemetry/store';

/** Label shown next to a readout for each sample state; null for LIVE (the value speaks for itself). */
export const SAMPLE_STATE_LABELS: Readonly<Record<SampleState, string | null>> = {
  live: null,
  degraded: 'DEGRADED',
  stale: 'STALE',
  invalid: 'INVALID',
  unavailable: 'UNAVAILABLE',
  notSupported: 'NOT SUPPORTED',
  noData: 'NO DATA',
};

const SAMPLE_STATE_SPOKEN: Readonly<Record<SampleState, string>> = {
  live: 'live',
  degraded: 'degraded',
  stale: 'stale',
  invalid: 'invalid',
  unavailable: 'unavailable',
  notSupported: 'not supported',
  noData: 'no data',
};

/** What a readout renders for one sample. */
export interface ReadoutPresentation {
  /**
   * The state shown: the sample's state, except that a LIVE or DEGRADED value this build cannot display (outside
   * what its format represents, e.g. a negative lap time) shows as INVALID rather than as a value or NO DATA.
   */
  readonly state: SampleState;
  /** Formatted value, or NO_VALUE. */
  readonly valueText: string;
  /** Display unit label ('' for none); kept while the value is missing so the layout does not move. */
  readonly unitLabel: string;
  /** State label, or null when the sample is LIVE. Always set when there is no value. */
  readonly stateLabel: string | null;
  /** Complete accessible label: name, value with spoken unit, and state. */
  readonly ariaLabel: string;
}

/** Fallback name for a channel id this build does not know. */
export function unknownChannelLabel(id: number): string {
  return `Channel 0x${id.toString(16).toUpperCase().padStart(4, '0')}`;
}

/** Presentation of `sample` under `label`. */
export function presentSample(
  sample: ChannelSample,
  label: string,
  options?: ValueFormatOptions,
): ReadoutPresentation {
  const def = sample.def;
  const unitLabel = def === null ? '' : channelUnitLabel(def).label;
  const formattedValue = def === null ? null : formatChannelValue(def, sample.value, options);
  if (formattedValue === null || formattedValue.text === NO_VALUE) {
    // No value to show: always say why. A LIVE or DEGRADED sample carries a VALID value, so if it cannot be shown
    // the value is out of its display domain: INVALID, never NO DATA (it was received). A LIVE state without a
    // value cannot happen, but would read NO DATA.
    const shown = sample.state === 'live' || sample.state === 'degraded';
    const state: SampleState = shown ? (sample.value === null ? 'noData' : 'invalid') : sample.state;
    return {
      state,
      valueText: NO_VALUE,
      unitLabel,
      stateLabel: SAMPLE_STATE_LABELS[state],
      ariaLabel: `${label}: ${SAMPLE_STATE_SPOKEN[state]}`,
    };
  }
  const degraded = sample.state === 'degraded';
  return {
    state: sample.state,
    valueText: formattedValue.text,
    unitLabel: formattedValue.unitLabel,
    stateLabel: SAMPLE_STATE_LABELS[sample.state],
    ariaLabel: `${label}: ${formattedValue.spoken}${degraded ? `, ${SAMPLE_STATE_SPOKEN.degraded}` : ''}`,
  };
}

/** Badge text and tooltip of each provenance (spec §107). */
export const PROVENANCE_PRESENTATION: Readonly<Record<Provenance, { readonly label: string; readonly description: string }>> =
  {
    Measured: { label: 'MEASURED', description: 'Directly from LMU' },
    Derived: { label: 'DERIVED', description: 'Calculated from measured data' },
    Estimated: { label: 'ESTIMATED', description: 'Calculated with assumptions or statistical inference' },
  };

/**
 * Data quality states (spec §87 GOOD / DEGRADED / STALE / INVALID, spec §126 LIVE / DEGRADED / STALE /
 * DISCONNECTED, plus NO DATA before anything arrived). The store yields live / degraded / stale / noData per
 * frequency class; invalid and disconnected come from link and validation status.
 */
export type DataQualityState = 'live' | 'degraded' | 'stale' | 'invalid' | 'noData' | 'disconnected';

export const DATA_QUALITY_LABELS: Readonly<Record<DataQualityState, string>> = {
  live: 'LIVE',
  degraded: 'DEGRADED',
  stale: 'STALE',
  invalid: 'INVALID',
  noData: 'NO DATA',
  disconnected: 'DISCONNECTED',
};

const DATA_QUALITY_SPOKEN: Readonly<Record<DataQualityState, string>> = {
  live: 'live',
  degraded: 'degraded',
  stale: 'stale',
  invalid: 'invalid',
  noData: 'no data',
  disconnected: 'disconnected',
};

/** Label and explanation shown when a state measures only the link to the Rust core (freshness.ts). */
export const LINK_ONLY_BASIS = {
  label: 'LINK ONLY',
  description: 'Freshness of the link only: the age of the LMU source data is not reported',
  spoken: 'link freshness only, game data age not reported',
} as const;

/** What a quality indicator renders. */
export interface QualityPresentation {
  readonly stateLabel: string;
  /** Age text with its unit ("3.4 s"), or null when no age is shown. */
  readonly ageText: string | null;
  /** LINK_ONLY_BASIS.label when the state measures only the link, else null. */
  readonly basisLabel: string | null;
  readonly ariaLabel: string;
}

/**
 * Presentation of a quality state with an optional data age in ms. `basis` 'linkOnly' says the state does not
 * cover LMU's own refresh (shown only for the states that claim a freshness: live, degraded, stale).
 */
export function presentQuality(
  label: string,
  state: DataQualityState,
  ageMs: number | null,
  basis: FreshnessBasis = 'endToEnd',
): QualityPresentation {
  const age = ageMs === null ? null : formatAge(ageMs);
  const ageText =
    age === null || age.text === NO_VALUE ? null : age.unitLabel === '' ? age.text : `${age.text} ${age.unitLabel}`;
  const spokenAge = age === null || age.spoken === '' ? '' : `, last update ${age.spoken} ago`;
  const linkOnly = basis === 'linkOnly' && (state === 'live' || state === 'degraded' || state === 'stale');
  return {
    stateLabel: DATA_QUALITY_LABELS[state],
    ageText,
    basisLabel: linkOnly ? LINK_ONLY_BASIS.label : null,
    ariaLabel: `${label}: ${DATA_QUALITY_SPOKEN[state]}${spokenAge}${linkOnly ? `, ${LINK_ONLY_BASIS.spoken}` : ''}`,
  };
}
