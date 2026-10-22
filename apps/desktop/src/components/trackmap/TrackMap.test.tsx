import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import {
  applyTransform,
  computeBounds,
  fitMapTransform,
  fitPointsFor,
  inverseTransform,
} from './transform';
import { buildSvgPath, classToColor, positionAt, wrapDistance } from './trackGeometry';
import { TrackMap } from './TrackMap';
import type { TrackMap as TrackMapData } from './types';

// Mock ResizeObserver for test environment
class MockResizeObserver {
  private cb: ResizeObserverCallback;
  constructor(cb: ResizeObserverCallback) {
    this.cb = cb;
  }
  observe(target: Element): void {
    this.cb(
      [
        {
          target,
          contentRect: {
            width: 800,
            height: 600,
            top: 0,
            left: 0,
            bottom: 600,
            right: 800,
            x: 0,
            y: 0,
            toJSON: () => ({}),
          },
        } as ResizeObserverEntry,
      ],
      this,
    );
  }
  unobserve(): void {
    // no-op in test mock
  }
  disconnect(): void {
    // no-op in test mock
  }
}

globalThis.ResizeObserver = MockResizeObserver;

const TEST_MAP: TrackMapData = {
  trackId: 'TEST_CIRCUIT_1000',
  version: 1,
  source: 'verified',
  lengthM: 1000,
  points: [
    { distM: 0, x: 0, z: 0 },
    { distM: 250, x: 250, z: 0 },
    { distM: 500, x: 250, z: 250 },
    { distM: 750, x: 0, z: 250 },
  ],
  sectors: [
    { index: 1, distM: 333, x: 250, z: 83 },
    { index: 2, distM: 666, x: 84, z: 250 },
  ],
  startFinish: { distM: 0, x: 0, z: 0 },
  pitEntry: { distM: 900, x: 0, z: 100 },
  pitExit: { distM: 50, x: 50, z: 0 },
};

describe('trackmap transform', () => {
  it('computes correct bounds from points', () => {
    const bounds = computeBounds(TEST_MAP.points);
    expect(bounds).not.toBeNull();
    expect(bounds?.minX).toBe(0);
    expect(bounds?.maxX).toBe(250);
    expect(bounds?.minZ).toBe(0);
    expect(bounds?.maxZ).toBe(250);
  });

  it('returns null bounds for empty or non-finite inputs', () => {
    expect(computeBounds([])).toBeNull();
    expect(computeBounds([{ x: NaN, z: 10 }])).toBeNull();
  });

  it('fits map transform within viewport and padding', () => {
    const bounds = computeBounds(TEST_MAP.points);
    if (!bounds) throw new Error('Bounds should be defined');
    const transform = fitMapTransform(bounds, TEST_MAP.points, 800, 600, 20);
    expect(transform).not.toBeNull();
    expect(transform?.scale).toBeGreaterThan(0);
    expect(transform?.offsetU).toBe(400);
    expect(transform?.offsetV).toBe(300);
  });

  it('inverts transformed coordinates accurately', () => {
    const bounds = computeBounds(TEST_MAP.points);
    if (!bounds) throw new Error('Bounds should be defined');
    const transform = fitMapTransform(bounds, TEST_MAP.points, 800, 600, 20, Math.PI / 4);
    if (!transform) throw new Error('Transform should be defined');
    const [u, v] = applyTransform(transform, 125, 75);
    const [x, z] = inverseTransform(transform, u, v);
    expect(x).toBeCloseTo(125, 4);
    expect(z).toBeCloseTo(75, 4);
  });
});

