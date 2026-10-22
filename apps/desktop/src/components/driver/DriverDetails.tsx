import type { JSX } from 'react';
import type { CoreStatus } from '../../lib/ipc';
import type { DriverTiles } from '../../lib/statusModel';
import { NO_VALUE, formatByteSize, formatSessionClock } from '../../lib/units';

export interface DriverDetailsProps {
  readonly status: CoreStatus | null;
  readonly tiles: DriverTiles;
  readonly nowMs?: number;
  readonly open?: boolean;
  readonly onToggle?: (open: boolean) => void;
  readonly activeSection?: 'lmu' | 'recording' | 'streaming' | 'engineers' | 'code' | 'pipeline' | null;
}

export function DriverDetails({
  status,
  tiles,
  nowMs,
  open,
  onToggle,
  activeSection,
}: DriverDetailsProps): JSX.Element {
  const lmu = status?.lmu;
  const network = status?.network;
  const recording = status?.recording;
  const rates = status?.rates;
  const drops = status?.drops;
  const session = status?.session;

  // Elapsed recording/session time (DR07)
  const elapsedText = (() => {
    if (!session?.started_at_unix_s || !nowMs) return NO_VALUE;
    const nowS = Math.floor(nowMs / 1000);
    const elapsedS = Math.max(0, nowS - session.started_at_unix_s);
    return formatSessionClock(elapsedS);
  })();

  const isLmuActive = activeSection === 'lmu';
  const isStreamingActive = activeSection === 'streaming' || activeSection === 'engineers' || activeSection === 'code';
  const isRecordingActive = activeSection === 'recording';
  const isPipelineActive = activeSection === 'pipeline';

  return (
    <details
      className="driver-details"
      open={open}
      onToggle={(e) => {
        onToggle?.((e.target as HTMLDetailsElement).open);
      }}
      data-testid="driver-details"
    >
      <summary className="driver-details-summary">
        Diagnostics
      </summary>

      <div className="driver-details-grid">
        {/* Source Column */}
        <div
          id="details-lmu"
          className={`driver-details-col ${isLmuActive ? 'driver-details-col-active' : ''}`}
          data-testid="details-section-lmu"
        >
          <h4 className="driver-details-title">LMU Telemetry Source</h4>
          <div className="driver-details-row">
            <span className="driver-details-label">State</span>
            <span className="driver-details-val">{lmu?.state ?? NO_VALUE}</span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Game Version</span>
            <span className="driver-details-val">
              {lmu?.game_version !== null && lmu?.game_version !== undefined
                ? String(lmu.game_version)
                : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Read Rate</span>
            <span className="driver-details-val">
              {rates?.telemetry_read_hz !== null && rates?.telemetry_read_hz !== undefined
                ? `${Math.round(rates.telemetry_read_hz)} Hz`
                : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Detail</span>
            <span className="driver-details-val">{lmu?.detail ?? NO_VALUE}</span>
          </div>
        </div>

        {/* Streaming Column */}
        <div
          id="details-streaming"
          className={`driver-details-col ${isStreamingActive ? 'driver-details-col-active' : ''}`}
          data-testid="details-section-streaming"
        >
          <h4 className="driver-details-title">Relay Streaming</h4>
          <div className="driver-details-row">
            <span className="driver-details-label">State</span>
            <span className="driver-details-val">{network?.state ?? NO_VALUE}</span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Send Rate</span>
            <span className="driver-details-val">
              {rates?.frames_sent_hz !== null && rates?.frames_sent_hz !== undefined
                ? `${Math.round(rates.frames_sent_hz)} Hz`
                : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Relay RTT</span>
            <span className="driver-details-val">
              {network?.rtt_ms !== null && network?.rtt_ms !== undefined
                ? `${Math.round(network.rtt_ms)} ms`
                : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Degraded Reason</span>
            <span className="driver-details-val">{network?.degraded_reason ?? NO_VALUE}</span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Last Error</span>
            <span className="driver-details-val">{network?.last_error ?? NO_VALUE}</span>
          </div>
        </div>

        {/* Recording Column */}
        <div
          id="details-recording"
          className={`driver-details-col ${isRecordingActive ? 'driver-details-col-active' : ''}`}
          data-testid="details-section-recording"
        >
          <h4 className="driver-details-title">Local Recording</h4>
          <div className="driver-details-row">
            <span className="driver-details-label">State</span>
            <span className="driver-details-val">{recording?.state ?? NO_VALUE}</span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Session Elapsed</span>
            <span className="driver-details-val">{elapsedText}</span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Frames Written</span>
            <span className="driver-details-val">
              {recording ? recording.frames_written.toLocaleString() : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Bytes Written</span>
            <span className="driver-details-val">
              {recording ? formatByteSize(recording.bytes_written) : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Disk Free</span>
            <span className="driver-details-val">
              {recording?.disk_free_bytes !== null && recording?.disk_free_bytes !== undefined
                ? formatByteSize(recording.disk_free_bytes)
                : NO_VALUE}
            </span>
          </div>
          {/* Dedicated error block to prevent label/value collision (DR04) */}
          {recording?.error ? (
            <div className="driver-details-error-block" data-testid="recording-error-block">
              <span className="driver-details-error-label">Error</span>
              <span className="driver-details-error-text">{recording.error}</span>
            </div>
          ) : (
            <div className="driver-details-row">
              <span className="driver-details-label">Error</span>
              <span className="driver-details-val">{NO_VALUE}</span>
            </div>
          )}
        </div>

        {/* Pipeline Health Column */}
        <div
          id="details-pipeline"
          className={`driver-details-col ${isPipelineActive ? 'driver-details-col-active' : ''}`}
          data-testid="details-section-pipeline"
        >
          <h4 className="driver-details-title">Pipeline Health</h4>
          <div className="driver-details-row">
            <span className="driver-details-label">Overall Health</span>
            <span className="driver-details-val">
              {tiles.pipeline.word} ({tiles.pipeline.label})
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Reader Overrun</span>
            <span className="driver-details-val">
              {drops ? drops.reader_overrun.toLocaleString() : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Pipeline Overrun</span>
            <span className="driver-details-val">
              {drops ? drops.pipeline_overrun.toLocaleString() : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Recorder Drops</span>
            <span className="driver-details-val">
              {drops ? drops.recorder_overrun.toLocaleString() : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">Net Dropped</span>
            <span className="driver-details-val">
              {drops ? drops.net_dropped_obsolete.toLocaleString() : NO_VALUE}
            </span>
          </div>
          <div className="driver-details-row">
            <span className="driver-details-label">UI Dropped</span>
            <span className="driver-details-val">
              {drops ? drops.ui_dropped.toLocaleString() : NO_VALUE}
            </span>
          </div>
        </div>
      </div>
    </details>
  );
}
