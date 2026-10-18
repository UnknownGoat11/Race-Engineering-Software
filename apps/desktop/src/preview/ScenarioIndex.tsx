/**
 * PREVIEW HARNESS ONLY. The scenario index (`/` or `?scenario`): every scenario as a link, grouped by screen.
 */
import type { JSX } from 'react';
import { SCENARIOS, type ScenarioGroup } from './scenarios';

const GROUP_TITLES: Record<ScenarioGroup, string> = {
  driver: 'Driver screen',
  engineer: 'Engineer screen',
  mode: 'Mode select',
  settings: 'Settings',
  app: 'App states',
};

const GROUP_ORDER: readonly ScenarioGroup[] = ['driver', 'engineer', 'mode', 'settings', 'app'];

export function ScenarioIndex({ unknownQuery }: { readonly unknownQuery: string | null }): JSX.Element {
  return (
    <main className="preview-index">
      <header className="preview-index-header">
        <h1>LMU Race Engineering — UI preview harness</h1>
        <span className="status-badge status-badge-warn">SYNTHETIC DATA · DEV ONLY</span>
      </header>
      <p className="panel-note">
        Every scenario runs the real app screens against a fake backend with synthetic data (never LMU data).
        Scenarios marked <em>simulated LMU origin</em> report <code>origin = lmu</code> to show the live-data look;
        their data is still synthetic and their pages carry a PREVIEW tag.
        Select with <code>?scenario=&lt;id&gt;</code> or <code>?screen=&lt;group&gt;&amp;state=&lt;state&gt;</code>.
      </p>
      {unknownQuery !== null && (
        <div className="alert alert-error" role="alert">
          <span className="alert-title">Unknown scenario</span>
          <span>
            <code>{unknownQuery}</code> matches no scenario.
          </span>
        </div>
      )}
      {GROUP_ORDER.map((group) => (
        <section key={group} className="panel">
          <div className="panel-header">
            <span>{GROUP_TITLES[group]}</span>
            <span className="num">{SCENARIOS.filter((s) => s.group === group).length}</span>
          </div>
          <div className="panel-body">
            <table className="preview-index-table">
              <tbody>
                {SCENARIOS.filter((s) => s.group === group).map((s) => (
                  <tr key={s.id}>
                    <td>
                      <a href={`?scenario=${s.id}`}>{s.title}</a>
                      {s.origin === 'lmu' && (
                        <span className="preview-origin-lmu" data-testid={`origin-lmu-${s.id}`}>
                          simulated LMU origin — preview only
                        </span>
                      )}
                    </td>
                    <td>
                      <code>
                        ?screen={s.group}&amp;state={s.state}
                      </code>
                    </td>
                    <td className="panel-note">{s.description}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      ))}
    </main>
  );
}
