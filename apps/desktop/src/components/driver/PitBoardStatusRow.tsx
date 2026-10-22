/**
 * Pit board status row (v1.3 audit D01): one 32 px row of chips so the driver can check, without leaving the
 * pit board, that LMU is live, the lap is recorded, the stream is up, who is watching and the engineer code.
 * Built from the same derived tiles as the Setup page (deriveDriverTiles); same levels and glyphs. Colour only
 * when a chip is not OK (warning, critical, demo); OK and off stay neutral.
 */
import type { JSX } from 'react';
import type { DriverTiles, Level } from '../../lib/statusModel';
import { levelGlyph, pitBoardChips } from './driverBoardModel';

const COLOURED: ReadonlySet<Level> = new Set<Level>(['warning', 'critical', 'synthetic']);

export interface PitBoardStatusRowProps {
  readonly tiles: DriverTiles;
}

export function PitBoardStatusRow({ tiles }: PitBoardStatusRowProps): JSX.Element {
  return (
    <div className="pit-board-status-row" role="group" aria-label="Connection status" data-testid="pit-board-status-row">
      {pitBoardChips(tiles).map((c) => {
        const glyph = levelGlyph(c.level);
        const colourClass = COLOURED.has(c.level) ? ` pit-status-chip--${c.level}` : '';
        return (
          <span
            key={c.id}
            className={`pit-status-chip${colourClass}`}
            data-testid={`pit-status-${c.id}`}
            data-level={c.level}
          >
            {glyph !== '' && (
              <span className="pit-status-chip__glyph" aria-hidden="true">
                {glyph}
              </span>
            )}
            <span className="pit-status-chip__label">{c.label}</span>
            {/* "REC REC" reads as a stutter: the value is omitted when it repeats the label. */}
            {c.value !== c.label && <span className="pit-status-chip__value">{c.value}</span>}
          </span>
        );
      })}
    </div>
  );
}
