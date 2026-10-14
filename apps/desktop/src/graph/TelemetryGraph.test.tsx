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
  const source = new LiveRingSource(s.store, { xMode: 'time' });
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
    const source = new StaticSource({ xs: [0, 1, 2], columns: new Map([[ch.RPM, [1000, 2000, 3000]]]) });
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
    const source = new StaticSource({ xs: [0, 1, 2], columns: new Map([[ch.RPM, [1, 2, 3]]]), synthetic: true });
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
  });
});
