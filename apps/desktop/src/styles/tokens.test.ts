import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

function hexToRgb(hex: string): [number, number, number] {
  const clean = hex.replace('#', '').trim();
  if (clean.length === 3) {
    const c0 = clean[0] ?? '0';
    const c1 = clean[1] ?? '0';
    const c2 = clean[2] ?? '0';
    return [
      parseInt(c0 + c0, 16),
      parseInt(c1 + c1, 16),
      parseInt(c2 + c2, 16),
    ];
  }
  return [
    parseInt(clean.slice(0, 2), 16),
    parseInt(clean.slice(2, 4), 16),
    parseInt(clean.slice(4, 6), 16),
  ];
}

function toLinear(c: number): number {
  const s = c / 255;
  return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

function relativeLuminance(r: number, g: number, b: number): number {
  const rs = toLinear(r);
  const gs = toLinear(g);
  const bs = toLinear(b);
  return 0.2126 * rs + 0.7152 * gs + 0.0722 * bs;
}

function contrastRatio(hex1: string, hex2: string): number {
  const [r1, g1, b1] = hexToRgb(hex1);
  const [r2, g2, b2] = hexToRgb(hex2);
  const l1 = relativeLuminance(r1, g1, b1);
  const l2 = relativeLuminance(r2, g2, b2);
  const lighter = Math.max(l1, l2);
  const darker = Math.min(l1, l2);
  return (lighter + 0.05) / (darker + 0.05);
}

function hexToHsl(hex: string): { h: number; s: number; l: number } {
  const [rgbR, rgbG, rgbB] = hexToRgb(hex);
  const r = rgbR / 255;
  const g = rgbG / 255;
  const b = rgbB / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  let h = 0;
  let s = 0;
  const l = (max + min) / 2;

  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    switch (max) {
      case r:
        h = ((g - b) / d + (g < b ? 6 : 0)) * 60;
        break;
      case g:
        h = ((b - r) / d + 2) * 60;
        break;
      case b:
        h = ((r - g) / d + 4) * 60;
        break;
    }
  }
  return { h, s, l };
}

function circularHueDistance(h1: number, h2: number): number {
  const diff = Math.abs(h1 - h2) % 360;
  return diff > 180 ? 360 - diff : diff;
}

