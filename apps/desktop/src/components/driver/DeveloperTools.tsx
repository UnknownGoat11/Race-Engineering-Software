import type { JSX } from 'react';
import {
  DEMO_SCENARIOS,
  type DemoScenario,
  type Environment,
} from '../../lib/ipc';
import { demoScenarioLabel } from '../../lib/labels';

export interface DeveloperToolsProps {
  readonly environment: Environment;
  readonly isSynthetic: boolean;
  readonly isStreaming: boolean;
  readonly isRecordingActive: boolean;
  readonly selectedScenario: DemoScenario;
  readonly onSelectScenario: (scenario: DemoScenario) => void;
  readonly onStartDemo: () => Promise<void> | void;
  readonly onStopDemo: () => Promise<void> | void;
}

export function DeveloperTools({
  environment,
  isSynthetic,
  isStreaming,
  isRecordingActive,
  selectedScenario,
  onSelectScenario,
  onStartDemo,
  onStopDemo,
}: DeveloperToolsProps): JSX.Element | null {
  // REDESIGN.md §3.1 and §4 W3.1: Developer disclosure visible only when environment !== 'production'
  if (environment === 'production') {
    return null;
  }

  const cannotStart = isStreaming || isRecordingActive;

  return (
    <details className="driver-developer" data-testid="driver-developer-tools">
      <summary className="driver-developer-summary">
        Developer tools
      </summary>

      <div className="driver-developer-content">
        <div className="driver-developer-bar">
          <label htmlFor="demo-scenario-select" className="driver-developer-label">
            Demo scenario:
          </label>
          <select
            id="demo-scenario-select"
            className="select driver-developer-select"
            value={selectedScenario}
            onChange={(e) => {
              onSelectScenario(e.target.value as DemoScenario);
            }}
            data-testid="demo-scenario-select"
          >
            {DEMO_SCENARIOS.map((sc) => (
              <option key={sc} value={sc}>
                {demoScenarioLabel(sc)} ({sc})
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn"
            onClick={() => {
              void onStartDemo();
            }}
            disabled={cannotStart}
            title={
              cannotStart
                ? 'Cannot start demo while streaming or recording'
                : 'Start synthetic demo stream'
            }
            data-testid="start-demo-btn"
          >
            Start demo
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              void onStopDemo();
            }}
            disabled={!isSynthetic}
            title={!isSynthetic ? 'Demo is not active' : 'Stop synthetic demo stream'}
            data-testid="stop-demo-btn"
          >
            Stop demo
          </button>
        </div>
      </div>
    </details>
  );
}
