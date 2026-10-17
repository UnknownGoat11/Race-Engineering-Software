import { useState, type JSX } from 'react';
import type { Mode } from '../lib/ipc';

interface ModeSelectScreenProps {
  /** Persists the chosen mode. Rejects with an Error whose message is shown to the user. */
  readonly onSelect: (mode: Mode) => Promise<void>;
}

const OPTIONS: readonly { readonly mode: Mode; readonly label: string }[] = [
  { mode: 'driver', label: 'DRIVER MODE' },
  { mode: 'engineer', label: 'ENGINEER MODE' },
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
    <main className="mode-select">
      <h1 className="mode-select-title">LMU RACE ENGINEERING</h1>
      <p className="mode-select-prompt" id="mode-select-prompt">
        Select operating mode:
      </p>
      <div className="mode-select-options" role="group" aria-labelledby="mode-select-prompt">
        {OPTIONS.map((option) => (
          <button
            key={option.mode}
            type="button"
            className="btn"
            disabled={pending !== null}
            aria-busy={pending === option.mode}
            onClick={() => {
              choose(option.mode);
            }}
          >
            {option.label}
          </button>
        ))}
      </div>
      {error !== null && (
        <div className="alert alert-error" role="alert">
          <span className="alert-title">Could not save the operating mode</span>
          <span>{error}</span>
        </div>
      )}
    </main>
  );
}
