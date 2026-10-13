/**
 * Graph data provenance (spec §317): where a plotted series comes from, shown with each series label next to
 * its MEASURED / DERIVED / ESTIMATED badge (spec §107).
 */
import { Source, type ChannelDef } from '../generated/channels';
import type { DataOrigin } from './types';

/**
 * "Source: …" texts of spec §317, plus the application's own diagnostics (none of those) and generated test data,
 * which must never be presented as LMU data (spec §108, §4 "synthetic telemetry must be clearly separated").
 */
export const GRAPH_SOURCE_LABELS = {
  live: 'Live LMU Telemetry',
  recording: 'Session Recording',
  calculated: 'Calculated',
  application: 'Application diagnostics',
  synthetic: 'Synthetic Test Data',
} as const;

/** One of `GRAPH_SOURCE_LABELS`. */
export type GraphSourceLabel = (typeof GRAPH_SOURCE_LABELS)[keyof typeof GRAPH_SOURCE_LABELS];

/**
 * Source label of a series of channel `def` read from a source of `origin`: every series of a synthetic source is
 * "Synthetic Test Data" and every series of a recording "Session Recording"; live channels computed by the
 * application are "Calculated", the application's own diagnostics are "Application diagnostics", everything else
 * read from LMU is "Live LMU Telemetry". Channels this build does not know are labelled by origin only.
 */
export function seriesSourceLabel(
  origin: DataOrigin,
  def: ChannelDef | null | undefined,
  synthetic = false,
): GraphSourceLabel {
  if (synthetic) return GRAPH_SOURCE_LABELS.synthetic;
  if (origin === 'recording') return GRAPH_SOURCE_LABELS.recording;
  if (def?.source === Source.Calculated) return GRAPH_SOURCE_LABELS.calculated;
  if (def?.source === Source.Application) return GRAPH_SOURCE_LABELS.application;
  return GRAPH_SOURCE_LABELS.live;
}

/** "Source: A · B" for the distinct labels of a graph's series, in first-appearance order. */
export function graphSourceText(labels: readonly GraphSourceLabel[]): string {
  const distinct = [...new Set(labels)];
  return distinct.length === 0 ? '' : `Source: ${distinct.join(' · ')}`;
}
