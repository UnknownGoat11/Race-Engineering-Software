import type { JSX } from 'react';
import { useChannelSample } from '../../telemetry/hooks';
import type { ChannelSample } from '../../telemetry/store';
import { presentSample, unknownChannelLabel } from './presentation';
import './telemetry.css';

export interface SampleReadoutProps {
  readonly sample: ChannelSample;
  /** Label shown with the value; defaults to the channel's label. */
  readonly label?: string;
  /** Overrides the default precision of the channel's unit (spec §339). */
  readonly decimals?: number;
  /** Show the unit label (default true). */
  readonly showUnit?: boolean;
}

/**
 * One channel sample: the formatted value with its unit, or "—" with an explicit UNAVAILABLE / NOT SUPPORTED
 * / INVALID / STALE / NO DATA state — never a zero. Tabular figures in a fixed-width slot keep digits from
 * shifting (spec §161); the accessible label reads the value with its unit name.
 */
export function SampleReadout({ sample, label, decimals, showUnit = true }: SampleReadoutProps): JSX.Element {
  const name = label ?? sample.def?.label ?? unknownChannelLabel(sample.id);
  const p = presentSample(sample, name, decimals === undefined ? undefined : { decimals });
  return (
    <div className="telemetry-readout" data-state={p.state} role="group" aria-label={p.ariaLabel}>
      <span className="telemetry-readout__label" aria-hidden="true">
        {name}
      </span>
      {p.stateLabel !== null && (
        <span className="telemetry-readout__state" aria-hidden="true">
          {p.stateLabel}
        </span>
      )}
      <span className="telemetry-readout__body" aria-hidden="true">
        <span className="telemetry-readout__value">{p.valueText}</span>
        {showUnit && p.unitLabel !== '' && <span className="telemetry-readout__unit">{p.unitLabel}</span>}
      </span>
    </div>
  );
}

export interface ValueReadoutProps extends Omit<SampleReadoutProps, 'sample'> {
  readonly channelId: number;
}

/**
 * Live readout of one channel from the TelemetryStore in context. Re-renders only when that channel's snapshot
 * changes, at most once per animation frame.
 */
export function ValueReadout({ channelId, ...rest }: ValueReadoutProps): JSX.Element {
  const sample = useChannelSample(channelId);
  return <SampleReadout sample={sample} {...rest} />;
}
