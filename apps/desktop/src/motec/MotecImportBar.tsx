import { useRef, type ChangeEvent, type JSX, type RefObject } from 'react';
import { ch } from '../generated/channels';
import { isTauri, pickMotecFile } from '../lib/ipc';
import { formatLapTime, isFiniteNumber, mpsToKmh, NO_VALUE } from '../lib/units';
import { sampleMotecAtDistance } from './motecSource';
import { useMotecDropZone } from './motecDropZone';
import { motecStore, useMotecSession } from './useMotecStore';
import './motec.css';

export interface MotecImportBarProps {
  readonly currentLapDistanceM?: number;
  /** Live speed in m/s (registry unit). */
  readonly currentSpeedMps?: number;
  readonly className?: string;
  /**
   * Compact (pitwall, audit V03/U15): with no file, no error and no drag in progress the bar renders nothing (the
   * graph toolbar carries `MotecAddButton` instead); it appears on drag-over, on an error and once a file is loaded.
   */
  readonly compact?: boolean;
  /** A file is being dragged over the drop zone (`useMotecDropZone`), compact mode only. */
  readonly dragActive?: boolean;
}

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Opens the native picker (Tauri) or the hidden browser file input. */
async function openMotecFileDialog(fileInput: RefObject<HTMLInputElement | null>): Promise<void> {
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
  fileInput.current?.click();
}

function HiddenFileInput({ inputRef }: { readonly inputRef: RefObject<HTMLInputElement | null> }): JSX.Element {
  return (
    <input
      ref={inputRef}
      type="file"
      accept=".ld,.ldx"
      multiple
      hidden
      onChange={(e: ChangeEvent<HTMLInputElement>) => {
        const files = e.target.files;
        if (files && files.length > 0) void motecStore.loadFiles(files);
      }}
      data-testid="motec-file-input"
    />
  );
}

/** "Add MoTeC .ld" for a toolbar; renders nothing while a file is loaded (the bar then has its own actions). */
export function MotecAddButton(): JSX.Element | null {
  const { parsedFile, isLoading } = useMotecSession();
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  if (parsedFile) return null;
  return (
    <>
      <HiddenFileInput inputRef={fileInputRef} />
      <button
        type="button"
        className="btn btn-xs"
        onClick={() => {
          void openMotecFileDialog(fileInputRef);
        }}
        disabled={isLoading}
        title="Load a MoTeC .ld file as the reference lap (or drop it on the graph)"
        data-testid="motec-add-button"
      >
        Add MoTeC .ld
      </button>
    </>
  );
}

