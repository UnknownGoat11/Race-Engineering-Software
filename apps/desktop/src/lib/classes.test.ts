import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  VEHICLE_CLASSES,
  classColorToken,
  classColorVar,
  parseVehicleClass,
  vehicleClassCode,
  vehicleClassName,
} from './classes';

describe('parseVehicleClass', () => {
  it.each([
    ['Hypercar', 'hypercar'],
    ['Hyper', 'hypercar'],
    ['Hypercar H1', 'hypercar'],
    ['hypercar', 'hypercar'],
    ['LMH', 'hypercar'],
    ['LMP2', 'lmp2'],
    ['LMP2_ELMS', 'lmp2'],
    ['lmp2', 'lmp2'],
    ['GT3', 'gt3'],
    ['LMGT3', 'gt3'],
    ['GT3 G3', 'gt3'],
    ['GTE', 'gte'],
    ['LMGTE', 'gte'],
    ['LM-GTE', 'gte'],
    ['LMP3', 'unknown'],
    ['', 'unknown'],
    ['   ', 'unknown'],
    ['Formula', 'unknown'],
  ])('%s → %s', (name, expected) => {
    expect(parseVehicleClass(name)).toBe(expected);
  });
  it('null / undefined → unknown', () => {
    expect(parseVehicleClass(null)).toBe('unknown');
    expect(parseVehicleClass(undefined)).toBe('unknown');
  });
});

describe('class codes, names and colours', () => {
  it('codes are the 3-char chips', () => {
    expect(vehicleClassCode('hypercar')).toBe('HY');
    expect(vehicleClassCode('LMP2')).toBe('P2');
    expect(vehicleClassCode('LMGT3')).toBe('GT3');
    expect(vehicleClassCode('gte')).toBe('GTE');
    expect(vehicleClassCode(null)).toBe('—');
    for (const c of VEHICLE_CLASSES) expect(vehicleClassCode(c).length).toBeLessThanOrEqual(3);
  });
  it('names', () => {
    expect(vehicleClassName('gt3')).toBe('LMGT3');
    expect(vehicleClassName('foo')).toBe('Unknown class');
  });
  it('colour tokens are CSS variables, never hex', () => {
    expect(classColorVar('Hypercar')).toBe('var(--class-hypercar)');
    expect(classColorVar('LMP2')).toBe('var(--class-lmp2)');
    expect(classColorVar('GT3')).toBe('var(--class-gt3)');
    expect(classColorVar('GTE')).toBe('var(--class-gte)');
    expect(classColorVar('LMP3')).toBe('var(--class-unknown)');
    expect(classColorToken('gt3')).toBe('--class-gt3');
  });
  it('tokens.css defines every class token with the approved colours', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/styles/tokens.css'), 'utf8');
    const value = (name: string): string | undefined =>
      new RegExp(`${name}:\\s*([^;]+);`).exec(css)?.[1]?.trim();
    expect(value('--class-hypercar')).toMatch(/#22d3ee|var\(--p-class-cyan\)/);
    expect(value('--class-lmp2')).toMatch(/#4d7cff|var\(--p-class-blue\)/);
    expect(value('--class-gt3')).toMatch(/#a3e635|var\(--p-class-lime\)/);
    expect(value('--class-gte')).toMatch(/#a3e635|var\(--p-class-lime\)/);
    expect(value('--class-unknown')).toMatch(/#8b95a3|var\(--p-class-grey\)/);
    expect(css).toMatch(/--p-class-cyan:\s*#22d3ee;/);
    expect(css).toMatch(/--p-class-blue:\s*#4d7cff;/);
    expect(css).toMatch(/--p-class-lime:\s*#a3e635;/);
    expect(css).toMatch(/--p-class-grey:\s*#8b95a3;/);
    for (const c of VEHICLE_CLASSES) expect(css).toContain(`${classColorToken(c)}:`);
  });
});
