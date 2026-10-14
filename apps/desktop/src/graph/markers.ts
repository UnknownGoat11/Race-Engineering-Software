/**
 * Layout of vertical markers (spec §62 sector / lap / event markers): every marker inside the plot gets its line;
 * labels are placed in a few rows at the top of the plot so they never overlap. Lap labels are placed first,
 * then sector labels, then event labels; a label that fits in no row is hidden (its line is still drawn). A
 * label that would leave the plot on the right is placed on the left of its line instead. Pure, for tests.
 */
import { applyAffine, type Affine } from './scale';
import type { GraphMarker, MarkerKind } from './types';

/** A marker positioned on the plot. */
export interface PlacedMarker {
  readonly marker: GraphMarker;
  /** Line position (CSS px). */
  readonly px: number;
  /** Label row (0 = top), meaningful when `labelVisible`. */
  readonly row: number;
  /** Left edge of the label (CSS px). */
  readonly labelX: number;
  readonly labelVisible: boolean;
}

/** Plot geometry and text measurement for marker layout. */
export interface MarkerLayoutOptions {
  readonly plotLeft: number;
  readonly plotRight: number;
  /** Label rows available. */
  readonly rows: number;
  /** Width of a label's text in CSS px. */
  readonly measure: (text: string) => number;
  /** Space between a line and its label, and the least space between two labels in a row (CSS px). */
  readonly gap: number;
}

/** Placement order: lap labels win over sector labels, which win over event labels. */
const KIND_PRIORITY: Readonly<Record<MarkerKind, number>> = { lap: 0, sector: 1, event: 2 };

/** Lays out `markers` (x in axis units) with the x → CSS px map `x`. The result is ordered by px. */
export function layoutMarkers(markers: readonly GraphMarker[], x: Affine, options: MarkerLayoutOptions): PlacedMarker[] {
  const { plotLeft, plotRight, rows, measure, gap } = options;
  const visible = markers
    .map((marker) => ({ marker, px: applyAffine(x, marker.x) }))
    .filter((m) => Number.isFinite(m.px) && m.px >= plotLeft && m.px <= plotRight);
  const order = [...visible].sort(
    (a, b) => KIND_PRIORITY[a.marker.kind] - KIND_PRIORITY[b.marker.kind] || a.px - b.px,
  );
  const occupied: [number, number][][] = Array.from({ length: Math.max(0, rows) }, () => []);
  const placed = new Map<(typeof visible)[number], PlacedMarker>();
  for (const m of order) {
    const width = measure(m.marker.label);
    const right = m.px + gap;
    const labelX = right + width <= plotRight ? right : m.px - gap - width;
    const start = labelX;
    const end = labelX + width;
    let row = -1;
    if (m.marker.label !== '' && start >= plotLeft) {
      row = occupied.findIndex((intervals) => intervals.every(([s, e]) => end + gap <= s || start >= e + gap));
    }
    if (row >= 0) occupied[row]?.push([start, end]);
    placed.set(m, { marker: m.marker, px: m.px, row: Math.max(row, 0), labelX, labelVisible: row >= 0 });
  }
  return visible
    .map((m) => placed.get(m))
    .filter((p): p is PlacedMarker => p !== undefined)
    .sort((a, b) => a.px - b.px);
}
