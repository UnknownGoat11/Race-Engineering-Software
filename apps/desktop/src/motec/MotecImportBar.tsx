import { useRef, type ChangeEvent, type DragEvent, type JSX } from 'react';
import { ch } from '../generated/channels';
import { isTauri, pickMotecFile } from '../lib/ipc';
import { formatLapTime } from '../lib/units';
import { motecStore, useMotecSession } from './useMotecStore';
import './motec.css';

export interface MotecImportBarProps {
  readonly currentLapDistanceM?: number;
  readonly currentSpeedKmh?: number;
  readonly className?: string;
}

export function MotecImportBar({
  currentLapDistanceM,
  currentSpeedKmh,
  className = '',
}: MotecImportBarProps): JSX.Element {
  const {
    parsedFile,
    isLoading,
    error,
    isOverlayEnabled,
    loadFile,
    loadBenchmarkLap,
    clear,
    setOverlayEnabled,
  } = useMotecSession();

  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const handleOpenFileDialog = async () => {
    if (isTauri()) {
      try {
        const picked = await pickMotecFile();
        if (picked && picked.bytes && picked.bytes.length > 0) {
          motecStore.loadBuffer(new Uint8Array(picked.bytes), picked.name);
          return;
        }
      } catch (err) {
        console.warn('Native dialog failed, falling back to browser file input:', err);
      }
    }
    fileInputRef.current?.click();
  };

  const handleFileChange = (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (file) {
      void loadFile(file);
    }
  };

  const handleDragOver = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
  };

  const handleDrop = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    const file = e.dataTransfer.files?.[0];
    if (file) {
      void loadFile(file);
    }
  };

  // Sample MoTeC benchmark at current live distance if available
  let motecSpeedAtDist: number | null = null;
  let speedDelta: number | null = null;

  if (parsedFile && typeof currentLapDistanceM === 'number' && typeof currentSpeedKmh === 'number') {
    const speedChan =
      parsedFile.channelsByAppId.get(ch.SPEED) ||
      parsedFile.channels.get('ground speed') ||
      parsedFile.channels.get('speed') ||
      parsedFile.channels.get('vcar');
    if (speedChan && parsedFile.distanceTimelineM.length > 0) {
      const xs = parsedFile.distanceTimelineM;
      let lo = 0;
      let hi = xs.length - 1;
      while (lo <= hi) {
        const mid = (lo + hi) >> 1;
        const v = xs[mid] ?? 0;
        if (v < currentLapDistanceM) lo = mid + 1;
        else hi = mid - 1;
      }
      const idx = Math.min(xs.length - 1, Math.max(0, lo));
      motecSpeedAtDist = speedChan.samples[idx] ?? null;
      if (motecSpeedAtDist !== null) {
        speedDelta = currentSpeedKmh - motecSpeedAtDist;
      }
    }
  }

  return (
    <div
      className={`motec-bar-container ${className}`}
      data-testid="motec-import-bar"
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      {/* Hidden file input */}
      <input
        ref={fileInputRef}
        type="file"
        accept=".ld,.ldx"
        style={{ display: 'none' }}
        onChange={handleFileChange}
        data-testid="motec-file-input"
      />

      <div className="motec-bar-left">
        <span className="motec-brand-badge">MoTeC i2</span>

        {parsedFile ? (
          <div className="motec-session-info">
            <span className="motec-file-name" title={parsedFile.filename}>
              📄 {parsedFile.filename}
            </span>
            <span className="motec-meta-pill">
              🏎️ {parsedFile.header.driver} • {parsedFile.header.vehicleId}
            </span>
            <span className="motec-meta-pill">
              ⏱️ Lap: <strong>{parsedFile.fastestLap ? formatLapTime(parsedFile.fastestLap.lapTimeS) : formatLapTime(parsedFile.durationS)}</strong>
            </span>
            <span className="motec-meta-pill">
              📡 {parsedFile.channels.size} ch @ 100Hz
            </span>

            {/* Line Color Indicator for Comparison */}
            <div className="motec-line-indicator" title="MoTeC comparison line color in telemetry graphs">
              <span className="motec-color-swatch" />
              <span className="motec-swatch-label">Orange Line = MoTeC Reference</span>
            </div>
          </div>
        ) : (
          <div className="motec-prompt-text">
            <span>No MoTeC (.ld) file loaded. Drag & drop a .ld file here or click &quot;Add MoTeC File (.ld)&quot;.</span>
          </div>
        )}
      </div>

      {/* Live Delta Readout when car is running */}
      {parsedFile && speedDelta !== null && (
        <div className="motec-live-delta-pill">
          <span className="delta-label">Δ Speed vs MoTeC:</span>
          <span className={`delta-val ${speedDelta >= 0 ? 'delta-faster' : 'delta-slower'}`}>
            {speedDelta >= 0 ? '+' : ''}{speedDelta.toFixed(1)} km/h
          </span>
        </div>
      )}

      {/* Action Buttons */}
      <div className="motec-bar-actions">
        {parsedFile ? (
          <>
            <button
              type="button"
              className={`btn btn-xs ${isOverlayEnabled ? 'btn-primary' : ''}`}
              onClick={() => {
                setOverlayEnabled(!isOverlayEnabled);
              }}
              title="Toggle MoTeC comparison line visibility on graphs"
              data-testid="motec-toggle-button"
            >
              {isOverlayEnabled ? '👁️ MoTeC Line ON' : '🙈 MoTeC Line OFF'}
            </button>
            <button
              type="button"
              className="btn btn-xs"
              onClick={() => {
                void handleOpenFileDialog();
              }}
              title="Choose a different MoTeC .ld file from your PC"
            >
              📁 Add Another .ld
            </button>
            <button
              type="button"
              className="btn btn-xs btn-danger"
              onClick={clear}
              title="Unload MoTeC reference telemetry"
              data-testid="motec-clear-button"
            >
              ✕ Unload
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="btn btn-xs btn-primary"
              onClick={() => {
                void handleOpenFileDialog();
              }}
              disabled={isLoading}
              data-testid="motec-import-button"
            >
              📁 Add MoTeC File (.ld)
            </button>
            <button
              type="button"
              className="btn btn-xs btn-secondary"
              onClick={loadBenchmarkLap}
              disabled={isLoading}
              title="Load Spa-Francorchamps Ferrari 499P Pole Lap (2:16.421) MoTeC demo reference"
              data-testid="motec-benchmark-button"
            >
              ⚡ Try Demo Benchmark Lap
            </button>
          </>
        )}
      </div>

      {error && <div className="motec-error-banner">⚠️ MoTeC Error: {error}</div>}
    </div>
  );
}
