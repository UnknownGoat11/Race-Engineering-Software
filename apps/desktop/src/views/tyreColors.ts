/**
 * V1.1 Tyre and Brake Thermal Color Scale.
 *
 * Implements the user's exact 6-tier specification:
 * - dark blue: cold (< 65°C)
 * - light blue: slight cold (65 - 75°C)
 * - green: optimal (75 - 95°C)
 * - light orange: getting warm (95 - 105°C)
 * - orange: warm (105 - 115°C)
 * - red: hot (> 115°C)
 */

export interface TyreThermalRating {
  readonly bg: string;
  readonly text: string;
  readonly label: string;
}

export function getTyreTempColor(tempC: number): TyreThermalRating {
  if (tempC < 65) {
    return { bg: '#1e3a8a', text: '#bfdbfe', label: 'COLD' }; // Dark Blue
  }
  if (tempC < 75) {
    return { bg: '#0284c7', text: '#e0f2fe', label: 'SLIGHT COLD' }; // Light Blue
  }
  if (tempC <= 95) {
    return { bg: '#16a34a', text: '#dcfce7', label: 'OPTIMAL' }; // Green
  }
  if (tempC <= 105) {
    return { bg: '#f97316', text: '#ffedd5', label: 'GETTING WARM' }; // Light Orange
  }
  if (tempC <= 115) {
    return { bg: '#ea580c', text: '#fff7ed', label: 'WARM' }; // Orange
  }
  return { bg: '#dc2626', text: '#fee2e2', label: 'HOT' }; // Red
}

export function getBrakeDiscColor(tempC: number): string {
  if (tempC < 250) return '#3b82f6';
  if (tempC < 450) return '#10b981';
  if (tempC < 650) return '#f59e0b';
  return '#ef4444';
}
