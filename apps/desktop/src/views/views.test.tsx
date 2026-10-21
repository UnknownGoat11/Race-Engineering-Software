import { act, fireEvent, render, screen, within } from '@testing-library/react';
import type { JSX, ReactNode } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { ch } from '../generated/channels';
import type { CompetitorEntryDto, ScoringStateDto } from '../hooks/useScoring';
import { NO_VALUE } from '../lib/units';
import { TelemetryStoreContext } from '../telemetry/hooks';
import { TelemetryStore } from '../telemetry/store';
import { FakeClock, FakeFrames } from '../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../test/tlvEncoder';
import { ViewsScreen } from '../screens/ViewsScreen';
import { DriverDnaView } from './components/DriverDnaView';
import { DrivingView } from './components/DrivingView';
import { FieldSpreadChartView } from './components/FieldSpreadChartView';
import { HypercarHybridView } from './components/HypercarHybridView';
import { HypercarVetQuickWidget } from './components/HypercarVetQuickWidget';
import { MinimalCockpitHudView } from './components/MinimalCockpitHudView';
import { MinisectorTimingView } from './components/MinisectorTimingView';
import { MulticarRadarCircleView } from './components/MulticarRadarCircleView';
import { MultihistogramView } from './components/MultihistogramView';
import { MultiplotterView } from './components/MultiplotterView';
import {
  LaptimeDataView,
  MulticarMapView,
  MulticarOneLapTimingView,
  MulticarOpponentStatsView,
  MulticarRaceHistoryView,
  MulticarSessionDataView,
  MulticarTelemetryView,
  MultilapTelemetryView,
  RaceInfoView,
  TimeTrialInfoView,
} from './components/OtherViews';
import { RawDataInspectorView } from './components/RawDataInspectorView';
import { StatisticsRedBlueView, StatisticsTableGraphsView, StatisticsTableView } from './components/StatisticsViews';
import { StintsDataView } from './components/StintsDataView';
import { TractionCircleView } from './components/TractionCircleView';
import { TyreCarTopView } from './components/TyreCarTopView';
import { TyreDegQuickSummary } from './components/TyreDegQuickSummary';
import { TyreDegradationView } from './components/TyreDegradationView';
import type { WheelReadings } from './types';
import { ALL_VIEWS, getTyreTempColor, getViewDefinition, searchViews } from './viewRegistry';

vi.mock('../telemetry/bridge', () => ({
  connectTauriTelemetry: vi.fn(() => ({ disconnect: vi.fn() })),
}));

function createHarness(): {
  store: TelemetryStore;
  send: (records: readonly FixtureRecord[]) => void;
  renderWithStore: (element: JSX.Element) => ReturnType<typeof render>;
} {
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
  let seq = 0;
  const send = (records: readonly FixtureRecord[]): void => {
    store.applyBatch(encodeUiBatch(0, seq, [encodeTlvBlock({ sourceEtS: seq * 0.01 + 1 }, records)]));
    seq += 1;
    act(() => {
      frames.runFrame();
    });
  };
  const renderWithStore = (element: JSX.Element): ReturnType<typeof render> => {
    const wrapper = ({ children }: { children: ReactNode }): JSX.Element => (
      <TelemetryStoreContext value={store}>{children}</TelemetryStoreContext>
    );
    return render(element, { wrapper });
  };
  return { store, send, renderWithStore };
}

/** Values the views used to show as hardcoded demo data; none may appear without real input. */
const FORMER_DEMO_STRINGS = [
  '218',
  '1:34.563',
  '-0.248',
  '42.5',
  '94.2%',
  '0.942',
  '615.6',
  '26.50',
  '7004',
  'Spa',
  'La Source',
  'Kobayashi',
  'GR010',
  'Ferrari',
  'Porsche',
  '1.45',
  '0.62',
  'DOUBLE-STINT: VIABLE',
  'Lap 14',
  '0 (0.00%)',
  '2,480',
  'AERO BALANCE',
  'DIFF LOCK',
  'Telemetry Tool',
];

