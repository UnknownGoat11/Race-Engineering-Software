import { useState, type JSX, type ChangeEvent, type SyntheticEvent } from 'react';
import { engineerCancel, engineerConfirm, engineerJoin, type IdentityCardDto } from '../lib/ipc';

export interface JoinSessionModalProps {
  readonly onJoined?: () => void;
  readonly onCancel?: () => void;
}

export function JoinSessionModal({ onJoined, onCancel }: JoinSessionModalProps): JSX.Element {
  const [code, setCode] = useState('');
  const [joining, setJoining] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [card, setCard] = useState<IdentityCardDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleCodeChange = (e: ChangeEvent<HTMLInputElement>) => {
    // Sanitize: max 6 alphanumeric characters, uppercase
    const raw = e.target.value.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 6);
    setCode(raw);
    setError(null);
  };

  const handleJoin = async (e?: SyntheticEvent) => {
    e?.preventDefault();
    const cleanCode = code.trim().toUpperCase();
    if (cleanCode.length !== 6) {
      setError('Please enter a valid 6-character connection code.');
      return;
    }

    setJoining(true);
    setError(null);
    try {
      const idCard = await engineerJoin(cleanCode);
      setCard(idCard);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setError(msg);
    } finally {
      setJoining(false);
    }
  };

  const handleConfirm = async () => {
    setConfirming(true);
    setError(null);
    try {
      await engineerConfirm();
      onJoined?.();
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setError(msg);
      setConfirming(false);
    }
  };

  const handleCancel = async () => {
    setError(null);
    try {
      await engineerCancel();
    } catch {
      // Ignore cancellation cleanup error
    }
    setCard(null);
    onCancel?.();
  };

  return (
    <div className="modal-backdrop" data-testid="join-session-modal">
      <div className="card modal-content" style={{ maxWidth: '520px', width: '100%', margin: '0 auto' }}>
        <div className="card-header">
          <span className="card-header-title">
            {card ? 'CONFIRM DRIVER IDENTITY' : 'CONNECT TO DRIVER RELAY'}
          </span>
        </div>
        <div className="card-body">
          {error && (
            <div
              className="status-badge status-badge-error"
              style={{ width: '100%', marginBottom: '12px', padding: '8px', textAlign: 'center' }}
              role="alert"
            >
              {error}
            </div>
          )}

          {!card ? (
            /* Phase 1: Code entry */
            <form onSubmit={(e) => { void handleJoin(e); }}>
              <p className="panel-note" style={{ marginBottom: '16px' }}>
                Ask the driver for their 6-character engineer session code from their Relay Streaming card.
              </p>

              <div style={{ textAlign: 'center', marginBottom: '20px' }}>
                <input
                  type="text"
                  className="input"
                  style={{
                    fontSize: '28px',
                    fontWeight: 700,
                    letterSpacing: '6px',
                    textAlign: 'center',
                    fontFamily: 'monospace',
                    width: '240px',
                    padding: '8px 12px',
                    textTransform: 'uppercase',
                  }}
                  placeholder="------"
                  value={code}
                  maxLength={6}
                  disabled={joining}
                  onChange={handleCodeChange}
                  autoFocus
                  data-testid="join-code-input"
                />
              </div>

              <div style={{ display: 'flex', gap: '12px', justifyContent: 'flex-end' }}>
                {onCancel && (
                  <button
                    type="button"
                    className="btn"
                    onClick={onCancel}
                    disabled={joining}
                  >
                    Cancel
                  </button>
                )}
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={code.length !== 6 || joining}
                  data-testid="btn-join-session"
                >
                  {joining ? 'Connecting...' : 'Join Driver'}
                </button>
              </div>
            </form>
          ) : (
            /* Phase 2: Driver Identity Card confirmation */
            <div data-testid="driver-identity-card">
              <p className="panel-note" style={{ marginBottom: '16px' }}>
                Please verify that you are connecting to the correct driver before confirming.
              </p>

              <div className="telemetry-grid-2x2" style={{ marginBottom: '16px' }}>
                <div className="card-row">
                  <span className="card-row-label">Driver</span>
                  <span className="card-row-value" style={{ fontWeight: 600 }}>
                    {card.driver_display_name} (@{card.driver_username})
                  </span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Vehicle</span>
                  <span className="card-row-value">{card.car ?? '—'}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Class</span>
                  <span className="card-row-value">{card.vehicle_class ?? '—'}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Track</span>
                  <span className="card-row-value">{card.track ?? '—'}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Session</span>
                  <span className="card-row-value">{card.session_type ?? '—'}</span>
                </div>
                <div className="card-row">
                  <span className="card-row-label">Schema Version</span>
                  <span className="card-row-value">v{card.schema_version}</span>
                </div>
              </div>

              <div style={{ display: 'flex', gap: '12px', justifyContent: 'flex-end' }}>
                <button
                  type="button"
                  className="btn"
                  onClick={() => { void handleCancel(); }}
                  disabled={confirming}
                  data-testid="btn-cancel-identity"
                >
                  Decline
                </button>
                <button
                  type="button"
                  className="btn btn-primary"
                  onClick={() => { void handleConfirm(); }}
                  disabled={confirming}
                  data-testid="btn-confirm-identity"
                >
                  {confirming ? 'Confirming...' : 'Confirm & Join'}
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
