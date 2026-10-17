import type { JSX } from 'react';
import type { Provenance } from '../../generated/channels';
import { PROVENANCE_PRESENTATION } from './presentation';
import './telemetry.css';

export interface ProvenanceBadgeProps {
  /** The channel's provenance from the generated metadata (`ChannelDef.provenance`). */
  readonly provenance: Provenance;
}

/**
 * MEASURED / DERIVED / ESTIMATED classification of a value (spec §107), with the meaning as tooltip and in the
 * accessible name. Told apart by text and border style, not by colour.
 */
export function ProvenanceBadge({ provenance }: ProvenanceBadgeProps): JSX.Element {
  const p = PROVENANCE_PRESENTATION[provenance];
  return (
    <span className="badge telemetry-provenance" data-provenance={provenance} title={p.description}>
      {p.label}
      <span className="telemetry-visually-hidden">: {p.description}</span>
    </span>
  );
}
