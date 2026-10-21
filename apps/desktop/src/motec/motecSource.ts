/**
 * MoTeC Static Graph Source and Reference Overlay Generator.
 *
 * Bridges parsed MoTeC .ld files into the TelemetryGraph engine, allowing imported runs to be rendered as
 * comparison traces with distinct colors. All values handed to the graph are in registry (SI) units: the parser
 * converts each mapped channel from its MoTeC unit and resamples it by time onto one timeline.
 */
import { ch } from '../generated/channels';
import { StaticSource } from '../graph/staticSource';
import type { DataKind, ReferenceSeries } from '../graph/types';
import type { MotecLapView, ParsedMotecFile } from './motecTypes';

/**
 * Creates a StaticSource from a parsed MoTeC file (or one extracted lap) on a distance axis (default) or a time
 * axis. Returns null for the distance axis when the file has no usable distance.
 *
 * `dataKind`: an imported .ld file is a recording of real telemetry and stays 'lmu' (the graph's DataKind type
 * has no separate "imported" kind); generated demo/test data must pass 'synthetic'.
 */
export function createMotecGraphSource(
  file: MotecLapView,
  axis: 'distance' | 'time' = 'distance',
  dataKind: DataKind = 'lmu',
): StaticSource | null {
  const xs = axis === 'distance' ? file.distanceTimelineM : file.timeTimelineS;
  if (!xs) return null;

  const columns = new Map<number, Float64Array>();
  for (const [appId, chanData] of file.channelsByAppId) {
    columns.set(appId, chanData.samples);
  }

  return new StaticSource({
    xs,
    ts: file.timeTimelineS,
    columns,
    origin: 'recording',
    dataKind,
  });
}

/**
 * Creates an overlay ReferenceSeries for TelemetryGraph (palette slot 2 = orange/amber by default).
 */
export function createMotecReferenceSeries(
  file: ParsedMotecFile,
  channelId: number,
  source: StaticSource,
  colorSlot = 2,
): ReferenceSeries {
  const chan = file.channelsByAppId.get(channelId);
  const chanName = chan?.meta.name ?? `Ch_${channelId}`;
  const lapText = file.fastestLap ? `lap ${file.fastestLap.lapNumber}` : 'whole file';

  return {
    key: `motec-ref-${channelId}`,
    label: `[MoTeC] ${chanName} (${lapText})`,
    channelId,
    source,
    color: colorSlot,
  };
}

/**
 * Samples a MoTeC channel (registry units) at a given lap distance in metres. Returns null when the channel or
 * distance is unavailable, the distance is outside the lap, or the sample has no valid value.
 */
export function sampleMotecAtDistance(file: MotecLapView, channelId: number, distM: number): number | null {
  const chan = file.channelsByAppId.get(channelId);
  const xs = file.distanceTimelineM;
  if (!chan || chan.samples.length === 0 || !xs || xs.length === 0 || !Number.isFinite(distM)) return null;
  if (file.maxDistanceM === null || distM < 0 || distM > file.maxDistanceM) return null;

  let lo = 0;
  let hi = xs.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const v = xs[mid] ?? Number.NaN;
    if (v < distM) lo = mid + 1;
    else hi = mid - 1;
  }
  if (lo > xs.length - 1) return null;
  const value = chan.samples[lo];
  return value !== undefined && Number.isFinite(value) ? value : null;
}

export interface MotecLiveSample {
  /** m/s */
  readonly speedMps: number | null;
  /** 0..1 */
  readonly throttleFrac: number | null;
  /** 0..1 */
  readonly brakeFrac: number | null;
}

export interface MotecDelta {
  readonly motecSpeedMps: number | null;
  readonly deltaSpeedMps: number | null;
  readonly motecThrottleFrac: number | null;
  readonly deltaThrottleFrac: number | null;
  readonly motecBrakeFrac: number | null;
  readonly deltaBrakeFrac: number | null;
}

function diff(live: number | null, ref: number | null): number | null {
  return live !== null && ref !== null && Number.isFinite(live) ? live - ref : null;
}

/**
 * Live minus MoTeC reference at a lap distance, in registry units. Any side missing -> null (never 0).
 */
export function computeMotecDelta(file: MotecLapView, distM: number, live: MotecLiveSample): MotecDelta {
  const refSpeed = sampleMotecAtDistance(file, ch.SPEED, distM);
  const refThrottle = sampleMotecAtDistance(file, ch.THROTTLE, distM);
  const refBrake = sampleMotecAtDistance(file, ch.BRAKE, distM);

  return {
    motecSpeedMps: refSpeed,
    deltaSpeedMps: diff(live.speedMps, refSpeed),
    motecThrottleFrac: refThrottle,
    deltaThrottleFrac: diff(live.throttleFrac, refThrottle),
    motecBrakeFrac: refBrake,
    deltaBrakeFrac: diff(live.brakeFrac, refBrake),
  };
}
