import { isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';

export interface CompetitorEntryDto {
  readonly slot_id: number;
  readonly driver: string | null;
  readonly vehicle_name: string | null;
  readonly vehicle_filename: string | null;
  readonly class_name: string | null;
  readonly pit_group: string | null;
  readonly place: number | null;
  readonly class_place: number | null;
  readonly laps: number | null;
  readonly lap_dist_m: number | null;
  readonly sector: number | null;
  readonly best_lap_s: number | null;
  readonly last_lap_s: number | null;
  readonly best_sector1_s: number | null;
  readonly best_sector12_s: number | null;
  readonly last_sector1_s: number | null;
  readonly last_sector12_s: number | null;
  readonly time_behind_next_s: number | null;
  readonly laps_behind_next: number | null;
  readonly time_behind_leader_s: number | null;
  readonly laps_behind_leader: number | null;
  readonly in_pits: boolean | null;
  readonly in_garage_stall: boolean | null;
  readonly pit_state: string | null;
  readonly num_pitstops: number | null;
  readonly is_player: boolean;
  readonly pos_x_m: number | null;
  readonly pos_z_m: number | null;
}

export interface SessionScoringDto {
  readonly track_name: string | null;
  readonly session_type: string | null;
  readonly session_time_remaining_s: number | null;
  readonly end_time_s: number | null;
  readonly lap_limit: number | null;
  readonly time_limit_s: number | null;
  readonly yellow_flag_state: string | null;
  readonly sector_flag?: readonly (string | null)[];
  readonly dark_cloud?: number | null;
  readonly raining?: number | null;
  readonly ambient_temp_c?: number | null;
  readonly track_temp_c?: number | null;
}

export interface ScoringStateDto {
  readonly session: SessionScoringDto;
  readonly vehicles: readonly CompetitorEntryDto[];
}

/**
 * Hook to subscribe to `core:scoring` (5 Hz) emitted across the Tauri bridge.
 * Cleanly unlistens on unmount.
 */
export function useScoring(): ScoringStateDto | null {
  const [scoring, setScoring] = useState<ScoringStateDto | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: UnlistenFn | null = null;
    let disposed = false;

    listen<ScoringStateDto>('core:scoring', (event) => {
      if (!disposed) {
        setScoring(event.payload);
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
        console.warn('Failed to listen to core:scoring', err);
      });

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
        unlisten = null;
      }
    };
  }, []);

  return scoring;
}
