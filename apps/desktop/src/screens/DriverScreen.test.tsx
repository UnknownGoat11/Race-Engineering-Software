import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as ipc from '../lib/ipc';
import { DriverScreen } from './DriverScreen';

// Mock clipboard
const writeTextMock = vi.fn().mockResolvedValue(undefined);
Object.assign(navigator, {
  clipboard: {
    writeText: writeTextMock,
  },
});

vi.mock('../lib/ipc', async () => {
  const actual = await vi.importActual<typeof ipc>('../lib/ipc');
  return {
    ...actual,
    getCoreStatus: vi.fn(),
    driverStartStreaming: vi.fn().mockResolvedValue(undefined),
    driverStopStreaming: vi.fn().mockResolvedValue(undefined),
    driverNewCode: vi.fn().mockResolvedValue(undefined),
    driverKickEngineer: vi.fn().mockResolvedValue(undefined),
    driverSetRecording: vi.fn().mockResolvedValue(undefined),
    driverAddMarker: vi.fn().mockResolvedValue(undefined),
    driverEndSession: vi.fn().mockResolvedValue(undefined),
    demoStart: vi.fn().mockResolvedValue(undefined),
    demoStop: vi.fn().mockResolvedValue(undefined),
  };
});

const MOCK_DRIVER_STATUS: ipc.CoreStatus = {
  mode: 'driver',
  origin: 'lmu',
  lmu: {
    state: 'LIVE',
    detail: null,
    game_version: 1041,
  },
  session: {
    session_id: 42,
    session_type: 'RACE',
    track: 'Spa-Francorchamps',
    car: 'BMW M Hybrid V8',
    vehicle_class: 'HYPERCAR',
    started_at_unix_s: 1700000000,
  },
  network: {
    state: 'CONNECTED',
    streaming: true,
    rtt_ms: 38,
    connect_code: '8F3A2C',
    code_expires_at_unix_s: 1700003600,
    engineers_connected: 1,
    engineers_max: 2,
    degraded_reason: null,
    driver_away: false,
    last_error: null,
    security_issue: false,
    failure: null,
    relogin_required: false,
    engineers: [
      {
        connection_id: 101,
        username: 'engineer_one',
        display_name: 'Chief Engineer',
        link: 'healthy',
        connected_since_unix_s: 1700000100,
      },
    ],
  },
  recording: {
    state: 'active',
    recording_id: 'rec_42',
    bytes_written: 1024 * 1024 * 50,
    frames_written: 30000,
    frames_lost: 0,
    disk_free_bytes: 1024 * 1024 * 1024 * 200,
    error: null,
  },
  rates: {
    telemetry_read_hz: 100,
    scoring_read_hz: 5,
    frames_sent_hz: 100,
    bytes_sent_per_s: 120000,
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
};

describe('DriverScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.getCoreStatus).mockResolvedValue(MOCK_DRIVER_STATUS);
  });

  it('renders driver status banner, car, track, and session type', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-lmu-state')).toHaveTextContent('LMU: LIVE');
    });

    expect(screen.getByTestId('driver-car')).toHaveTextContent('BMW M Hybrid V8');
    expect(screen.getByTestId('driver-track')).toHaveTextContent('Spa-Francorchamps');
    expect(screen.getByTestId('driver-session-type')).toHaveTextContent('RACE');
    expect(screen.getByTestId('driver-class')).toHaveTextContent('HYPERCAR');
  });

  it('displays connection code and copies it to clipboard', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-connect-code')).toHaveTextContent('8F3A2C');
    });

    const copyBtn = screen.getByTestId('copy-code-btn');
    fireEvent.click(copyBtn);

    expect(writeTextMock).toHaveBeenCalledWith('8F3A2C');
    await waitFor(() => {
      expect(screen.getByText('Copied!')).toBeInTheDocument();
    });
  });

  it('allows kicking a connected engineer', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('kick-engineer-101')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByTestId('kick-engineer-101'));
    expect(ipc.driverKickEngineer).toHaveBeenCalledWith(101);
  });

  it('allows toggling streaming', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('toggle-streaming-btn')).toHaveTextContent('Stop Streaming');
    });

    fireEvent.click(screen.getByTestId('toggle-streaming-btn'));
    expect(ipc.driverStopStreaming).toHaveBeenCalled();
  });

  it('allows requesting a new connection code', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('new-code-btn')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByTestId('new-code-btn'));
    expect(ipc.driverNewCode).toHaveBeenCalled();
  });

  it('allows pausing/resuming recording', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('toggle-recording-btn')).toHaveTextContent('Pause Recording');
    });

    fireEvent.click(screen.getByTestId('toggle-recording-btn'));
    expect(ipc.driverSetRecording).toHaveBeenCalledWith(false);
  });

  it('allows adding session marker notes', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('marker-input')).toBeInTheDocument();
    });

    const input = screen.getByTestId('marker-input');
    fireEvent.change(input, { target: { value: 'Pit entry on lap 15' } });

    const addBtn = screen.getByTestId('add-marker-btn');
    fireEvent.click(addBtn);

    expect(ipc.driverAddMarker).toHaveBeenCalledWith('Pit entry on lap 15');
  });

  it('switches to telemetry dashboard tab', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByText('Telemetry Dashboard')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Telemetry Dashboard'));

    // Without an active store context in this unit test, it shows the empty state cleanly
    await waitFor(() => {
      expect(screen.getByTestId('driver-telemetry-empty')).toBeInTheDocument();
    });
  });

  it('displays synthetic banner when origin is synthetic', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...MOCK_DRIVER_STATUS,
      origin: 'synthetic',
    });

    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('synthetic-badge')).toHaveTextContent('SYNTHETIC DATA');
    });
  });
});