function expectNoDemoData(container: HTMLElement): void {
  const text = container.textContent;
  for (const demo of FORMER_DEMO_STRINGS) {
    expect(text, `found former demo value "${demo}"`).not.toContain(demo);
  }
}

const NO_DATA_WHEEL: WheelReadings = {
  leftC: null,
  middleC: null,
  rightC: null,
  surfaceAvgC: null,
  carcassC: null,
  pressurePa: null,
  wearRaw: null,
  brakeC: null,
  brakePeakC: null,
};

function vehicle(overrides: Partial<CompetitorEntryDto>): CompetitorEntryDto {
  return {
    slot_id: 0,
    driver: null,
    vehicle_name: null,
    vehicle_filename: null,
    class_name: null,
    pit_group: null,
    place: null,
    class_place: null,
    laps: null,
    lap_dist_m: null,
    sector: null,
    best_lap_s: null,
    last_lap_s: null,
    best_sector1_s: null,
    best_sector12_s: null,
    last_sector1_s: null,
    last_sector12_s: null,
    time_behind_next_s: null,
    laps_behind_next: null,
    time_behind_leader_s: null,
    laps_behind_leader: null,
    in_pits: null,
    in_garage_stall: null,
    pit_state: null,
    num_pitstops: null,
    is_player: false,
    pos_x_m: null,
    pos_z_m: null,
    ...overrides,
  };
}

const SCORING: ScoringStateDto = {
  session: {
    track_name: 'Test Circuit',
    session_type: 'race',
    session_time_remaining_s: 600,
    end_time_s: null,
    lap_limit: null,
    time_limit_s: null,
    yellow_flag_state: null,
  },
  vehicles: [
    vehicle({ slot_id: 1, driver: 'Driver A', vehicle_name: 'Car A', class_name: 'Class X', place: 1, lap_dist_m: 1000, is_player: true, last_lap_s: 100.5, laps: 3 }),
    vehicle({ slot_id: 2, driver: 'Driver B', vehicle_name: 'Car B', class_name: 'Class X', place: 2, lap_dist_m: 2500, time_behind_leader_s: 1.25, laps: 3, in_pits: false }),
  ],
};

describe('Engineering view catalogue', () => {
  it('contains all 30 views in the catalogue', () => {
    expect(ALL_VIEWS.length).toBe(30);
    expect(searchViews('').length).toBe(30);
    expect(searchViews('radar').some((v) => v.id === 'multicar-circle')).toBe(true);
    expect(getViewDefinition('driving').category).toBe('performance');
    // @ts-expect-error testing invalid ID
    expect(() => getViewDefinition('non-existent')).toThrow();
  });

  it('maps tyre temperatures to the specified colour bands', () => {
    expect(getTyreTempColor(58).label).toBe('COLD');
    expect(getTyreTempColor(70).label).toBe('SLIGHT COLD');
    expect(getTyreTempColor(86).label).toBe('OPTIMAL');
    expect(getTyreTempColor(100).label).toBe('GETTING WARM');
    expect(getTyreTempColor(110).label).toBe('WARM');
    expect(getTyreTempColor(122).label).toBe('HOT');
  });
});

