import { describe, expect, it, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MotecAddButton, MotecImportBar } from './MotecImportBar';
import { motecStore } from './useMotecStore';
import { generateMultiLapMotecBuffer, generateSyntheticDemoMotecBuffer } from './motecWriter';

function loadMultiLap(): void {
  act(() => {
    motecStore.loadBuffer(generateMultiLapMotecBuffer(), 'session.ld', { dataKind: 'synthetic' });
  });
}

describe('MoTeC Telemetry Store & UI Integration', () => {
  beforeEach(() => {
    motecStore.clear();
  });

  it('renders the empty state without any demo/benchmark button', () => {
    render(<MotecImportBar />);
    expect(screen.getByTestId('motec-import-bar')).toBeInTheDocument();
    expect(screen.getByTestId('motec-import-button')).toBeInTheDocument();
    expect(screen.queryByTestId('motec-benchmark-button')).toBeNull();
    expect(screen.getByText(/No MoTeC \(\.ld\) file loaded/i)).toBeInTheDocument();
  });

  it('shows real metadata: quickest lap, sample rate from the file, unknown header fields', () => {
    render(<MotecImportBar />);
    act(() => {
      motecStore.loadBuffer(generateMultiLapMotecBuffer(), 'session.ld');
    });
    expect(screen.getByTestId('motec-lap-pill')).toHaveTextContent(/Quickest: Lap 3 \(2:14\.500\)/);
    expect(screen.getByTestId('motec-lap-pill')).toHaveTextContent(/2 valid laps/);
    expect(screen.getByTestId('motec-channel-pill')).toHaveTextContent('@ 10 Hz');
    // The reference is drawn dashed in the graph's reference colour (graph/engine.ts), not orange.
    expect(screen.getByText(/Dashed line = MoTeC reference/i)).toBeInTheDocument();
    expect(screen.queryByTestId('motec-synthetic-badge')).toBeNull();
  });

  it('labels synthetic data and never shows a real driver name', () => {
    render(<MotecImportBar />);
    act(() => {
      motecStore.loadBuffer(generateSyntheticDemoMotecBuffer(), 'demo.ld', { dataKind: 'synthetic' });
    });
    expect(screen.getByTestId('motec-synthetic-badge')).toHaveTextContent('Synthetic test data');
    expect(screen.getByTestId('motec-header-pill')).toHaveTextContent(/Synthetic data/);
    expect(screen.getByTestId('motec-lap-pill')).toHaveTextContent(/No complete lap found/);
    expect(motecStore.getState().graphSource?.dataKind).toBe('synthetic');
  });

  it('shows unknown header fields as unknown', () => {
    render(<MotecImportBar />);
    act(() => {
      // Patch the driver/vehicle fields of the synthetic file to empty.
      const buf = generateMultiLapMotecBuffer();
      buf.fill(0, 158, 158 + 64);
      buf.fill(0, 222, 222 + 64);
      motecStore.loadBuffer(buf, 'nohdr.ld');
    });
    expect(screen.getByTestId('motec-header-pill')).toHaveTextContent('Driver unknown • Car unknown');
  });

  it('toggles MoTeC comparison line visibility', () => {
    render(<MotecImportBar />);
    loadMultiLap();
    const toggleBtn = screen.getByTestId('motec-toggle-button');
    expect(toggleBtn).toHaveTextContent(/MoTeC Line ON/i);
    fireEvent.click(toggleBtn);
    expect(toggleBtn).toHaveTextContent(/MoTeC Line OFF/i);
    expect(motecStore.getState().isOverlayEnabled).toBe(false);
    fireEvent.click(toggleBtn);
    expect(motecStore.getState().isOverlayEnabled).toBe(true);
  });

  it('clears loaded MoTeC session when Unload is clicked', () => {
    render(<MotecImportBar />);
    loadMultiLap();
    fireEvent.click(screen.getByTestId('motec-clear-button'));
    expect(screen.getByText(/No MoTeC \(\.ld\) file loaded/i)).toBeInTheDocument();
    expect(motecStore.getState().parsedFile).toBeNull();
  });

  it('clears the previous file and shows the error when a new file fails to parse', () => {
    render(<MotecImportBar />);
    loadMultiLap();
    act(() => {
      motecStore.loadBuffer(new Uint8Array(4096), 'junk.ld');
    });
    expect(motecStore.getState().parsedFile).toBeNull();
    expect(motecStore.getState().graphSource).toBeNull();
    expect(screen.getByTestId('motec-error')).toHaveTextContent(/Not a MoTeC \.ld file/);
  });

  it('reports an empty file explicitly', () => {
    render(<MotecImportBar />);
    act(() => {
      motecStore.loadBuffer(new Uint8Array(0), 'empty.ld');
    });
    expect(screen.getByTestId('motec-error')).toHaveTextContent(/"empty\.ld" is empty/);
  });

  it('reports an empty File object explicitly', async () => {
    await motecStore.loadFiles([new File([], 'empty.ld')]);
    expect(motecStore.getState().error).toMatch(/empty/);
    expect(motecStore.getState().parsedFile).toBeNull();
  });

  it('shows the live speed delta in km/h from SI inputs', () => {
    loadMultiLap();
    render(<MotecImportBar currentLapDistanceM={1000} currentSpeedMps={80} />);
    expect(screen.getByText(/Δ Speed vs MoTeC:/i)).toBeInTheDocument();
  });

  it('shows no speed delta past the end of the reference lap', () => {
    loadMultiLap();
    render(<MotecImportBar currentLapDistanceM={1e9} currentSpeedMps={80} />);
    expect(screen.queryByText(/Δ Speed vs MoTeC:/i)).toBeNull();
  });

  it('compact (pitwall): nothing without a file; the toolbar button carries the import (V03)', () => {
    const { container } = render(<MotecImportBar compact />);
    expect(container.firstChild).toBeNull();
    render(<MotecAddButton />);
    expect(screen.getByTestId('motec-add-button')).toHaveTextContent('Add MoTeC .ld');
  });

  it('compact: the bar appears on drag-over, on an error and once a file is loaded', () => {
    const { rerender } = render(<MotecImportBar compact dragActive />);
    expect(screen.getByTestId('motec-import-bar')).toHaveTextContent(/Drop the \.ld file/);
    rerender(<MotecImportBar compact />);
    expect(screen.queryByTestId('motec-import-bar')).toBeNull();
    act(() => {
      motecStore.loadBuffer(new Uint8Array(4096), 'junk.ld');
    });
    expect(screen.getByTestId('motec-error')).toBeInTheDocument();
    act(() => {
      motecStore.clear();
    });
    loadMultiLap();
    expect(screen.getByTestId('motec-lap-pill')).toBeInTheDocument();
    render(<MotecAddButton />);
    expect(screen.queryByTestId('motec-add-button')).toBeNull();
  });

  it('uses no emoji in its controls (V13)', () => {
    render(<MotecImportBar />);
    loadMultiLap();
    const text = screen.getByTestId('motec-import-bar').textContent;
    expect(text).not.toMatch(/\p{Extended_Pictographic}/u);
  });
});
