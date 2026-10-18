import { useCallback, useEffect, useState, type JSX } from 'react';
import { useLoginState } from './hooks/useLoginState';
import { getAppInfo, getConfig, setMode, type AppInfo, type ConfigSnapshot, type Mode } from './lib/ipc';
import { LoginScreen } from './screens/LoginScreen';
import { ModeSelectScreen } from './screens/ModeSelectScreen';
import { ShellScreen } from './screens/ShellScreen';

type LoadState =
  | { readonly kind: 'loading' }
  | { readonly kind: 'error'; readonly message: string }
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
    Promise.all([getAppInfo(), getConfig()]).then(
      ([info, snapshot]) => {
        if (!cancelled) setState({ kind: 'ready', info, snapshot });
      },
      (error: unknown) => {
        if (!cancelled) setState({ kind: 'error', message: errorMessage(error) });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [loadAttempt]);

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
        <main className="fullscreen-state" aria-busy="true">
          <p>Loading…</p>
        </main>
      );
    case 'error':
      return (
        <main className="fullscreen-state" role="alert">
          <h1>Backend unavailable</h1>
          <pre>{state.message}</pre>
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
        </main>
      );
    case 'ready': {
      const mode = state.snapshot.config.application.mode;
      if (mode === null) {
        return <ModeSelectScreen onSelect={changeMode} />;
      }
      if (login.loading && login.state === null) {
        return (
          <main className="fullscreen-state" aria-busy="true">
            <p>Loading…</p>
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
