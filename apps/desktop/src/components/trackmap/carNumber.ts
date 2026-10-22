/**
 * Car number extraction shared by the timing tower and the track map (v1.3 U10/V04).
 *
 * LMU scoring carries the car number only inside the vehicle name ("#51 Ferrari 499P", "Oreca 07 #81", "#92").
 * A number marked with '#' wins over any other digits (so "Oreca 07 #81" is 81, not 07).
 */
import { NO_VALUE } from '../../lib/units';

const HASH_NUMBER = /#(\d{1,3})\b/;
// A name that is only the number ("92"). Other digits are usually model names ("Porsche 963", "Oreca 07"), not a
// car number, so a name without '#' and without a bare number has no number ("—").
const BARE_NUMBER = /^\s*(\d{1,3})\s*$/;

/** Car number from a vehicle name, or null when the name carries none. */
export function parseCarNumber(vehicleName: string | null | undefined): string | null {
  if (!vehicleName) return null;
  const hash = HASH_NUMBER.exec(vehicleName);
  if (hash?.[1]) return hash[1];
  const bare = BARE_NUMBER.exec(vehicleName);
  if (bare?.[1]) return bare[1];
  return null;
}

/** Car number for a table cell: the number, or "—" when the name carries none. */
export function extractCarNumber(vehicleName: string | null | undefined): string {
  return parseCarNumber(vehicleName) ?? NO_VALUE;
}

/**
 * Short marker label for the track map: the car number, else the first 3 characters of the name, else "".
 * The full name belongs in the marker's tooltip, never on the marker.
 */
export function carMarkerLabel(vehicleName: string | null | undefined): string {
  const num = parseCarNumber(vehicleName);
  if (num !== null) return num;
  const trimmed = (vehicleName ?? '').trim();
  return trimmed.slice(0, 3);
}

/** Vehicle part of a tooltip: a bare number gets its '#' ("23" -> "#23"), a full name is kept as is. */
export function carVehicleTitle(vehicleName: string | null | undefined): string {
  const name = (vehicleName ?? '').trim();
  return /^\d{1,3}$/.test(name) ? `#${name}` : name;
}
