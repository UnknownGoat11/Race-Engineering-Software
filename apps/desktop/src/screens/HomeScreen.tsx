import { useState, type JSX } from 'react';
import { useCoreStatus } from '../hooks/useCoreStatus';
import { driverNewCode, engineerJoin, type Mode } from '../lib/ipc';
import { modeLabel } from '../lib/labels';
import '../styles/home.css';

export interface HomeScreenProps {
  readonly currentMode: Mode;
  readonly onChangeMode: (mode: Mode) => Promise<void>;
  readonly onEnterSession: (targetPage?: 'status' | 'pitwall' | 'views') => void;
  readonly displayName?: string | null;
}

export function HomeScreen({
  currentMode,
  onChangeMode,
  onEnterSession,
  displayName,
}: HomeScreenProps): JSX.Element {
  const { status } = useCoreStatus();
  const [hostRole, setHostRole] = useState<Mode>(currentMode);
  const [joinRole, setJoinRole] = useState<Mode>('engineer');
  const [joinCode, setJoinCode] = useState('');
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Only a code issued by the relay is ever shown: engineers can join with nothing else.
  const networkCode = status?.network.connect_code?.trim();
  const activeCode = networkCode !== undefined && networkCode.length > 0 ? networkCode : null;

  const handleCopyCode = async (code: string): Promise<void> => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => {
        setCopied(false);
      }, 2500);
    } catch {
      setError('Could not copy to clipboard.');
    }
  };

  const handleHostSession = async (): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      if (hostRole !== currentMode) {
        await onChangeMode(hostRole);
      }
      if (hostRole === 'driver') {
        // The code arrives through core status once the relay issues it; a failure is shown, never replaced.
        await driverNewCode();
      } else {
        // Engineers do not host: they join a driver's session with the driver's code.
        onEnterSession('pitwall');
      }
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleJoinSession = async (): Promise<void> => {
    const cleanCode = joinCode.trim().toUpperCase();
    if (!cleanCode) {
      setError('Please enter a 6-character session code to join.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (joinRole !== currentMode) {
        await onChangeMode(joinRole);
      }
      if (joinRole === 'engineer') {
        try {
          await engineerJoin(cleanCode);
        } catch {
          // If relay is not connected, proceed to pitwall directly with code
        }
        onEnterSession('pitwall');
      } else {
        onEnterSession('status');
      }
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="home-screen" data-testid="home-screen">
      <div className="home-container">
        {/* Hero & Greeting */}
        <header className="home-hero">
          <div className="home-hero-text">
            <div className="home-brand-tag">
              <span className="login-pulse-dot" />
              <span>Race Engineering Tool Lobby</span>
            </div>
            <h1 className="home-title">Session Lobby</h1>
            <p className="home-subtitle">
              Host a live telemetry session or join your racing team with a code.
            </p>
          </div>
          <div className="home-user-badge">
            <span className="home-user-name">{displayName ?? 'Local User'}</span>
            <span className="badge badge-mode">{modeLabel(currentMode)}</span>
          </div>
        </header>

        {/* Error Alert */}
        {error && (
          <div className="alert alert-error" role="alert">
            <span>{error}</span>
          </div>
        )}

        {/* Deliverable Code Banner when a session code is active */}
        {activeCode && (
          <section className="home-active-banner" data-testid="home-active-banner">
            <div className="home-active-header">
              <span className="home-active-tag">
                <span className="login-pulse-dot" />
                Session Hosted • Ready for Pitwall
              </span>
              <span className="home-active-role">
                Active as: <strong>{modeLabel(currentMode)}</strong>
              </span>
            </div>
            <div className="home-code-display">
              <div className="home-code-box">
                <span className="home-code-label">Deliverable Team Code</span>
                <span className="home-code-value" data-testid="home-code-value">
                  {activeCode}
                </span>
              </div>
              <div className="home-code-actions">
                <button
                  type="button"
                  className="home-btn-copy"
                  onClick={() => {
                    void handleCopyCode(activeCode);
                  }}
                  data-testid="home-copy-code-btn"
                >
                  {copied ? '✓ Copied!' : 'Copy Code'}
                </button>
                <button
                  type="button"
                  className="home-btn-enter"
                  onClick={() => {
                    onEnterSession(currentMode === 'driver' ? 'status' : 'pitwall');
                  }}
                  data-testid="home-enter-session-btn"
                >
                  Enter {currentMode === 'driver' ? 'Dashboard' : 'Pitwall'} →
                </button>
              </div>
            </div>
          </section>
        )}

        {/* Action Cards: Host vs Join */}
        <div className="home-cards-grid">
          {/* Card 1: Host a Session */}
          <div className="home-card" data-testid="home-card-host">
            <div className="home-card-header">
              <span className="home-card-badge">Option 1</span>
              <h2 className="home-card-title">Host a Session</h2>
              <p className="home-card-desc">
                Start a live telemetry session and generate a shareable code deliverable to other engineers or drivers.
              </p>
            </div>

            <div className="home-role-picker">
              <span className="home-role-label">Choose Active Role:</span>
              <div className="home-role-pills" role="radiogroup" aria-label="Host active role">
                <button
                  type="button"
                  className="home-role-btn"
                  aria-pressed={hostRole === 'driver'}
                  onClick={() => {
                    setHostRole('driver');
                  }}
                >
                  Driver (In-Car)
                </button>
                <button
                  type="button"
                  className="home-role-btn"
                  aria-pressed={hostRole === 'engineer'}
                  onClick={() => {
                    setHostRole('engineer');
                  }}
                >
                  Engineer (Pitwall)
                </button>
              </div>
            </div>

            <button
              type="button"
              className="home-card-action-btn btn-accent"
              disabled={busy}
              onClick={() => {
                void handleHostSession();
              }}
              data-testid="home-host-btn"
            >
              {busy ? 'Setting up…' : 'Host Session & Get Code'}
            </button>
          </div>

          {/* Card 2: Join a Session */}
          <div className="home-card" data-testid="home-card-join">
            <div className="home-card-header">
              <span className="home-card-badge">Option 2</span>
              <h2 className="home-card-title">Join a Session</h2>
              <p className="home-card-desc">
                Connect to an existing session using a code delivered by your driver or teammate.
              </p>
            </div>

            <div className="home-role-picker">
              <span className="home-role-label">Join Active Role:</span>
              <div className="home-role-pills" role="radiogroup" aria-label="Join active role">
                <button
                  type="button"
                  className="home-role-btn"
                  aria-pressed={joinRole === 'engineer'}
                  onClick={() => {
                    setJoinRole('engineer');
                  }}
                >
                  Engineer (Pitwall)
                </button>
                <button
                  type="button"
                  className="home-role-btn"
                  aria-pressed={joinRole === 'driver'}
                  onClick={() => {
                    setJoinRole('driver');
                  }}
                >
                  Driver / Observer
                </button>
              </div>
            </div>

            <div className="home-input-group">
              <label className="home-role-label" htmlFor="join-code-input">
                Enter 6-Character Code:
              </label>
              <input
                id="join-code-input"
                type="text"
                maxLength={8}
                placeholder="e.g. K7Q2XM"
                className="home-code-input"
                value={joinCode}
                onChange={(e) => {
                  setJoinCode(e.target.value.toUpperCase());
                }}
                disabled={busy}
                data-testid="home-join-code-input"
              />
            </div>

            <button
              type="button"
              className="home-card-action-btn"
              disabled={busy || !joinCode.trim()}
              onClick={() => {
                void handleJoinSession();
              }}
              data-testid="home-join-submit-btn"
            >
              {busy ? 'Connecting…' : 'Connect & Join Lobby'}
            </button>
          </div>

          {/* Card 3: V1.1 View Suite */}
          <div className="home-card" data-testid="home-card-views">
            <div className="home-card-header">
              <span className="home-card-badge">V1.1 New</span>
              <h2 className="home-card-title">Explore Engineering Views</h2>
              <p className="home-card-desc">
                27 high-precision analysis views: Driver DNA, Cockpit Driving HUD with 3-part tyres, Polar Circle Radar, Minisectors, and Gap Charts.
              </p>
            </div>

            <button
              type="button"
              className="home-card-action-btn"
              onClick={() => {
                onEnterSession('views');
              }}
              data-testid="home-views-submit-btn"
            >
              Open V1.1 View Suite (27 Views) →
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
