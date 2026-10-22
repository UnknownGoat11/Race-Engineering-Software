import { act, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ch } from '../generated/channels';
import { FakeClock, FakeFrames } from '../test/fakeScheduler';
import { RecordingContext } from '../test/recordingContext';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../test/tlvEncoder';
import { TelemetryStore } from '../telemetry/store';
import { UI_BATCH_FLAG_RESET } from '../telemetry/uiBatch';
import type { GraphEngine } from './engine';
import { LiveRingSource } from './liveSource';
import type { RenderFrame, SeriesRenderer } from './renderer';
import { StaticSource } from './staticSource';
import { TelemetryGraph } from './TelemetryGraph';
import type { GraphDataSource } from './types';
import { WORKSHEETS } from './worksheets';

class FakeRenderer implements SeriesRenderer {
  readonly kind = 'canvas2d' as const;
  readonly frames: RenderFrame[] = [];
  mount(): void {
    // The fake draws nothing.
  }
  setSize(): void {
    // Size is irrelevant to the fake.
  }
  render(frame: RenderFrame): void {
    this.frames.push(frame);
  }
  destroy(): void {
    // Nothing to release.
  }
}

function setup(): { store: TelemetryStore; frames: FakeFrames; clock: FakeClock } {
  // jsdom has no canvas: hand every canvas a recording context.
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
    (() => new RecordingContext()) as unknown as HTMLCanvasElement['getContext'],
  );
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({ now: clock.now, scheduleFrame: frames.schedule, scheduleTimeout: clock.scheduleTimeout });
  const blocks = Array.from({ length: 100 }, (_, i) =>
    encodeTlvBlock({ sourceEtS: 100 + i * 0.01 }, [rec.f32(ch.SPEED, i / 10), rec.f32(ch.RPM, 5000 + i)] as FixtureRecord[]),
  );
  const result = store.applyBatch(encodeUiBatch(UI_BATCH_FLAG_RESET, 0, blocks));
  if (!result.ok) throw new Error('batch refused');
  return { store, frames, clock };
}

function mount(onEngine: (e: GraphEngine | null) => void): ReturnType<typeof setup> & { unmount: () => void; renderer: FakeRenderer } {
  const s = setup();
  const renderer = new FakeRenderer();
  const source = new LiveRingSource(s.store, { xMode: 'time', dataKind: 'lmu' });
  const { unmount } = render(
    <TelemetryGraph
      title="Speed and RPM"
      source={source}
      xMode="time"
      series={[{ channelId: ch.SPEED }, { channelId: ch.RPM }]}
      renderer={() => renderer}
      scheduleFrame={s.frames.schedule}
      onEngine={onEngine}
    />,
  );
  return { ...s, unmount, renderer };
}

