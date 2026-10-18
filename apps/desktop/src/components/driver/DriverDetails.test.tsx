import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { DriverDetails } from './DriverDetails';
import { EMPTY_HISTORY, deriveDriverTiles } from '../../lib/statusModel';
import type { CoreStatus } from '../../lib/ipc';

function createStatus(overrides?: Partial<CoreStatus>): CoreStatus {
  return {
    mode: 'driver',
    origin: 'lmu',
    lmu: {
      state: 'LIVE',
      detail: 'Running on patch 1.0.4',
      game_version: 1041,
    },
    session: {
      session_id: 1,
      session_type: 'RACE',
      track: 'Sebring',
      car: 'Porsche 911 GT3 R',
      vehicle_class: 'GT3',
      started_at_unix_s: Math.floor(Date.now() / 1000) - 3600,
    },
    recording: {
      state: 'active',
      recording_id: 'rec_1',
      bytes_written: 1024 * 1024 * 120,
      frames_written: 36000,
      frames_lost: 0,
      disk_free_bytes: 1024 * 1024 * 1024 * 50,
      error: null,
    },
    network: {
      state: 'CONNECTED',
      streaming: true,
      rtt_ms: 32,
      connect_code: 'A1B2C3',
      code_expires_at_unix_s: Math.floor(Date.now() / 1000) + 600,
      engineers_connected: 1,
      engineers_max: 2,
      degraded_reason: null,
      driver_away: false,
      last_error: null,
      security_issue: false,
      failure: null,
      relogin_required: false,
      engineers: [],
    },
    rates: {
      telemetry_read_hz: 99.8,
      scoring_read_hz: 5,
      frames_sent_hz: 100,
      bytes_sent_per_s: 150000,
      frames_dropped_total: 0,
    },
    drops: {
      reader_overrun: 0,
      pipeline_overrun: 0,
      recorder_overrun: 0,
      net_dropped_obsolete: 0,
      net_events_dropped: 0,
      ui_dropped: 0,
    },
    engineer: null,
    ...overrides,
  };
}

describe('DriverDetails', () => {
  it('renders all four diagnostic columns', () => {
    const status = createStatus();
    const tiles = deriveDriverTiles(status, EMPTY_HISTORY, { nowMs: Date.now() });

    render(<DriverDetails status={status} tiles={tiles} open={true} />);

    expect(screen.getByText('LMU Telemetry Source')).toBeInTheDocument();
    expect(screen.getByText('Relay Streaming')).toBeInTheDocument();
    expect(screen.getByText('Local Recording')).toBeInTheDocument();
    expect(screen.getByText('Pipeline Health')).toBeInTheDocument();

    // Verify values rendered
    expect(screen.getAllByText('100 Hz')).toHaveLength(2);
    expect(screen.getByText('32 ms')).toBeInTheDocument();
    expect(screen.getByText('36,000')).toBeInTheDocument();
  });

  it('renders dedicated error block for recording errors without text collision', () => {
    const status = createStatus({
      recording: {
        state: 'failed',
        recording_id: null,
        bytes_written: 0,
        frames_written: 0,
        frames_lost: 0,
        disk_free_bytes: 1024 * 1024 * 100,
        error: 'Disk full: 100 MiB free on D:\\ (1 GiB needed). Recording stopped.',
      },
    });
    const tiles = deriveDriverTiles(status, EMPTY_HISTORY, { nowMs: Date.now() });

    render(<DriverDetails status={status} tiles={tiles} open={true} />);

    const errorBlock = screen.getByTestId('recording-error-block');
    expect(errorBlock).toBeInTheDocument();
    expect(errorBlock).toHaveTextContent('Disk full: 100 MiB free on D:\\ (1 GiB needed). Recording stopped.');
  });

  it('renders dash placeholder when status is null', () => {
    const tiles = deriveDriverTiles(null, EMPTY_HISTORY, { nowMs: Date.now() });

    render(<DriverDetails status={null} tiles={tiles} open={true} />);

    // Values should be dashes
    const dashes = screen.getAllByText('—');
    expect(dashes.length).toBeGreaterThan(5);
  });

  it('applies active styling to the designated section', () => {
    const status = createStatus();
    const tiles = deriveDriverTiles(status, EMPTY_HISTORY, { nowMs: Date.now() });

    render(
      <DriverDetails
        status={status}
        tiles={tiles}
        open={true}
        activeSection="recording"
      />,
    );

    const recordingCol = screen.getByTestId('details-section-recording');
    expect(recordingCol).toHaveClass('driver-details-col-active');
  });
});
