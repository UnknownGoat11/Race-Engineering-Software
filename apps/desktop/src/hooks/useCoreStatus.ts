/**
 * React hook for consuming real-time `CoreStatus` (4 Hz) from the backend.
 * Provides fallback states when running in offline/mock/browser environments.
 */
import { useEffect, useState } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCoreStatus, type CoreStatus } from '../lib/ipc';

export interface UseCoreStatusResult {
  readonly status: CoreStatus | null;
  readonly error: string | null;
  readonly refresh: () => Promise<void>;
}

export function useCoreStatus(): UseCoreStatusResult {
  const [status, setStatus] = useState<CoreStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const res = await getCoreStatus();
      setStatus(res);
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let mounted = true;

    // Initial fetch
    getCoreStatus()
      .then((res) => {
        if (mounted) {
          setStatus(res);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (mounted) {
          setError(err instanceof Error ? err.message : String(err));
        }
      });

    // Event listener
    if (isTauri()) {
      listen<CoreStatus>('core:status', (event) => {
        if (mounted) {
          setStatus(event.payload);
        }
      })
        .then((fn) => {
          if (!mounted) {
            fn();
          } else {
            unlisten = fn;
          }
        })
        .catch((err: unknown) => {
          if (mounted) {
            setError(err instanceof Error ? err.message : String(err));
          }
        });
    }

    return () => {
      mounted = false;
      if (unlisten) {
        unlisten();
      }
    };
  }, []);

  return { status, error, refresh };
}
