import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { CoreStatus } from '../../lib/ipc';
import { EMPTY_HISTORY, deriveDriverTiles } from '../../lib/statusModel';
import { PitBoardStatusRow } from './PitBoardStatusRow';

const NOW_S = 1_800_000_000;

function status(overrides: Partial<CoreStatus> = {}): CoreStatus {
  return {
    mode: 'driver',
    origin: 'lmu',
    lmu: { state: 'LIVE', detail: null, game_version: 1041 },
    session: {
      session_id: 1,
      session_type: 'Race 1',
      track: 'Sebring',
      car: 'Porsche 911 GT3 R',
      vehicle_class: 'GT3',
      started_at_unix_s: NOW_S - 600,
    },
    recording: {
      state: 'active',
      recording_id: 'rec_1',
      bytes_written: 1024,
      frames_written: 10,
      frames_lost: 0,
      disk_free_bytes: 50 * 1024 ** 3,
      error: null,
    },
    network: {
      state: 'CONNECTED',
      streaming: true,
      rtt_ms: 32,
      connect_code: 'K7Q2XM',
      code_expires_at_unix_s: NOW_S + 600,
      engineers_connected: 1,
      engineers_max: 2,
      degraded_reason: null,
      driver_away: false,
      last_error: null,
      security_issue: false,
      failure: null,
      relogin_required: false,
      engineers: [{ connection_id: 1, username: 'ana', display_name: 'Ana', link: 'healthy', connected_since_unix_s: NOW_S - 60 }],
    },
    rates: { telemetry_read_hz: 99.8, scoring_read_hz: 5, frames_sent_hz: 100, bytes_sent_per_s: 1, frames_dropped_total: 0 },
    drops: { reader_overrun: 0, pipeline_overrun: 0, recorder_overrun: 0, net_dropped_obsolete: 0, net_events_dropped: 0, ui_dropped: 0 },
    engineer: null,
    ...overrides,
  };
}

function renderRow(s: CoreStatus): void {
  const tiles = deriveDriverTiles(s, EMPTY_HISTORY, { nowMs: NOW_S * 1000, nowUnixS: NOW_S });
  render(<PitBoardStatusRow tiles={tiles} />);
}

describe('PitBoardStatusRow (D01)', () => {
  it('shows the five chips with the derived tile values', () => {
    renderRow(status());
    expect(screen.getByTestId('pit-status-lmu')).toHaveTextContent('LMU');
    expect(screen.getByTestId('pit-status-eng')).toHaveTextContent('ENG1/2');
    expect(screen.getByTestId('pit-status-code')).toHaveTextContent('CODEK7Q2XM');
  });

  it('colours a chip only when it is not OK', () => {
    renderRow(status({ lmu: { state: 'STALE', detail: null, game_version: 1041 } }));
    expect(screen.getByTestId('pit-status-lmu')).toHaveClass('pit-status-chip--critical');
    expect(screen.getByTestId('pit-status-code').className).toBe('pit-status-chip');
  });

  it('shows "—" for the code in the demo, never a made-up code', () => {
    renderRow(status({ origin: 'synthetic' }));
    expect(screen.getByTestId('pit-status-code')).toHaveTextContent('CODE—');
    expect(screen.getByTestId('pit-status-lmu')).toHaveClass('pit-status-chip--synthetic');
  });
});