describe('Views without telemetry render no data, never demo values', () => {
  const views: readonly [string, () => JSX.Element][] = [
    ['DrivingView', () => <DrivingView />],
    ['TractionCircleView', () => <TractionCircleView />],
    ['HypercarHybridView', () => <HypercarHybridView />],
    ['HypercarVetQuickWidget', () => <HypercarVetQuickWidget />],
    ['TyreDegQuickSummary', () => <TyreDegQuickSummary />],
    ['TyreDegradationView', () => <TyreDegradationView />],
    ['StintsDataView', () => <StintsDataView />],
    ['MinisectorTimingView', () => <MinisectorTimingView />],
    ['FieldSpreadChartView', () => <FieldSpreadChartView />],
    ['MulticarRadarCircleView', () => <MulticarRadarCircleView scoring={null} />],
    ['DriverDnaView', () => <DriverDnaView />],
    ['MultiplotterView', () => <MultiplotterView />],
    ['MultihistogramView', () => <MultihistogramView />],
    ['StatisticsTableView', () => <StatisticsTableView />],
    ['StatisticsTableGraphsView', () => <StatisticsTableGraphsView />],
    ['StatisticsRedBlueView', () => <StatisticsRedBlueView />],
    ['RawDataInspectorView', () => <RawDataInspectorView />],
    ['MinimalCockpitHudView', () => <MinimalCockpitHudView />],
    ['LaptimeDataView', () => <LaptimeDataView />],
    ['RaceInfoView', () => <RaceInfoView scoring={null} />],
    ['TimeTrialInfoView', () => <TimeTrialInfoView />],
    ['MulticarOneLapTimingView', () => <MulticarOneLapTimingView scoring={null} />],
    ['MulticarOpponentStatsView', () => <MulticarOpponentStatsView />],
    ['MulticarRaceHistoryView', () => <MulticarRaceHistoryView />],
    ['MulticarSessionDataView', () => <MulticarSessionDataView scoring={null} />],
    ['MulticarTelemetryView', () => <MulticarTelemetryView />],
    ['MultilapTelemetryView', () => <MultilapTelemetryView />],
  ];

  it.each(views)('%s shows a no-data state and none of the former demo values', (_name, make) => {
    const { container } = render(make());
    expect(screen.getAllByText(/NO DATA|—/).length).toBeGreaterThan(0);
    expectNoDemoData(container);
  });

  it('DrivingView: every readout is NO_VALUE and the dead engine-map / brake-bias / limiter controls are gone', () => {
    render(<DrivingView />);
    expect(screen.getByTestId('driving-not-connected')).toBeInTheDocument();
    expect(screen.getByTestId('driving-speed')).toHaveTextContent(NO_VALUE);
    expect(screen.getByTestId('driving-gear')).toHaveTextContent(NO_VALUE);
    expect(screen.getByTestId('driving-brake-bias')).toHaveTextContent(NO_VALUE);
    expect(screen.getByTestId('driving-motor-map')).toHaveTextContent(`MOTOR MAP: ${NO_VALUE}`);
    expect(screen.queryAllByRole('button')).toHaveLength(0);
    expect(screen.queryByText(/SWITCH QUAL|REAR \(-0.5%\)|FRONT \(\+0.5%\)/)).toBeNull();
    expect(screen.getByTestId('engine-oil-temp')).toHaveTextContent(NO_VALUE);
  });

  it('TractionCircleView: no live dot and no scatter without samples', () => {
    render(<TractionCircleView />);
    expect(screen.getByTestId('traction-combined')).toHaveTextContent(NO_VALUE);
    expect(screen.queryByTestId('traction-live-dot')).toBeNull();
    expect(screen.getByTestId('traction-trail').childElementCount).toBe(0);
    expect(within(screen.getByTestId('traction-peaks')).getAllByText(NO_VALUE)).toHaveLength(4);
  });

  it('Tyre degradation: no wear, rate, cliff, double-stint or lock-up numbers', () => {
    render(<TyreDegradationView />);
    expect(screen.getByTestId('deg-wear-fl')).toHaveTextContent(`Wear (raw): ${NO_VALUE}`);
    expect(screen.getByTestId('deg-curves-no-data')).toBeInTheDocument();
    expect(screen.getByTestId('deg-double-stint-no-data')).toBeInTheDocument();
    expect(screen.getByTestId('deg-lockup-no-data')).toBeInTheDocument();
    expect(screen.queryByText(/% \/ Lap/)).toBeNull();
  });

  it('RawDataInspectorView: no fake stream counters, real channel ids', () => {
    render(<RawDataInspectorView />);
    expect(screen.getByTestId('raw-not-connected')).toBeInTheDocument();
    expect(screen.queryByText('LIVE')).toBeNull();
    // Registry ids, not invented ones (THROTTLE is 0x0010, G_LAT 0x0521).
    expect(within(screen.getByTestId('raw-row-0x0010')).getByText('THROTTLE')).toBeInTheDocument();
    expect(within(screen.getByTestId('raw-row-0x0521')).getByText('G_LAT')).toBeInTheDocument();
  });

  it('TyreCarTopView renders NO_VALUE for every missing reading', () => {
    render(
      <TyreCarTopView
        wheels={{ fl: NO_DATA_WHEEL, fr: NO_DATA_WHEEL, rl: NO_DATA_WHEEL, rr: NO_DATA_WHEEL }}
        engineWaterTempC={null}
        engineOilTempC={null}
        clutchRpm={null}
      />,
    );
    expect(screen.getByTestId('engine-water-temp')).toHaveTextContent(NO_VALUE);
    expect(within(screen.getByTestId('wheel-fl')).getByText(`Wear (raw): ${NO_VALUE}`)).toBeInTheDocument();
    expect(screen.queryByText(/AERO BALANCE|DIFF LOCK/)).toBeNull();
  });
});

