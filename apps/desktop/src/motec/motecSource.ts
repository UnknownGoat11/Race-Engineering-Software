/**
 * MoTeC Static Graph Source and Reference Overlay Generator.
 *
 * Bridges parsed MoTeC .ld files into the TelemetryGraph engine, allowing
 * imported runs to be rendered as comparison traces with distinct colors.
 */
import { StaticSource } from '../graph/staticSource';
import type { ReferenceSeries } from '../graph/types';
import type { ParsedMotecFile } from './motecTypes';

/**
 * Creates a high-performance StaticSource from a parsed MoTeC file on either
 * a distance axis (default) or a time axis.
 */
export function createMotecGraphSource(
  file: ParsedMotecFile,
  axis: 'distance' | 'time' = 'distance',
): StaticSource {
  const columns = new Map<number, Float64Array>();

  for (const [appId, chanData] of file.channelsByAppId) {
    columns.set(appId, chanData.samples);
  }

  const xs = axis === 'distance' ? file.distanceTimelineM : file.timeTimelineS;
  const ts = file.timeTimelineS;

  return new StaticSource({
    xs,
    ts,
    columns,
    origin: 'recording',
    dataKind: 'lmu',
  });
}

/**
 * Creates an overlay ReferenceSeries for TelemetryGraph.
 *
 * When plotted alongside live or synthetic telemetry, it renders as a
 * distinct high-contrast comparison curve (e.g. palette slot 2 = orange/amber).
 */
export function createMotecReferenceSeries(
  file: ParsedMotecFile,
  channelId: number,
  source: StaticSource,
  colorSlot = 2,
): ReferenceSeries {
  const chan = file.channelsByAppId.get(channelId);
  const chanName = chan?.meta.name || `Ch_${channelId}`;
  const driverShort = file.header.driver.split(' ')[0] || 'MoTeC';

  return {
    key: `motec-ref-${channelId}`,
    label: `[MoTeC] ${chanName} (${driverShort})`,
    channelId,
    source,
    color: colorSlot,
  };
}

/**
 * Samples a MoTeC channel at a given lap distance in meters.
 * Performs fast O(log N) binary search on the non-decreasing distance timeline.
 */
export function sampleMotecAtDistance(
  file: ParsedMotecFile,
  channelId: number,
  distM: number,
): number | null {
  const chan = file.channelsByAppId.get(channelId);
  if (!chan || chan.samples.length === 0) return null;

  const xs = file.distanceTimelineM;
  let lo = 0;
  let hi = xs.length - 1;

  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const v = xs[mid] ?? 0;
    if (v < distM) lo = mid + 1;
    else hi = mid - 1;
  }

  const idx = Math.min(xs.length - 1, Math.max(0, lo));
  return chan.samples[idx] ?? null;
}

/**
 * Computes live comparative telemetry deltas between current car and MoTeC benchmark.
 */
export function computeMotecDelta(
  file: ParsedMotecFile,
  distM: number,
  liveSpeedKmh: number,
  liveThrottlePct: number,
  liveBrakePct: number,
): {
  readonly motecSpeedKmh: number;
  readonly deltaSpeedKmh: number;
  readonly motecThrottlePct: number;
  readonly deltaThrottlePct: number;
  readonly motecBrakePct: number;
  readonly deltaBrakePct: number;
} {
  const refSpeed = sampleMotecAtDistance(file, 0x0001, distM) ?? 0; // Speed
  const refThrottle = sampleMotecAtDistance(file, 0x0010, distM) ?? 0; // Throttle
  const refBrake = sampleMotecAtDistance(file, 0x0011, distM) ?? 0; // Brake

  return {
    motecSpeedKmh: refSpeed,
    deltaSpeedKmh: liveSpeedKmh - refSpeed,
    motecThrottlePct: refThrottle,
    deltaThrottlePct: liveThrottlePct - refThrottle,
    motecBrakePct: refBrake,
    deltaBrakePct: liveBrakePct - refBrake,
  };
}
