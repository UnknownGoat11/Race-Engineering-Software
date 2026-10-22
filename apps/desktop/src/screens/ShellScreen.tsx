import { useState, type JSX } from 'react';
import { ConfigIssueBanner } from '../components/ConfigIssueBanner';
import { PanelErrorBoundary } from '../components/ErrorBoundary';
import { useCoreStatus } from '../hooks/useCoreStatus';
import { logout, type AppInfo, type ConfigSnapshot, type LoginStateDto, type Mode } from '../lib/ipc';
import { COPY, environmentLabel, modeLabel } from '../lib/labels';
import { DriverScreen } from './DriverScreen';
import { EngineerScreen } from './EngineerScreen';
import { SettingsPage } from './SettingsPage';
import { ViewsScreen } from './ViewsScreen';
import type { ViewId } from '../views/types';

export type ShellPage = 'views' | 'status' | 'pitwall' | 'settings';

interface ShellScreenProps {
  readonly info: AppInfo;
  readonly snapshot: ConfigSnapshot;
  readonly mode: Mode;
  readonly onChangeMode: (mode: Mode) => Promise<void>;
  readonly loginState?: LoginStateDto | null;
  readonly onLogout?: () => Promise<void> | void;
  readonly onRequireLogin?: () => void;
  readonly initialPage?: ShellPage;
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
  initialPage,
}: ShellScreenProps): JSX.Element {
  const queryParam = typeof window !== 'undefined' ? new URLSearchParams(window.location.search) : null;
  const pageParam = queryParam?.get('page') as ShellPage | null;
  const viewParam = queryParam?.get('view') as ViewId | null;

  // The home page of each mode: drivers land on the driver page, engineers on the pitwall (no lobby).
  const homePage: ShellPage = mode === 'driver' ? 'status' : 'pitwall';
  // Pages this mode can show. Analysis (the views suite) is for engineers only.
  const allowedPages: readonly ShellPage[] =
    mode === 'driver' ? ['status', 'settings'] : ['pitwall', 'views', 'settings'];
  const isAllowed = (candidate: ShellPage | null | undefined): candidate is ShellPage =>
    candidate !== null && candidate !== undefined && allowedPages.includes(candidate);

  const defaultPage: ShellPage = isAllowed(initialPage) ? initialPage : isAllowed(pageParam) ? pageParam : homePage;
  const [requestedPage, setPage] = useState<ShellPage>(defaultPage);
  // A mode change can leave a page that the new mode does not offer: fall back to the mode's home page.
  const page: ShellPage = isAllowed(requestedPage) ? requestedPage : homePage;
  const [loggingOut, setLoggingOut] = useState(false);
  const [selectedViewId] = useState<ViewId>(viewParam ?? 'driving');

  const pages: readonly { readonly id: ShellPage; readonly label: string }[] =
    mode === 'driver'
      ? [
          { id: 'status', label: COPY.driver },
          { id: 'settings', label: COPY.settings },
        ]
      : [
          { id: 'pitwall', label: COPY.pitwall },
          { id: 'views', label: COPY.analysis },
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
  const { status } = useCoreStatus();
  const isEngineerConnected =
    status?.engineer?.phase === 'monitoring' || status?.engineer?.phase === 'ended';
  const hideShellHeader = mode === 'engineer' && page === 'pitwall' && isEngineerConnected;

  return (
    <div className={hideShellHeader ? 'shell shell--bare' : 'shell'}>
      {!hideShellHeader && (
        <header className="shell-header">
          <span className="shell-title">Race Engineering Tool</span>
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
      )}

      {snapshot.issue !== null && (
        <div className="shell-banner">
          <ConfigIssueBanner issue={snapshot.issue} />
        </div>
      )}

      <main className={`shell-content ${page === 'settings' ? 'shell-content--page' : 'shell-content--screen'}`}>
        {mode === 'engineer' && (
          // Kept mounted while Settings or Analysis is open: the alert strip, session log and telemetry store
          // live in the pitwall, and the core sends alerts only when they are raised or cleared.
          <div className="shell-keepalive" hidden={page !== 'pitwall'} data-testid="engineer-keepalive">
            <PanelErrorBoundary name="Engineer Screen">
              <EngineerScreen
                onOpenSettings={() => {
                  setPage('settings');
                }}
                onOpenAnalysis={() => {
                  setPage('views');
                }}
              />
            </PanelErrorBoundary>
          </div>
        )}
        {page === 'settings' ? (
          <PanelErrorBoundary name="Settings">
            <SettingsPage info={info} snapshot={snapshot} mode={mode} onChangeMode={onChangeMode} />
          </PanelErrorBoundary>
        ) : page === 'views' ? (
          <PanelErrorBoundary name="Analysis">
            <ViewsScreen initialViewId={selectedViewId} />
          </PanelErrorBoundary>
        ) : mode === 'driver' ? (
          <PanelErrorBoundary name="Driver Screen">
            {onRequireLogin ? <DriverScreen onRequireLogin={onRequireLogin} /> : <DriverScreen />}
          </PanelErrorBoundary>
        ) : null}
      </main>
    </div>
  );
}