describe('Views with live telemetry show the real values', () => {
  it('TyreCarTopView shows real per-wheel readings', () => {
    render(
      <TyreCarTopView
        wheels={{
          fl: { ...NO_DATA_WHEEL, middleC: 85, pressurePa: 178_400, wearRaw: 0.9 },
          fr: NO_DATA_WHEEL,
          rl: NO_DATA_WHEEL,
          rr: NO_DATA_WHEEL,
        }}
        engineWaterTempC={89}
        engineOilTempC={98}
        clutchRpm={0}
      />,
    );
    const fl = screen.getByTestId('wheel-fl');
    expect(within(fl).getByText('1.78 bar')).toBeInTheDocument();
    expect(within(fl).getByText('Wear (raw): 0.90')).toBeInTheDocument();
    expect(screen.getByTestId('engine-water-temp')).toHaveTextContent('89 °C');
  });

  it('DrivingView converts speed from m/s and labels lateral G with positive = left', () => {
    const { send, renderWithStore } = createHarness();
    renderWithStore(<DrivingView />);
    send([rec.f32(ch.SPEED, 60), rec.f32(ch.G_LAT, 1.2), rec.f32(ch.G_LONG, -0.5), rec.f32(ch.THROTTLE, 0.25)]);
    expect(screen.queryByTestId('driving-not-connected')).toBeNull();
    expect(screen.getByTestId('driving-speed')).toHaveTextContent('216 km/h');
    expect(screen.getByTestId('driving-throttle')).toHaveTextContent('25%');
    expect(screen.getByTestId('driving-g-values')).toHaveTextContent('Lat: 1.20G L | Long: -0.50G');
  });

  it('TractionCircleView plots positive G_LAT as a left-hand load', () => {
    const { send, renderWithStore } = createHarness();
    renderWithStore(<TractionCircleView />);
    send([rec.f32(ch.G_LAT, 1.5), rec.f32(ch.G_LONG, 0.2)]);
    expect(screen.getByTestId('traction-lateral')).toHaveTextContent('1.50 G (Left)');
    // Left of centre (150) in the SVG.
    const circle = screen.getByTestId('traction-live-dot').querySelector('circle');
    expect(Number(circle?.getAttribute('cx'))).toBeLessThan(150);
    send([rec.f32(ch.G_LAT, -0.8), rec.f32(ch.G_LONG, -1.1)]);
    expect(screen.getByTestId('traction-lateral')).toHaveTextContent('0.80 G (Right)');
    expect(screen.getByTestId('traction-longitudinal')).toHaveTextContent('-1.10 G (Brake)');
    expect(screen.getByTestId('traction-trail').childElementCount).toBeGreaterThan(0);
  });

  it('Hypercar views show virtual energy as a fraction, no MJ figures', () => {
    const { send, renderWithStore } = createHarness();
    renderWithStore(<HypercarHybridView />);
    send([rec.f32(ch.VIRTUAL_ENERGY, 0.5), rec.f32(ch.VE_LAPS_REMAINING, 4), rec.f32(ch.FUEL_LAPS_REMAINING, 6)]);
    expect(screen.getByTestId('hybrid-ve-remaining')).toHaveTextContent('50.0%');
    expect(screen.getByTestId('vet-remaining')).toHaveTextContent('50.0%');
    expect(screen.getByTestId('hybrid-stint-limiter')).toHaveTextContent('VIRTUAL ENERGY');
    expect(screen.queryByText(/MJ/)).toBeNull();
  });

  it('RawDataInspectorView shows real values, states and store counters', () => {
    const { send, renderWithStore } = createHarness();
    renderWithStore(<RawDataInspectorView />);
    send([rec.f32(ch.SPEED, 50)]);
    const row = screen.getByTestId('raw-row-0x0001');
    expect(within(row).getByText('SPEED')).toBeInTheDocument();
    expect(within(row).getByText('180 km/h')).toBeInTheDocument();
    expect(within(row).getByText('LIVE')).toBeInTheDocument();
    expect(screen.getByTestId('raw-batches-applied')).toHaveTextContent('1');
  });

  it('Multihistogram and Multiplotter use real live-ring samples', () => {
    const { send, renderWithStore } = createHarness();
    renderWithStore(
      <>
        <MultihistogramView />
        <MultiplotterView />
      </>,
    );
    for (let i = 0; i < 12; i++) {
      send([rec.f32(ch.SPEED, 20 + i), rec.f32(ch.STEERING_ANGLE, 0.01 * i), rec.f32(ch.G_LAT, 0.1 * i)]);
    }
    expect(within(screen.getByTestId('hist-speed')).getByText('12 samples')).toBeInTheDocument();
    expect(screen.getByTestId('plotter-r')).toHaveTextContent('1.000');
  });

  it('Multi-car views use live scoring', () => {
    const { send, renderWithStore } = createHarness();
    renderWithStore(
      <>
        <MulticarRadarCircleView scoring={SCORING} />
        <MulticarOneLapTimingView scoring={SCORING} />
        <MulticarSessionDataView scoring={SCORING} />
        <RaceInfoView scoring={SCORING} />
      </>,
    );
    send([rec.f64(ch.TRACK_LENGTH, 5000)]);
    expect(screen.getByTestId('circle-car-1')).toBeInTheDocument();
    expect(screen.getByTestId('circle-car-2')).toBeInTheDocument();
    expect(within(screen.getByTestId('one-lap-table')).getByText('1:40.500')).toBeInTheDocument();
    expect(within(screen.getByTestId('session-data-table')).getByText('Driver B')).toBeInTheDocument();
    expect(within(screen.getByTestId('race-info-live')).getByText('Test Circuit')).toBeInTheDocument();
  });

  it('MulticarMapView passes no invented track name or length', () => {
    render(<MulticarMapView scoring={null} />);
    expect(screen.getByTestId('multicar-map-view')).toBeInTheDocument();
    expectNoDemoData(screen.getByTestId('multicar-map-view'));
  });
});

describe('ViewsScreen', () => {
  it('provides live telemetry to the views and switches views via the dropdown', () => {
    render(<ViewsScreen initialViewId="driving" />);
    expect(screen.getByTestId('driving-view')).toBeInTheDocument();
    // Connected to a store, but nothing received yet: values are NO_VALUE, never demo values.
    expect(screen.queryByTestId('driving-not-connected')).toBeNull();
    expect(screen.getByTestId('driving-speed')).toHaveTextContent(NO_VALUE);
    expectNoDemoData(screen.getByTestId('views-screen'));

    fireEvent.click(screen.getByTestId('view-switcher-button'));
    fireEvent.click(screen.getByTestId('view-item-driver-dna'));
    expect(screen.getByTestId('driver-dna-view')).toBeInTheDocument();
    expect(screen.getByTestId('driver-dna-no-data')).toBeInTheDocument();
  });

  it('every catalogue view renders inside ViewsScreen without demo values', () => {
    for (const def of ALL_VIEWS) {
      if (def.id === 'lap-comparison') continue; // owned and tested separately
      const { unmount } = render(<ViewsScreen initialViewId={def.id} />);
      expectNoDemoData(screen.getByTestId(`view-container-${def.id}`));
      unmount();
    }
  });
});