describe('trackmap geometry', () => {
  it('wraps distances correctly into lap length', () => {
    expect(wrapDistance(250, 1000)).toBe(250);
    expect(wrapDistance(1250, 1000)).toBe(250);
    expect(wrapDistance(-100, 1000)).toBe(900);
  });

  it('interpolates world position along circuit line at given distance', () => {
    // Exactly at point 1
    const p1 = positionAt(TEST_MAP, 250);
    expect(p1).not.toBeNull();
    expect(p1?.[0]).toBeCloseTo(250, 4);
    expect(p1?.[1]).toBeCloseTo(0, 4);

    // Halfway between point 0 and point 1 (dist 125)
    const mid = positionAt(TEST_MAP, 125);
    expect(mid).not.toBeNull();
    expect(mid?.[0]).toBeCloseTo(125, 4);
    expect(mid?.[1]).toBeCloseTo(0, 4);

    // On closing segment (dist 875, halfway between 750 and 1000)
    const closing = positionAt(TEST_MAP, 875);
    expect(closing).not.toBeNull();
    expect(closing?.[0]).toBeCloseTo(0, 4);
    expect(closing?.[1]).toBeCloseTo(125, 4);
  });

  it('builds valid SVG path string', () => {
    const bounds = computeBounds(TEST_MAP.points);
    if (!bounds) throw new Error('Bounds should be defined');
    const transform = fitMapTransform(bounds, TEST_MAP.points, 800, 600, 20);
    if (!transform) throw new Error('Transform should be defined');
    const path = buildSvgPath(TEST_MAP.points, transform);
    expect(path.startsWith('M ')).toBe(true);
    expect(path.endsWith(' Z')).toBe(true);
  });

  it('maps vehicle class to designated color token (C1)', () => {
    expect(classToColor('hypercar')).toBe('var(--class-hypercar)');
    expect(classToColor('lmp2')).toBe('var(--class-lmp2)');
    expect(classToColor('gt3')).toBe('var(--class-gt3)');
    expect(classToColor('gte')).toBe('var(--class-gte)');
    expect(classToColor('unknown')).toBe('var(--class-unknown)');
  });
});

