import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as ipc from '../lib/ipc';
import { LoginScreen } from './LoginScreen';

vi.mock('../lib/ipc', async (importOriginal) => {
  const actual = await importOriginal<typeof ipc>();
  return {
    ...actual,
    login: vi.fn(),
    registerAccount: vi.fn(),
  };
});

describe('LoginScreen', () => {
  const onLoginSuccess = vi.fn();
  const onChangeMode = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders sign in tab by default with username and password inputs', () => {
    render(<LoginScreen mode="driver" onLoginSuccess={onLoginSuccess} onChangeMode={onChangeMode} />);

    expect(screen.getByTestId('login-screen')).toBeInTheDocument();
    expect(screen.getByTestId('login-mode-text')).toHaveTextContent('DRIVER');
    expect(screen.getByTestId('login-tab-signin')).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByTestId('login-tab-register')).toHaveAttribute('aria-selected', 'false');
    expect(screen.getByLabelText('Username')).toBeInTheDocument();
    expect(screen.getByLabelText('Password')).toBeInTheDocument();
    expect(screen.queryByLabelText('Display name')).not.toBeInTheDocument();
    expect(screen.getByTestId('login-submit-btn')).toHaveTextContent('Sign in');
  });

  it('validates required fields before calling login', () => {
    render(<LoginScreen mode="driver" onLoginSuccess={onLoginSuccess} />);

    // Empty fields
    fireEvent.click(screen.getByTestId('login-submit-btn'));
    expect(screen.getByRole('alert')).toHaveTextContent('Please enter your username.');
    expect(ipc.login).not.toHaveBeenCalled();

    // Fill username only
    fireEvent.change(screen.getByTestId('login-username-input'), { target: { value: 'alice' } });
    fireEvent.click(screen.getByTestId('login-submit-btn'));
    expect(screen.getByRole('alert')).toHaveTextContent('Please enter your password.');
    expect(ipc.login).not.toHaveBeenCalled();
  });

  it('calls login with username and password, then notifies onLoginSuccess', async () => {
    const account: ipc.AccountInfoDto = {
      account: '1234567890abcdef1234567890abcdef',
      username: 'alice',
      display_name: 'Alice Driver',
      role: 'driver',
    };
    vi.mocked(ipc.login).mockResolvedValue(account);

    render(<LoginScreen mode="driver" onLoginSuccess={onLoginSuccess} />);

    fireEvent.change(screen.getByTestId('login-username-input'), { target: { value: 'alice' } });
    fireEvent.change(screen.getByTestId('login-password-input'), { target: { value: 'secret123' } });
    fireEvent.click(screen.getByTestId('login-submit-btn'));

    await waitFor(() => {
      expect(ipc.login).toHaveBeenCalledWith('alice', 'secret123');
      expect(onLoginSuccess).toHaveBeenCalledWith(account);
    });
  });

  it('shows error message if login fails', async () => {
    vi.mocked(ipc.login).mockRejectedValue(new Error('Invalid username or password'));

    render(<LoginScreen mode="driver" onLoginSuccess={onLoginSuccess} />);

    fireEvent.change(screen.getByTestId('login-username-input'), { target: { value: 'alice' } });
    fireEvent.change(screen.getByTestId('login-password-input'), { target: { value: 'wrongpw' } });
    fireEvent.click(screen.getByTestId('login-submit-btn'));

    expect(await screen.findByRole('alert')).toHaveTextContent('Invalid username or password');
    expect(onLoginSuccess).not.toHaveBeenCalled();
  });

  it('supports registration flow on the Create Account tab', async () => {
    const account: ipc.AccountInfoDto = {
      account: 'abcdef1234567890abcdef1234567890',
      username: 'bob',
      display_name: 'Bob Eng',
      role: 'engineer',
    };
    vi.mocked(ipc.registerAccount).mockResolvedValue(account);

    render(<LoginScreen mode="engineer" onLoginSuccess={onLoginSuccess} />);

    fireEvent.click(screen.getByTestId('login-tab-register'));

    expect(screen.getByTestId('login-tab-register')).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByLabelText('Display name')).toBeInTheDocument();
    expect(screen.getByTestId('login-submit-btn')).toHaveTextContent('Create account');

    fireEvent.change(screen.getByTestId('login-username-input'), { target: { value: 'bob' } });
    fireEvent.change(screen.getByTestId('login-displayname-input'), { target: { value: 'Bob Eng' } });
    fireEvent.change(screen.getByTestId('login-password-input'), { target: { value: 'pass1234' } });
    fireEvent.click(screen.getByTestId('login-submit-btn'));

    await waitFor(() => {
      expect(ipc.registerAccount).toHaveBeenCalledWith('bob', 'Bob Eng', 'engineer', 'pass1234');
      expect(onLoginSuccess).toHaveBeenCalledWith(account);
    });
  });

  it('allows switching mode from the login screen', () => {
    render(<LoginScreen mode="driver" onLoginSuccess={onLoginSuccess} onChangeMode={onChangeMode} />);

    fireEvent.click(screen.getByTestId('login-change-mode-btn'));
    expect(onChangeMode).toHaveBeenCalledWith('engineer');
  });
});