export function MotecImportBar({
  currentLapDistanceM,
  currentSpeedMps,
  className = '',
  compact = false,
  dragActive = false,
}: MotecImportBarProps): JSX.Element | null {
  const { parsedFile, dataKind, isLoading, error, isOverlayEnabled, clear, setOverlayEnabled } = useMotecSession();
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const ownDrop = useMotecDropZone();

  if (compact && !parsedFile && !error && !dragActive && !isLoading) return null;

  // Live speed minus the MoTeC reference at the current lap distance (both m/s), or null.
  let speedDeltaMps: number | null = null;
  if (parsedFile && isFiniteNumber(currentLapDistanceM) && isFiniteNumber(currentSpeedMps)) {
    const ref = sampleMotecAtDistance(parsedFile, ch.SPEED, currentLapDistanceM);
    if (ref !== null) speedDeltaMps = currentSpeedMps - ref;
  }

  const completeLaps = parsedFile ? parsedFile.laps.filter((l) => l.isValid).length : 0;
  const warnings = parsedFile?.warnings ?? [];
  const dropping = dragActive || ownDrop.dragActive;

  return (
    <div
      className={`motec-bar-container ${compact ? 'motec-bar-compact' : ''} ${dropping ? 'motec-bar-drop' : ''} ${className}`}
      data-testid="motec-import-bar"
      {...(compact ? {} : ownDrop.dropZoneProps)}
    >
      <HiddenFileInput inputRef={fileInputRef} />

      <div className="motec-bar-left">
        <span className="motec-brand-badge">MoTeC</span>

        {parsedFile ? (
          <div className="motec-session-info">
            {dataKind === 'synthetic' && (
              <span className="motec-meta-pill motec-synthetic-pill" data-testid="motec-synthetic-badge">
                Synthetic test data
              </span>
            )}
            <span className="motec-file-name" title={parsedFile.filename}>
              {parsedFile.filename}
            </span>
            <span className="motec-meta-pill" data-testid="motec-header-pill">
              {parsedFile.header.driver ?? 'Driver unknown'} • {parsedFile.header.vehicleId ?? 'Car unknown'}
            </span>
            <span className="motec-meta-pill" data-testid="motec-lap-pill">
              {parsedFile.fastestLap ? (
                <>
                  Quickest: <strong className="num">Lap {parsedFile.fastestLap.lapNumber} ({formatLapTime(parsedFile.fastestLap.lapTimeS)})</strong>{' '}
                  • {completeLaps} valid {completeLaps === 1 ? 'lap' : 'laps'}
                </>
              ) : (
                <>
                  No complete lap found • whole file <strong className="num">{formatLapTime(parsedFile.durationS)}</strong>
                </>
              )}
            </span>
            <span className="motec-meta-pill" data-testid="motec-channel-pill">
              <span className="num">{parsedFile.channels.size}</span> ch (<span className="num">{parsedFile.channelsByAppId.size}</span> mapped) @{' '}
              <span className="num">{parsedFile.primaryFrequencyHz}</span> Hz
            </span>
            {parsedFile.distanceTimelineM === null && (
              <span className="motec-meta-pill" data-testid="motec-no-distance">
                Distance: {NO_VALUE} (no lap distance or speed channel)
              </span>
            )}
            {warnings.length > 0 && (
              <span className="motec-meta-pill" title={warnings.join('\n')} data-testid="motec-warnings">
                {warnings.length} import {warnings.length === 1 ? 'warning' : 'warnings'}
              </span>
            )}

            <span className="motec-line-indicator" title="How the MoTeC reference is drawn in the telemetry graph">
              <span className="motec-color-swatch" aria-hidden="true" />
              <span className="motec-swatch-label">Dashed line = MoTeC reference</span>
            </span>
          </div>
        ) : dropping ? (
          <div className="motec-prompt-text">Drop the .ld file to load it as the MoTeC reference.</div>
        ) : isLoading ? (
          <div className="motec-prompt-text">Loading the MoTeC file…</div>
        ) : compact ? null : (
          <div className="motec-prompt-text">
            No MoTeC (.ld) file loaded. Drop a .ld file here or choose Add MoTeC .ld.
          </div>
        )}
      </div>

      {/* Live Delta Readout when car is running */}
      {parsedFile && speedDeltaMps !== null && (
        <div className="motec-live-delta-pill">
          <span className="delta-label">Δ Speed vs MoTeC:</span>
          <span className={`num delta-val ${speedDeltaMps >= 0 ? 'delta-faster' : 'delta-slower'}`}>
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
              className="btn btn-xs"
              aria-pressed={isOverlayEnabled}
              onClick={() => {
                setOverlayEnabled(!isOverlayEnabled);
              }}
              title="Show or hide the MoTeC reference line in the graph"
              data-testid="motec-toggle-button"
            >
              {isOverlayEnabled ? 'MoTeC line on' : 'MoTeC line off'}
            </button>
            <button
              type="button"
              className="btn btn-xs"
              onClick={() => {
                void openMotecFileDialog(fileInputRef);
              }}
              title="Choose a different MoTeC .ld file"
            >
              Add another .ld
            </button>
            <button
              type="button"
              className="btn btn-xs btn-danger"
              onClick={clear}
              title="Unload the MoTeC reference"
              data-testid="motec-clear-button"
            >
              Unload
            </button>
          </>
        ) : (
          !compact && (
            <button
              type="button"
              className="btn btn-xs btn-primary"
              onClick={() => {
                void openMotecFileDialog(fileInputRef);
              }}
              disabled={isLoading}
              data-testid="motec-import-button"
            >
              Add MoTeC .ld
            </button>
          )
        )}
        {compact && error && !parsedFile && (
          <button type="button" className="btn btn-xs" onClick={clear} data-testid="motec-dismiss-error">
            Dismiss
          </button>
        )}
      </div>

      {error && (
        <div className="motec-error-banner" role="alert" data-testid="motec-error">
          MoTeC error: {error}
        </div>
      )}
    </div>
  );
}
