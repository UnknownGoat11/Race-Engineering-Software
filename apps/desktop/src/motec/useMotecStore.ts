/**
 * Global MoTeC Telemetry Store & React Hook.
 *
 * Manages the imported MoTeC .ld session, generates ReferenceSeries for TelemetryGraph,
 * and calculates real-time deltas against live telemetry.
 */
import { useSyncExternalStore, useMemo } from 'react';
import { parseMotecLd } from './motecParser';
import { createMotecGraphSource, createMotecReferenceSeries, sampleMotecAtDistance } from './motecSource';
import type { ParsedMotecFile } from './motecTypes';
import { generateSpaBenchmarkMotecBuffer } from './motecWriter';
import type { ReferenceSeries } from '../graph/types';
import type { StaticSource } from '../graph/staticSource';

interface MotecState {
  parsedFile: ParsedMotecFile | null;
  graphSource: StaticSource | null;
  isLoading: boolean;
  error: string | null;
  isOverlayEnabled: boolean;
}

let state: MotecState = {
  parsedFile: null,
  graphSource: null,
  isLoading: false,
  error: null,
  isOverlayEnabled: true,
};

const listeners = new Set<() => void>();

function emit(): void {
  for (const l of listeners) l();
}

export const motecStore = {
  getState(): MotecState {
    return state;
  },

  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },

  async loadFile(file: File, ldxFile?: File | string): Promise<void> {
    state = { ...state, isLoading: true, error: null };
    emit();
    try {
      const buffer = await file.arrayBuffer();
      let ldxText: string | undefined;
      if (typeof ldxFile === 'string') {
        ldxText = ldxFile;
      } else if (ldxFile instanceof File) {
        ldxText = await ldxFile.text();
      }
      const parsed = parseMotecLd(buffer, file.name, ldxText);
      const graphSource = createMotecGraphSource(parsed, 'distance');
      state = {
        parsedFile: parsed,
        graphSource,
        isLoading: false,
        error: null,
        isOverlayEnabled: true,
      };
      emit();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      state = { ...state, isLoading: false, error: msg };
      emit();
    }
  },

  async loadFiles(files: FileList | readonly File[]): Promise<void> {
    const fileArray = Array.from(files);
    const ldFile = fileArray.find((f) => f.name.toLowerCase().endsWith('.ld'));
    const ldxFile = fileArray.find((f) => f.name.toLowerCase().endsWith('.ldx'));

    if (!ldFile) {
      state = { ...state, error: 'No .ld file found in the selected files.' };
      emit();
      return;
    }

    await motecStore.loadFile(ldFile, ldxFile);
  },

  loadBuffer(buffer: ArrayBuffer | Uint8Array, filename = 'motec_log.ld', ldxContent?: string): void {
    try {
      const parsed = parseMotecLd(buffer, filename, ldxContent);
      const graphSource = createMotecGraphSource(parsed, 'distance');
      state = {
        parsedFile: parsed,
        graphSource,
        isLoading: false,
        error: null,
        isOverlayEnabled: true,
      };
      emit();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      state = { ...state, error: msg };
      emit();
    }
  },

  loadBenchmarkLap(): void {
    const buffer = generateSpaBenchmarkMotecBuffer();
    motecStore.loadBuffer(buffer, 'spa_ferrari_499p_pole.ld');
  },

  clear(): void {
    state = {
      parsedFile: null,
      graphSource: null,
      isLoading: false,
      error: null,
      isOverlayEnabled: false,
    };
    emit();
  },

  setOverlayEnabled(enabled: boolean): void {
    state = { ...state, isOverlayEnabled: enabled };
    emit();
  },
};

/**
 * Hook subscribing to the active MoTeC imported session.
 */
export function useMotecSession() {
  const current = useSyncExternalStore(motecStore.subscribe, motecStore.getState);

  return {
    parsedFile: current.parsedFile,
    graphSource: current.graphSource,
    isLoading: current.isLoading,
    error: current.error,
    isOverlayEnabled: current.isOverlayEnabled,
    loadFile: motecStore.loadFile,
    loadBenchmarkLap: motecStore.loadBenchmarkLap,
    clear: motecStore.clear,
    setOverlayEnabled: motecStore.setOverlayEnabled,
  };
}

/**
 * Returns TelemetryGraph ReferenceSeries array for the specified channels when
 * MoTeC overlay is loaded and enabled.
 */
export function useMotecReferences(channelIds: readonly number[]): readonly ReferenceSeries[] {
  const { parsedFile, graphSource, isOverlayEnabled } = useMotecSession();

  return useMemo(() => {
    if (!isOverlayEnabled || !parsedFile || !graphSource) {
      return [];
    }

    const refs: ReferenceSeries[] = [];
    for (const chId of channelIds) {
      if (parsedFile.channelsByAppId.has(chId)) {
        refs.push(createMotecReferenceSeries(parsedFile, chId, graphSource, 2)); // slot 2 = orange/amber
      }
    }
    return refs;
  }, [parsedFile, graphSource, isOverlayEnabled, channelIds]);
}

/**
 * Computes live delta for any lap distance.
 */
export function useMotecSample(channelId: number, distanceM: number): number | null {
  const { parsedFile, isOverlayEnabled } = useMotecSession();
  if (!isOverlayEnabled || !parsedFile) return null;
  return sampleMotecAtDistance(parsedFile, channelId, distanceM);
}
