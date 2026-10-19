import { describe, expect, it, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { MotecImportBar } from './MotecImportBar';
import { motecStore } from './useMotecStore';

describe('MoTeC Telemetry Store & UI Integration', () => {
  beforeEach(() => {
    motecStore.clear();
  });

  it('renders initial empty state with import and benchmark buttons', () => {
    render(<MotecImportBar />);

    expect(screen.getByTestId('motec-import-bar')).toBeInTheDocument();
    expect(screen.getByTestId('motec-import-button')).toBeInTheDocument();
    expect(screen.getByTestId('motec-benchmark-button')).toBeInTheDocument();
    expect(screen.getByText(/No MoTeC \(\.ld\) file loaded/i)).toBeInTheDocument();
  });

  it('loads MoTeC benchmark lap and displays metadata and comparison line indicator', () => {
    render(<MotecImportBar />);

    const loadBtn = screen.getByTestId('motec-benchmark-button');
    fireEvent.click(loadBtn);

    // Verify metadata shown
    expect(screen.getByText(/Ferrari 499P/i)).toBeInTheDocument();
    expect(screen.getByText(/Antonio Fuoco/i)).toBeInTheDocument();
    expect(screen.getByText(/Orange Line = MoTeC Reference/i)).toBeInTheDocument();
    expect(screen.getByTestId('motec-toggle-button')).toBeInTheDocument();
    expect(screen.getByTestId('motec-clear-button')).toBeInTheDocument();
  });

  it('toggles MoTeC comparison line visibility', () => {
    render(<MotecImportBar />);

    fireEvent.click(screen.getByTestId('motec-benchmark-button'));
    const toggleBtn = screen.getByTestId('motec-toggle-button');
    expect(toggleBtn).toHaveTextContent(/MoTeC Line ON/i);

    fireEvent.click(toggleBtn);
    expect(toggleBtn).toHaveTextContent(/MoTeC Line OFF/i);
    expect(motecStore.getState().isOverlayEnabled).toBe(false);

    fireEvent.click(toggleBtn);
    expect(toggleBtn).toHaveTextContent(/MoTeC Line ON/i);
    expect(motecStore.getState().isOverlayEnabled).toBe(true);
  });

  it('clears loaded MoTeC session when Unload is clicked', () => {
    render(<MotecImportBar />);

    fireEvent.click(screen.getByTestId('motec-benchmark-button'));
    expect(screen.getByText(/Ferrari 499P/i)).toBeInTheDocument();

    const clearBtn = screen.getByTestId('motec-clear-button');
    fireEvent.click(clearBtn);

    expect(screen.getByText(/No MoTeC \(\.ld\) file loaded/i)).toBeInTheDocument();
    expect(motecStore.getState().parsedFile).toBeNull();
  });

  it('computes and displays live speed delta against MoTeC when distance is provided', () => {
    render(<MotecImportBar currentLapDistanceM={1000} currentSpeedKmh={290} />);

    fireEvent.click(screen.getByTestId('motec-benchmark-button'));

    expect(screen.getByText(/Δ Speed vs MoTeC:/i)).toBeInTheDocument();
  });
});
