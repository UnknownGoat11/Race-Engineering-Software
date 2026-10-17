import { isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useEffect, useState, useCallback } from 'react';
import type { AlertDto } from '../lib/ipc';

export interface UseAlertsResult {
  readonly alerts: readonly AlertDto[];
  readonly dismissAlert: (id: string) => void;
  readonly clearAllAlerts: () => void;
}

/**
 * Hook to listen to `core:alert` events, maintaining an active list of alerts.
 */
export function useAlerts(): UseAlertsResult {
  const [alerts, setAlerts] = useState<readonly AlertDto[]>([]);

  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: UnlistenFn | null = null;
    let disposed = false;

    listen<AlertDto>('core:alert', (event) => {
      if (disposed) return;
      const alert = event.payload;

      setAlerts((prev) => {
        if (!alert.raised) {
          // Alert is cleared
          return prev.filter((a) => a.id !== alert.id);
        }
        // If alert already present, update it; otherwise append
        const index = prev.findIndex((a) => a.id === alert.id);
        if (index >= 0) {
          const next = [...prev];
          next[index] = alert;
          return next;
        }
        return [...prev, alert];
      });
    })
      .then((fn) => {
        if (disposed) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((err: unknown) => {
        console.warn('Failed to listen to core:alert', err);
      });

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
        unlisten = null;
      }
    };
  }, []);

  const dismissAlert = useCallback((id: string) => {
    setAlerts((prev) => prev.filter((a) => a.id !== id));
  }, []);

  const clearAllAlerts = useCallback(() => {
    setAlerts([]);
  }, []);

  return { alerts, dismissAlert, clearAllAlerts };
}