describe('TelemetryGraph', () => {
  it('shows the channels with provenance badges, the data source and the LIVE state', () => {
    let engine: GraphEngine | null = null;
    mount((e) => (engine = e));
    act(() => engine?.flush());
    expect(screen.getByRole('heading', { name: 'Speed and RPM' })).toBeInTheDocument();
    // Speed is calculated from the velocity vector, engine speed comes straight from LMU (spec §317).
    expect(screen.getByText('Source: Calculated · Live LMU Telemetry')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('LIVE');
    expect(screen.getByText('DERIVED')).toBeInTheDocument();
    expect(screen.getByText('MEASURED')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Show Speed' })).toBeChecked();
  });

  it('shows readouts of the current sample, formatted through units.ts', () => {
    let engine: GraphEngine | null = null;
    mount((e) => (engine = e));
    act(() => engine?.flush());
    // Newest speed 9.9 m/s = 35.64 km/h.
    expect(screen.getByRole('group', { name: /^Speed: 36 kilometres per hour/ })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: /^Engine speed: 5099/ })).toBeInTheDocument();
  });

  it('turns a channel off and on from the legend', () => {
    let engine: GraphEngine | null = null;
    const { renderer } = mount((e) => (engine = e));
    act(() => engine?.flush());
    fireEvent.click(screen.getByRole('checkbox', { name: 'Show Speed' }));
    act(() => engine?.flush());
    expect(renderer.frames.at(-1)?.series.map((s) => s.key)).toEqual([`s:${ch.RPM}`]);
    expect(screen.getByRole('checkbox', { name: 'Show Speed' })).not.toBeChecked();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Show Speed' }));
    act(() => engine?.flush());
    expect(renderer.frames.at(-1)?.series).toHaveLength(2);
  });

  it('fixes an axis at its current scale and returns it to autoscale', () => {
    const holder: { engine: GraphEngine | null } = { engine: null };
    mount((e) => (holder.engine = e));
    const engine = holder.engine;
    act(() => engine?.flush());
    const button = screen.getByRole('button', { name: /^rpm AUTO/ });
    fireEvent.click(button);
    act(() => engine?.flush());
    const setting = Object.values(holder.engine?.config.yScales ?? {})[0];
    expect(setting?.mode).toBe('fixed');
    expect(screen.getByRole('button', { name: /^rpm FIXED/ })).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(screen.getByRole('button', { name: /^rpm FIXED/ }));
    act(() => engine?.flush());
    expect(screen.getByRole('button', { name: /^rpm AUTO/ })).toBeInTheDocument();
  });

  it('shows PAUSED when scrolled back and returns to LIVE from the button', () => {
    let engine: GraphEngine | null = null;
    mount((e) => (engine = e));
    act(() => {
      engine?.flush();
      engine?.dispatch({ type: 'zoomTo', min: 100.2, max: 100.5 });
      engine?.flush();
    });
    expect(screen.getByRole('status')).toHaveTextContent('PAUSED · scrolled back');
    fireEvent.click(screen.getByRole('button', { name: 'Back to live' }));
    act(() => engine?.flush());
    expect(screen.getByRole('status')).toHaveTextContent('LIVE');
  });

  it('labels recordings as such and detaches the engine on unmount', () => {
    let engine: GraphEngine | null = null;
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
      (() => new RecordingContext()) as unknown as HTMLCanvasElement['getContext'],
    );
    const source = new StaticSource({ dataKind: 'lmu', xs: [0, 1, 2], columns: new Map([[ch.RPM, [1000, 2000, 3000]]]) });
    const frames = new FakeFrames();
    const { container, unmount } = render(
      <TelemetryGraph
        source={source}
        xMode="time"
        series={[{ channelId: ch.RPM }]}
        renderer={() => new FakeRenderer()}
        scheduleFrame={frames.schedule}
        onEngine={(e) => (engine = e)}
      />,
    );
    act(() => engine?.flush());
    expect(screen.getByText('Source: Session Recording')).toBeInTheDocument();
    // A recording shows no LIVE indicator.
    expect(screen.queryByRole('status')).toBeNull();
    expect(container.querySelectorAll('.telemetry-graph__plot > *')).toHaveLength(3);
    const plot = container.querySelector('.telemetry-graph__plot');
    unmount();
    expect(engine).toBeNull();
    expect(plot?.children).toHaveLength(0);
  });

  it('labels a synthetic source as Synthetic Test Data, never as LMU telemetry', () => {
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
      (() => new RecordingContext()) as unknown as HTMLCanvasElement['getContext'],
    );
    const source = new StaticSource({ dataKind: 'synthetic', xs: [0, 1, 2], columns: new Map([[ch.RPM, [1, 2, 3]]]) });
    render(
      <TelemetryGraph
        source={source}
        xMode="time"
        series={[{ channelId: ch.RPM }]}
        renderer={() => new FakeRenderer()}
        scheduleFrame={new FakeFrames().schedule}
      />,
    );
    expect(screen.getByText('Source: Synthetic Test Data')).toBeInTheDocument();
    expect(screen.queryByText(/Live LMU Telemetry/)).toBeNull();
    // No "MEASURED: Directly from LMU" badge on synthetic data (silent-failure H4).
    expect(screen.queryByText('MEASURED')).toBeNull();
    expect(screen.getByText('SYNTHETIC')).toBeInTheDocument();
  });

  it('labels a synthetic live feed SYNTHETIC everywhere and never as LMU data (silent-failure H2, H4)', () => {
    let engine: GraphEngine | null = null;
    const s = setup();
    render(
      <TelemetryGraph
        source={new LiveRingSource(s.store, { xMode: 'time', dataKind: 'synthetic' })}
        xMode="time"
        series={[{ channelId: ch.SPEED }, { channelId: ch.RPM }]}
        renderer={() => new FakeRenderer()}
        scheduleFrame={s.frames.schedule}
        onEngine={(e) => (engine = e)}
      />,
    );
    act(() => engine?.flush());
    expect(screen.getByText('Source: Synthetic Test Data')).toBeInTheDocument();
    expect(screen.queryByText(/LMU Telemetry|Calculated/)).toBeNull();
    expect(screen.getByRole('status')).toHaveTextContent('LIVE · SYNTHETIC');
    expect(screen.queryByText('MEASURED')).toBeNull();
    // DS04: Graph legend renders one common SYNTHETIC pill rather than duplicating per series
    expect(screen.getAllByText('SYNTHETIC')).toHaveLength(1);
  });

  it('labels a source without a valid data kind as unverified, never as LMU (silent-failure H2 fail-safe)', () => {
    let engine: GraphEngine | null = null;
    const s = setup();
    const inner = new LiveRingSource(s.store, { xMode: 'time', dataKind: 'lmu' });
    const source = {
      origin: 'live',
      extent: () => inner.extent(),
      read: (range, ids) => inner.read(range, ids),
      current: () => inner.current(),
      latest: (id) => inner.latest(id),
      subscribe: (listener) => inner.subscribe(listener),
    } as Omit<GraphDataSource, 'dataKind'> as GraphDataSource;
    render(
      <TelemetryGraph
        source={source}
        xMode="time"
        series={[{ channelId: ch.RPM }]}
        renderer={() => new FakeRenderer()}
        scheduleFrame={s.frames.schedule}
        onEngine={(e) => (engine = e)}
      />,
    );
    act(() => engine?.flush());
    expect(screen.getByText('Source: Unverified Source (not LMU)')).toBeInTheDocument();
    expect(screen.queryByText(/LMU Telemetry/)).toBeNull();
    expect(screen.queryByText('MEASURED')).toBeNull();
    expect(screen.getByRole('status')).toHaveTextContent('LIVE · UNVERIFIED');
  });
  it('shows a failing graph as GRAPH ERROR with the reason and a Retry button (silent-failure C1)', () => {
    let engine: GraphEngine | null = null;
    const s = setup();
    const inner = new LiveRingSource(s.store, { xMode: 'time', dataKind: 'lmu' });
    let fail = true;
    const source: GraphDataSource = {
      origin: 'live',
      dataKind: 'lmu',
      extent: () => inner.extent(),
      read: (range, ids) => {
        if (fail) throw new Error('boom');
        return inner.read(range, ids);
      },
      current: () => inner.current(),
      latest: (id) => inner.latest(id),
      subscribe: (listener) => inner.subscribe(listener),
    };
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    render(
      <TelemetryGraph
        source={source}
        xMode="time"
        series={[{ channelId: ch.RPM }]}
        renderer={() => new FakeRenderer()}
        scheduleFrame={s.frames.schedule}
        onEngine={(e) => (engine = e)}
      />,
    );
    act(() => engine?.flush());
    expect(screen.getByRole('status')).toHaveTextContent('GRAPH ERROR');
    expect(screen.getByRole('alert')).toHaveTextContent('Graph unavailable: GRAPH ERROR · not updating: boom');
    expect(screen.getByRole('group', { name: 'Engine speed: unavailable' })).toBeInTheDocument();
    expect(errors).toHaveBeenCalled();
    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    act(() => engine?.flush());
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByRole('status')).toHaveTextContent('LIVE');
    vi.restoreAllMocks();
  });
  it('shows LIVE only while the feed is fresh: DEGRADED, STALE, NO DATA after a reset (silent-failure H1)', () => {
    let engine: GraphEngine | null = null;
    const { store, clock, frames } = mount((e) => (engine = e));
    const pump = (): void => {
      act(() => {
        frames.runFrame();
        frames.runFrame();
        engine?.flush();
      });
    };
    act(() => engine?.flush());
    expect(screen.getByRole('status')).toHaveTextContent(/^LIVE$/);
    act(() => { clock.advance(300); });
    pump();
    expect(screen.getByRole('status')).toHaveTextContent(/^DEGRADED$/);
    expect(screen.getByRole('status')).toHaveAttribute('data-mode', 'degraded');
    act(() => { clock.advance(5_000); });
    pump();
    expect(screen.getByRole('status')).toHaveTextContent(/^STALE$/);
    act(() => {
      engine?.dispatch({ type: 'zoomTo', min: 100.2, max: 100.5 });
    });
    pump();
    expect(screen.getByRole('status')).toHaveTextContent('PAUSED · scrolled back · STALE');
    act(() => { store.reset(); });
    pump();
    expect(screen.getByRole('status')).toHaveTextContent('NO DATA');
    expect(screen.getByRole('status')).toHaveAttribute('data-mode', 'noData');
  });

  it('marks readouts at an older cursor position as HISTORY with the cursor time (silent-failure H3)', () => {
    let engine: GraphEngine | null = null;
    mount((e) => (engine = e));
    act(() => {
      engine?.flush();
      engine?.setCursor(100.2);
      engine?.flush();
    });
    expect(screen.getAllByText('HISTORY @ 1:40.20')).toHaveLength(2);
    act(() => {
      engine?.setCursor(null);
      engine?.flush();
    });
    expect(screen.queryByText(/HISTORY/)).toBeNull();
  });

  it('renders stacked worksheets with multiple panes, titles, and proportions (G02, G03, G04)', () => {
    const s = setup();
    const source = new LiveRingSource(s.store, { xMode: 'time', dataKind: 'lmu' });
    const frames = new FakeFrames();

    const { container } = render(
      <TelemetryGraph
        title="Driver Telemetry"
        source={source}
        xMode="time"
        panes={WORKSHEETS.driver.panes}
        renderer={() => new FakeRenderer()}
        scheduleFrame={frames.schedule}
      />,
    );

    expect(container.querySelector('.telemetry-graph--stacked')).toBeInTheDocument();
    const panes = container.querySelectorAll('.telemetry-graph__pane');
    expect(panes).toHaveLength(5);

    const paneTitles = Array.from(container.querySelectorAll('.telemetry-graph__pane-title')).map((el) => el.textContent);
    expect(paneTitles).toEqual(['Speed', 'Delta', 'Throttle & Brake', 'Gear & RPM', 'Steering']);

    // One compact header row per pane: no "(35%)" proportion text (V02).
    expect(container.querySelector('.telemetry-graph__pane-proportion')).toBeNull();
    expect(container.textContent).not.toContain('35%');
    // Exactly one x axis row: only the bottom pane draws the x tick labels.
    const xAxisPanes = Array.from(panes).map((el) => el.getAttribute('data-x-axis'));
    expect(xAxisPanes).toEqual(['false', 'false', 'false', 'false', 'true']);
  });

  it('makes each legend row the label of its checkbox (A08) and toggles once per click', () => {
    const s = setup();
    const source = new LiveRingSource(s.store, { xMode: 'time', dataKind: 'lmu' });
    const { container } = render(
      <TelemetryGraph
        source={source}
        xMode="time"
        panes={WORKSHEETS.driver.panes}
        renderer={() => new FakeRenderer()}
        scheduleFrame={new FakeFrames().schedule}
      />,
    );
    const box = screen.getByRole('checkbox', { name: 'Show Speed' });
    const row = box.closest('label');
    expect(row).toHaveClass('telemetry-graph__legend-row');
    expect(row?.querySelector('.telemetry-graph__swatch')).not.toBeNull();
    // Clicking the name (not the box) toggles the series exactly once.
    const name = row?.querySelector('.telemetry-readout, .telemetry-graph__series-name');
    expect(name).not.toBeNull();
    if (name instanceof HTMLElement) fireEvent.click(name);
    expect(box).not.toBeChecked();
    expect(container.querySelectorAll('.telemetry-graph__readout-click')).toHaveLength(0);
  });

  it('shows only the panes that fit their minimum plot, names the rest and moves the x axis to the last shown pane', () => {
    const s = setup();
    const source = new LiveRingSource(s.store, { xMode: 'time', dataKind: 'lmu' });
    const height = vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (this: HTMLElement) {
      // 300 px for the panes: room for 3 panes at the 48 px minimum plot (see layout.planStackedPanes).
      return this.classList.contains('telemetry-graph__panes') ? 300 : 0;
    });
    try {
      const { container } = render(
        <TelemetryGraph
          source={source}
          xMode="time"
          panes={WORKSHEETS.driver.panes}
          renderer={() => new FakeRenderer()}
          scheduleFrame={new FakeFrames().schedule}
        />,
      );
      const panes = Array.from(container.querySelectorAll<HTMLElement>('.telemetry-graph__pane'));
      expect(panes.map((el) => el.getAttribute('data-pane-id'))).toEqual(['speed', 'delta', 'inputs']);
      expect(panes.map((el) => el.getAttribute('data-x-axis'))).toEqual(['false', 'false', 'true']);
      // Pane heights fill, and never exceed, the measured space.
      const total = panes.reduce((sum, el) => sum + Number.parseFloat(el.style.getPropertyValue('--dyn-height')), 0);
      expect(total).toBeLessThanOrEqual(300);
      expect(total).toBeGreaterThan(290);
      const chip = container.querySelector('.telemetry-graph__hidden-panes');
      expect(chip?.textContent).toContain('+2 panes');
      expect(chip?.textContent).toContain('Gear & RPM, Steering');
    } finally {
      height.mockRestore();
    }
  });

  it('automatically derives dashed reference overlays from live source reference lap on lap axes (G01, G02)', () => {
    const s = setup();
    const source = new LiveRingSource(s.store, {
      xMode: 'lapDistance',
      xChannel: ch.LAP_DISTANCE,
      dataKind: 'lmu',
    });

    // Provide a reference lap to the source
    const refData = new StaticSource({
      dataKind: 'lmu',
      xs: [0, 100, 200],
      columns: new Map([[ch.SPEED, [30, 40, 50]]]),
    });
    source.setReferenceLap(refData);

    const frames = new FakeFrames();
    const { container } = render(
      <TelemetryGraph
        source={source}
        xMode="lapDistance"
        series={[{ channelId: ch.SPEED }]}
        renderer={() => new FakeRenderer()}
        scheduleFrame={frames.schedule}
      />,
    );

    expect(screen.getByText('Ref PB Speed')).toBeInTheDocument();
    const refSwatch = container.querySelector('.telemetry-graph__swatch--reference');
    expect(refSwatch).toBeInTheDocument();
  });
});

