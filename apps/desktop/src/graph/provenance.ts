/**
 * Graph data provenance (spec §317): where a plotted series comes from, shown with each series label next to
 * its MEASURED / DERIVED / ESTIMATED badge (spec §107).
 */
import { Source, type ChannelDef } from '../generated/channels';
import type { DataKind, DataOrigin } from './types';

/** Every valid `DataKind`. */
const DATA_KINDS: readonly DataKind[] = ['lmu', 'synthetic'];

/**
 * Returns `kind` if it is a `DataKind`; throws a TypeError otherwise (a source wired without a valid kind is a bug
 * that must be visible, not data that defaults to LMU).
 */
export function checkDataKind(kind: unknown): DataKind {
  if (typeof kind === 'string' && (DATA_KINDS as readonly string[]).includes(kind)) return kind as DataKind;
  throw new TypeError(`graph source dataKind must be 'lmu' or 'synthetic', got ${String(kind)}`);
}

/** True only for a source that states exactly that its rows are LMU data (fail-safe, silent-failure H2). */
export function isLmuData(source: { readonly dataKind?: unknown }): boolean {
  return source.dataKind === 'lmu';
}

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
  unverified: 'Unverified Source (not LMU)',
} as const;

/** One of `GRAPH_SOURCE_LABELS`. */
export type GraphSourceLabel = (typeof GRAPH_SOURCE_LABELS)[keyof typeof GRAPH_SOURCE_LABELS];

/**
 * Source label of a series of channel `def` read from a source of `origin` holding `dataKind` rows. Fail-safe:
 * only `dataKind === 'lmu'` is ever labelled as LMU data; 'synthetic' is "Synthetic Test Data" and anything else
 * (a missing or unknown kind) "Unverified Source (not LMU)". For LMU data: every series of a recording is
 * "Session Recording"; live channels computed by the application are "Calculated", the application's own
 * diagnostics are "Application diagnostics", everything else is "Live LMU Telemetry".
 */
export function seriesSourceLabel(
  origin: DataOrigin,
  def: ChannelDef | null | undefined,
  dataKind: unknown,
): GraphSourceLabel {
  if (dataKind === 'synthetic') return GRAPH_SOURCE_LABELS.synthetic;
  if (dataKind !== 'lmu') return GRAPH_SOURCE_LABELS.unverified;
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
