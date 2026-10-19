/**
 * V1.1 Engineering View Registry.
 *
 * Implements the full catalogue matching the user's specification, neatly categorized
 * into Performance, Timing, Multi-Car, and Engineering workspaces.
 */
import type { ViewCategory, ViewDefinition, ViewId } from './types';

export const ALL_VIEWS: readonly ViewDefinition[] = [
  // --- Category: Performance & Driver DNA ---
  {
    id: 'driver-dna',
    title: 'Driver DNA',
    category: 'performance',
    description: 'Biometric driving traits radar: trail-braking decay rate, throttle aggression, and consistency score.',
    glyph: '🧬',
    badge: 'V1.1',
  },
  {
    id: 'driving',
    title: 'Driving',
    category: 'performance',
    description: 'Vehicle dynamics cockpit HUD: 4-wheel top view with 3-part tyre thermals, friction circle, and input meters.',
    glyph: '🏎️',
    badge: 'CORE',
  },
  {
    id: 'traction-circle',
    title: 'Traction Circle (G-G)',
    category: 'performance',
    description: 'Dynamic G-G friction ellipse diagram: lateral vs longitudinal G-force scatter, trail-braking grip utilization, and chassis limit envelope.',
    glyph: '🎯',
    badge: 'V1.1',
  },
  {
    id: 'lap-comparison',
    title: 'Lap Comparison',
    category: 'performance',
    description: 'Side-by-side comparative analysis: continuous delta-t time line, speed delta, and pedal trace overlay.',
    glyph: '⚖️',
  },
  {
    id: 'multilap-telemetry',
    title: 'Multilap Telemetry',
    category: 'performance',
    description: 'Synchronous 5-lap trace overlay on distance axis revealing tyre wear braking drift and line variance.',
    glyph: '📈',
  },
  {
    id: 'multihistogram',
    title: 'Multihistogram',
    category: 'performance',
    description: 'Statistical frequency distribution bins for speed bands, throttle percentage, and braking pressure.',
    glyph: '📊',
  },
  {
    id: 'multiplotter',
    title: 'Multiplotter',
    category: 'performance',
    description: 'X-Y correlation scatter plots: speed vs RPM, lateral G vs steering angle, and braking deceleration.',
    glyph: '📐',
  },

  // --- Category: Timing, Stints & Statistics ---
  {
    id: 'laptime-data',
    title: 'Laptime Data',
    category: 'timing',
    description: 'Tabular lap breakdown: sector splits (S1/S2/S3), speed traps, average speed, fuel burn, and validity.',
    glyph: '⏱️',
  },
  {
    id: 'stints-data',
    title: 'Stints Data',
    category: 'timing',
    description: 'Endurance stint ledger: laps completed per tyre set, degradation rates (%/lap), and stationary pit stop times.',
    glyph: '📋',
  },
  {
    id: 'tyre-degradation',
    title: 'Tyre Deg & Flatspot Logger',
    category: 'timing',
    description: 'Multi-lap tyre degradation wear curves, performance cliff predictor, double-stint viability, and chronological wheel lock-up flatspot event log.',
    glyph: '🛞',
    badge: 'V1.1',
  },
  {
    id: 'statistics-table',
    title: 'Statistics Table',
    category: 'timing',
    description: 'Comprehensive pace statistics: mean, median, trimmed average, theoretical optimal lap, and standard deviation.',
    glyph: '🧮',
  },
  {
    id: 'statistics-table-graphs',
    title: 'Statistics Table Graphs',
    category: 'timing',
    description: 'Graphical stint pace progression, tyre performance cliff regression curve, and sector distribution.',
    glyph: '📉',
  },
  {
    id: 'statistics-red-blue',
    title: 'Statistics: RED,BLUE & SES',
    category: 'timing',
    description: 'Head-to-head benchmark matrix: current car (BLUE) vs reference best (RED) vs session field average (SES).',
    glyph: '🎯',
  },
  {
    id: 'race-info',
    title: 'Race Info',
    category: 'timing',
    description: 'Race control command center: session countdown, ambient & track temperature curves, and incident logs.',
    glyph: '🏁',
  },
  {
    id: 'time-trial',
    title: 'Time Trial Info',
    category: 'timing',
    description: 'High-precision micro-split pacing against all-time track records, ghost car target, and delta rate.',
    glyph: '⚡',
  },

  // --- Category: Field Intelligence & Multi-Car ---
  {
    id: 'multicar-map',
    title: 'Multi-car Map',
    category: 'multicar',
    description: 'Live 2D GPS circuit track map with competitor positions color-coded by class, sector markers, and speed traps.',
    glyph: '🗺️',
  },
  {
    id: 'multicar-circle',
    title: 'Multi-car Map as circle',
    category: 'multicar',
    description: 'Circular polar track radar: relative field density around the loop, traffic clusters, and pit exit merge gaps.',
    glyph: '⭕',
  },
  {
    id: 'multicar-one-lap-timing',
    title: 'Multi-car One-lap Timing',
    category: 'multicar',
    description: 'Active lap micro-delta tracking: real-time sector split projections across all competitors currently on track.',
    glyph: '⏱️',
  },
  {
    id: 'multicar-race-timing',
    title: 'Multi-car Race Timing',
    category: 'multicar',
    description: 'Official pitwall timing tower: class & overall standings, gaps to leader, intervals ahead/behind, and tyre age.',
    glyph: '🏆',
  },
  {
    id: 'multicar-minisector-timing',
    title: 'Multi-car Minisector Timing',
    category: 'multicar',
    description: 'High-resolution 24-minisector track board with purple, green, and yellow split heatmaps across the field.',
    glyph: '🚥',
  },
  {
    id: 'multicar-opponent-stats',
    title: 'Multi-car Opponent Stats',
    category: 'multicar',
    description: 'Direct rival scouting: sector strengths and weaknesses, straight-line top speeds, and pit stop strategy deltas.',
    glyph: '🔍',
  },
  {
    id: 'multicar-race-field-spread',
    title: 'Multi-car Race Field Spread',
    category: 'multicar',
    description: 'Classic motorsport gap chart: gap to leader over race laps illustrating pit stop windows and undercut tactics.',
    glyph: '🌌',
  },
  {
    id: 'multicar-race-history',
    title: 'Multi-car Race History',
    category: 'multicar',
    description: 'Position bump chart: lap-by-lap overtakes, pit stop reshuffles, and driver stint chronology.',
    glyph: '📜',
  },
  {
    id: 'multicar-session-data',
    title: 'Multi-car Session Data',
    category: 'multicar',
    description: 'Field overview matrix: vehicle models, team classifications, tyre compounds, trap speeds, and penalties.',
    glyph: '💾',
  },
  {
    id: 'multicar-telemetry',
    title: 'Multi-car Telemetry',
    category: 'multicar',
    description: 'Direct competitor telemetry trace overlay: compare speed, throttle, and braking with any chosen opponent.',
    glyph: '👥',
  },

  // --- Category: Engineering Workspaces & Raw Data ---
  {
    id: 'workspace',
    title: 'Workspace',
    category: 'engineering',
    description: 'Modular 4-column pitwall workspace combining telemetry waveforms, track map, strategy, and tyre thermals.',
    glyph: '🖥️',
  },
  {
    id: 'telemetry',
    title: 'Telemetry',
    category: 'engineering',
    description: 'MoTeC-style multi-pane synchronous telemetry viewer with cursor inspection, zoom, and channel selection.',
    glyph: '📡',
  },
  {
    id: 'hypercar-hybrid',
    title: 'Hypercar VET & Hybrid',
    category: 'engineering',
    description: 'WEC Hypercar Virtual Energy Tank (MJ/stint), MGU-K electric motor deploy curve, front-axle 190 km/h speed gate, and braking energy regen ledger.',
    glyph: '⚡',
    badge: 'HYPERCAR',
  },
  {
    id: 'raw-data',
    title: '~ Raw Data ~',
    category: 'engineering',
    description: 'Low-level wire TLV stream inspector: live Hz frame rate, channel IDs, quality flags, and packet bytes.',
    glyph: '🔩',
  },
  {
    id: 'minimal-ui',
    title: '~ Minimal UI ~',
    category: 'engineering',
    description: 'Ultra-clean distraction-free cockpit HUD: high-contrast delta, fuel to finish, tyre temperatures, and flags.',
    glyph: '🕶️',
  },
];

