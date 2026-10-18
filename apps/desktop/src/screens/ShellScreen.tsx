import { useState, type JSX } from 'react';
import { ConfigIssueBanner } from '../components/ConfigIssueBanner';
import { PanelErrorBoundary } from '../components/ErrorBoundary';
import { logout, type AppInfo, type ConfigSnapshot, type LoginStateDto, type Mode } from '../lib/ipc';
import { COPY, environmentLabel, modeLabel } from '../lib/labels';
import { DriverScreen } from './DriverScreen';
import { EngineerScreen } from './EngineerScreen';
import { SettingsPage } from './SettingsPage';

export type ShellPage = 'status' | 'pitwall' | 'settings';

interface ShellScreenProps {
  readonly info: AppInfo;
  readonly snapshot: ConfigSnapshot;
  readonly mode: Mode;
  readonly onChangeMode: (mode: Mode) => Promise<void>;
  readonly loginState?: LoginStateDto | null;
  readonly onLogout?: () => Promise<void> | void;
  readonly onRequireLogin?: () => void;
}

/** Minimal application shell: header (mode, version, environment, account), page navigation, content. */
export function ShellScreen({
  info,
  snapshot,
  mode,
  onChangeMode,
  loginState,
  onLogout,
  onRequireLogin,
}: ShellScreenProps): JSX.Element {
  const defaultPage: ShellPage = mode === 'driver' ? 'status' : 'pitwall';
  const [page, setPage] = useState<ShellPage>(defaultPage);
  const [loggingOut, setLoggingOut] = useState(false);

  const pages: readonly { readonly id: ShellPage; readonly label: string }[] =
    mode === 'driver'
      ? [
          { id: 'status', label: COPY.status },
          { id: 'settings', label: COPY.settings },
        ]
      : [
          { id: 'pitwall', label: COPY.pitwall },
          { id: 'settings', label: COPY.settings },
        ];

  const handleLogout = async (): Promise<void> => {
    setLoggingOut(true);
    try {
      await logout();
    } catch {
      // Ignore logout backend error and proceed to notify
    } finally {
      if (onLogout) {
        await onLogout();
      }
      setLoggingOut(false);
    }
  };

  const accountDisplayName =
    loginState?.account?.display_name ?? loginState?.account?.username ?? null;

  return (
    <div className="shell">
      <header className="shell-header">
        <span className="shell-title">LMU RACE ENGINEERING</span>
        <span className="badge badge-mode" data-testid="header-mode">
          {modeLabel(mode)}
        </span>
        <nav className="shell-nav" aria-label="Main">
          {pages.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className="btn"
              aria-current={page === entry.id ? 'page' : undefined}
              onClick={() => {
                setPage(entry.id);
              }}
            >
              {entry.label}
            </button>
          ))}
        </nav>
        <div className="shell-meta">
          {accountDisplayName && (
            <span className="badge badge-subtle" data-testid="header-account">
              {accountDisplayName}
            </span>
          )}
          {loginState?.authenticated && (
            <button
              type="button"
              className="btn btn-sm"
              disabled={loggingOut}
              onClick={() => {
                void handleLogout();
              }}
              data-testid="header-logout"
            >
              {COPY.logOut}
            </button>
          )}
          <span className="num" data-testid="header-version">
            v{info.version}
          </span>
          <span className={`badge badge-env-${info.environment}`} data-testid="header-environment">
            {environmentLabel(info.environment)}
          </span>
        </div>
      </header>

      <div className="shell-banner">{snapshot.issue !== null && <ConfigIssueBanner issue={snapshot.issue} />}</div>

      <main className={`shell-content ${page === 'settings' ? 'shell-content--page' : 'shell-content--screen'}`}>
        {page === 'settings' ? (
          <PanelErrorBoundary name="Settings">
            <SettingsPage info={info} snapshot={snapshot} mode={mode} onChangeMode={onChangeMode} />
          </PanelErrorBoundary>
        ) : mode === 'driver' ? (
          <PanelErrorBoundary name="Driver Screen">
            {onRequireLogin ? <DriverScreen onRequireLogin={onRequireLogin} /> : <DriverScreen />}
          </PanelErrorBoundary>
        ) : (
          <PanelErrorBoundary name="Engineer Screen">
            <EngineerScreen />
          </PanelErrorBoundary>
        )}
      </main>
    </div>
  );
}
