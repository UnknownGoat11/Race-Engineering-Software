import type { JSX } from 'react';
import type { Provenance } from '../../generated/channels';
import { PROVENANCE_PRESENTATION } from './presentation';
import './telemetry.css';

export interface ProvenanceBadgeProps {
  /** The channel's provenance from the generated metadata (`ChannelDef.provenance`). */
  readonly provenance: Provenance;
  /** Optional micro variant: renders 'est' tag for Estimated per REDESIGN §2.6 / T25 */
  readonly variant?: 'default' | 'micro' | undefined;
}

/**
 * MEASURED / DERIVED / ESTIMATED classification of a value (spec §107), with the meaning as tooltip and in the
 * accessible name. Told apart by text and border style, not by colour.
 * Supports micro variant 'est' tag per REDESIGN §2.6 (T25).
 */
export function ProvenanceBadge({ provenance, variant = 'default' }: ProvenanceBadgeProps): JSX.Element | null {
  const p = PROVENANCE_PRESENTATION[provenance];
  if (variant === 'micro') {
    if (provenance !== 'Estimated') return null;
    return (
      <span className="badge-micro telemetry-est" title={p.description}>
        est
        <span className="telemetry-visually-hidden">: {p.description}</span>
      </span>
    );
  }

  return (
    <span className="badge telemetry-provenance" data-provenance={provenance} title={p.description}>
      {p.label}
      <span className="telemetry-visually-hidden">: {p.description}</span>
    </span>
  );
}
