import { createContext, useContext } from 'react';
import type { Provenance } from '../../generated/channels';
import { NO_VALUE, channelUnitLabel, formatAge, formatChannelValue, type ValueFormatOptions } from '../../lib/units';
import type { FreshnessBasis } from '../../telemetry/freshness';
import type { ChannelSample, SampleState } from '../../telemetry/store';

/** Scope-level data quality state from context (C3, C5, §4 W1.3/W1.5). */
export type ScopeDq = 'live' | 'degraded' | 'stale' | 'untrusted' | 'ended' | 'unavailable' | null;

export const ScopeDqContext = createContext<ScopeDq>(null);

/** Returns the nearest ScopeDq provided by an ancestor (e.g. EngineerScreen or panel container). */
export function useScopeDq(): ScopeDq {
  return useContext(ScopeDqContext);
}

/** Display states for a readout: SampleState plus scope-level untrusted and ended. */
export type ReadoutState = SampleState | 'untrusted' | 'ended';

/** Label shown next to a readout for each sample state; null for LIVE (the value speaks for itself). */
export const SAMPLE_STATE_LABELS: Readonly<Record<ReadoutState, string | null>> = {
  live: null,
  degraded: 'DEGRADED',
  stale: 'STALE',
  invalid: 'INVALID',
  unavailable: 'UNAVAILABLE',
  notSupported: 'NOT SUPPORTED',
  noData: 'NO DATA',
  untrusted: 'UNTRUSTED',
  ended: 'ENDED',
};

const SAMPLE_STATE_SPOKEN: Readonly<Record<ReadoutState, string>> = {
  live: 'live',
  degraded: 'degraded',
  stale: 'stale',
  invalid: 'invalid',
  unavailable: 'unavailable',
  notSupported: 'not supported',
  noData: 'no data',
  untrusted: 'untrusted',
  ended: 'ended',
};

/** What a readout renders for one sample. */
export interface ReadoutPresentation {
  /**
   * The state shown: the sample's state or scope DQ override, except that a LIVE or DEGRADED value
   * this build cannot display shows as INVALID rather than as a value or NO DATA.
   */
  readonly state: ReadoutState;
  /** Formatted value, or NO_VALUE. */
  readonly valueText: string;
  /** Display unit label ('' for none); kept while the value is missing so the layout does not move. */
  readonly unitLabel: string;
  /** State label, or null when the sample is LIVE. Always set when there is no value. */
  readonly stateLabel: string | null;
  /** Complete accessible label: name, value with spoken unit, and state. */
  readonly ariaLabel: string;
}

/** Options for presentSample, extending standard value formatting with scope DQ and data age. */
export interface PresentSampleOptions extends ValueFormatOptions {
  readonly scopeDq?: ScopeDq | undefined;
  readonly ageMs?: number | null | undefined;
}

/** Fallback name for a channel id this build does not know. */
export function unknownChannelLabel(id: number): string {
  return `Channel 0x${id.toString(16).toUpperCase().padStart(4, '0')}`;
}

/** Cache of last known formatted values per channel ID for C5 STALE display. */
const lastKnownFormattedValues = new Map<number, { text: string; unitLabel: string; spoken: string }>();

/** Testing helper to clear the last known values cache. */
export function resetLastKnownFormattedValues(): void {
  lastKnownFormattedValues.clear();
}

/** Presentation of `sample` under `label`. */
export function presentSample(
  sample: ChannelSample,
  label: string,
  options?: PresentSampleOptions,
  scopeDqParam?: ScopeDq,
): ReadoutPresentation {
  const scopeDq = options?.scopeDq ?? scopeDqParam ?? null;
  const def = sample.def;
  const unitLabel = def === null ? '' : channelUnitLabel(def).label;

  // Format value if present
  const formattedValue = def === null ? null : formatChannelValue(def, sample.value, options);
  if (formattedValue !== null && formattedValue.text !== NO_VALUE) {
    lastKnownFormattedValues.set(sample.id, formattedValue);
  }

  // Scope-level UNTRUSTED: format mismatch, greys out values
  if (scopeDq === 'untrusted') {
    const cached = lastKnownFormattedValues.get(sample.id);
    const valText = formattedValue?.text ?? cached?.text ?? NO_VALUE;
    const uLabel = formattedValue?.unitLabel ?? cached?.unitLabel ?? unitLabel;
    return {
      state: 'untrusted',
      valueText: valText,
      unitLabel: uLabel,
      stateLabel: SAMPLE_STATE_LABELS.untrusted,
      ariaLabel: `${label}: ${valText !== NO_VALUE ? valText : 'untrusted'}, untrusted`,
    };
  }

  // Scope-level ENDED: session ended
  if (scopeDq === 'ended') {
    const cached = lastKnownFormattedValues.get(sample.id);
    const valText = formattedValue?.text ?? cached?.text ?? NO_VALUE;
    const uLabel = formattedValue?.unitLabel ?? cached?.unitLabel ?? unitLabel;
    return {
      state: 'ended',
      valueText: valText,
      unitLabel: uLabel,
      stateLabel: SAMPLE_STATE_LABELS.ended,
      ariaLabel: `${label}: ${valText !== NO_VALUE ? valText : 'ended'}, ended`,
    };
  }

  // STALE handling (C5): keep last value, format in stale neutral grey
  const isStale = sample.state === 'stale' || scopeDq === 'stale';
  if (isStale) {
    const cached = lastKnownFormattedValues.get(sample.id);
    const activeFormatted = (formattedValue !== null && formattedValue.text !== NO_VALUE) ? formattedValue : cached ?? null;
    if (activeFormatted !== null && activeFormatted.text !== NO_VALUE) {
      const age = options?.ageMs != null ? formatAge(options.ageMs) : null;
      const ageSuffix = age !== null && age.text !== NO_VALUE ? ` ${age.text} ${age.unitLabel}`.trimEnd() : '';
      const stateLabel = `STALE${ageSuffix}`;
      return {
        state: 'stale',
        valueText: activeFormatted.text,
        unitLabel: activeFormatted.unitLabel,
        stateLabel,
        ariaLabel: `${label}: ${activeFormatted.spoken}, stale`,
      };
    }
  }

  if (formattedValue === null || formattedValue.text === NO_VALUE) {
    // No value to show: always say why.
    const shown = sample.state === 'live' || sample.state === 'degraded';
    const state: ReadoutState = shown ? (sample.value === null ? 'noData' : 'invalid') : sample.state;
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