describe('TrackMap Component', () => {
  it('renders LapDistanceBar fallback HUD when no map geometry is provided', () => {
    render(
      <TrackMap
        map={null}
        trackName="Circuit de Spa-Francorchamps"
        trackLengthM={7004}
        driverPosition={{ x: null, z: null, lapDistM: 3502 }}
        competitors={[
          {
            id: 'car-46',
            carNumber: '46',
            driverName: 'Driver 46',
            vehicleClass: 'gt3',
            lapDistM: 3000,
          },
        ]}
      />,
    );

    expect(screen.getByTestId('trackmap-title')).toHaveTextContent('Circuit de Spa-Francorchamps');
    expect(screen.getByTestId('trackmap-length')).toHaveTextContent('7.004 km');
    expect(screen.getByTestId('lap-distance-bar')).toBeInTheDocument();
    expect(screen.getByTestId('lap-dist-caption')).toHaveTextContent('2 cars (positions only)');
    expect(screen.getByTestId('competitor-chip-46')).toBeInTheDocument();
  });

  it('hides empty track name chip when unknown (EN15)', () => {
    render(
      <TrackMap
        map={null}
        trackName=""
        trackLengthM={null}
      />,
    );
    expect(screen.queryByTestId('trackmap-title')).not.toBeInTheDocument();
    expect(screen.getByTestId('lap-distance-bar')).toBeInTheDocument();
  });

  it('renders vector circuit, sectors, start/finish and pit markers when map is provided', () => {
    render(
      <TrackMap
        map={TEST_MAP}
        driverPosition={{ x: 100, z: 0, headingRad: 0 }}
        competitors={[
          {
            id: 'car-51',
            carNumber: '51',
            driverName: 'Driver 51',
            vehicleClass: 'hypercar',
            x: 250,
            z: 125,
          },
        ]}
      />,
    );

    expect(screen.getByTestId('trackmap-svg')).toBeInTheDocument();
    expect(screen.getByTestId('sector-1')).toBeInTheDocument();
    expect(screen.getByTestId('sector-2')).toBeInTheDocument();
    expect(screen.getByTestId('start-finish-line')).toBeInTheDocument();
    expect(screen.getByTestId('pit-entry')).toBeInTheDocument();
    expect(screen.getByTestId('pit-exit')).toBeInTheDocument();
    expect(screen.getByTestId('driver-marker')).toBeInTheDocument();
    expect(screen.getByTestId('competitor-51')).toBeInTheDocument();
  });

  it('renders stale tag and hollow competitor markers when dq is stale', () => {
    render(
      <TrackMap
        map={TEST_MAP}
        dq="stale"
        driverPosition={{ x: 100, z: 0, headingRad: 0 }}
        competitors={[
          {
            id: 'car-51',
            carNumber: '51',
            driverName: 'Driver 51',
            vehicleClass: 'hypercar',
            x: 250,
            z: 125,
          },
        ]}
      />,
    );

    expect(screen.getByTestId('trackmap-stale-tag')).toHaveTextContent('STALE');
    const group = screen.getByTestId('competitor-51');
    expect(group).toHaveClass('stale');
  });

  it('renders ended styling when dq is ended', () => {
    render(
      <TrackMap
        map={TEST_MAP}
        dq="ended"
        driverPosition={{ x: 100, z: 0, headingRad: 0 }}
      />,
    );

    expect(screen.getByTestId('trackmap-container')).toHaveClass('trackmap-ended');
  });

  it('triggers onRotationChange when rotation buttons are clicked', () => {
    const handleRotation = vi.fn();
    render(
      <TrackMap
        map={TEST_MAP}
        rotationRad={0}
        onRotationChange={handleRotation}
      />,
    );

    const ccwBtn = screen.getByTitle('Rotate counter-clockwise 15°');
    fireEvent.click(ccwBtn);
    expect(handleRotation).toHaveBeenCalledWith(Math.PI / 12);

    const resetBtn = screen.getByTitle('Reset rotation');
    fireEvent.click(resetBtn);
    expect(handleRotation).toHaveBeenCalledWith(0);
  });

  it('renders role="img" and descriptive aria summary for track map (A07)', () => {
    render(
      <TrackMap
        map={TEST_MAP}
        trackName="Sebring"
        driverPlace={7}
        driverSector={2}
        driverPosition={{ x: 100, z: 0, headingRad: 0, lapDistM: 400 }}
        competitors={[
          {
            id: 'car-51',
            carNumber: '51',
            driverName: 'Driver 51',
            vehicleClass: 'hypercar',
            x: 250,
            z: 125,
          },
        ]}
      />,
    );

    const svg = screen.getByTestId('trackmap-svg');
    expect(svg).toHaveAttribute('role', 'img');
    expect(svg).toHaveAttribute('aria-label', 'Sebring, 2 cars, driver P7 in sector 2');
  });

  it('renders legend chips showing only classes present in the session (A09)', () => {
    render(
      <TrackMap
        map={TEST_MAP}
        driverPosition={{ x: 100, z: 0 }}
        competitors={[
          {
            id: 'c1',
            carNumber: '51',
            driverName: 'Driver 51',
            vehicleClass: 'hypercar',
            x: 250,
            z: 125,
          },
          {
            id: 'c2',
            carNumber: '92',
            driverName: 'Driver 92',
            vehicleClass: 'gt3',
            x: 0,
            z: 200,
          },
        ]}
      />,
    );

    const legend = screen.getByTestId('trackmap-legend');
    expect(legend).toBeInTheDocument();
    expect(screen.getByTestId('trackmap-legend-hy')).toBeInTheDocument();
    expect(screen.getByTestId('trackmap-legend-gt3')).toBeInTheDocument();
    expect(screen.queryByTestId('trackmap-legend-p2')).not.toBeInTheDocument();
    expect(screen.queryByTestId('trackmap-legend-gte')).not.toBeInTheDocument();
  });

  it('disables rotation controls when no map geometry is loaded', () => {
    const handleRotation = vi.fn();
    render(
      <TrackMap
        map={null}
        rotationRad={0}
        onRotationChange={handleRotation}
      />,
    );

    const ccwBtn = screen.getByTestId('trackmap-btn-rotate-ccw');
    expect(ccwBtn).toBeDisabled();
    expect(ccwBtn).toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(ccwBtn);
    expect(handleRotation).not.toHaveBeenCalled();
  });

  it('renders competitor markers with legible car numbers at marker size >= 12px', () => {
    render(
      <TrackMap
        map={TEST_MAP}
        competitors={[
          {
            id: 'car-92',
            carNumber: '92',
            driverName: 'Driver 92',
            vehicleClass: 'gt3',
            x: 250,
            z: 125,
          },
        ]}
      />,
    );

    const carGroup = screen.getByTestId('competitor-92');
    const circle = carGroup.querySelector('circle');
    expect(circle).toBeInTheDocument();
    // Radius 10 means diameter 20px >= 12px
    const r = Number(circle?.getAttribute('r'));
    expect(r).toBeGreaterThanOrEqual(6);
    expect(carGroup).toHaveTextContent('92');
  });

  it('labels a marker with the car number only, even when given a full vehicle name (v1.3 U10/V04)', () => {
    render(
      <TrackMap
        map={TEST_MAP}
        driverPosition={{ x: 100, z: 0 }}
        driverCarNumber="#7 Toyota GR010"
        competitors={[
          {
            id: 23,
            carNumber: '#23 Mercedes-AMG LMGT3',
            driverName: 'A. Driver',
            vehicleClass: 'gt3',
            x: 250,
            z: 125,
          },
          {
            id: 24,
            carNumber: '51',
            driverName: 'B. Driver',
            vehicleClass: 'hypercar',
            x: 0,
            z: 250,
          },
        ]}
      />,
    );

    const car23 = screen.getByTestId('competitor-23');
    expect(car23.querySelector('text')?.textContent).toBe('23');
    // Full name only in the SVG tooltip
    expect(car23.querySelector('title')?.textContent).toBe('#23 Mercedes-AMG LMGT3 · A. Driver');
    const car51 = screen.getByTestId('competitor-51');
    expect(car51.querySelector('text')?.textContent).toBe('51');
    expect(car51.querySelector('title')?.textContent).toBe('#51 · B. Driver');

    const driver = screen.getByTestId('driver-marker');
    expect(driver.querySelector('text')?.textContent).toBe('7');
    expect(driver.querySelector('title')?.textContent).toBe('#7 Toyota GR010');
  });

  it('draws pit IN/OUT with neutral token classes, not warning amber or LMP2 blue (v1.3 V04)', () => {
    render(<TrackMap map={TEST_MAP} />);
    for (const id of ['pit-entry', 'pit-exit']) {
      const g = screen.getByTestId(id);
      const circle = g.querySelector('circle');
      expect(circle).toHaveClass('trackmap-pit-dot');
      expect(circle?.getAttribute('fill')).toBeNull();
      expect(g.querySelector('text')?.getAttribute('fill')).toBeNull();
      expect(g.innerHTML).not.toMatch(/#f5b83d|#4c9dff/i);
    }
  });

  it('keeps every car inside the viewport while generated geometry is partial (v1.3 V04)', () => {
    const partial: TrackMapData = { ...TEST_MAP, source: 'generated', pitEntry: null, pitExit: null };
    render(
      <TrackMap
        map={partial}
        driverPosition={{ x: -400, z: 125 }}
        competitors={[
          { id: 1, carNumber: '9', driverName: 'Far', vehicleClass: 'lmp2', x: 900, z: 700 },
        ]}
      />,
    );
    const positions = [
      screen.getByTestId('competitor-9').querySelector('circle'),
      screen.getByTestId('driver-marker').querySelector('circle'),
    ];
    for (const c of positions) {
      const cx = Number(c?.getAttribute('cx'));
      const cy = Number(c?.getAttribute('cy'));
      expect(cx).toBeGreaterThanOrEqual(0);
      expect(cx).toBeLessThanOrEqual(800);
      expect(cy).toBeGreaterThanOrEqual(0);
      expect(cy).toBeLessThanOrEqual(600);
    }
  });
});

describe('fitPointsFor (v1.3 V04)', () => {
  const geometry = [
    { x: 0, z: 0 },
    { x: 10, z: 10 },
  ];

  it('uses the geometry alone when it is complete (verified)', () => {
    expect(fitPointsFor(geometry, [{ x: 500, z: 500 }], true)).toBe(geometry);
  });

  it('adds every car with a finite position while geometry is partial', () => {
    const pts = fitPointsFor(
      geometry,
      [{ x: 500, z: -20 }, { x: null, z: 3 }, { x: Number.NaN, z: 1 }, {}],
      false,
    );
    expect(pts).toEqual([...geometry, { x: 500, z: -20 }]);
    expect(computeBounds(pts)).toEqual({ minX: 0, maxX: 500, minZ: -20, maxZ: 10 });
  });
});
