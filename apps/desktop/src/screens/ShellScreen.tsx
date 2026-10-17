import { useState, type JSX } from 'react';
import { ConfigIssueBanner } from '../components/ConfigIssueBanner';
import { PanelErrorBoundary } from '../components/ErrorBoundary';
import type { AppInfo, ConfigSnapshot, Mode } from '../lib/ipc';
import { environmentLabel, modeLabel } from '../lib/labels';
import { DriverScreen } from './DriverScreen';
import { EngineerScreen } from './EngineerScreen';
import { SettingsPage } from './SettingsPage';

type Page = 'overview' | 'settings';

const PAGES: readonly { readonly id: Page; readonly label: string }[] = [
  { id: 'overview', label: 'Overview' },
  { id: 'settings', label: 'Settings' },
];

interface ShellScreenProps {
  readonly info: AppInfo;
  readonly snapshot: ConfigSnapshot;
  readonly mode: Mode;
  readonly onChangeMode: (mode: Mode) => Promise<void>;
}

/** Minimal application shell: header (mode, version, environment), page navigation, content. */
export function ShellScreen({ info, snapshot, mode, onChangeMode }: ShellScreenProps): JSX.Element {
  const [page, setPage] = useState<Page>('overview');

  return (
    <div className="shell">
      <header className="shell-header">
        <span className="shell-title">LMU RACE ENGINEERING</span>
        <span className="badge badge-mode" data-testid="header-mode">
          {modeLabel(mode)}
        </span>
        <nav className="shell-nav" aria-label="Main">
          {PAGES.map((entry) => (
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
          <span className="num" data-testid="header-version">
            v{info.version}
          </span>
          <span className={`badge badge-env-${info.environment}`} data-testid="header-environment">
            {environmentLabel(info.environment)}
          </span>
        </div>
      </header>

      <div className="shell-banner">{snapshot.issue !== null && <ConfigIssueBanner issue={snapshot.issue} />}</div>

      <main className={`shell-content ${page === 'overview' ? 'shell-content--screen' : 'shell-content--page'}`}>
        {page === 'overview' ? (
          mode === 'driver' ? (
            <PanelErrorBoundary name="Driver Screen">
              <DriverScreen />
            </PanelErrorBoundary>
          ) : (
            <PanelErrorBoundary name="Engineer Screen">
              <EngineerScreen />
            </PanelErrorBoundary>
          )
        ) : (
          <PanelErrorBoundary name="Settings">
            <SettingsPage info={info} snapshot={snapshot} mode={mode} onChangeMode={onChangeMode} />
          </PanelErrorBoundary>
        )}
      </main>
    </div>
  );
}
