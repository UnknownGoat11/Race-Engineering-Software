import { useCallback, useEffect, useState, type JSX } from 'react';
import { useLoginState } from './hooks/useLoginState';
import { getAppInfo, getConfig, setMode, type AppInfo, type ConfigSnapshot, type Mode } from './lib/ipc';
import { LoginScreen } from './screens/LoginScreen';
import { ModeSelectScreen } from './screens/ModeSelectScreen';
import { ShellScreen } from './screens/ShellScreen';

import './styles/entry.css';

type LoadState =
  | { readonly kind: 'loading' }
  | { readonly kind: 'error'; readonly message: string; readonly logDir: string | null }
  | { readonly kind: 'ready'; readonly info: AppInfo; readonly snapshot: ConfigSnapshot };

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function App(): JSX.Element {
  const [state, setState] = useState<LoadState>({ kind: 'loading' });
  // Incremented to re-run the initial load after a failure.
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [requireLogin, setRequireLogin] = useState(false);
  const login = useLoginState();

  useEffect(() => {
    let cancelled = false;
    async function load(): Promise<void> {
      let logDir: string | null = null;
      try {
        const info = await getAppInfo();
        logDir = info.log_dir;
        const snapshot = await getConfig();
        if (!cancelled) {
          setState({ kind: 'ready', info, snapshot });
        }
      } catch (error: unknown) {
        if (!cancelled) {
          setState({ kind: 'error', message: errorMessage(error), logDir });
        }
      }
    }
    void load();
    return () => {
      cancelled = true;
    };
  }, [loadAttempt]);

  useEffect(() => {
    const userScale = state.kind === 'ready' ? state.snapshot.config.ui.scale : 1.0;

    const applyScale = (): void => {
      if (typeof window === 'undefined' || typeof document === 'undefined') return;
      const width = window.innerWidth;
      let baseScale = 1.0;
      if (width >= 2400) {
        baseScale = 1.333;
      } else if (width <= 1440) {
        baseScale = 0.923;
      }
      const effectiveScale = baseScale * userScale;
      document.documentElement.style.setProperty('--ui-scale', effectiveScale.toFixed(4));
    };

    applyScale();
    window.addEventListener('resize', applyScale);
    return () => {
      window.removeEventListener('resize', applyScale);
    };
  }, [state]);

  const changeMode = useCallback(
    async (mode: Mode): Promise<void> => {
      // Errors propagate to the calling screen, which shows them next to the control that failed.
      const snapshot = await setMode(mode);
      setState((previous) => (previous.kind === 'ready' ? { ...previous, snapshot } : previous));
      setRequireLogin(false);
      await login.refresh();
    },
    [login],
  );

  switch (state.kind) {
    case 'loading':
      return (
        <main className="entry-fullscreen" role="status" aria-busy="true">
          <div className="entry-loading-container">
            <p className="entry-loading-text">Loading…</p>
          </div>
        </main>
      );
    case 'error':
      return (
        <main className="entry-fullscreen" role="alert">
          <div className="entry-error-panel" data-testid="backend-unavailable-panel">
            <div className="entry-error-panel__header">
              <h1>Backend unavailable</h1>
            </div>
            <div className="entry-error-panel__body">
              <p className="entry-error-message">
                The Race Engineering Tool backend process could not be contacted.
              </p>
              <pre className="entry-error-details">{state.message}</pre>
              <div className="entry-error-log">
                <span className="entry-error-log__label">Log directory:</span>
                <code className="entry-error-log__path" data-testid="backend-log-path">
                  {state.logDir ?? 'Default app logs directory'}
                </code>
              </div>
            </div>
            <div className="entry-error-panel__actions">
              <button
                type="button"
                className="btn btn-secondary"
                data-testid="copy-details-btn"
                onClick={() => {
                  void navigator.clipboard.writeText(
                    `Backend error: ${state.message}\nLog directory: ${state.logDir ?? 'unknown'}`
                  );
                }}
              >
                Copy details
              </button>
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => {
                  setState({ kind: 'loading' });
                  setLoadAttempt((attempt) => attempt + 1);
                }}
              >
                Retry
              </button>
            </div>
          </div>
        </main>
      );
    case 'ready': {
      const mode = state.snapshot.config.application.mode;
      if (mode === null) {
        return <ModeSelectScreen onSelect={changeMode} />;
      }
      if (login.loading && login.state === null) {
        return (
          <main className="entry-fullscreen" role="status" aria-busy="true">
            <div className="entry-loading-container">
              <p className="entry-loading-text">Loading…</p>
            </div>
          </main>
        );
      }
      if (!login.authenticated || requireLogin) {
        return (
          <LoginScreen
            mode={mode}
            info={state.info}
            onLoginSuccess={() => {
              setRequireLogin(false);
              void login.refresh();
            }}
            onChangeMode={changeMode}
          />
        );
      }
      return (
        <ShellScreen
          info={state.info}
          snapshot={state.snapshot}
          mode={mode}
          onChangeMode={changeMode}
          loginState={login.state}
          onLogout={login.refresh}
          onRequireLogin={() => {
            setRequireLogin(true);
          }}
        />
      );
    }
  }
}
