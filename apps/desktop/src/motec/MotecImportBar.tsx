import { useRef, type ChangeEvent, type DragEvent, type JSX } from 'react';
import { ch } from '../generated/channels';
import { isTauri, pickMotecFile } from '../lib/ipc';
import { formatLapTime, isFiniteNumber, mpsToKmh, NO_VALUE } from '../lib/units';
import { sampleMotecAtDistance } from './motecSource';
import { motecStore, useMotecSession } from './useMotecStore';
import './motec.css';

export interface MotecImportBarProps {
  readonly currentLapDistanceM?: number;
  /** Live speed in m/s (registry unit). */
  readonly currentSpeedMps?: number;
  readonly className?: string;
}

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export function MotecImportBar({
  currentLapDistanceM,
  currentSpeedMps,
  className = '',
}: MotecImportBarProps): JSX.Element {
  const { parsedFile, dataKind, isLoading, error, isOverlayEnabled, clear, setOverlayEnabled } = useMotecSession();

  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const handleOpenFileDialog = async () => {
    if (isTauri()) {
      try {
        const picked = await pickMotecFile();
        if (picked === null) return; // user cancelled
        if (picked.bytes.length === 0) {
          motecStore.setError(`"${picked.name}" is empty (0 bytes).`);
          return;
        }
        motecStore.loadBuffer(new Uint8Array(picked.bytes), picked.name);
      } catch (err) {
        motecStore.setError(`Could not open the MoTeC file: ${messageOf(err)}`);
      }
      return;
    }
    fileInputRef.current?.click();
  };

  const handleFileChange = (e: ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (files && files.length > 0) {
      void motecStore.loadFiles(files);
    }
  };

  const handleDragOver = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
  };

  const handleDrop = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    const files = e.dataTransfer.files;
    if (files.length > 0) {
      void motecStore.loadFiles(files);
    }
  };

  // Live speed minus the MoTeC reference at the current lap distance (both m/s), or null.
  let speedDeltaMps: number | null = null;
  if (parsedFile && isFiniteNumber(currentLapDistanceM) && isFiniteNumber(currentSpeedMps)) {
    const ref = sampleMotecAtDistance(parsedFile, ch.SPEED, currentLapDistanceM);
    if (ref !== null) speedDeltaMps = currentSpeedMps - ref;
  }

  const completeLaps = parsedFile ? parsedFile.laps.filter((l) => l.isValid).length : 0;
  const warnings = parsedFile?.warnings ?? [];

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
        multiple
        style={{ display: 'none' }}
        onChange={handleFileChange}
        data-testid="motec-file-input"
      />

      <div className="motec-bar-left">
        <span className="motec-brand-badge">MoTeC i2</span>

        {parsedFile ? (
          <div className="motec-session-info">
            {dataKind === 'synthetic' && (
              <span className="motec-meta-pill" data-testid="motec-synthetic-badge">
                Synthetic test data
              </span>
            )}
            <span className="motec-file-name" title={parsedFile.filename}>
              📄 {parsedFile.filename}
            </span>
            <span className="motec-meta-pill" data-testid="motec-header-pill">
              🏎️ {parsedFile.header.driver ?? 'Driver unknown'} • {parsedFile.header.vehicleId ?? 'Car unknown'}
            </span>
            <span className="motec-meta-pill" data-testid="motec-lap-pill">
              ⏱️{' '}
              {parsedFile.fastestLap ? (
                <>
                  Quickest: <strong>Lap {parsedFile.fastestLap.lapNumber} ({formatLapTime(parsedFile.fastestLap.lapTimeS)})</strong>{' '}
                  • {completeLaps} valid {completeLaps === 1 ? 'lap' : 'laps'}
                </>
              ) : (
                <>
                  No complete lap found • whole file <strong>{formatLapTime(parsedFile.durationS)}</strong>
                </>
              )}
            </span>
            <span className="motec-meta-pill" data-testid="motec-channel-pill">
              📡 {parsedFile.channels.size} ch ({parsedFile.channelsByAppId.size} mapped) @ {parsedFile.primaryFrequencyHz} Hz
            </span>
            {parsedFile.distanceTimelineM === null && (
              <span className="motec-meta-pill" data-testid="motec-no-distance">
                Distance: {NO_VALUE} (no lap distance or speed channel)
              </span>
            )}
            {warnings.length > 0 && (
              <span className="motec-meta-pill" title={warnings.join('\n')} data-testid="motec-warnings">
                ⚠️ {warnings.length} import {warnings.length === 1 ? 'warning' : 'warnings'}
              </span>
            )}

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
      {parsedFile && speedDeltaMps !== null && (
        <div className="motec-live-delta-pill">
          <span className="delta-label">Δ Speed vs MoTeC:</span>
          <span className={`delta-val ${speedDeltaMps >= 0 ? 'delta-faster' : 'delta-slower'}`}>
            {speedDeltaMps >= 0 ? '+' : ''}
            {mpsToKmh(speedDeltaMps).toFixed(1)} km/h
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
        )}
      </div>

      {error && (
        <div className="motec-error-banner" role="alert" data-testid="motec-error">
          ⚠️ MoTeC Error: {error}
        </div>
      )}
    </div>
  );
}
