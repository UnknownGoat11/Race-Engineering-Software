/**
 * Login state for the active mode's role (`get_login_state`), REDESIGN.md W1.1 / W1.7.
 *
 * The backend keeps the refresh token in the Windows Credential Manager, so the login persists across restarts;
 * this hook only reads it. There is no login event: call `refresh()` after `login`, `registerAccount`, `logout`
 * or a mode change. `state` stays null until the first answer arrives (unknown is not "signed out").
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { IpcError, getLoginState, type LoginStateDto } from '../lib/ipc';

export interface UseLoginStateResult {
  /** The last answer, or null before the first one (or after a failure with no earlier answer). */
  readonly state: LoginStateDto | null;
  /** True while a request is in flight. */
  readonly loading: boolean;
  /** The last failure's message, or null after a successful answer. */
  readonly error: string | null;
  /** Stable `IpcError.code` of the last failure (e.g. `backend_unavailable`), or null. */
  readonly errorCode: string | null;
  /** True only when the backend answered `authenticated: true`. */
  readonly authenticated: boolean;
  /** Re-reads the login state; resolves when the answer (or failure) has been applied. */
  readonly refresh: () => Promise<void>;
}

export function useLoginState(): UseLoginStateResult {
  const [state, setState] = useState<LoginStateDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [errorCode, setErrorCode] = useState<string | null>(null);
  const mounted = useRef(true);
  /** Only the newest request may apply its result (an older, slower answer must not overwrite it). */
  const requestSeq = useRef(0);

  /** Applies an answer (or failure) if it belongs to the newest request and the hook is still mounted. */
  const settle = useCallback((seq: number, result: { ok: LoginStateDto } | { err: unknown }): void => {
    if (!mounted.current || seq !== requestSeq.current) return;
    if ('ok' in result) {
      setState(result.ok);
      setError(null);
      setErrorCode(null);
    } else {
      const err = result.err;
      setError(err instanceof Error ? err.message : String(err));
      setErrorCode(err instanceof IpcError ? err.code : null);
    }
    setLoading(false);
  }, []);

  /** Starts a request; the state is only touched in its promise callbacks (never synchronously). */
  const request = useCallback((): Promise<void> => {
    const seq = ++requestSeq.current;
    return getLoginState().then(
      (ok) => {
        settle(seq, { ok });
      },
      (err: unknown) => {
        settle(seq, { err });
      },
    );
  }, [settle]);

  const refresh = useCallback(async (): Promise<void> => {
    setLoading(true);
    await request();
  }, [request]);

  useEffect(() => {
    mounted.current = true;
    void request();
    return () => {
      mounted.current = false;
    };
  }, [request]);

  return { state, loading, error, errorCode, authenticated: state?.authenticated === true, refresh };
}