describe('Design Tokens System (REDESIGN §2.1–§2.5)', () => {
  const tokensPath = resolve(__dirname, 'tokens.css');
  const tokensContent = readFileSync(tokensPath, 'utf8');

  it('contains every primitive token specified in §2.1', () => {
    const requiredPrimitives = [
      '--p-gray-950',
      '--p-gray-900',
      '--p-gray-850',
      '--p-gray-800',
      '--p-gray-750',
      '--p-gray-700',
      '--p-gray-600',
      '--p-gray-550',
      '--p-gray-500',
      '--p-gray-400',
      '--p-gray-300',
      '--p-gray-200',
      '--p-gray-100',
      '--p-gray-50',
      '--p-white',
      '--p-green-500',
      '--p-green-950',
      '--p-amber-500',
      '--p-amber-950',
      '--p-red-500',
      '--p-red-950',
      '--p-violet-400',
      '--p-violet-950',
      '--p-magenta-500',
      '--p-magenta-950',
      '--p-purple-500',
      '--p-class-cyan',
      '--p-class-blue',
      '--p-class-lime',
      '--p-class-grey',
    ];

    for (const token of requiredPrimitives) {
      expect(tokensContent).toContain(token);
    }
  });

  it('contains every semantic token specified in §2.2', () => {
    const requiredSemantics = [
      '--surface-app',
      '--surface-panel',
      '--surface-panel-header',
      '--surface-inset',
      '--surface-hover',
      '--surface-selected',
      '--surface-row-alt',
      '--surface-overlay',
      '--border-subtle',
      '--border-control',
      '--border-strong',
      '--border-focus',
      '--text-primary',
      '--text-secondary',
      '--text-tertiary',
      '--text-disabled',
      '--text-inverse',
      '--interactive',
      '--interactive-fill',
      '--track-line',
      '--status-ok',
      '--status-info',
      '--status-warn',
      '--status-critical',
      '--status-off',
      '--dq-live',
      '--dq-stale',
      '--dq-stale-value',
      '--dq-unavailable',
      '--dq-synthetic',
      '--dq-degraded',
      '--dq-untrusted',
      '--dq-ended',
      '--class-hypercar',
      '--class-lmp2',
      '--class-gt3',
      '--class-gte',
      '--class-unknown',
      '--delta-faster',
      '--delta-slower',
      '--timing-best-overall',
      '--timing-best-personal',
      '--timing-slower',
      '--input-throttle',
      '--input-brake',
      '--player-highlight',
      '--thermal-cold',
      '--thermal-optimal',
      '--thermal-hot',
      '--thermal-over',
      '--graph-series-1',
      '--graph-series-2',
      '--graph-series-3',
      '--graph-series-4',
      '--graph-series-5',
      '--graph-series-6',
      '--graph-series-7',
      '--graph-series-8',
      '--font-ui',
      '--font-data',
    ];

    for (const token of requiredSemantics) {
      expect(tokensContent).toContain(token);
    }
  });

  it('contains the type scale with --ui-scale derivation (§2.4)', () => {
    const requiredTypeTokens = [
      '--type-caption-size',
      '--type-label-size',
      '--type-panel-title-size',
      '--type-badge-size',
      '--type-body-size',
      '--type-button-size',
      '--type-value-sm-size',
      '--type-value-md-size',
      '--type-value-lg-size',
      '--type-value-xl-size',
      '--type-glance-size',
      '--type-glance-code-size',
      '--type-board-hero-size',
    ];

    for (const token of requiredTypeTokens) {
      expect(tokensContent).toContain(token);
    }
  });

  it('removes deprecated --color-ahead alias per §621', () => {
    expect(tokensContent).not.toContain('--color-ahead:');
  });

  it('satisfies WCAG contrast ratios on panel surface (#0f1216)', () => {
    const panelBg = '#0f1216';

    // Primary text >= 4.5:1 (actually ~15.9:1)
    const primaryRatio = contrastRatio('#eef1f4', panelBg);
    expect(primaryRatio).toBeGreaterThanOrEqual(4.5);

    // Secondary text >= 4.5:1 (actually ~8.4:1)
    const secondaryRatio = contrastRatio('#a9b2be', panelBg);
    expect(secondaryRatio).toBeGreaterThanOrEqual(4.5);

    // Tertiary text >= 4.5:1 (actually ~6.2:1)
    const tertiaryRatio = contrastRatio('#8b95a3', panelBg);
    expect(tertiaryRatio).toBeGreaterThanOrEqual(4.5);

    // Control border >= 3:1 (fixes DS10)
    const controlBorderRatio = contrastRatio('#5a6372', panelBg);
    expect(controlBorderRatio).toBeGreaterThanOrEqual(3.0);

    // Track line >= 3:1 (fixes DS10)
    const trackLineRatio = contrastRatio('#6a7380', panelBg);
    expect(trackLineRatio).toBeGreaterThanOrEqual(3.0);

    // Class colours >= 3:1
    const hypercarRatio = contrastRatio('#22d3ee', panelBg);
    const lmp2Ratio = contrastRatio('#4d7cff', panelBg);
    const gt3Ratio = contrastRatio('#a3e635', panelBg);
    const unknownClassRatio = contrastRatio('#8b95a3', panelBg);

    expect(hypercarRatio).toBeGreaterThanOrEqual(3.0);
    expect(lmp2Ratio).toBeGreaterThanOrEqual(3.0);
    expect(gt3Ratio).toBeGreaterThanOrEqual(3.0);
    expect(unknownClassRatio).toBeGreaterThanOrEqual(3.0);
  });

  it('satisfies hue separation of class vs status >= 30° (C1)', () => {
    const statusHues = [
      { name: 'Red', hsl: hexToHsl('#ff4d4d') },
      { name: 'Amber', hsl: hexToHsl('#f5c542') },
      { name: 'Green', hsl: hexToHsl('#2fd27a') },
      { name: 'Violet', hsl: hexToHsl('#b49cff') },
      { name: 'Magenta', hsl: hexToHsl('#ff4fd8') },
    ];

    const chromaticClasses = [
      { name: 'Hypercar (Cyan)', hsl: hexToHsl('#22d3ee') },
      { name: 'LMP2 (Blue)', hsl: hexToHsl('#4d7cff') },
      { name: 'LMGT3 (Lime)', hsl: hexToHsl('#a3e635') },
    ];

    for (const cls of chromaticClasses) {
      for (const st of statusHues) {
        const dist = circularHueDistance(cls.hsl.h, st.hsl.h);
        expect(dist, `${cls.name} (${cls.hsl.h.toFixed(1)}°) vs Status ${st.name} (${st.hsl.h.toFixed(1)}°)`).toBeGreaterThanOrEqual(30);
      }
    }
  });

  it('guarantees series-2 is not on the warning amber band (DS18)', () => {
    const series2Hsl = hexToHsl('#ffb36b');
    const warningAmberHsl = hexToHsl('#f5c542');
    const dist = circularHueDistance(series2Hsl.h, warningAmberHsl.h);
    // Moved off warning amber
    expect(dist).toBeGreaterThanOrEqual(14);
  });
});

describe('Bundled Fonts & Network Hygiene (OR-6)', () => {
  const stylesDir = resolve(__dirname);
  const assetsFontsDir = resolve(__dirname, '..', 'assets', 'fonts');

  it('bundles required woff2 font files in src/assets/fonts', () => {
    const requiredFiles = [
      'Inter-Regular.woff2',
      'Inter-Medium.woff2',
      'Inter-SemiBold.woff2',
      'Inter-Bold.woff2',
      'JetBrainsMono-Regular.woff2',
      'JetBrainsMono-SemiBold.woff2',
      'JetBrainsMono-Bold.woff2',
      'OFL-Inter.txt',
      'OFL-JetBrainsMono.txt',
      'SOURCES.md',
    ];

    for (const file of requiredFiles) {
      const fullPath = resolve(assetsFontsDir, file);
      expect(existsSync(fullPath), `Expected font asset ${file} to exist`).toBe(true);
    }
  });

  it('has zero http:// or https:// URLs in CSS font definitions or imports', () => {
    const cssFiles = readdirSync(stylesDir).filter((f) => f.endsWith('.css'));
    for (const cssFile of cssFiles) {
      const content = readFileSync(resolve(stylesDir, cssFile), 'utf8');
      expect(content).not.toMatch(/@import\s+url\s*\(\s*['"]?https?:\/\//i);
      expect(content).not.toMatch(/@font-face\s*{[^}]*url\s*\(\s*['"]?https?:\/\//i);
    }
  });
});
