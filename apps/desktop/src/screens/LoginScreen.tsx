/**
 * Login and registration screen (REDESIGN.md W1.7, closes T11, B12).
 * Shown after mode selection when unauthenticated, or on explicit relogin request.
 * Password is typed by the user only and never stored or logged.
 */
import { useState, type JSX, type SyntheticEvent } from 'react';
import {
  login,
  registerAccount,
  type AccountInfoDto,
  type AppInfo,
  type Mode,
  type Role,
} from '../lib/ipc';
import { modeLabel } from '../lib/labels';
import '../styles/login.css';

export interface LoginScreenProps {
  readonly mode: Mode;
  readonly onLoginSuccess: (account?: AccountInfoDto) => void;
  readonly onChangeMode?: (mode: Mode) => Promise<void>;
  readonly info?: AppInfo;
}

type Tab = 'signin' | 'register';

export function LoginScreen({
  mode,
  onLoginSuccess,
  onChangeMode,
  info,
}: LoginScreenProps): JSX.Element {
  const [tab, setTab] = useState<Tab>('signin');
  const [username, setUsername] = useState('');
  const [displayName, setDisplayName] = useState('');
  const [password, setPassword] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const role: Role = mode;

  const handleSubmit = async (e: SyntheticEvent): Promise<void> => {
    e.preventDefault();
    const cleanUser = username.trim();
    if (!cleanUser) {
      setError('Please enter your username.');
      return;
    }
    if (!password) {
      setError('Please enter your password.');
      return;
    }

    setSubmitting(true);
    setError(null);

    try {
      if (tab === 'signin') {
        const account = await login(cleanUser, password);
        setPassword('');
        onLoginSuccess(account);
      } else {
        const cleanDisplay = displayName.trim() || cleanUser;
        const account = await registerAccount(cleanUser, cleanDisplay, role, password);
        setPassword('');
        onLoginSuccess(account);
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setError(msg);
    } finally {
      setSubmitting(false);
    }
  };

  const handleTabChange = (nextTab: Tab): void => {
    setTab(nextTab);
    setError(null);
  };

  return (
    <main className="login-screen" data-testid="login-screen">
      <div className="login-card">
        <header className="login-header">
          <div className="login-brand-tag">Motorsport telemetry and pitwall</div>
          <h1 className="login-title">Race Engineering Tool</h1>
          <div className="login-role-badge">
            <span className="login-role-text">
              Active mode: <strong data-testid="login-mode-text">{modeLabel(mode)}</strong>
            </span>
            {info && (
              <span className="num" data-testid="login-version">
                v{info.version}
              </span>
            )}
          </div>
          <div className="login-tabs" role="tablist" aria-label="Authentication mode">
            <button
              type="button"
              role="tab"
              id="tab-signin"
              className="login-tab-btn"
              aria-selected={tab === 'signin'}
              aria-controls="panel-auth"
              disabled={submitting}
              onClick={() => {
                handleTabChange('signin');
              }}
              data-testid="login-tab-signin"
            >
              Sign in
            </button>
            <button
              type="button"
              role="tab"
              id="tab-register"
              className="login-tab-btn"
              aria-selected={tab === 'register'}
              aria-controls="panel-auth"
              disabled={submitting}
              onClick={() => {
                handleTabChange('register');
              }}
              data-testid="login-tab-register"
            >
              Create account
            </button>
          </div>
        </header>

        <form
          id="panel-auth"
          role="tabpanel"
          aria-labelledby={tab === 'signin' ? 'tab-signin' : 'tab-register'}
          className="login-form"
          noValidate
          onSubmit={(e) => {
            void handleSubmit(e);
          }}
          data-testid="login-form"
        >
          <div className="login-field">
            <label className="login-label" htmlFor="login-username">
              Username
            </label>
            <input
              id="login-username"
              type="text"
              className="login-input"
              value={username}
              onChange={(e) => {
                setUsername(e.target.value);
              }}
              disabled={submitting}
              autoComplete="username"
              required
              data-testid="login-username-input"
            />
          </div>

          {tab === 'register' && (
            <div className="login-field">
              <label className="login-label" htmlFor="login-displayname">
                Display name
              </label>
              <input
                id="login-displayname"
                type="text"
                className="login-input"
                placeholder={username || 'Display name'}
                value={displayName}
                onChange={(e) => {
                  setDisplayName(e.target.value);
                }}
                disabled={submitting}
                autoComplete="name"
                data-testid="login-displayname-input"
              />
              <span className="login-hint">Name visible to team members</span>
            </div>
          )}

          <div className="login-field">
            <label className="login-label" htmlFor="login-password">
              Password
            </label>
            <input
              id="login-password"
              type="password"
              className="login-input"
              value={password}
              onChange={(e) => {
                setPassword(e.target.value);
              }}
              disabled={submitting}
              autoComplete={tab === 'signin' ? 'current-password' : 'new-password'}
              required
              data-testid="login-password-input"
            />
          </div>

          {error && (
            <div className="alert alert-error" role="alert" data-testid="login-error">
              <span>{error}</span>
            </div>
          )}

          <div className="login-actions">
            <button
              type="submit"
              className="btn btn-primary"
              disabled={submitting}
              aria-busy={submitting}
              data-testid="login-submit-btn"
            >
              {submitting
                ? tab === 'signin'
                  ? 'Signing in…'
                  : 'Creating account…'
                : tab === 'signin'
                  ? 'Sign in'
                  : 'Create account'}
            </button>
          </div>
        </form>

        <footer className="login-footer">
          <span className="login-footer-text">
            {role === 'driver' ? 'Driver account' : 'Engineer account'}
          </span>
          {onChangeMode && (
            <button
              type="button"
              className="btn btn-sm"
              disabled={submitting}
              onClick={() => {
                const otherMode: Mode = mode === 'driver' ? 'engineer' : 'driver';
                void onChangeMode(otherMode);
              }}
              data-testid="login-change-mode-btn"
            >
              Switch to {mode === 'driver' ? 'Engineer' : 'Driver'}
            </button>
          )}
        </footer>
      </div>
    </main>
  );
}
