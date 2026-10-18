import type { JSX, ReactNode } from 'react';
import '../../styles/panel.css';

export type StatusLevel = 'ok' | 'info' | 'warn' | 'critical' | 'off';
export type DqLevel = 'live' | 'stale' | 'unavailable' | 'synthetic' | 'degraded' | 'untrusted' | 'ended';
export type CarClassLevel = 'hypercar' | 'lmp2' | 'gt3' | 'gte' | 'unknown';

export interface StatusBadgeProps {
  readonly level?: StatusLevel | undefined;
  readonly dq?: DqLevel | undefined;
  readonly carClass?: CarClassLevel | undefined;
  readonly label?: string | undefined;
  readonly glyph?: string | boolean | undefined;
  readonly className?: string | undefined;
  readonly title?: string | undefined;
  readonly testId?: string | undefined;
  readonly children?: ReactNode;
}

function getDefaultGlyph(level?: StatusLevel, dq?: DqLevel): string | null {
  if (dq) {
    switch (dq) {
      case 'live':
        return '●';
      case 'degraded':
        return '▲';
      case 'untrusted':
        return '■';
      case 'unavailable':
        return '—';
      default:
        return null;
    }
  }
  if (level) {
    switch (level) {
      case 'ok':
        return '●';
      case 'info':
        return 'ℹ';
      case 'warn':
        return '▲';
      case 'critical':
        return '■';
      case 'off':
        return '○';
    }
  }
  return null;
}

/**
 * StatusBadge per REDESIGN §2.6:
 * Min-height 18px, padding 0 6px, radius 2, --type-badge, fill = level-bg, text = level color, 1px border.
 */
export function StatusBadge({
  level,
  dq,
  carClass,
  label,
  glyph = true,
  className = '',
  title,
  testId,
  children,
}: StatusBadgeProps): JSX.Element {
  let variantClass = 'status-badge-info';
  if (carClass) {
    variantClass = `status-badge-${carClass}`;
  } else if (dq) {
    variantClass = `status-badge-${dq}`;
  } else if (level) {
    variantClass = `status-badge-${level}`;
  }

  const activeGlyph = typeof glyph === 'string' ? glyph : glyph ? getDefaultGlyph(level, dq) : null;
  const content = children ?? label;

  return (
    <span
      className={`status-badge ${variantClass} ${className}`.trim()}
      title={title}
      data-testid={testId}
    >
      {activeGlyph && <span className="status-badge-glyph">{activeGlyph}</span>}
      {content}
    </span>
  );
}
