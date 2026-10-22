/**
 * Pure helpers of the driver pages (v1.3 UI pass): level glyphs, pit board status chips (D01), flag strip (U04),
 * pit limiter (U05) and the remembered driver sub-tab (N03). Kept apart from the components so they can be
 * unit-tested and the component files export components only (fast refresh).
 */
import type { DriverTiles, Level, Tile } from '../../lib/statusModel';
import { NO_VALUE } from '../../lib/units';

/**
 * Shape glyph of a level (●◆■▨). Off has no glyph: an 8 px "□" reads as a missing-font box (V12), and an
 * off tile already says so with its word and neutral colour.
 */
export function levelGlyph(level: Level): string {
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
      return '';
  }
}

export interface Chip {
  readonly id: string;
  readonly label: string;
  readonly value: string;
  readonly level: Level;
}

function chip(id: string, label: string, t: Tile, value: string = t.word): Chip {
  return { id, label, value, level: t.level };
}

/** Engineers chip value without the inner spaces ("1 / 2" -> "1/2") so the row stays one line. */
function compactCount(word: string): string {
  return word.replace(/\s*\/\s*/g, '/');
}

export function pitBoardChips(tiles: DriverTiles): readonly Chip[] {
  return [
    chip('lmu', 'LMU', tiles.lmu),
    chip('rec', 'REC', tiles.recording),
    chip('stream', 'STREAM', tiles.streaming),
    chip('eng', 'ENG', tiles.engineers, compactCount(tiles.engineers.word)),
    chip('code', 'CODE', tiles.code, tiles.code.code ?? NO_VALUE),
  ];
}

export type FlagStripKind = 'clear' | 'warn' | 'blue' | 'critical' | 'chequered' | 'unknown';

export interface FlagStripView {
  readonly kind: FlagStripKind;
  /** Shape glyph from the shared set (never emoji); rendered aria-hidden. */
  readonly glyph: string;
  readonly text: string;
}

/**
 * Flag strip content. `flags.summary` 0 means green (channel label: 0 green, 1 local yellow, 2 FCY/SC, 3 blue,
 * 4 red, 5 chequered). When the summary is unknown, a positive yellow or blue channel still shows its flag;
 * otherwise the strip is neutral "Flag —" (no colour), never an invented green.
 */
export function flagStripView(
  flagSummary: number | null,
  yellowState: number | null,
  blueFlag: number | null,
): FlagStripView {
  switch (flagSummary) {
    case 0:
      return { kind: 'clear', glyph: '●', text: 'Green flag' };
    case 1:
      return { kind: 'warn', glyph: '◆', text: 'Local yellow' };
    case 2:
      return { kind: 'warn', glyph: '◆', text: 'Full course yellow / safety car' };
    case 3:
      return { kind: 'blue', glyph: '◆', text: 'Blue flag — let the car through' };
    case 4:
      return { kind: 'critical', glyph: '■', text: 'Red flag — session stopped' };
    case 5:
      return { kind: 'chequered', glyph: '□', text: 'Chequered flag' };
    default:
      break;
  }
  if (flagSummary === null) {
    // Raw yellow state 7 is a race halt (red); 1-6 is a full-course yellow / safety car period.
    if (yellowState === 7) return { kind: 'critical', glyph: '■', text: 'Red flag — session stopped' };
    if (yellowState !== null && yellowState >= 1 && yellowState <= 6) {
      return { kind: 'warn', glyph: '◆', text: 'Yellow flag' };
    }
    if (blueFlag !== null && blueFlag > 0) return { kind: 'blue', glyph: '◆', text: 'Blue flag' };
  }
  return { kind: 'unknown', glyph: '', text: `Flag ${NO_VALUE}` };
}

export interface LimiterView {
  /** 'on' | 'off', or 'unknown' when neither limiter channel has a value. */
  readonly state: 'on' | 'off' | 'unknown';
  readonly word: string;
  readonly caption: string;
}

/** Pit limiter: 0x010D (active) wins; the legacy 0x010C is read only when 0x010D has no value. */
export function limiterView(active: number | null, legacy: number | null): LimiterView {
  const value = active ?? legacy;
  if (value === null) return { state: 'unknown', word: NO_VALUE, caption: 'No data' };
  return value > 0
    ? { state: 'on', word: 'ON', caption: 'Limiter on' }
    : { state: 'off', word: 'OFF', caption: 'Limiter off' };
}

export type DriverSubTab = 'setup' | 'pitboard';

/** localStorage key of the last driver sub-tab (N03). */
export const DRIVER_SUBTAB_STORAGE_KEY = 'ret.driver.subTab';

/** Last sub-tab from localStorage; "setup" when storage is unavailable or holds anything else. */
export function readStoredSubTab(): DriverSubTab {
  try {
    const value = window.localStorage.getItem(DRIVER_SUBTAB_STORAGE_KEY);
    return value === 'pitboard' ? 'pitboard' : 'setup';
  } catch {
    return 'setup';
  }
}

export function storeSubTab(tab: DriverSubTab): void {
  try {
    window.localStorage.setItem(DRIVER_SUBTAB_STORAGE_KEY, tab);
  } catch {
    // Storage blocked (private window, policy): the choice simply is not remembered.
  }
}

