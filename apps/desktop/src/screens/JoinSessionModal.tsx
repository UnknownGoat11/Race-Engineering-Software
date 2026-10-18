import { useId, useRef, useState, type JSX, type ChangeEvent, type SyntheticEvent } from 'react';
import { Modal } from '../components/ui/Modal';
import { classColorVar } from '../lib/classes';
import { IpcError, engineerCancel, engineerConfirm, engineerJoin, type IdentityCardDto } from '../lib/ipc';
import { COPY, joinErrorCopy, vehicleClassCode, type JoinErrorCopy } from '../lib/labels';
import { NO_VALUE } from '../lib/units';
import '../styles/modal.css';

export interface JoinSessionModalProps {
  readonly isOpen?: boolean;
  readonly onJoined?: () => void;
  readonly onCancel?: () => void;
}

function extractErrorCode(err: unknown): string {
  if (err instanceof IpcError) return err.code;
  if (typeof err === 'object' && err !== null && 'code' in err) {
    const code = err.code;
    if (typeof code === 'string') return code;
  }
  const msg = err instanceof Error ? err.message : String(err);
  if (msg.toLowerCase().includes('session full') || msg.toLowerCase().includes('already has 2')) {
    return 'session_full';
  }
  if (msg.toLowerCase().includes('expired')) return 'code_expired';
  if (msg.toLowerCase().includes('invalid')) return 'code_invalid';
  return msg;
}

function formatStartedAgo(startedAtNs: number | null): string | null {
  if (startedAtNs == null || startedAtNs <= 0) return null;
  const nowNs = Date.now() * 1_000_000;
  const diffNs = nowNs - startedAtNs;
  if (diffNs < 0) return 'started just now';
  const diffS = Math.floor(diffNs / 1_000_000_000);
  if (diffS < 60) return 'started just now';
  const mins = Math.floor(diffS / 60);
  if (mins < 60) return `started ${mins}m ago`;
  const hours = Math.floor(mins / 60);
  return `started ${hours}h ${mins % 60}m ago`;
}

