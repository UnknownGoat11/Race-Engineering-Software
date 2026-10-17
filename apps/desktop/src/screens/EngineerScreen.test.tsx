import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { CoreStatus } from '../lib/ipc';
import * as ipc from '../lib/ipc';
import { EngineerScreen } from './EngineerScreen';

vi.mock('../lib/ipc', async () => {
  const actual = await vi.importActual<typeof ipc>('../lib/ipc');
  return {
    ...actual,
    getCoreStatus: vi.fn(),
    engineerJoin: vi.fn().mockResolvedValue(undefined),
    engineerConfirm: vi.fn().mockResolvedValue(undefined),
    engineerCancel: vi.fn().mockResolvedValue(undefined),
    engineerDisconnect: vi.fn().mockResolvedValue(undefined),
    engineerRequestSnapshot: vi.fn().mockResolvedValue(undefined),
    bridgeSubscribe: vi.fn().mockResolvedValue({ subscription_id: 1, origin: 'lmu' }),
    bridgeUnsubscribe: vi.fn().mockResolvedValue(undefined),
  };
});

vi.mock('../telemetry/bridge', () => ({
  connectTauriTelemetry: vi.fn(() => ({
    disconnect: vi.fn(),
    origin: 'lmu',
  })),
}));

const MOCK_STATUS: CoreStatus = {
  mode: 'engineer',
  origin: 'lmu',
  lmu: {
    state: 'LIVE',
    game_version: 1041,
    detail: 'Connected',
  },
  session: {
    session_id: 42,
    track: 'Circuit de la Sarthe',
    car: 'Ferrari 499P',
    vehicle_class: 'Hypercar',
    session_type: 'Race',
    started_at_unix_s: 1_700_000_000,
  },
  recording: {
    state: 'off',
    recording_id: null,
    frames_written: 0,
    frames_lost: 0,
    bytes_written: 0,
    disk_free_bytes: 10_000_000_000,
    error: null,
  },
  network: {
    state: 'CONNECTED',
    streaming: true,
    rtt_ms: 32.5,
    connect_code: null,
    code_expires_at_unix_s: null,
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
    telemetry_read_hz: null,
    scoring_read_hz: 5,
    frames_sent_hz: 100,
    bytes_sent_per_s: 12000,
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
  engineer: {
    phase: 'monitoring',
    driver_name: 'Jane Doe',
    ended_reason: null,
    stream_health: 'healthy',
    stream_age_s: 0.12,
    driver_away: false,
    incompatible_schema: false,
  },
};

describe('EngineerScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.getCoreStatus).mockResolvedValue(MOCK_STATUS);
  });

  it('renders top bar with LIVE stream health, driver, car, track, and latency', async () => {
    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('stream-health-badge')).toHaveTextContent('LIVE STREAM');
      expect(screen.getByTestId('engineer-driver-name')).toHaveTextContent('Jane Doe');
      expect(screen.getByTestId('engineer-car')).toHaveTextContent('Ferrari 499P (Hypercar)');
      expect(screen.getByTestId('engineer-track')).toHaveTextContent('Circuit de la Sarthe');
      expect(screen.getByTestId('engineer-rtt')).toHaveTextContent('33 ms');
      expect(screen.getByTestId('engineer-stream-age')).toHaveTextContent('0.1s');
    });
  });

  it('renders pitwall 3-column workspace with timing tower, track map, and telemetry panels', async () => {
    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('engineer-workspace')).toBeInTheDocument();
      expect(screen.getByTestId('timing-tower')).toBeInTheDocument();
      expect(screen.getByText('LIVE VECTOR TRACK MAP')).toBeInTheDocument();
      expect(screen.getByText('SYNCHRONIZED TELEMETRY GRAPH')).toBeInTheDocument();
      expect(screen.getByTestId('panel-timing')).toBeInTheDocument();
      expect(screen.getByTestId('panel-fuel')).toBeInTheDocument();
    });
  });

  it('switches tabs in the right calculation panel column', async () => {
    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('panel-timing')).toBeInTheDocument();
    });

    // Switch to Tyres & Brakes tab
    fireEvent.click(screen.getByRole('button', { name: 'Tyres & Brakes' }));

    await waitFor(() => {
      expect(screen.getByTestId('panel-tyres')).toBeInTheDocument();
      expect(screen.getByTestId('panel-brakes')).toBeInTheDocument();
    });

    // Switch to Inputs & Energy tab
    fireEvent.click(screen.getByRole('button', { name: 'Inputs & Energy' }));

    await waitFor(() => {
      expect(screen.getByTestId('panel-driver-inputs')).toBeInTheDocument();
      expect(screen.getByTestId('panel-energy')).toBeInTheDocument();
    });
  });

  it('requests snapshot when snapshot button is clicked', async () => {
    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('btn-request-snapshot')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByTestId('btn-request-snapshot'));

    expect(ipc.engineerRequestSnapshot).toHaveBeenCalled();
    await waitFor(() => {
      expect(screen.getByText('Snapshot requested from driver.')).toBeInTheDocument();
    });
  });

  it('disconnects when disconnect button is clicked', async () => {
    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('btn-engineer-disconnect')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByTestId('btn-engineer-disconnect'));

    expect(ipc.engineerDisconnect).toHaveBeenCalled();
  });

  it('shows driver away badge when driver is away', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...MOCK_STATUS,
      engineer: MOCK_STATUS.engineer
        ? {
            ...MOCK_STATUS.engineer,
            driver_away: true,
          }
        : null,
    });

    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-away-badge')).toBeInTheDocument();
    });
  });
});
