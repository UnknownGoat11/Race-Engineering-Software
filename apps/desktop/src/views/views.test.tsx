import { describe, expect, it } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ALL_VIEWS, getTyreTempColor, getViewDefinition, searchViews } from './viewRegistry';
import { TyreCarTopView } from './components/TyreCarTopView';
import { DrivingView } from './components/DrivingView';
import { DriverDnaView } from './components/DriverDnaView';
import { LapComparisonView } from './components/LapComparisonView';
import { MulticarRadarCircleView } from './components/MulticarRadarCircleView';
import { MinisectorTimingView } from './components/MinisectorTimingView';
import { FieldSpreadChartView } from './components/FieldSpreadChartView';
import { ViewsScreen } from '../screens/ViewsScreen';

describe('V1.1 Engineering View Suite', () => {
  it('contains all 27 views from the user catalogue across 4 categories', () => {
    expect(ALL_VIEWS.length).toBe(27);

    // Verify presence of all key views from user's image
    const titles = ALL_VIEWS.map((v) => v.title);
    expect(titles).toContain('Driver DNA');
    expect(titles).toContain('Driving');
    expect(titles).toContain('Lap Comparison');
    expect(titles).toContain('Laptime Data');
    expect(titles).toContain('Multihistogram');
    expect(titles).toContain('Multilap Telemetry');
    expect(titles).toContain('Multiplotter');
    expect(titles).toContain('Race Info');
    expect(titles).toContain('Statistics Table');
    expect(titles).toContain('Statistics Table Graphs');
    expect(titles).toContain('Statistics: RED,BLUE & SES');
    expect(titles).toContain('Stints Data');
    expect(titles).toContain('Telemetry');
    expect(titles).toContain('Time Trial Info');
    expect(titles).toContain('Workspace');
    expect(titles).toContain('~ Raw Data ~');
    expect(titles).toContain('Multi-car Map');
    expect(titles).toContain('Multi-car Map as circle');
    expect(titles).toContain('Multi-car One-lap Timing');
    expect(titles).toContain('Multi-car Race Timing');
    expect(titles).toContain('Multi-car Minisector Timing');
    expect(titles).toContain('Multi-car Opponent Stats');
    expect(titles).toContain('Multi-car Race Field Spread');
    expect(titles).toContain('Multi-car Race History');
    expect(titles).toContain('Multi-car Session Data');
    expect(titles).toContain('Multi-car Telemetry');
    expect(titles).toContain('~ Minimal UI ~');
  });

  describe('Tyre Temperature Color Scale (User Specification)', () => {
    it('maps < 65°C to dark blue (cold)', () => {
      const c = getTyreTempColor(58);
      expect(c.label).toBe('COLD');
      expect(c.bg).toBe('#1e3a8a');
    });

    it('maps 65 - 75°C to light blue (slight cold)', () => {
      const c = getTyreTempColor(70);
      expect(c.label).toBe('SLIGHT COLD');
      expect(c.bg).toBe('#0284c7');
    });

    it('maps 75 - 95°C to green (optimal)', () => {
      const c = getTyreTempColor(86);
      expect(c.label).toBe('OPTIMAL');
      expect(c.bg).toBe('#16a34a');
    });

    it('maps 95 - 105°C to light orange (getting warm)', () => {
      const c = getTyreTempColor(100);
      expect(c.label).toBe('GETTING WARM');
      expect(c.bg).toBe('#f97316');
    });

    it('maps 105 - 115°C to orange (warm)', () => {
      const c = getTyreTempColor(110);
      expect(c.label).toBe('WARM');
      expect(c.bg).toBe('#ea580c');
    });

    it('maps > 115°C to red (hot)', () => {
      const c = getTyreTempColor(122);
      expect(c.label).toBe('HOT');
      expect(c.bg).toBe('#dc2626');
    });
  });

  describe('searchViews', () => {
    it('returns all views when query is empty', () => {
      expect(searchViews('').length).toBe(27);
    });

    it('filters correctly by title keyword', () => {
      const results = searchViews('radar');
      expect(results.some((v) => v.id === 'multicar-circle')).toBe(true);
    });

    it('filters correctly by category keyword', () => {
      const results = searchViews('timing');
      expect(results.length).toBeGreaterThan(0);
    });
  });

  describe('getViewDefinition', () => {
    it('retrieves definition for a valid view ID', () => {
      const def = getViewDefinition('driving');
      expect(def.title).toBe('Driving');
      expect(def.category).toBe('performance');
    });

    it('throws error for an unknown view ID', () => {
      // @ts-expect-error testing invalid ID
      expect(() => getViewDefinition('non-existent')).toThrow();
    });
  });

  describe('TyreCarTopView Component', () => {
    it('renders all 4 wheels in top view with 3 parts and numerical data', () => {
      const mockWheels = {
        fl: {
          outerLeftC: 88,
          coreC: 85,
          outerRightC: 81,
          surfaceC: 86.8,
          carcassC: 84.1,
          pressureKpa: 178.4,
          wearFraction: 0.942,
          brakeDiscTempC: 440,
          brakePeakTempC: 560,
          slipPercentage: 2.1,
          lockupActive: false,
        },
        fr: {
          outerLeftC: 79,
          coreC: 83,
          outerRightC: 87,
          surfaceC: 84.2,
          carcassC: 82.0,
          pressureKpa: 177.1,
          wearFraction: 0.938,
          brakeDiscTempC: 432,
          brakePeakTempC: 554,
          slipPercentage: 1.8,
          lockupActive: false,
        },
        rl: {
          outerLeftC: 91,
          coreC: 89,
          outerRightC: 87,
          surfaceC: 90.4,
          carcassC: 88.2,
          pressureKpa: 176.0,
          wearFraction: 0.915,
          brakeDiscTempC: 385,
          brakePeakTempC: 490,
          slipPercentage: 3.4,
          lockupActive: false,
        },
        rr: {
          outerLeftC: 86,
          coreC: 88,
          outerRightC: 91,
          surfaceC: 89.1,
          carcassC: 87.0,
          pressureKpa: 175.4,
          wearFraction: 0.912,
          brakeDiscTempC: 378,
          brakePeakTempC: 482,
          slipPercentage: 2.9,
          lockupActive: false,
        },
      };

      render(
        <TyreCarTopView
          wheels={mockWheels}
          engineWaterTempC={89}
          engineOilTempC={98}
          clutchRpm={0}
        />,
      );

      expect(screen.getByTestId('tyre-car-topview')).toBeInTheDocument();
      expect(screen.getByTestId('wheel-fl')).toBeInTheDocument();
      expect(screen.getByTestId('wheel-fr')).toBeInTheDocument();
      expect(screen.getByTestId('wheel-rl')).toBeInTheDocument();
      expect(screen.getByTestId('wheel-rr')).toBeInTheDocument();
      expect(screen.getByTestId('engine-thermals-block')).toBeInTheDocument();

      // Check pressure and wear numbers
      expect(screen.getByText('178.4 kPa')).toBeInTheDocument();
      expect(screen.getByText('94.2%')).toBeInTheDocument();
    });
  });

  describe('DrivingView Component', () => {
    it('renders the complete cockpit HUD with vertical pedal gauges, gear, friction circle, and aids', () => {
      render(<DrivingView />);
      expect(screen.getByTestId('driving-view')).toBeInTheDocument();
      expect(screen.getByTestId('friction-circle')).toBeInTheDocument();
      expect(screen.getByText('ENGINE MAP: #1')).toBeInTheDocument();
      expect(screen.getByText('G-FORCE & FRICTION ENVELOPE')).toBeInTheDocument();
      expect(screen.getByText('DRIVER AIDS & SYSTEMS')).toBeInTheDocument();
    });
  });

  describe('DriverDnaView Component', () => {
    it('renders 6-axis spider radar chart and rating', () => {
      render(<DriverDnaView />);
      expect(screen.getByTestId('driver-dna-view')).toBeInTheDocument();
      expect(screen.getByText('Trail-Braking Slope')).toBeInTheDocument();
      expect(screen.getByText('Steering Smoothness')).toBeInTheDocument();
      expect(screen.getByText('OVERALL DRIVER INDEX')).toBeInTheDocument();
    });
  });

  describe('LapComparisonView Component', () => {
    it('renders reference lap, comparison lap, and delta-t breakdown', () => {
      render(<LapComparisonView />);
      expect(screen.getByTestId('lap-comparison-view')).toBeInTheDocument();
      expect(screen.getByText('REFERENCE LAP')).toBeInTheDocument();
      expect(screen.getByText('COMPARISON LAP')).toBeInTheDocument();
      expect(screen.getByText('TOTAL TIME DELTA (Δt)')).toBeInTheDocument();
    });
  });

  describe('MulticarRadarCircleView Component', () => {
    it('renders polar circular radar map and car markers', () => {
      render(<MulticarRadarCircleView />);
      expect(screen.getByTestId('multicar-circle-view')).toBeInTheDocument();
      expect(screen.getByText('S/F LINE')).toBeInTheDocument();
      expect(screen.getByText('PIT LANE MERGE')).toBeInTheDocument();
    });
  });

  describe('MinisectorTimingView Component', () => {
    it('renders 24 micro-sector splits', () => {
      render(<MinisectorTimingView />);
      expect(screen.getByTestId('minisector-timing-view')).toBeInTheDocument();
      expect(screen.getByText('PURPLE: Overall Fastest')).toBeInTheDocument();
    });
  });

  describe('FieldSpreadChartView Component', () => {
    it('renders gap-to-leader chart', () => {
      render(<FieldSpreadChartView />);
      expect(screen.getByTestId('field-spread-view')).toBeInTheDocument();
      expect(screen.getByText(/Multi-Car Race Field Spread/)).toBeInTheDocument();
    });
  });

  describe('ViewsScreen Component & Navigation', () => {
    it('renders initial driving view and allows switching views via dropdown', () => {
      render(<ViewsScreen initialViewId="driving" />);
      expect(screen.getByTestId('views-screen')).toBeInTheDocument();
      expect(screen.getByTestId('driving-view')).toBeInTheDocument();

      // Open dropdown
      const button = screen.getByTestId('view-switcher-button');
      fireEvent.click(button);

      // Select Driver DNA
      const dnaItem = screen.getByTestId('view-item-driver-dna');
      fireEvent.click(dnaItem);

      // Verify Driver DNA view is now active
      expect(screen.getByTestId('driver-dna-view')).toBeInTheDocument();
    });
  });
});
