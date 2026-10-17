import type { JSX } from 'react';
import type { FrequencyClass } from '../../generated/channels';
import type { FreshnessBasis } from '../../telemetry/freshness';
import { useStreamAgeMs, useStreamBasis, useStreamState } from '../../telemetry/hooks';
import { LINK_ONLY_BASIS, presentQuality, type DataQualityState } from './presentation';
import './telemetry.css';

export interface DataQualityIndicatorProps {
  /** What the state is about, e.g. "Telemetry", "Timing", "Weather" (spec §87). */
  readonly label: string;
  readonly state: DataQualityState;
  /** Age of the newest data in ms, shown when given (typically while DEGRADED or STALE). */
  readonly ageMs?: number | null;
  /** 'linkOnly' when the state measures only the link to the Rust core (labelled LINK ONLY); default 'endToEnd'. */
  readonly basis?: FreshnessBasis;
}

/**
 * LIVE / DEGRADED / STALE / INVALID / NO DATA / DISCONNECTED with an optional age (spec §87, §126). Each state
 * has its own label and marker shape, so colour is never the only signal (spec §162).
 */
export function DataQualityIndicator({
  label,
  state,
  ageMs = null,
  basis = 'endToEnd',
}: DataQualityIndicatorProps): JSX.Element {
  const p = presentQuality(label, state, ageMs, basis);
  return (
    <div className="telemetry-quality" data-state={state} data-basis={basis} role="group" aria-label={p.ariaLabel}>
      <span className="telemetry-quality__marker" aria-hidden="true" />
      <span className="telemetry-quality__label" aria-hidden="true">
        {label}
      </span>
      <span className="telemetry-quality__state" aria-hidden="true">
        {p.stateLabel}
      </span>
      {p.ageText !== null && (
        <span className="telemetry-quality__age" aria-hidden="true">
          {p.ageText}
        </span>
      )}
      {p.basisLabel !== null && (
        <span className="telemetry-quality__basis" title={LINK_ONLY_BASIS.description} aria-hidden="true">
          {p.basisLabel}
        </span>
      )}
    </div>
  );
}

export interface StreamQualityIndicatorProps {
  readonly label: string;
  /** Frequency class whose stream state is shown. */
  readonly frequency: FrequencyClass;
}

/**
 * Stream state of one frequency class from the TelemetryStore in context; while DEGRADED or STALE it also
 * shows the class's data age (refreshed four times a second). Labelled LINK ONLY while the Rust core does not
 * report the LMU source age for the class, so it never claims more freshness than it measures.
 */
export function StreamQualityIndicator({ label, frequency }: StreamQualityIndicatorProps): JSX.Element {
  const state = useStreamState(frequency);
  const basis = useStreamBasis(frequency);
  const ageMs = useStreamAgeMs(frequency, state === 'degraded' || state === 'stale');
  return <DataQualityIndicator label={label} state={state} ageMs={ageMs} basis={basis} />;
}
