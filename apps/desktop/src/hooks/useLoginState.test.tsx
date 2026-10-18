import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as ipc from '../lib/ipc';
import { useLoginState } from './useLoginState';

vi.mock('../lib/ipc', async () => {
  const actual = await vi.importActual<typeof ipc>('../lib/ipc');
  return { ...actual, getLoginState: vi.fn() };
});

const getLoginState = vi.mocked(ipc.getLoginState);

const SIGNED_IN: ipc.LoginStateDto = {
  authenticated: true,
  role: 'driver',
  account: { account: '0'.repeat(32), username: 'driverone', display_name: 'DriverOne', role: 'driver' },
};
const SIGNED_OUT: ipc.LoginStateDto = { authenticated: false, role: 'driver', account: null };

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void } {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe('useLoginState', () => {
  beforeEach(() => {
    getLoginState.mockReset();
  });

  it('is unknown (not signed out) until the first answer', async () => {
    const d = deferred<ipc.LoginStateDto>();
    getLoginState.mockReturnValueOnce(d.promise);
    const { result } = renderHook(() => useLoginState());
    expect(result.current.state).toBeNull();
    expect(result.current.loading).toBe(true);
    expect(result.current.authenticated).toBe(false);
    await act(async () => {
      d.resolve(SIGNED_IN);
      await d.promise;
    });
    expect(result.current.state).toEqual(SIGNED_IN);
    expect(result.current.authenticated).toBe(true);
    expect(result.current.loading).toBe(false);
  });

  it('reports a failure with its code and keeps state null', async () => {
    getLoginState.mockRejectedValueOnce(new ipc.IpcError('get_login_state', 'backend_unavailable', 'no backend'));
    const { result } = renderHook(() => useLoginState());
    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });
    expect(result.current.error).toBe('no backend');
    expect(result.current.errorCode).toBe('backend_unavailable');
    expect(result.current.state).toBeNull();
    expect(result.current.authenticated).toBe(false);
  });

  it('refresh re-reads (e.g. after logout) and clears the error', async () => {
    getLoginState.mockRejectedValueOnce(new Error('boom'));
    const { result } = renderHook(() => useLoginState());
    await waitFor(() => {
      expect(result.current.error).toBe('boom');
    });
    expect(result.current.errorCode).toBeNull();
    getLoginState.mockResolvedValueOnce(SIGNED_OUT);
    await act(async () => {
      await result.current.refresh();
    });
    expect(result.current.state).toEqual(SIGNED_OUT);
    expect(result.current.error).toBeNull();
    expect(result.current.authenticated).toBe(false);
  });

  it('an older, slower answer does not overwrite a newer one', async () => {
    const first = deferred<ipc.LoginStateDto>();
    getLoginState.mockReturnValueOnce(first.promise);
    const { result } = renderHook(() => useLoginState());
    getLoginState.mockResolvedValueOnce(SIGNED_OUT);
    await act(async () => {
      await result.current.refresh();
    });
    await act(async () => {
      first.resolve(SIGNED_IN);
      await first.promise;
    });
    expect(result.current.state).toEqual(SIGNED_OUT);
  });

  it('does not update after unmount', async () => {
    const d = deferred<ipc.LoginStateDto>();
    getLoginState.mockReturnValueOnce(d.promise);
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const { unmount } = renderHook(() => useLoginState());
    unmount();
    await act(async () => {
      d.resolve(SIGNED_IN);
      await d.promise;
    });
    expect(errorSpy).not.toHaveBeenCalled();
    errorSpy.mockRestore();
  });
});
