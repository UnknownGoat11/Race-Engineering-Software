import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AttentionRow } from './AttentionRow';
import type { AttentionItem } from '../../lib/statusModel';

describe('AttentionRow', () => {
  it('renders critical alert with alert role and action', () => {
    const item: AttentionItem = {
      id: 'recording-disk-full',
      level: 'critical',
      title: 'Recording failed',
      detail: 'disk full: 212 MiB free on D:\\ (1 GiB needed)',
      action: 'open_recordings',
    };
    const onAction = vi.fn();

    render(<AttentionRow item={item} onAction={onAction} />);

    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText('■')).toBeInTheDocument();
    expect(screen.getByText('Recording failed')).toBeInTheDocument();
    expect(screen.getByText(/disk full/)).toBeInTheDocument();

    const btn = screen.getByTestId('attention-action-open-folder');
    expect(btn).toHaveTextContent('Open folder');
    fireEvent.click(btn);
    expect(onAction).toHaveBeenCalledWith('open_recordings', item);
  });

  it('renders warning alert with status role and sign in action', () => {
    const item: AttentionItem = {
      id: 'streaming-relogin',
      level: 'warning',
      title: 'Streaming: RECONNECTING',
      detail: 'Login token expiring soon',
      action: 'sign_in',
    };
    const onAction = vi.fn();

    render(<AttentionRow item={item} onAction={onAction} />);

    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByText('◆')).toBeInTheDocument();
    expect(screen.getByText('Streaming: RECONNECTING')).toBeInTheDocument();

    const btn = screen.getByTestId('attention-action-sign-in');
    expect(btn).toHaveTextContent('Sign in again');
    fireEvent.click(btn);
    expect(onAction).toHaveBeenCalledWith('sign_in', item);
  });

  it('renders new code action', () => {
    const item: AttentionItem = {
      id: 'code-expired',
      level: 'warning',
      title: 'Engineer code: EXPIRED',
      detail: 'The code expired. Request a new one.',
      action: 'new_code',
    };
    const onAction = vi.fn();

    render(<AttentionRow item={item} onAction={onAction} />);

    const btn = screen.getByTestId('attention-action-new-code');
    expect(btn).toHaveTextContent('New code');
    fireEvent.click(btn);
    expect(onAction).toHaveBeenCalledWith('new_code', item);
  });
});