export const CATEGORY_LABELS: Record<ViewCategory, { readonly label: string; readonly glyph: string }> = {
  performance: { label: 'Performance & Driver DNA', glyph: '🧬' },
  timing: { label: 'Timing & Strategy', glyph: '⏱️' },
  multicar: { label: 'Multi-Car & Field Intelligence', glyph: '🏁' },
  engineering: { label: 'Engineering & Diagnostics', glyph: '🛠️' },
};

export function getViewDefinition(id: ViewId): ViewDefinition {
  const found = ALL_VIEWS.find((v) => v.id === id);
  if (!found) {
    throw new Error(`View '${id}' not found in view registry.`);
  }
  return found;
}

export function getViewsByCategory(category: ViewCategory): readonly ViewDefinition[] {
  return ALL_VIEWS.filter((v) => v.category === category);
}

export function searchViews(query: string): readonly ViewDefinition[] {
  const q = query.trim().toLowerCase();
  if (!q) return ALL_VIEWS;
  return ALL_VIEWS.filter(
    (v) =>
      v.title.toLowerCase().includes(q) ||
      v.description.toLowerCase().includes(q) ||
      v.category.toLowerCase().includes(q) ||
      v.id.toLowerCase().includes(q),
  );
}

export { getTyreTempColor } from './tyreColors';
