import { useState, type JSX } from 'react';
import type { Mode } from '../lib/ipc';

interface ModeSelectScreenProps {
  /** Persists the chosen mode. Rejects with an Error whose message is shown to the user. */
  readonly onSelect: (mode: Mode) => Promise<void>;
}

const OPTIONS: readonly {
  readonly mode: Mode;
  readonly label: string;
  readonly description: string;
  readonly note: string;
}[] = [
  {
    mode: 'driver',
    label: 'DRIVER MODE',
    description: 'In-car live dashboard, lap telemetry recording, and 1-click sharing with your pitwall engineer.',
    note: 'For simulator drivers',
  },
  {
    mode: 'engineer',
    label: 'ENGINEER MODE',
    description: 'Pitwall telemetry analysis workstation, multi-channel graphs, timing tower, and track map.',
    note: 'For race engineers & spotters',
  },
];

/** First-launch screen: the operating mode must be chosen before anything else is shown. */
export function ModeSelectScreen({ onSelect }: ModeSelectScreenProps): JSX.Element {
  const [pending, setPending] = useState<Mode | null>(null);
  const [error, setError] = useState<string | null>(null);

  const choose = (mode: Mode): void => {
    setPending(mode);
    setError(null);
    onSelect(mode).catch((reason: unknown) => {
      setError(reason instanceof Error ? reason.message : String(reason));
      setPending(null);
    });
  };

  return (
    <main className="mode-select-screen">
      <div className="mode-select-card">
        <h1 className="mode-select-title">LMU RACE ENGINEERING</h1>
        <p className="mode-select-prompt" id="mode-select-prompt">
          Select operating mode:
        </p>
        <div className="mode-select-options" role="group" aria-labelledby="mode-select-prompt">
          {OPTIONS.map((option) => (
            <button
              key={option.mode}
              type="button"
              className={`mode-select-tile ${pending === option.mode ? 'is-pending' : ''}`}
              disabled={pending !== null}
              aria-busy={pending === option.mode}
              aria-label={option.label}
              onClick={() => {
                choose(option.mode);
              }}
            >
              <div className="mode-select-tile__header">
                <span className="mode-select-tile__title">{option.label}</span>
                {pending === option.mode && <span className="mode-select-tile__pending">Saving…</span>}
              </div>
              <p className="mode-select-tile__desc">{option.description}</p>
              <div className="mode-select-tile__footer">{option.note}</div>
            </button>
          ))}
        </div>
        <p className="mode-select-note">You can change this later in Settings.</p>
        {error !== null && (
          <div className="alert alert-error" role="alert">
            <span className="alert-title">Could not save the operating mode</span>
            <span>{error}</span>
          </div>
        )}
      </div>
    </main>
  );
}
