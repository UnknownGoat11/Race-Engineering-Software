import type { JSX, KeyboardEvent, ReactNode } from 'react';
import type { Level } from '../../lib/statusModel';

export interface GlanceTileProps {
  readonly id: 'lmu' | 'recording' | 'streaming' | 'engineers' | 'code';
  readonly label: string;
  readonly level: Level;
  readonly word: string;
  readonly detail?: ReactNode;
  readonly reason?: ReactNode;
  readonly children?: ReactNode;
  readonly isCode?: boolean;
  readonly isSessionFull?: boolean;
  readonly testId?: string;
  readonly onClick?: () => void;
}

function levelGlyph(level: Level): string {
  switch (level) {
    case 'ok':
      return '●';
    case 'warning':
      return '◆';
    case 'critical':
      return '■';
    case 'synthetic':
      return '▨';
    case 'off':
    default:
      return '□';
  }
}

export function GlanceTile({
  id,
  label,
  level,
  word,
  detail,
  reason,
  children,
  isCode = false,
  isSessionFull = false,
  testId,
  onClick,
}: GlanceTileProps): JSX.Element {
  const isInteractive = typeof onClick === 'function';

  const handleKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (isInteractive && (e.key === 'Enter' || e.key === ' ')) {
      e.preventDefault();
      onClick();
    }
  };

  const tileClasses = [
    'glance-tile',
    `glance-tile-${level}`,
    `glance-tile-${id}`,
    isInteractive ? 'glance-tile-interactive' : '',
  ]
    .filter(Boolean)
    .join(' ');

  let wordContent: ReactNode;
  if (isCode) {
    if (isSessionFull) {
      wordContent = (
        <span className="glance-code-full" data-testid="driver-connect-code">
          SESSION FULL
        </span>
      );
    } else if (word && word !== '—' && word !== '——————') {
      wordContent = (
        <span className="glance-code-text" data-testid="driver-connect-code">
          {word}
        </span>
      );
    } else {
      wordContent = (
        <span className="glance-code-slots" data-testid="driver-connect-code">
          ——————
        </span>
      );
    }
  } else {
    wordContent = word;
  }

  return (
    <div
      className={tileClasses}
      data-testid={testId ?? `tile-${id}`}
      role={isInteractive ? 'button' : undefined}
      tabIndex={isInteractive ? 0 : undefined}
      onClick={onClick}
      onKeyDown={isInteractive ? handleKeyDown : undefined}
      aria-label={isInteractive ? `${label}: ${word}. Click for diagnostics details.` : undefined}
    >
      <div className="glance-header">
        <span className="glance-label">{label}</span>
        <span className="glance-glyph" aria-hidden="true">
          {levelGlyph(level)}
        </span>
      </div>

      <div className="glance-body">
        <div className="glance-word" data-testid={id === 'engineers' ? 'engineers-count' : undefined}>
          {wordContent}
        </div>
      </div>

      <div className="glance-footer">
        {detail && <div className="glance-detail">{detail}</div>}
        {reason && <div className="glance-reason">{reason}</div>}
        {children && (
          <div
            className="glance-extra"
            onClick={(e) => {
              // Prevent triggering tile click when interacting with child controls
              e.stopPropagation();
            }}
          >
            {children}
          </div>
        )}
      </div>
    </div>
  );
}
