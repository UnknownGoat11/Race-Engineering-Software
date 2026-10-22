import type { JSX, ReactNode } from 'react';
import type { Level } from '../../lib/statusModel';
import { levelGlyph } from './driverBoardModel';

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
  /** Opens the diagnostics for this tile. Rendered as a small "Details" button in the tile header (A02). */
  readonly onClick?: () => void;
}

function getWordSizeModifier(word: string): string {
  const trimmed = word.trim();
  const len = trimmed.length;
  // Short words e.g. 'LIVE', 'OFF', 'REC', 'DEMO', '0 / 2', '100%'
  if (len <= 5) {
    return 'glance-word--lg';
  }
  // Single medium word without spaces e.g. 'PAUSED', 'FAILED', 'STALE', 'ERROR', 'STALLED'
  if (len <= 7 && !trimmed.includes(' ')) {
    return 'glance-word--md';
  }
  // Longer words or phrases e.g. 'NOT RUNNING', 'NOT SET UP', 'SUSPENDED', 'DEGRADED', 'CHECKING'
  if (len <= 11) {
    return 'glance-word--sm';
  }
  // Extra long phrases e.g. 'NOT AVAILABLE', 'RECONNECTING', 'OFF IN SETTINGS', 'AUTHENTICATING'
  return 'glance-word--xs';
}

/**
 * One driver status tile. The tile is a plain labelled region (role="group", named by its label), so its own
 * buttons (Remove, Copy code, New code) are never nested inside another interactive element (A02), and a
 * screen reader reads the whole content, including the reason line.
 */
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
  const labelId = `glance-label-${id}`;
  const tileClasses = ['glance-tile', `glance-tile-${level}`, `glance-tile-${id}`].join(' ');

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

  const wordModifier = typeof word === 'string' && !isCode ? getWordSizeModifier(word) : '';
  const wordClasses = ['glance-word', wordModifier].filter(Boolean).join(' ');
  const glyph = levelGlyph(level);

  return (
    <div className={tileClasses} data-testid={testId ?? `tile-${id}`} role="group" aria-labelledby={labelId}>
      <div className="glance-header">
        <span className="glance-label" id={labelId}>
          {label}
        </span>
        <span className="glance-header-end">
          {glyph !== '' && (
            <span className="glance-glyph" aria-hidden="true">
              {glyph}
            </span>
          )}
          {onClick && (
            <button
              type="button"
              className="glance-details-btn"
              onClick={onClick}
              aria-label={`${label} details`}
              data-testid={`tile-${id}-details`}
            >
              Details
            </button>
          )}
        </span>
      </div>

      <div className="glance-body">
        <div className={wordClasses} data-testid={id === 'engineers' ? 'engineers-count' : undefined}>
          {wordContent}
        </div>
      </div>

      <div className="glance-footer">
        {detail && <div className="glance-detail">{detail}</div>}
        {reason && <div className="glance-reason">{reason}</div>}
        {children && <div className="glance-extra">{children}</div>}
      </div>
    </div>
  );
}
