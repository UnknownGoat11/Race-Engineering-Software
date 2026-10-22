import { describe, expect, it } from 'vitest';
import { carMarkerLabel, extractCarNumber, parseCarNumber } from './carNumber';

describe('car number extraction (v1.3 U10/V04)', () => {
  it('prefers the #-marked number', () => {
    expect(parseCarNumber('#51 Ferrari 499P')).toBe('51');
    expect(parseCarNumber('Oreca 07 #81')).toBe('81');
    expect(parseCarNumber('#92')).toBe('92');
    expect(parseCarNumber('#23 Mercedes-AMG LMGT3')).toBe('23');
  });

  it('accepts a bare number; model digits without # are never a car number', () => {
    expect(parseCarNumber('44')).toBe('44');
    expect(parseCarNumber('BMW M4 44')).toBeNull();
    expect(parseCarNumber('Porsche 963')).toBeNull();
    expect(parseCarNumber('Oreca 07')).toBeNull();
    expect(parseCarNumber('Ferrari 499P')).toBeNull();
    expect(parseCarNumber('Mercedes-AMG LMGT3')).toBeNull();
  });

  it('table cell shows an em dash when there is no number', () => {
    expect(extractCarNumber(null)).toBe('—');
    expect(extractCarNumber('Ferrari 499P')).toBe('—');
  });

  it('map label is the number, else the first 3 characters, never the full name', () => {
    expect(carMarkerLabel('#23 Mercedes-AMG LMGT3')).toBe('23');
    expect(carMarkerLabel('Ferrari 499P')).toBe('Fer');
    expect(carMarkerLabel('  ')).toBe('');
    expect(carMarkerLabel(null)).toBe('');
    expect(carMarkerLabel('7')).toBe('7');
  });
});
