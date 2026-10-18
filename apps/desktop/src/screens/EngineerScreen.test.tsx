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

  it('renders top bar with LIVE stream health, driver, car, track, and latency when LMU origin', async () => {
    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('stream-health-badge')).toHaveTextContent('LIVE STREAM');
      expect(screen.getByTestId('engineer-driver-name')).toHaveTextContent('Jane Doe');
      expect(screen.getByTestId('engineer-car')).toHaveTextContent('Ferrari 499P (HY)');
      expect(screen.getByTestId('engineer-track')).toHaveTextContent('Circuit de la Sarthe');
      expect(screen.getByTestId('engineer-rtt')).toHaveTextContent('33 ms');
      expect(screen.getByTestId('engineer-stream-age')).toHaveTextContent('0.1s');
    });
  });

  it('renders SYNTHETIC badge and no "LIVE" text when origin is synthetic (T04)', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...MOCK_STATUS,
      origin: 'synthetic',
    });

    render(<EngineerScreen />);

    await waitFor(() => {
      const badge = screen.getByTestId('stream-health-badge');
      expect(badge).toHaveTextContent('SYNTHETIC');
      expect(badge.textContent).not.toContain('LIVE');
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

  it('requires confirmation to leave session and disconnects on confirm (EN04)', async () => {
    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('btn-engineer-disconnect-trigger')).toBeInTheDocument();
    });

    // First click opens the inline confirm prompt
    fireEvent.click(screen.getByTestId('btn-engineer-disconnect-trigger'));

    expect(ipc.engineerDisconnect).not.toHaveBeenCalled();
    expect(screen.getByText("Leave this driver's session?")).toBeInTheDocument();
    expect(
      screen.getByText('You need a new engineer code from the driver to rejoin.'),
    ).toBeInTheDocument();

    // Confirm button click executes disconnect
    fireEvent.click(screen.getByTestId('btn-engineer-disconnect-confirm'));

    await waitFor(() => {
      expect(ipc.engineerDisconnect).toHaveBeenCalled();
    });
  });

  it('shows driver away banner and badge when driver is away (T08)', async () => {
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
      expect(screen.getByTestId('state-banner-away')).toBeInTheDocument();
      expect(screen.getByTestId('state-banner-away')).toHaveTextContent('Driver away');
    });
  });

  it('renders incompatible banner and untrusted panels when schema is incompatible (T05)', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...MOCK_STATUS,
      engineer: MOCK_STATUS.engineer
        ? {
            ...MOCK_STATUS.engineer,
            incompatible_schema: true,
          }
        : null,
    });

    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('state-banner-incompatible')).toBeInTheDocument();
      expect(screen.getByTestId('state-banner-incompatible')).toHaveTextContent('Update required');
      expect(screen.getAllByText('UNTRUSTED').length).toBeGreaterThan(0);
    });
  });

  it('renders ended banner with reason and rejoin button when session ends (T06)', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...MOCK_STATUS,
      engineer: MOCK_STATUS.engineer
        ? {
            ...MOCK_STATUS.engineer,
            phase: 'ended',
            ended_reason: 'The driver ended the session.',
          }
        : null,
    });

    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('state-banner-ended')).toBeInTheDocument();
      expect(screen.getByTestId('state-banner-ended')).toHaveTextContent('Session ended by Jane Doe');
      expect(screen.getByTestId('btn-rejoin')).toBeInTheDocument();
      expect(screen.getByTestId('btn-rejoin')).toHaveTextContent('Join with new code');
    });
  });

  it('does not render "Driver" placeholder or workspace when not connected (T09, EN01)', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...MOCK_STATUS,
      engineer: MOCK_STATUS.engineer
        ? {
            ...MOCK_STATUS.engineer,
            phase: 'idle',
            driver_name: null,
          }
        : null,
    });

    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.queryByTestId('engineer-driver-name')).not.toBeInTheDocument();
      expect(screen.queryByText(/^Driver$/i)).not.toBeInTheDocument();
      expect(screen.getByTestId('not-connected-panel')).toBeInTheDocument();
      expect(screen.getByText('Not connected to a driver')).toBeInTheDocument();
      expect(screen.queryByTestId('engineer-workspace')).not.toBeInTheDocument();
    });
  });

  it('hides header completely while awaiting confirmation (T09, OR-3)', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...MOCK_STATUS,
      engineer: MOCK_STATUS.engineer
        ? {
            ...MOCK_STATUS.engineer,
            phase: 'awaiting_confirmation',
          }
        : null,
    });

    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.queryByTestId('engineer-header')).not.toBeInTheDocument();
      expect(screen.queryByTestId('engineer-driver-name')).not.toBeInTheDocument();
    });
  });

  it('opens MoTeC channel selector, allows choosing channels, and updates graph with custom series', async () => {
    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('open-channel-selector-btn')).toBeInTheDocument();
    });

    // Click "+ Channels..." button
    fireEvent.click(screen.getByTestId('open-channel-selector-btn'));

    // Modal should be visible
    expect(screen.getByTestId('channel-selector-modal')).toBeInTheDocument();

    // Select Tyres preset in the modal
    fireEvent.click(screen.getByTestId('preset-tyres-btn'));

    // Apply channels
    fireEvent.click(screen.getByTestId('channel-selector-apply-btn'));

    // Modal should close and custom preset indicator should be visible
    await waitFor(() => {
      expect(screen.queryByTestId('channel-selector-modal')).not.toBeInTheDocument();
      expect(screen.getByTestId('preset-custom-badge')).toHaveTextContent('CUSTOM (4 CH)');
    });
  });

  it('renders announcer, announces refresh requested, and supports preset a11y states', async () => {
    render(<EngineerScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('screen-announcer')).toBeInTheDocument();
      expect(screen.getByTestId('btn-request-snapshot')).toBeInTheDocument();
    });

    // Test snapshot request announcement (EN14, A11)
    fireEvent.click(screen.getByTestId('btn-request-snapshot'));
    await waitFor(() => {
      expect(screen.getByTestId('screen-announcer-polite')).toHaveTextContent('Refresh requested');
    });

    // Test graph worksheets role="group" and aria-pressed states (A04, W5.1)
    const presetGroup = screen.getByRole('group', { name: 'Graph presets' });
    expect(presetGroup).toBeInTheDocument();

    const driverBtn = screen.getByRole('button', { name: 'Driver' });
    expect(driverBtn).toHaveAttribute('aria-pressed', 'true');

    const powertrainBtn = screen.getByRole('button', { name: 'Powertrain' });
    expect(powertrainBtn).toHaveAttribute('aria-pressed', 'false');

    // Switch worksheet and verify aria-pressed updates
    fireEvent.click(powertrainBtn);
    expect(powertrainBtn).toHaveAttribute('aria-pressed', 'true');
    expect(driverBtn).toHaveAttribute('aria-pressed', 'false');

    // Test X-axis mode selector (W5.1, G01)
    const xModeGroup = screen.getByRole('group', { name: 'X-axis mode' });
    expect(xModeGroup).toBeInTheDocument();

    const distanceBtn = screen.getByRole('button', { name: 'Distance' });
    expect(distanceBtn).toHaveAttribute('aria-pressed', 'true');

    const lapTimeBtn = screen.getByRole('button', { name: 'Lap Time' });
    expect(lapTimeBtn).toHaveAttribute('aria-pressed', 'false');

    fireEvent.click(lapTimeBtn);
    expect(lapTimeBtn).toHaveAttribute('aria-pressed', 'true');
    expect(distanceBtn).toHaveAttribute('aria-pressed', 'false');
  });
});
