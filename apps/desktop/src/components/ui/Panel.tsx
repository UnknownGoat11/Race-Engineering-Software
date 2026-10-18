import type { JSX, ReactNode } from 'react';
import { useScopeDq, type ScopeDq } from '../telemetry/presentation';
import { StatusBadge } from './StatusBadge';
import '../../styles/panel.css';

export interface PanelProps {
  readonly title: string;
  readonly dq?: ScopeDq | 'synthetic' | undefined;
  readonly ageS?: number | null | undefined;
  readonly reason?: string | null | undefined;
  readonly meta?: ReactNode | undefined;
  readonly actions?: ReactNode | undefined;
  readonly footer?: ReactNode | undefined;
  readonly className?: string | undefined;
  readonly testId?: string | undefined;
  readonly children: ReactNode;
}

/**
 * Shared Panel container per REDESIGN §2.6:
 * - 1px --border-subtle, radius 2
 * - Header 28px --surface-panel-header, padding 0 8px
 * - DQ glyph, panel title uppercase, meta tags, actions slot
 * - Body padding 8px --surface-panel
 * - Supports DQ states: live, stale, unavailable, synthetic, degraded, untrusted, ended
 */
export function Panel({
  title,
  dq: explicitDq,
  ageS,
  reason,
  meta,
  actions,
  footer,
  className = '',
  testId,
  children,
}: PanelProps): JSX.Element {
  const scopeDq = useScopeDq();
  const activeDq = explicitDq !== undefined ? explicitDq : scopeDq;

  const renderDqGlyph = (): JSX.Element | null => {
    if (!activeDq || activeDq === 'live') {
      return <span className="panel-dq-glyph panel-dq-glyph--live" aria-hidden="true">●</span>;
    }
    if (activeDq === 'stale') {
      return <span className="panel-dq-glyph panel-dq-glyph--stale" aria-hidden="true">●</span>;
    }
    if (activeDq === 'degraded') {
      return <span className="panel-dq-glyph panel-dq-glyph--degraded" aria-hidden="true">▲</span>;
    }
    if (activeDq === 'untrusted') {
      return <span className="panel-dq-glyph panel-dq-glyph--untrusted" aria-hidden="true">■</span>;
    }
    if (activeDq === 'unavailable') {
      return <span className="panel-dq-glyph panel-dq-glyph--unavailable" aria-hidden="true">—</span>;
    }
    if (activeDq === 'ended') {
      return <span className="panel-dq-glyph panel-dq-glyph--ended" aria-hidden="true">○</span>;
    }
    return null;
  };

  const renderDefaultMeta = (): JSX.Element | null => {
    if (activeDq === 'stale') {
      const ageText = ageS != null ? ` ${ageS.toFixed(1)} s` : '';
      return <StatusBadge dq="stale" label={`STALE${ageText}`} glyph={false} />;
    }
    if (activeDq === 'untrusted') {
      return <StatusBadge dq="untrusted" label="UNTRUSTED" glyph={false} />;
    }
    if (activeDq === 'synthetic') {
      return <StatusBadge dq="synthetic" label="SYNTHETIC" glyph={false} />;
    }
    if (activeDq === 'ended') {
      return <StatusBadge dq="ended" label="ENDED" glyph={false} />;
    }
    return null;
  };

  const isUnavailable = activeDq === 'unavailable';
  const isUntrusted = activeDq === 'untrusted';

  return (
    <div
      className={`panel ${className}`.trim()}
      data-dq={activeDq ?? undefined}
      data-testid={testId}
    >
      <div className="panel-header">
        <div className="panel-header-left">
          {renderDqGlyph()}
          <span className="panel-title">{title}</span>
        </div>
        <div className="panel-header-right">
          {renderDefaultMeta()}
          {meta && <div className="panel-meta">{meta}</div>}
          {actions && <div className="panel-actions">{actions}</div>}
        </div>
      </div>

      <div className="panel-body">
        {isUnavailable ? (
          <div className="panel-unavailable-note">
            Unavailable — {reason ?? 'Not supported by this vehicle/session'}
          </div>
        ) : (
          <>
            {isUntrusted && (
              <div className="panel-untrusted-note">Untrusted — data format mismatch</div>
            )}
            {children}
          </>
        )}
      </div>

      {footer && <div className="panel-footer">{footer}</div>}
    </div>
  );
}
