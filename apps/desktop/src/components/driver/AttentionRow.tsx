import type { JSX } from 'react';
import type { AttentionItem, TileAction } from '../../lib/statusModel';

export interface AttentionRowProps {
  readonly item: AttentionItem;
  readonly onAction?: (action: TileAction, item: AttentionItem) => void;
  readonly testId?: string;
}

export function AttentionRow({ item, onAction, testId }: AttentionRowProps): JSX.Element {
  const glyph = item.level === 'critical' ? '■' : '◆';

  const renderAction = () => {
    if (!item.action || !onAction) return null;

    switch (item.action) {
      case 'sign_in':
        return (
          <button
            type="button"
            className="btn btn-sm btn-primary attention-action-btn"
            onClick={() => {
              onAction('sign_in', item);
            }}
            data-testid="attention-action-sign-in"
          >
            Sign in again
          </button>
        );
      case 'new_code':
        return (
          <button
            type="button"
            className="btn btn-sm attention-action-btn"
            onClick={() => {
              onAction('new_code', item);
            }}
            data-testid="attention-action-new-code"
          >
            New code
          </button>
        );
      case 'open_recordings':
        return (
          <button
            type="button"
            className="btn btn-sm attention-action-btn"
            onClick={() => {
              onAction('open_recordings', item);
            }}
            data-testid="attention-action-open-folder"
          >
            Open folder
          </button>
        );
      case 'remove_engineer':
        return (
          <button
            type="button"
            className="btn btn-sm btn-danger attention-action-btn"
            onClick={() => {
              onAction('remove_engineer', item);
            }}
            data-testid="attention-action-remove-engineer"
          >
            Remove
          </button>
        );
      default:
        return null;
    }
  };

  return (
    <div
      className={`attention-row attention-row-${item.level}`}
      data-testid={testId ?? `attention-${item.id}`}
      role={item.level === 'critical' ? 'alert' : 'status'}
    >
      <span className="attention-glyph" aria-hidden="true">
        {glyph}
      </span>
      <div className="attention-body">
        <strong className="attention-title">{item.title}</strong>
        {item.detail && <span className="attention-detail"> — {item.detail}</span>}
      </div>
      {renderAction()}
    </div>
  );
}
