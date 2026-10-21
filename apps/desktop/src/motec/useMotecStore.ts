/**
 * Global MoTeC Telemetry Store & React Hook.
 *
 * Manages the imported MoTeC .ld session, generates ReferenceSeries for TelemetryGraph,
 * and samples the reference at a lap distance.
 *
 * A failed load clears any previously loaded file, so a stale overlay is never shown next to an error.
 */
import { useSyncExternalStore, useMemo } from 'react';
import { parseMotecLd } from './motecParser';
import { createMotecGraphSource, createMotecReferenceSeries, sampleMotecAtDistance } from './motecSource';
import type { ParsedMotecFile } from './motecTypes';
import type { DataKind, ReferenceSeries } from '../graph/types';
import type { StaticSource } from '../graph/staticSource';

interface MotecState {
  parsedFile: ParsedMotecFile | null;
  /** Distance-axis source, or null when the file has no usable distance. */
  graphSource: StaticSource | null;
  /** Time-axis source (always available once a file is loaded). */
  timeGraphSource: StaticSource | null;
  /** 'synthetic' for generated data, 'lmu' for an imported recording. */
  dataKind: DataKind | null;
  isLoading: boolean;
  error: string | null;
  isOverlayEnabled: boolean;
}

const EMPTY: MotecState = {
  parsedFile: null,
  graphSource: null,
  timeGraphSource: null,
  dataKind: null,
  isLoading: false,
  error: null,
  isOverlayEnabled: false,
};

let state: MotecState = EMPTY;

const listeners = new Set<() => void>();

function emit(): void {
  for (const l of listeners) l();
}

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export interface MotecLoadOptions {
  readonly ldxContent?: string;
  /** Default 'lmu' (an imported recording). Generated data must pass 'synthetic'. */
  readonly dataKind?: DataKind;
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

  /** Shows an error and unloads any previous file. */
  setError(message: string): void {
    state = { ...EMPTY, error: message };
    emit();
  },

  async loadFile(file: File, ldxFile?: File | string): Promise<void> {
    state = { ...state, isLoading: true, error: null };
    emit();
    try {
      if (file.size === 0) throw new Error(`"${file.name}" is empty (0 bytes).`);
      const buffer = await file.arrayBuffer();
      let ldxText: string | undefined;
      if (typeof ldxFile === 'string') {
        ldxText = ldxFile;
      } else if (ldxFile instanceof File) {
        ldxText = await ldxFile.text();
      }
      motecStore.loadBuffer(buffer, file.name, ldxText === undefined ? {} : { ldxContent: ldxText });
    } catch (err) {
      motecStore.setError(messageOf(err));
    }
  },

  async loadFiles(files: FileList | readonly File[]): Promise<void> {
    const fileArray = Array.from(files);
    const ldFile = fileArray.find((f) => f.name.toLowerCase().endsWith('.ld'));
    const ldxFile = fileArray.find((f) => f.name.toLowerCase().endsWith('.ldx'));

    if (!ldFile) {
      motecStore.setError('No .ld file found in the selected files.');
      return;
    }

    await motecStore.loadFile(ldFile, ldxFile);
  },

  loadBuffer(buffer: ArrayBuffer | Uint8Array, filename = 'motec_log.ld', options: MotecLoadOptions = {}): void {
    try {
      if (buffer.byteLength === 0) throw new Error(`"${filename}" is empty (0 bytes).`);
      const parsed = parseMotecLd(buffer, filename, options.ldxContent);
      const dataKind = options.dataKind ?? 'lmu';
      state = {
        parsedFile: parsed,
        graphSource: createMotecGraphSource(parsed, 'distance', dataKind),
        timeGraphSource: createMotecGraphSource(parsed, 'time', dataKind),
        dataKind,
        isLoading: false,
        error: null,
        isOverlayEnabled: true,
      };
      emit();
    } catch (err) {
      motecStore.setError(messageOf(err));
    }
  },

  clear(): void {
    state = EMPTY;
    emit();
  },

  setOverlayEnabled(enabled: boolean): void {
    state = { ...state, isOverlayEnabled: enabled };
    emit();
  },
};

const subscribe = (listener: () => void): (() => void) => motecStore.subscribe(listener);
const getState = (): MotecState => motecStore.getState();

/**
 * Hook subscribing to the active MoTeC imported session.
 */
export function useMotecSession() {
  const current = useSyncExternalStore(subscribe, getState);

  return {
    parsedFile: current.parsedFile,
    graphSource: current.graphSource,
    timeGraphSource: current.timeGraphSource,
    dataKind: current.dataKind,
    isLoading: current.isLoading,
    error: current.error,
    isOverlayEnabled: current.isOverlayEnabled,
    loadFile: (file: File, ldxFile?: File | string) => motecStore.loadFile(file, ldxFile),
    clear: () => {
      motecStore.clear();
    },
    setOverlayEnabled: (enabled: boolean) => {
      motecStore.setOverlayEnabled(enabled);
    },
  };
}

/**
 * Returns TelemetryGraph ReferenceSeries array for the specified channels when MoTeC overlay is loaded and
 * enabled. `axis` must match the graph's x axis: 'distance' (default) or 'time'. No references are returned on the
 * distance axis when the file has no usable distance.
 */
export function useMotecReferences(
  channelIds: readonly number[],
  axis: 'distance' | 'time' = 'distance',
): readonly ReferenceSeries[] {
  const { parsedFile, graphSource, timeGraphSource, isOverlayEnabled } = useMotecSession();
  const source = axis === 'distance' ? graphSource : timeGraphSource;

  return useMemo(() => {
    if (!isOverlayEnabled || !parsedFile || !source) {
      return [];
    }

    const refs: ReferenceSeries[] = [];
    for (const chId of channelIds) {
      if (parsedFile.channelsByAppId.has(chId)) {
        refs.push(createMotecReferenceSeries(parsedFile, chId, source, 2)); // slot 2 = orange/amber
      }
    }
    return refs;
  }, [parsedFile, source, isOverlayEnabled, channelIds]);
}

/**
 * MoTeC reference value (registry units) at a lap distance, or null.
 */
export function useMotecSample(channelId: number, distanceM: number): number | null {
  const { parsedFile, isOverlayEnabled } = useMotecSession();
  if (!isOverlayEnabled || !parsedFile) return null;
  return sampleMotecAtDistance(parsedFile, channelId, distanceM);
}
