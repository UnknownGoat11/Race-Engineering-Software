import { useEffect, useMemo, useRef, useState } from 'react';
import { ch } from '../../generated/channels';
import type { ScoringStateDto } from '../../hooks/useScoring';
import type { TelemetryStore } from '../../telemetry/store';
import { TrackGeometryBuilder } from './trackGeometry';
import type { TrackLengthSource, TrackMap } from './types';

export interface UseTrackGeometryResult {
  readonly map: TrackMap | null;
  readonly progress: number;
  readonly lengthSource: TrackLengthSource;
}

/**
 * Hook to reconstruct online track geometry from live TelemetryStore frames and Scoring frames.
 * Returns the locked `TrackMap` once one valid lap has been completed, or the current reconstruction
 * progress [0, 1].
 */
export function useTrackGeometry(
  store?: TelemetryStore | null,
  scoring?: ScoringStateDto | null,
  trackLengthM?: number | null,
  trackName?: string | null,
): UseTrackGeometryResult {
  const builder = useMemo(() => new TrackGeometryBuilder({ minBinsCoveredRatio: 0.65 }), []);
  const lastProcessedTimeRef = useRef<number>(-Infinity);
  const [result, setResult] = useState<UseTrackGeometryResult>(() => ({
    map: null,
    progress: 0,
    lengthSource: 'observed',
  }));

  // Track name override
  useEffect(() => {
    if (trackName) {
      builder.setTrackName(trackName);
    } else if (scoring?.session.track_name) {
      builder.setTrackName(scoring.session.track_name);
    }
  }, [builder, trackName, scoring?.session.track_name]);

  // Track length override
  useEffect(() => {
    if (trackLengthM && trackLengthM > 0) {
      builder.setTrackLength(trackLengthM, 'reported');
    }
  }, [builder, trackLengthM]);

  // Ingest from store
  useEffect(() => {
    if (!store) return;

    // Check TRACK_LENGTH channel sample
    const trackLenSample = store.getSample(ch.TRACK_LENGTH);
    if (trackLenSample.value !== null && typeof trackLenSample.value === 'number' && trackLenSample.value > 0) {
      builder.setTrackLength(trackLenSample.value, 'reported');
    }

    const ingestFromRing = () => {
      if (builder.isLocked()) return;
      const ring = store.ring;
      const len = ring.length;
      if (len === 0) return;

      const posXId = ch.POS_X;
      const posZId = ch.POS_Z;
      const lapDistId = ch.LAP_DISTANCE;

      let latestTime = lastProcessedTimeRef.current;
      for (let i = 0; i < len; i++) {
        const t = ring.timeAt(i);
        if (t === null || t <= lastProcessedTimeRef.current) continue;
        const x = ring.valueAt(posXId, i);
        const z = ring.valueAt(posZId, i);
        const dist = ring.valueAt(lapDistId, i);
        if (x !== null && z !== null && dist !== null) {
          builder.ingest(x, z, dist);
          if (t > latestTime) latestTime = t;
        }
      }
      lastProcessedTimeRef.current = latestTime;

      const built = builder.build();
      if (built) {
        setResult({
          map: built,
          progress: 1,
          lengthSource: builder.getTrackLengthSource(),
        });
      } else {
        const prog = builder.progress();
        setResult((prev) => (Math.abs(prev.progress - prog) > 0.02 ? { ...prev, progress: prog } : prev));
      }
    };

    // Ingest any rows currently in the ring
    ingestFromRing();

    // Subscribe to frame-based ring notifications
    const unlistenRing = store.subscribeRing(() => {
      ingestFromRing();
    });

    return () => {
      unlistenRing();
    };
  }, [store, builder]);

  // Ingest from scoring vehicles
  useEffect(() => {
    if (!scoring || builder.isLocked()) return;

    if (scoring.session.track_name) {
      builder.setTrackName(scoring.session.track_name);
    }

    if (scoring.vehicles.length > 0) {
      for (const v of scoring.vehicles) {
        if (v.pos_x_m !== null && v.pos_z_m !== null && v.lap_dist_m !== null) {
          builder.ingest(v.pos_x_m, v.pos_z_m, v.lap_dist_m);
        }
      }

      const built = builder.build();
      queueMicrotask(() => {
        if (built) {
          setResult({
            map: built,
            progress: 1,
            lengthSource: builder.getTrackLengthSource(),
          });
        } else {
          const prog = builder.progress();
          setResult((prev) => (Math.abs(prev.progress - prog) > 0.02 ? { ...prev, progress: prog } : prev));
        }
      });
    }
  }, [scoring, builder]);

  return result;
}
