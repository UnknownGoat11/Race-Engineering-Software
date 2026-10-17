import { isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';

export interface TimingSnapshotDto {
  readonly current_lap: number;
  readonly last_lap_time_s: number | null;
  readonly best_lap_time_s: number | null;
  readonly predicted_lap_time_s: number | null;
  readonly delta_pb_s: number | null;
  readonly s1_time_s: number | null;
  readonly s2_time_s: number | null;
  readonly s3_time_s: number | null;
}

export interface FuelEstimateDto {
  readonly remaining_litres: number;
  readonly avg_consumption_per_lap: number | null;
  readonly laps_remaining: number | null;
  readonly fuel_to_finish: number | null;
  readonly fuel_to_add: number | null;
}

export interface StintSnapshotDto {
  readonly current_stint: number;
  readonly stint_laps: number;
  readonly stint_start_time_s: number;
  readonly tyre_age_laps: number;
}

export interface EnergySnapshotDto {
  readonly virtual_energy_remaining: number | null;
  readonly virtual_energy_per_lap: number | null;
  readonly virtual_energy_laps_remaining: number | null;
  readonly battery_charge_pct: number | null;
}

export interface EngineStateDto {
  readonly timing?: TimingSnapshotDto;
  readonly fuel?: FuelEstimateDto;
  readonly stint?: StintSnapshotDto;
  readonly energy?: EnergySnapshotDto;
  readonly tyre?: Record<string, unknown>;
  readonly brake?: Record<string, unknown>;
  readonly aids?: Record<string, unknown>;
}

/**
 * Hook to subscribe to `core:engine_state` (2 Hz) emitted across the Tauri bridge.
 * Cleanly unlistens on unmount.
 */
export function useEngineState(): EngineStateDto | null {
  const [state, setState] = useState<EngineStateDto | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: UnlistenFn | null = null;
    let disposed = false;

    listen<EngineStateDto>('core:engine_state', (event) => {
      if (!disposed) {
        setState(event.payload);
      }
    })
      .then((fn) => {
        if (disposed) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((err: unknown) => {
        console.warn('Failed to listen to core:engine_state', err);
      });

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
        unlisten = null;
      }
    };
  }, []);

  return state;
}
