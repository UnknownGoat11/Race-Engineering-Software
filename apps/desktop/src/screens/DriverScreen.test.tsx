import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as ipc from '../lib/ipc';
import { DRIVER_SUBTAB_STORAGE_KEY, readStoredSubTab } from '../components/driver/driverBoardModel';
import { DriverScreen } from './DriverScreen';
import { ch } from '../generated/channels';
import { TelemetryStore } from '../telemetry/store';
import { TelemetryStoreContext } from '../telemetry/hooks';
import { DriverTelemetryTab } from '../components/telemetry/DriverTelemetryTab';
import { encodeTlvBlock, encodeUiBatch, rec } from '../test/tlvEncoder';

// Mock clipboard
const writeTextMock = vi.fn().mockResolvedValue(undefined);
Object.assign(navigator, {
  clipboard: {
    writeText: writeTextMock,
  },
});

vi.mock('../telemetry/bridge', () => ({
  connectTauriTelemetry: vi.fn(() => ({
    disconnect: vi.fn(),
    origin: 'lmu',
  })),
}));

vi.mock('../lib/ipc', async () => {
  const actual = await vi.importActual<typeof ipc>('../lib/ipc');
  return {
    ...actual,
    getCoreStatus: vi.fn(),
    getAppInfo: vi.fn().mockResolvedValue({
      name: 'Race Engineering Tool',
      version: '1.2.0',
      identifier: 'com.lmu.race-engineering',
      build_profile: 'debug',
      environment: 'development',
      log_dir: 'D:\\RaceEngineeringData\\logs',
    }),
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

function createMockStatus(): ipc.CoreStatus {
  return {
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
      code_expires_at_unix_s: Math.floor(Date.now() / 1000) + 3600,
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
}

describe('DriverScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.getCoreStatus).mockResolvedValue(createMockStatus());
    window.localStorage.clear();
  });

  it('renders the session context as one line: car, class chip, track, session (D02)', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-car')).toHaveTextContent('BMW M Hybrid V8');
    });

    expect(screen.getByTestId('driver-track')).toHaveTextContent('Spa-Francorchamps');
    expect(screen.getByTestId('driver-session-type')).toHaveTextContent('RACE');
    expect(screen.getByTestId('driver-class')).toHaveTextContent('HY');
    expect(screen.getByTestId('driver-context').tagName).toBe('P');
    // The LMU state badge is gone: the LMU tile says it (D02).
    expect(screen.queryByTestId('driver-lmu-state')).not.toBeInTheDocument();
  });

  it('shows "—" in the context line where a value is missing, never a placeholder sentence', async () => {
    const base = createMockStatus();
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...base,
      session: { ...base.session, car: null, track: null, vehicle_class: null, session_type: null },
    });
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-car')).toHaveTextContent('—');
    });
    expect(screen.getByTestId('driver-track')).toHaveTextContent('—');
    expect(screen.getByTestId('driver-class')).toHaveTextContent('—');
    expect(screen.getByTestId('driver-session-type')).toHaveTextContent('—');
  });

  it('displays connection code and copies it to clipboard', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-connect-code')).toHaveTextContent('8F3A2C');
      expect(screen.getByTestId('copy-code-btn')).toBeInTheDocument();
    });

    const copyBtn = screen.getByTestId('copy-code-btn');
    fireEvent.click(copyBtn);

    expect(writeTextMock).toHaveBeenCalledWith('8F3A2C');
    await waitFor(() => {
      expect(screen.getByText('Copied')).toBeInTheDocument();
    });
  });

  it('allows removing a connected engineer with confirmation', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('kick-engineer-101')).toBeInTheDocument();
    });

    // Initial click on Remove trigger
    const trigger = screen.getByTestId('kick-engineer-101-trigger');
    fireEvent.click(trigger);

    // Confirmation step is displayed
    const confirmBtn = await screen.findByTestId('kick-engineer-101-confirm');
    expect(confirmBtn).toBeInTheDocument();

    // Confirm removal
    fireEvent.click(confirmBtn);
    expect(ipc.driverKickEngineer).toHaveBeenCalledWith(101);
  });

  it('allows toggling streaming', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('toggle-streaming-btn')).toHaveTextContent('Stop streaming');
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
    await waitFor(() => {
      expect(ipc.driverNewCode).toHaveBeenCalled();
    });
  });

  it('allows pausing/resuming recording', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('toggle-recording-btn')).toHaveTextContent('Pause recording');
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

  it('switches to the pit board, shows the status row and remembers the sub-tab (N03, D01)', async () => {
    const { unmount } = render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('tab-telemetry')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByTestId('tab-telemetry'));

    await waitFor(() => {
      expect(screen.getByTestId('driver-telemetry-board')).toBeInTheDocument();
    });
    expect(screen.getByTestId('pit-board-status-row')).toBeInTheDocument();
    expect(window.localStorage.getItem(DRIVER_SUBTAB_STORAGE_KEY)).toBe('pitboard');

    unmount();
    render(<DriverScreen />);
    expect(screen.getByRole('button', { name: 'Pit board' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('opens on Setup when localStorage is unavailable (N03)', () => {
    const spy = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    try {
      expect(readStoredSubTab()).toBe('setup');
    } finally {
      spy.mockRestore();
    }
  });

  it('displays synthetic banner and demo state when origin is synthetic', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...createMockStatus(),
      origin: 'synthetic',
    });

    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-demo-band')).toBeInTheDocument();
    });
    // U14: the demo is marked by the band and the LMU tile only; no amber header badges.
    expect(screen.queryByTestId('synthetic-badge')).not.toBeInTheDocument();
    expect(screen.queryByTestId('driver-lmu-state')).not.toBeInTheDocument();
    expect(screen.queryByText('Not available in demo.')).not.toBeInTheDocument();

    // Start streaming is disabled in demo mode
    const streamBtn = screen.getByTestId('toggle-streaming-btn');
    expect(streamBtn).toBeDisabled();
  });

  it('never renders 0 when status is null', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue(null as unknown as ipc.CoreStatus);

    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('tile-lmu')).toBeInTheDocument();
    });

    // All primary tiles must show "—" when status is unavailable, never 0
    expect(within(screen.getByTestId('tile-lmu')).getByText('—')).toBeInTheDocument();
    expect(within(screen.getByTestId('tile-recording')).getByText('—')).toBeInTheDocument();
    expect(within(screen.getByTestId('tile-streaming')).getByText('—')).toBeInTheDocument();
    expect(within(screen.getByTestId('tile-engineers')).getByText('—')).toBeInTheDocument();
    expect(within(screen.getByTestId('tile-code')).getByText('——————')).toBeInTheDocument();
  });

  it('End session requires confirm', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('end-session-btn-trigger')).toBeInTheDocument();
    });

    // Clicking trigger enters confirmation step without ending session immediately
    fireEvent.click(screen.getByTestId('end-session-btn-trigger'));
    expect(ipc.driverEndSession).not.toHaveBeenCalled();

    // Confirm button must be present with stated consequence
    const confirmBtn = await screen.findByTestId('end-session-btn-confirm');
    expect(confirmBtn).toBeInTheDocument();
    expect(screen.getByText('Ends the session for you and all connected engineers.')).toBeInTheDocument();

    // Clicking confirm invokes driverEndSession
    fireEvent.click(confirmBtn);
    expect(ipc.driverEndSession).toHaveBeenCalled();
  });

  it('shows attention items for pipeline degraded and recording failed', async () => {
    const mock = createMockStatus();
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({
      ...mock,
      recording: {
        ...mock.recording,
        state: 'failed',
        error: 'Disk full: 212 MiB free on D:\\. Recording stopped.',
      },
      drops: {
        ...mock.drops,
        reader_overrun: 45,
        pipeline_overrun: 12,
      },
    });

    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-attention-strip')).toBeInTheDocument();
    });

    expect(screen.getByTestId('attention-recording')).toBeInTheDocument();
    expect(screen.getByTestId('attention-pipeline')).toBeInTheDocument();
  });

  it('Details table is collapsed by default and opens when a tile is clicked', async () => {
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-details')).toBeInTheDocument();
    });

    const details = screen.getByTestId('driver-details');
    expect(details).not.toHaveAttribute('open');

    // The tile's "Details" button opens Details and focuses that section (A02)
    fireEvent.click(screen.getByTestId('tile-recording-details'));

    expect(details).toHaveAttribute('open');
    expect(screen.getByTestId('details-section-recording')).toHaveClass('driver-details-col-active');
  });

  it('Developer tools are hidden in production environment', async () => {
    render(<DriverScreen environment="production" />);

    await waitFor(() => {
      expect(screen.getByTestId('tile-lmu')).toBeInTheDocument();
    });

    expect(screen.queryByTestId('driver-developer-tools')).not.toBeInTheDocument();
  });

  it('Developer tools are visible in development environment', async () => {
    render(<DriverScreen environment="development" />);

    await waitFor(() => {
      expect(screen.getByTestId('driver-developer-tools')).toBeInTheDocument();
    });
  });

  it('dashboard renders 4 glance items when the store has samples', async () => {
    const testStore = new TelemetryStore();
    testStore.applyBatch(
      encodeUiBatch(0, 0, [
        encodeTlvBlock({ sourceEtS: 100 }, [
          rec.f64(ch.LAP_TIME, 94.215),
          rec.f64(ch.DELTA_PERSONAL_BEST, -0.342),
          rec.f32(ch.FUEL, 45.2),
          rec.f32(ch.FUEL_LAPS_REMAINING, 12.5),
        ]),
      ]),
    );

    render(
      <TelemetryStoreContext value={testStore}>
        <DriverTelemetryTab />
      </TelemetryStoreContext>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('driver-delta-value')).toHaveTextContent('-0.342');
      expect(screen.getByTestId('driver-laps-remaining')).toHaveTextContent('12.5');
      expect(screen.getByTestId('driver-fuel-tile')).toHaveTextContent('Fuel 45.2 L');
      expect(screen.getByTestId('driver-limiter-tile')).toBeInTheDocument();
      expect(screen.getByTestId('driver-flag-banner')).toBeInTheDocument();
    });
  });

  it('renders live announcer and supports W4.2 screen a11y requirements', async () => {
    const base = createMockStatus();
    const mockStatus: ipc.CoreStatus = {
      ...base,
      network: {
        ...base.network,
        connect_code: 'ABC123',
        streaming: true,
        engineers: [
          {
            connection_id: 1,
            username: 'pitwall_ana',
            display_name: 'Ana',
            link: 'strong',
            connected_since_unix_s: 1700000000,
          },
        ],
      },
    };
    vi.mocked(ipc.getCoreStatus).mockResolvedValue(mockStatus);

    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('screen-announcer')).toBeInTheDocument();
      expect(screen.getByTestId('copy-code-btn')).toBeInTheDocument();
    });

    // Test code copy announcement
    fireEvent.click(screen.getByTestId('copy-code-btn'));
    await waitFor(() => {
      expect(screen.getByTestId('screen-announcer-polite')).toHaveTextContent(
        'Engineer code copied to clipboard',
      );
    });

    // Test accessible name on Remove engineer button (DR03)
    const removeBtn = screen.getByRole('button', { name: 'Remove Ana (@pitwall_ana)' });
    expect(removeBtn).toBeInTheDocument();

    // Sub-tabs are plain toggle buttons (A07)
    const group = screen.getByRole('group', { name: 'Driver views' });
    expect(group).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Setup' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'Pit board' })).toHaveAttribute('aria-pressed', 'false');
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument();

    // U13: the name appears once (the row), with the link state in the row
    const engineersTile = screen.getByTestId('tile-engineers');
    expect(within(engineersTile).getAllByText('Ana')).toHaveLength(1);

    // Test marker input label association (A06)
    const markerInput = screen.getByLabelText('Log marker note');
    expect(markerInput).toBeInTheDocument();
  });

  it('puts End session alone at the right end and explains the disabled marker input (D05)', async () => {
    const base = createMockStatus();
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({ ...base, recording: { ...base.recording, state: 'off' } });
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('marker-input')).toBeDisabled();
    });
    expect(screen.getByTestId('marker-input')).toHaveAttribute('title', 'Start recording to add markers');
    const bar = screen.getByTestId('driver-action-bar');
    const groups = bar.querySelectorAll(':scope > .action-bar-group');
    const last = groups[groups.length - 1];
    expect(last).toHaveClass('driver-action-end');
    expect(last).toContainElement(screen.getByTestId('end-session-btn-trigger'));
  });

  it('disables Remove in the demo with the reason (U14)', async () => {
    vi.mocked(ipc.getCoreStatus).mockResolvedValue({ ...createMockStatus(), origin: 'synthetic' });
    render(<DriverScreen />);

    await waitFor(() => {
      expect(screen.getByTestId('kick-engineer-101-trigger')).toBeDisabled();
    });
    expect(screen.getByText('Not available in demo')).toBeInTheDocument();
  });
});
