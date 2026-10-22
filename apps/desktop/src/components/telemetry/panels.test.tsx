import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, render, screen } from '@testing-library/react';
import type { JSX, ReactNode } from 'react';
import { beforeEach, describe, expect, it } from 'vitest';
import { CHANNEL_BY_ID, Quality, ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { FakeClock, FakeFrames } from '../../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../../test/tlvEncoder';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { TelemetryStore, type ChannelSample } from '../../telemetry/store';
import { BrakePanel } from './BrakePanel';
import { DriverInputsPanel } from './DriverInputsPanel';
import { EnergyPanel } from './EnergyPanel';
import { FuelPanel } from './FuelPanel';
import { TimingPanel } from './TimingPanel';
import { TyrePanel } from './TyrePanel';
import {
  ScopeDqContext,
  presentSample,
  resetLastKnownFormattedValues,
  type ScopeDq,
} from './presentation';

function createPanelHarness(): {
  store: TelemetryStore;
  clock: FakeClock;
  frames: FakeFrames;
  send: (records: readonly FixtureRecord[]) => void;
  renderWithStore: (element: JSX.Element, scopeDq?: ScopeDq) => ReturnType<typeof render>;
} {
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({
    now: clock.now,
    scheduleFrame: frames.schedule,
    scheduleTimeout: clock.scheduleTimeout,
  });
  let seq = 0;
  const send = (records: readonly FixtureRecord[]): void => {
    store.applyBatch(encodeUiBatch(0, seq, [encodeTlvBlock({ sourceEtS: seq }, records)]));
    seq += 1;
  };
  const renderWithStore = (element: JSX.Element, scopeDq: ScopeDq = null): ReturnType<typeof render> => {
    const wrapper = ({ children }: { children: ReactNode }): JSX.Element => (
      <TelemetryStoreContext value={store}>
        <ScopeDqContext value={scopeDq}>{children}</ScopeDqContext>
      </TelemetryStoreContext>
    );
    return render(element, { wrapper });
  };
  return { store, clock, frames, send, renderWithStore };
}

describe('Telemetry Panels (REDESIGN W1.5)', () => {
  beforeEach(() => {
    resetLastKnownFormattedValues();
  });

  describe('TimingPanel', () => {
    it('renders sector 0-based index correctly as S1 and S2 (T13)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<TimingPanel />);

      // Sector 0 -> S1
      send([rec.u8(ch.SECTOR, 0), rec.i32(ch.LAP_NUMBER, 5)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByText(/L5 \/ S1/)).toBeInTheDocument();

      // Sector 1 -> S2
      send([rec.u8(ch.SECTOR, 1), rec.i32(ch.LAP_NUMBER, 5)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByText(/L5 \/ S2/)).toBeInTheDocument();
    });

    it('applies delta deadband correctly (ahead, behind, neutral)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<TimingPanel />);

      // Ahead: -0.150 s -> delta-ahead
      send([rec.f64(ch.DELTA_PERSONAL_BEST, -0.150)]);
      act(() => {
        frames.runFrame();
      });
      const aheadEl = screen.getByTestId('timing-delta-pb');
      expect(aheadEl).toHaveClass('delta-ahead');
      expect(aheadEl).toHaveTextContent('-0.150');

      // Behind: +0.250 s -> delta-behind
      send([rec.f64(ch.DELTA_PERSONAL_BEST, 0.250)]);
      act(() => {
        frames.runFrame();
      });
      const behindEl = screen.getByTestId('timing-delta-pb');
      expect(behindEl).toHaveClass('delta-behind');
      expect(behindEl).toHaveTextContent('+0.250');

      // Neutral within ±0.050 s deadband: -0.020 s -> delta-neutral
      send([rec.f64(ch.DELTA_PERSONAL_BEST, -0.020)]);
      act(() => {
        frames.runFrame();
      });
      const neutralEl = screen.getByTestId('timing-delta-pb');
      expect(neutralEl).toHaveClass('delta-neutral');
    });

    it('renders unavailable timing as em dash, never 0 (T03)', () => {
      const { renderWithStore } = createPanelHarness();
      renderWithStore(<TimingPanel />);
      expect(screen.getByTestId('timing-delta-pb')).toHaveTextContent(NO_VALUE);
      expect(screen.getByText(new RegExp(`L${NO_VALUE} \\/ S${NO_VALUE}`))).toBeInTheDocument();
    });

    it('shows STALE tag in header when scope DQ is stale (T07, C5)', () => {
      const { renderWithStore } = createPanelHarness();
      renderWithStore(<TimingPanel />, 'stale');
      expect(screen.getByText('STALE')).toBeInTheDocument();
    });
  });

  describe('DriverInputsPanel', () => {
    it('converts STEERING_ANGLE from radians to degrees: 0.5 rad -> 29° (T14)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<DriverInputsPanel />);

      send([rec.f32(ch.STEERING_ANGLE, 0.5)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByText('29°')).toBeInTheDocument();
    });

    it('hides RPM bar when RPM_MAX is missing or null, renders bar when max is present (T15)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<DriverInputsPanel />);

      // Only RPM given, no RPM_MAX -> no rpm bar
      send([rec.f32(ch.RPM, 6500)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByText('6,500')).toBeInTheDocument();
      expect(screen.queryByTestId('rpm-bar')).toBeNull();

      // Now supply RPM_MAX -> rpm bar is rendered
      send([rec.f32(ch.RPM_MAX, 8500)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByTestId('rpm-bar')).toBeInTheDocument();
    });

    it('formats gear correctly for Reverse, Neutral, and Drive gears', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<DriverInputsPanel />);

      send([rec.i32(ch.GEAR, -1)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByTestId('inputs-gear')).toHaveTextContent('R');

      send([rec.i32(ch.GEAR, 0)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByTestId('inputs-gear')).toHaveTextContent('N');

      send([rec.i32(ch.GEAR, 4)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByTestId('inputs-gear')).toHaveTextContent('4');
    });
  });

  describe('TyrePanel', () => {
    it('renders tyre balance in °C, not % (T16)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<TyrePanel />);

      send([
        rec.f32(ch.TYRE_TEMP_BALANCE_FRONT_REAR, 2.5),
        rec.f32(ch.TYRE_TEMP_BALANCE_LEFT_RIGHT, -1.2),
      ]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByText('F/R +2.5°C')).toBeInTheDocument();
      expect(screen.getByText('L/R -1.2°C')).toBeInTheDocument();
      expect(screen.queryByText(/%/)).toBeNull();
    });

    it('renders tyre wear as raw value: Wear (raw): 0.90 (T17, C17)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<TyrePanel />);

      send([rec.f32(ch.TYRE_WEAR_FL, 0.90)]);
      act(() => {
        frames.runFrame();
      });
      // v1.3 V06: footer is a label/value list ("Wear (raw)" / "0.90"), not run-together text
      expect(screen.getByTestId('tyre-fl-wear')).toHaveTextContent('0.90');
      expect(screen.getAllByText('Wear (raw)')).toHaveLength(4);
      expect(screen.getByTestId('tyre-fr-wear')).toHaveTextContent(NO_VALUE);
      expect(screen.getByTestId('tyre-fl-carcass')).toHaveTextContent(NO_VALUE);
    });

    it('shows the corner code only, with the full name in title and aria-label (v1.3 V06/U17)', () => {
      const { renderWithStore } = createPanelHarness();
      renderWithStore(<TyrePanel />);

      const fl = screen.getByTestId('tyre-fl');
      expect(fl).toHaveAttribute('aria-label', 'Front left tyre');
      const title = fl.querySelector('.tyre-corner-title');
      expect(title?.textContent).toBe('FL');
      expect(title).toHaveAttribute('title', 'Front left');
      expect(screen.queryByText(/FRONT LEFT/)).toBeNull();
    });

    it('renders all 4 tyre corners in a 2x2 grid (B01)', () => {
      const { renderWithStore } = createPanelHarness();
      renderWithStore(<TyrePanel />);

      expect(screen.getByTestId('tyre-fl')).toBeInTheDocument();
      expect(screen.getByTestId('tyre-fr')).toBeInTheDocument();
      expect(screen.getByTestId('tyre-rl')).toBeInTheDocument();
      expect(screen.getByTestId('tyre-rr')).toBeInTheDocument();
    });
  });

  describe('BrakePanel', () => {
    it('uses neutral interim styling with No threshold set badge (T18)', () => {
      const { renderWithStore } = createPanelHarness();
      renderWithStore(<BrakePanel />);

      expect(screen.getByText('No threshold set')).toBeInTheDocument();
      expect(screen.queryByText(/DISC TEMPERATURE CRITICAL/)).toBeNull();
    });

    it('renders brake peak and current temperatures without fabricated max 0 (T03, B02)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<BrakePanel />);

      send([
        rec.f32(ch.BRAKE_TEMP_FL, 450),
        rec.f32(ch.BRAKE_TEMP_PEAK_FL, 580),
      ]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByText(/450\s*°C/)).toBeInTheDocument();
      // v1.3 V06: one line per corner, "FL 450 °C · peak 580 · wear —"
      const fl = screen.getByTestId('brake-fl');
      expect(fl.textContent).toBe(`FL450 °C· peak 580wear ${NO_VALUE}`);
      expect(fl).toHaveAttribute('aria-label', 'Front left brake');
      expect(screen.getByTestId('brake-fr').textContent).toContain(`peak ${NO_VALUE}`);
    });
  });

  describe('EnergyPanel', () => {
    it('splits non-hybrid and VE accurately: Hybrid: none and VE: not reported (T21)', () => {
      const { renderWithStore } = createPanelHarness();
      renderWithStore(<EnergyPanel />);

      expect(screen.getByText('VE: not reported by this car')).toBeInTheDocument();
      expect(screen.getAllByText('Hybrid: none').length).toBeGreaterThanOrEqual(1);
    });

    it('renders VE when present and omits Regen Power completely (T20, T21)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<EnergyPanel />);

      send([
        rec.f32(ch.VIRTUAL_ENERGY, 0.755),
        rec.f32(ch.BATTERY_SOC, 0.60),
        rec.f32(ch.HYBRID_MOTOR_TORQUE, 120.5),
      ]);
      act(() => {
        frames.runFrame();
      });

      expect(screen.getByText('75.5%')).toBeInTheDocument();
      expect(screen.getByText('60.0%')).toBeInTheDocument();
      expect(screen.getByText('120.5 Nm')).toBeInTheDocument();
      expect(screen.queryByText('Regen Power')).toBeNull();
    });
  });

  describe('FuelPanel', () => {
    it('hides the visual fuel bar when fuel is unknown / null (T03)', () => {
      const { renderWithStore } = createPanelHarness();
      renderWithStore(<FuelPanel />);

      expect(screen.queryByTestId('fuel-bar')).toBeNull();
    });

    it('shows the visual fuel bar when fuel percentage is known', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<FuelPanel />);

      send([rec.f32(ch.FUEL, 50), rec.f32(ch.FUEL_CAPACITY, 100)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByTestId('fuel-bar')).toBeInTheDocument();
      expect(screen.getByText('(50%)')).toBeInTheDocument();
    });

    it('keeps the reserve caveat on Fuel to add as a title, value on one line (T26, v1.3 V07/C06)', () => {
      const { renderWithStore } = createPanelHarness();
      renderWithStore(<FuelPanel />);

      const row = screen.getByTestId('fuel-to-add-row');
      expect(row).toHaveAttribute('title', 'Reserve not reported');
      expect(row.querySelector('dt')?.textContent).toBe('Fuel to add');
      expect(row.querySelector('dd')?.textContent).toBe(NO_VALUE);
      expect(screen.queryByText('(reserve not reported)')).toBeNull();
    });
  });

  describe('Scope Data Quality & presentSample', () => {
    const baseSample: ChannelSample = {
      id: ch.SPEED,
      def: CHANNEL_BY_ID.get(ch.SPEED) ?? null,
      state: 'live',
      quality: Quality.Valid,
      value: 83.333, // ~300 km/h
      receivedAtMs: 1000,
      captureMonoNs: 0n,
      captureRelayNsEst: null,
      sourceEtS: 1,
    };

    it('stale keeps last value and formats state tag (C5)', () => {
      // 1. Initial live sample formats and caches the value
      const live = presentSample(baseSample, 'Speed');
      expect(live.state).toBe('live');
      expect(live.valueText).toBe('300');

      // 2. Now sample goes stale with null value -> value is kept!
      const staleSample: ChannelSample = {
        ...baseSample,
        state: 'stale',
        value: null,
      };
      const stale = presentSample(staleSample, 'Speed', { ageMs: 6200 });
      expect(stale.state).toBe('stale');
      expect(stale.valueText).toBe('300');
      expect(stale.stateLabel).toBe('STALE 6.2 s');
    });

    it('untrusted scope greys out and sets state untrusted', () => {
      const untrusted = presentSample(baseSample, 'Speed', { scopeDq: 'untrusted' });
      expect(untrusted.state).toBe('untrusted');
      expect(untrusted.stateLabel).toBe('UNTRUSTED');
      expect(untrusted.valueText).toBe('300');
      expect(untrusted.ariaLabel).toContain('untrusted');
    });

    it('unavailable renders em dash, never 0', () => {
      const unavailSample: ChannelSample = {
        ...baseSample,
        state: 'unavailable',
        value: null,
      };
      const unavail = presentSample(unavailSample, 'Speed');
      expect(unavail.state).toBe('unavailable');
      expect(unavail.valueText).toBe(NO_VALUE);
      expect(unavail.stateLabel).toBe('UNAVAILABLE');
    });

    it('panel renders untrusted note when ScopeDqContext is untrusted (T05)', () => {
      const { renderWithStore } = createPanelHarness();
      renderWithStore(<TimingPanel />, 'untrusted');

      expect(screen.getByText('Untrusted — data format mismatch')).toBeInTheDocument();
      expect(screen.getByTestId('panel-timing')).toHaveAttribute('data-dq', 'untrusted');
    });
  });

  describe('W2.2 Design System Requirements', () => {
    it('applies C12 delta word cues: FASTER when < -0.050s, SLOWER when > +0.050s', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<TimingPanel />);

      send([rec.f64(ch.DELTA_PERSONAL_BEST, -0.120)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByText('▲ FASTER')).toBeInTheDocument();

      send([rec.f64(ch.DELTA_PERSONAL_BEST, 0.180)]);
      act(() => {
        frames.runFrame();
      });
      expect(screen.getByText('▼ SLOWER')).toBeInTheDocument();
    });

    it('evaluates tyre thermal bands relative to optimal temperature (C11, A09)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<TyrePanel />);

      // Optimal temp FL = 85 °C
      // Inner = 88 °C (diff +3 -> Optimal)
      // Middle = 100 °C (diff +15 -> Warm)
      // Outer = 55 °C (diff -30 -> Cold)
      send([
        rec.f32(ch.TYRE_OPTIMAL_TEMP_FL, 85),
        rec.f32(ch.TYRE_TEMP_INNER_FL, 88),
        rec.f32(ch.TYRE_TEMP_MIDDLE_FL, 100),
        rec.f32(ch.TYRE_TEMP_OUTER_FL, 55),
      ]);
      act(() => {
        frames.runFrame();
      });

      // Verify glyphs and modifiers in FL corner
      const flCorner = screen.getByTestId('tyre-fl');
      expect(flCorner).toBeInTheDocument();
      // Optimal cell has ● glyph
      expect(flCorner.querySelector('.tyre-tread-cell--optimal')).toBeInTheDocument();
      // Warm cell has ◆ glyph
      expect(flCorner.querySelector('.tyre-tread-cell--warm')).toBeInTheDocument();
      // Cold cell has ▼ glyph
      expect(flCorner.querySelector('.tyre-tread-cell--cold')).toBeInTheDocument();
    });

    it('shows neutral tyre cells and "Optimal window not reported" when optimal is missing (C11)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<TyrePanel />);

      // Only temps, no optimal
      send([
        rec.f32(ch.TYRE_TEMP_INNER_FL, 90),
        rec.f32(ch.TYRE_TEMP_MIDDLE_FL, 90),
        rec.f32(ch.TYRE_TEMP_OUTER_FL, 90),
      ]);
      act(() => {
        frames.runFrame();
      });

      expect(screen.getByText('Optimal window not reported')).toBeInTheDocument();
      const flCorner = screen.getByTestId('tyre-fl');
      const cells = flCorner.querySelectorAll('.tyre-tread-cell--neutral');
      expect(cells.length).toBe(3);
    });

    it('renders combined Virtual Energy laps and consumption in FuelPanel (EN10)', () => {
      const { frames, send, renderWithStore } = createPanelHarness();
      renderWithStore(<FuelPanel />);

      send([
        rec.f32(ch.FUEL, 40),
        rec.f32(ch.FUEL_CAPACITY, 100),
        rec.f32(ch.VIRTUAL_ENERGY, 0.65),
        rec.f32(ch.VE_LAPS_REMAINING, 8.4),
        rec.f32(ch.VE_PER_LAP, 0.078),
      ]);
      act(() => {
        frames.runFrame();
      });

      expect(screen.getByText('8.4 laps')).toBeInTheDocument();
      expect(screen.getByText('7.8%')).toBeInTheDocument();
    });

    it('has zero forbidden inline styles in panel components (only dynamic width allowed)', () => {
      const panelFiles = [
        'TimingPanel.tsx',
        'FuelPanel.tsx',
        'TyrePanel.tsx',
        'BrakePanel.tsx',
        'EnergyPanel.tsx',
        'DriverInputsPanel.tsx',
      ];

      for (const file of panelFiles) {
        const fullPath = resolve(__dirname, file);
        const content = readFileSync(fullPath, 'utf8');
        // Match style={{ ... }}
        const matches = content.match(/style=\{\{([^}]+)\}\}/g) ?? [];
        for (const match of matches) {
          // Only width is allowed
          expect(match).toMatch(/style=\{\{\s*width:\s*`\$\{/);
        }
      }
    });
  });
});