export function JoinSessionModal({ isOpen = true, onJoined, onCancel }: JoinSessionModalProps): JSX.Element {
  const [code, setCode] = useState('');
  const [redeemedCode, setRedeemedCode] = useState('');
  const [joining, setJoining] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [card, setCard] = useState<IdentityCardDto | null>(null);
  const [errorCopy, setErrorCopy] = useState<JoinErrorCopy | null>(null);
  const [rawError, setRawError] = useState<string | null>(null);

  const titleId = useId();
  const inputRef = useRef<HTMLInputElement | null>(null);
  const identityHeadingRef = useRef<HTMLSpanElement | null>(null);

  const handleCodeChange = (e: ChangeEvent<HTMLInputElement>) => {
    // Sanitize: max 6 alphanumeric characters, uppercase
    const raw = e.target.value.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 6);
    setCode(raw);
    setErrorCopy(null);
    setRawError(null);
  };

  const handleJoin = async (e?: SyntheticEvent) => {
    e?.preventDefault();
    const cleanCode = code.trim().toUpperCase();
    if (cleanCode.length !== 6) {
      const copy = joinErrorCopy('code_malformed');
      setErrorCopy(copy);
      setRawError(null);
      return;
    }

    setJoining(true);
    setErrorCopy(null);
    setRawError(null);
    try {
      const idCard = await engineerJoin(cleanCode);
      setCard(idCard);
      setRedeemedCode(cleanCode);
    } catch (err: unknown) {
      const codeKey = extractErrorCode(err);
      const copy = joinErrorCopy(codeKey);
      const raw = err instanceof Error ? err.message : String(err);
      setErrorCopy(copy);
      setRawError(raw);
      if (copy.clearField) {
        setCode('');
        setTimeout(() => {
          inputRef.current?.focus();
        }, 0);
      }
    } finally {
      setJoining(false);
    }
  };

  const handleConfirm = async () => {
    setConfirming(true);
    setErrorCopy(null);
    setRawError(null);
    try {
      await engineerConfirm();
      onJoined?.();
    } catch (err: unknown) {
      const codeKey = extractErrorCode(err);
      const copy = joinErrorCopy(codeKey);
      const raw = err instanceof Error ? err.message : String(err);
      setErrorCopy(copy);
      setRawError(raw);
      setConfirming(false);
    }
  };

  const handleCancel = async () => {
    setErrorCopy(null);
    setRawError(null);
    if (card) {
      try {
        await engineerCancel();
      } catch {
        // Ignore cancellation cleanup error
      }
      // Returning from identity card to code input (A01 / REDESIGN.md §4 W4.1)
      setCard(null);
      setRedeemedCode('');
      setCode('');
    } else {
      onCancel?.();
    }
  };

  const startedText = card ? formatStartedAgo(card.started_at_ns) : null;

  return (
    <Modal
      isOpen={isOpen}
      onClose={() => {
        void handleCancel();
      }}
      ariaLabelledBy={titleId}
      size="md"
      initialFocusRef={card ? identityHeadingRef : inputRef}
      focusKey={card ? 'card' : 'code'}
      testId="join-session-modal"
    >
      <div className="card modal-content" style={{ width: '100%', margin: '0' }}>
        <div className="card-header">
          <span
            ref={card ? identityHeadingRef : undefined}
            tabIndex={card ? -1 : undefined}
            className="card-header-title"
            id={titleId}
          >
            {card ? 'CONFIRM DRIVER IDENTITY' : 'CONNECT TO DRIVER RELAY'}
          </span>
        </div>
        <div className="card-body">
          {errorCopy && (
            <div
              className="status-badge status-badge-error join-error-block"
              style={{
                width: '100%',
                marginBottom: '16px',
                padding: '10px 14px',
                textAlign: 'left',
                display: 'flex',
                flexDirection: 'column',
                gap: '4px',
                textTransform: 'none',
                letterSpacing: 'normal',
                fontWeight: 'normal',
              }}
              role="alert"
              data-testid="join-error-banner"
            >
              <strong style={{ fontWeight: 700, color: 'var(--color-error)' }}>
                {errorCopy.title}
              </strong>
              <span style={{ fontSize: '12px', color: 'var(--text-secondary)' }}>
                {errorCopy.helper}
              </span>
              {rawError && rawError !== errorCopy.title && rawError !== errorCopy.helper && (
                <span style={{ fontSize: '11px', opacity: 0.8 }}>({rawError})</span>
              )}
            </div>
          )}

          {!card ? (
            /* Phase 1: Code entry (A06) */
            <form onSubmit={(e) => { void handleJoin(e); }}>
              <p className="panel-note" style={{ marginBottom: '16px' }}>
                Ask the driver for their 6-character engineer session code from their Relay Streaming card.
              </p>

              <label htmlFor="engineer-code-input" className="join-code-label">
                Engineer code
              </label>

              <div style={{ textAlign: 'center', marginBottom: '8px' }}>
                <input
                  id="engineer-code-input"
                  ref={inputRef}
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
                  aria-label="Engineer code"
                  autoComplete="one-time-code"
                  inputMode="text"
                />
              </div>

              <div className="join-code-paste-hint">
                Paste 6-character code from clipboard (Ctrl+V)
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
                  {joining ? 'Connecting...' : COPY.joinDriver}
                </button>
              </div>
            </form>
          ) : (
            /* Phase 2: Driver Identity Card confirmation (D-022, OR-3) */
            <div data-testid="driver-identity-card">
              <p className="panel-note" style={{ marginBottom: '14px' }}>
                Please verify that you are connecting to the correct driver before confirming.
              </p>

              <dl className="identity-card-dl" style={{ marginBottom: '16px' }}>
                <div className="identity-card-row">
                  <dt>Driver</dt>
                  <dd>
                    <span style={{ fontWeight: 600 }}>
                      {card.driver_display_name} (@{card.driver_username})
                    </span>
                  </dd>
                </div>
                <div className="identity-card-row">
                  <dt>Vehicle</dt>
                  <dd>{card.car ?? NO_VALUE}</dd>
                </div>
                <div className="identity-card-row">
                  <dt>Class</dt>
                  <dd style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                    <span
                      className="class-chip"
                      style={{
                        display: 'inline-block',
                        padding: '1px 6px',
                        fontSize: '11px',
                        fontWeight: 700,
                        borderRadius: '2px',
                        backgroundColor: 'rgba(255, 255, 255, 0.08)',
                        color: classColorVar(vehicleClassCode(card.vehicle_class)),
                        border: `1px solid ${classColorVar(vehicleClassCode(card.vehicle_class))}`,
                      }}
                    >
                      {vehicleClassCode(card.vehicle_class)}
                    </span>
                    <span>{card.vehicle_class ?? NO_VALUE}</span>
                  </dd>
                </div>
                <div className="identity-card-row">
                  <dt>Track</dt>
                  <dd>{card.track ?? NO_VALUE}</dd>
                </div>
                <div className="identity-card-row">
                  <dt>Session</dt>
                  <dd>
                    <span>{card.session_type ?? NO_VALUE}</span>
                    {startedText && (
                      <span style={{ color: 'var(--text-tertiary)', marginLeft: '6px' }}>
                        ({startedText})
                      </span>
                    )}
                  </dd>
                </div>
                <div className="identity-card-row">
                  <dt>Code</dt>
                  <dd>
                    <code
                      style={{
                        fontFamily: 'monospace',
                        fontSize: '14px',
                        letterSpacing: '2px',
                        fontWeight: 700,
                        color: 'var(--text-primary)',
                      }}
                    >
                      {redeemedCode}
                    </code>
                  </dd>
                </div>
                <div className="identity-card-row">
                  <dt>Schema Version</dt>
                  <dd>v{card.schema_version}</dd>
                </div>
              </dl>

              <p
                className="panel-note"
                style={{
                  margin: '14px 0 16px',
                  color: 'var(--color-warn)',
                  fontStyle: 'italic',
                }}
              >
                Only confirm if this is the driver you expect.
              </p>

              <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', alignItems: 'flex-end' }}>
                <div style={{ display: 'flex', gap: '12px', justifyContent: 'flex-end' }}>
                  <button
                    type="button"
                    className="btn"
                    onClick={() => { void handleCancel(); }}
                    disabled={confirming}
                    data-testid="btn-cancel-identity"
                    aria-describedby="decline-consequence"
                  >
                    {COPY.notThisDriver}
                  </button>
                  <button
                    type="button"
                    className="btn btn-primary"
                    onClick={() => { void handleConfirm(); }}
                    disabled={confirming}
                    data-testid="btn-confirm-identity"
                  >
                    {confirming ? 'Confirming...' : COPY.confirmJoin}
                  </button>
                </div>
                <span
                  id="decline-consequence"
                  style={{ fontSize: '11px', color: 'var(--text-tertiary)', textAlign: 'right' }}
                >
                  {COPY.notThisDriverConsequence}
                </span>
              </div>
            </div>
          )}
        </div>
      </div>
    </Modal>
  );
}
