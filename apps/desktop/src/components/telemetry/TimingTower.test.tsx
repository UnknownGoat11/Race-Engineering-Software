import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { TimingTower } from './TimingTower';
import type { CompetitorEntryDto, ScoringStateDto } from '../../hooks/useScoring';
import { ScopeDqContext } from './presentation';

function makeVehicle(overrides: Partial<CompetitorEntryDto> = {}): CompetitorEntryDto {
  return {
    slot_id: 1,
    driver: 'A. Driver',
    vehicle_name: '#51 Ferrari 499P',
    vehicle_filename: null,
    class_name: 'Hypercar',
    pit_group: 'AF Corse',
    place: 1,
    class_place: 1,
    laps: 10,
    lap_dist_m: 2500,
    sector: 2,
    best_lap_s: 95.123,
    last_lap_s: 95.456,
    best_sector1_s: 31.0,
    best_sector12_s: 64.0,
    last_sector1_s: 31.2,
    last_sector12_s: 64.3,
    time_behind_next_s: 0,
    laps_behind_next: 0,
    time_behind_leader_s: 0,
    laps_behind_leader: 0,
    in_pits: false,
    in_garage_stall: false,
    pit_state: 'NONE',
    num_pitstops: 1,
    is_player: false,
    pos_x_m: 100,
    pos_z_m: 200,
    ...overrides,
  };
}

function makeScoring(vehicles: CompetitorEntryDto[], sessionType = 'Race'): ScoringStateDto {
  return {
    session: {
      track_name: 'Sebring',
      session_type: sessionType,
      session_time_remaining_s: 3600,
      end_time_s: 7200,
      lap_limit: 100,
      time_limit_s: 7200,
      yellow_flag_state: 'NONE',
    },
    vehicles,
  };
}

describe('TimingTower Component', () => {
  it('renders car numbers extracted from various vehicle name formats', () => {
    const v1 = makeVehicle({ slot_id: 1, place: 1, vehicle_name: '#51 Ferrari 499P' });
    const v2 = makeVehicle({ slot_id: 2, place: 2, vehicle_name: 'Oreca 07 #81' });
    const v3 = makeVehicle({ slot_id: 3, place: 3, vehicle_name: '#92' });
    const v4 = makeVehicle({ slot_id: 4, place: 4, vehicle_name: 'BMW M4 44' });
    const v5 = makeVehicle({ slot_id: 5, place: 5, vehicle_name: null });

    render(<TimingTower scoring={makeScoring([v1, v2, v3, v4, v5])} />);

    expect(screen.getByTestId('standings-row-1').querySelector('.standings-col-num')?.textContent).toBe('51');
    expect(screen.getByTestId('standings-row-2').querySelector('.standings-col-num')?.textContent).toBe('81');
    expect(screen.getByTestId('standings-row-3').querySelector('.standings-col-num')?.textContent).toBe('92');
    expect(screen.getByTestId('standings-row-4').querySelector('.standings-col-num')?.textContent).toBe('44');
    expect(screen.getByTestId('standings-row-5').querySelector('.standings-col-num')?.textContent).toBe('—');
  });

  it('renders leader with LDR in Gap and Int (C9)', () => {
    const v1 = makeVehicle({ slot_id: 1, place: 1, driver: 'A. Leader' });
    const v2 = makeVehicle({ slot_id: 2, place: 2, driver: 'B. Chaser', time_behind_leader_s: 2.1, time_behind_next_s: 2.1 });
    render(<TimingTower scoring={makeScoring([v1, v2])} />);

    expect(screen.getByTestId('timing-tower')).toBeInTheDocument();
    const rows = screen.getAllByRole('row');
    // Header is row 0; leader is row 1
    const leaderCells = rows[1]?.querySelectorAll('td');
    // P, CLS, #, DRIVER, INT, GAP, LAST, PIT
    expect(leaderCells?.[4]?.textContent).toBe('LDR');
    expect(leaderCells?.[5]?.textContent).toBe('LDR');

    const chaserCells = rows[2]?.querySelectorAll('td');
    expect(chaserCells?.[4]?.textContent).toBe('+2.100');
    expect(chaserCells?.[5]?.textContent).toBe('+2.100');
  });

  it('computes lap gaps correctly in race', () => {
    const v1 = makeVehicle({ slot_id: 1, place: 1 });
    const v2 = makeVehicle({ slot_id: 2, place: 2, laps_behind_leader: 1, laps_behind_next: 1 });
    render(<TimingTower scoring={makeScoring([v1, v2])} />);

    const chaserCells = screen.getByTestId('standings-row-2').querySelectorAll('td');
    expect(chaserCells[4]?.textContent).toBe('+1 LAP');
    expect(chaserCells[5]?.textContent).toBe('+1 LAP');
  });

  it('calculates best-lap gaps in qualifying session (T23)', () => {
    const v1 = makeVehicle({ slot_id: 1, place: 1, best_lap_s: 90.0 });
    const v2 = makeVehicle({ slot_id: 2, place: 2, best_lap_s: 90.35 });
    const v3 = makeVehicle({ slot_id: 3, place: 3, best_lap_s: 90.85 });

    render(<TimingTower scoring={makeScoring([v1, v2, v3], 'Qualifying')} />);

    const r1Cells = screen.getByTestId('standings-row-1').querySelectorAll('td');
    expect(r1Cells[4]?.textContent).toBe('LDR');
    expect(r1Cells[5]?.textContent).toBe('LDR');

    const r2Cells = screen.getByTestId('standings-row-2').querySelectorAll('td');
    expect(r2Cells[4]?.textContent).toBe('+0.350');
    expect(r2Cells[5]?.textContent).toBe('+0.350');

    const r3Cells = screen.getByTestId('standings-row-3').querySelectorAll('td');
    expect(r3Cells[4]?.textContent).toBe('+0.500');
    expect(r3Cells[5]?.textContent).toBe('+0.850');
  });

  it('renders DRIVER badge on player car and NEVER YOU (EN13)', () => {
    const player = makeVehicle({ slot_id: 5, place: 3, driver: 'Player One', is_player: true });
    render(<TimingTower scoring={makeScoring([player])} />);

    expect(screen.getByTestId('driver-pill')).toHaveTextContent('DRIVER');
    expect(screen.queryByText('YOU')).toBeNull();
    expect(screen.getByTestId('standings-row-5')).toHaveClass('standings-row-driver');
  });

  it('formats unknown / null pitstops as em dash, never 0 (T03)', () => {
    const v = makeVehicle({ slot_id: 1, num_pitstops: null });
    render(<TimingTower scoring={makeScoring([v])} />);

    const row = screen.getByTestId('standings-row-1');
    const pitCell = row.querySelector('.standings-col-pit');
    expect(pitCell?.textContent).toBe('—');
  });

  it('renders PIT and GAR badges when car is in pit or garage stall', () => {
    const vPit = makeVehicle({ slot_id: 1, in_pits: true });
    const vGar = makeVehicle({ slot_id: 2, in_garage_stall: true });
    render(<TimingTower scoring={makeScoring([vPit, vGar])} />);

    expect(screen.getAllByText('PIT').length).toBeGreaterThan(0);
    expect(screen.getAllByText('GAR').length).toBeGreaterThan(0);
  });

  it('handles class filtering and displays class position in P column (C8, T22)', () => {
    const hy = makeVehicle({ slot_id: 1, place: 1, class_place: 1, class_name: 'Hypercar', driver: 'H. Driver' });
    const gt1 = makeVehicle({ slot_id: 2, place: 5, class_place: 1, class_name: 'GT3', driver: 'GT First', time_behind_leader_s: 30.0 });
    const gt2 = makeVehicle({ slot_id: 3, place: 6, class_place: 2, class_name: 'GT3', driver: 'GT Second', time_behind_leader_s: 32.5 });

    render(<TimingTower scoring={makeScoring([hy, gt1, gt2])} />);

    // Click GT3 filter button (code 'GT3')
    const gtBtn = screen.getByRole('button', { name: 'GT3' });
    fireEvent.click(gtBtn);

    // Hypercar row should be filtered out
    expect(screen.queryByText('H. Driver')).toBeNull();
    expect(screen.getByText('GT First')).toBeInTheDocument();
    expect(screen.getByText('GT Second')).toBeInTheDocument();

    // GT First is class leader: position in class is 1, Gap = LDR, Int = LDR
    const gt1Row = screen.getByTestId('standings-row-2');
    const gt1Cells = gt1Row.querySelectorAll('td');
    expect(gt1Cells[0]?.textContent).toBe('1');
    expect(gt1Cells[4]?.textContent).toBe('LDR');
    expect(gt1Cells[5]?.textContent).toBe('LDR');

    // GT Second: class position is 2, Gap = +2.500
    const gt2Row = screen.getByTestId('standings-row-3');
    const gt2Cells = gt2Row.querySelectorAll('td');
    expect(gt2Cells[0]?.textContent).toBe('2');
    expect(gt2Cells[4]?.textContent).toBe('+2.500');
    expect(gt2Cells[5]?.textContent).toBe('+2.500');
  });

  it('renders STALE header badge and applies stale row styling when dq is stale (T07)', () => {
    const v = makeVehicle({ slot_id: 1 });
    render(<TimingTower scoring={makeScoring([v])} dq="stale" />);

    expect(screen.getByTestId('tower-stale-badge')).toHaveTextContent('STALE');
    expect(screen.getByTestId('standings-row-1')).toHaveClass('standings-row--stale');
  });

  it('renders STALE header badge from ScopeDqContext', () => {
    const v = makeVehicle({ slot_id: 1 });
    render(
      <ScopeDqContext.Provider value="stale">
        <TimingTower scoring={makeScoring([v])} />
      </ScopeDqContext.Provider>,
    );

    expect(screen.getByTestId('tower-stale-badge')).toHaveTextContent('STALE');
  });

  it('renders ENDED header badge and applies ended table styling (T06)', () => {
    const v = makeVehicle({ slot_id: 1 });
    render(<TimingTower scoring={makeScoring([v])} dq="ended" />);

    expect(screen.getByTestId('tower-ended-badge')).toHaveTextContent('ENDED');
  });

  it('renders UNTRUSTED header badge when dq is untrusted', () => {
    const v = makeVehicle({ slot_id: 1 });
    render(<TimingTower scoring={makeScoring([v])} dq="untrusted" />);

    expect(screen.getByTestId('tower-untrusted-badge')).toHaveTextContent('UNTRUSTED');
  });

  it('renders class chip with tokenized class color and code via classes.ts (DS02)', () => {
    const hy = makeVehicle({ slot_id: 1, class_name: 'Hypercar' });
    const lmp = makeVehicle({ slot_id: 2, class_name: 'LMP2' });
    const gt = makeVehicle({ slot_id: 3, class_name: 'LMGT3' });

    render(<TimingTower scoring={makeScoring([hy, lmp, gt])} />);

    const row1 = screen.getByTestId('standings-row-1');
    expect(row1.querySelector('.class-chip')).toHaveTextContent('HY');

    const row2 = screen.getByTestId('standings-row-2');
    expect(row2.querySelector('.class-chip')).toHaveTextContent('P2');

    const row3 = screen.getByTestId('standings-row-3');
    expect(row3.querySelector('.class-chip')).toHaveTextContent('GT3');
  });

  it('fires onSelectSlot callback when a row is clicked', () => {
    const onSelect = vi.fn();
    const v = makeVehicle({ slot_id: 7 });
    render(<TimingTower scoring={makeScoring([v])} onSelectSlot={onSelect} />);

    fireEvent.click(screen.getByTestId('standings-row-7'));
    expect(onSelect).toHaveBeenCalledWith(7);
  });

  it('applies selected row class when selectedSlotId matches', () => {
    const v1 = makeVehicle({ slot_id: 1 });
    const v2 = makeVehicle({ slot_id: 2 });
    render(<TimingTower scoring={makeScoring([v1, v2])} selectedSlotId={2} />);

    expect(screen.getByTestId('standings-row-2')).toHaveClass('standings-row-selected');
    expect(screen.getByTestId('standings-row-1')).not.toHaveClass('standings-row-selected');
  });

  it('renders awaiting data message when scoring is empty or null', () => {
    const { rerender } = render(<TimingTower scoring={null} />);
    expect(screen.getByText('Awaiting live scoring data...')).toBeInTheDocument();

    rerender(<TimingTower scoring={makeScoring([])} />);
    expect(screen.getByText('Awaiting live scoring data...')).toBeInTheDocument();
  });

  describe('W3.3 Column sets per variant', () => {
    it('renders all 8 columns in full variant (1920)', () => {
      const v = makeVehicle({ slot_id: 1, last_lap_s: 98.5, num_pitstops: 2 });
      render(<TimingTower scoring={makeScoring([v])} variant="full" />);

      const headers = screen.getAllByRole('columnheader');
      const headerTexts = headers.map((h) => h.textContent.trim());
      expect(headerTexts).toEqual(['P', 'CLS', '#', 'DRIVER', 'INT', 'GAP', 'LAST', 'PIT']);

      const row = screen.getByTestId('standings-row-1');
      expect(row.querySelector('.standings-col-last')).toBeInTheDocument();
      expect(row.querySelector('.standings-col-pit')).toBeInTheDocument();
      expect(row.querySelector('.standings-col-last')?.textContent).toBe('1:38.500');
      expect(row.querySelector('.standings-col-pit')?.textContent).toBe('2');
    });

    it('drops Last and Pit columns in compact variant (1366)', () => {
      const v = makeVehicle({ slot_id: 1, last_lap_s: 98.5, num_pitstops: 2 });
      render(<TimingTower scoring={makeScoring([v])} variant="compact" />);

      const headers = screen.getAllByRole('columnheader');
      const headerTexts = headers.map((h) => h.textContent.trim());
      expect(headerTexts).toEqual(['P', 'CLS', '#', 'DRIVER', 'INT', 'GAP']);

      const row = screen.getByTestId('standings-row-1');
      expect(row.querySelector('.standings-col-last')).toBeNull();
      expect(row.querySelector('.standings-col-pit')).toBeNull();
    });

    it('renders 56px rail mode with only P and # columns (rail)', () => {
      const v1 = makeVehicle({ slot_id: 1, place: 1, vehicle_name: '#51 Ferrari' });
      const v2 = makeVehicle({ slot_id: 2, place: 2, vehicle_name: '#83 Ferrari' });
      render(<TimingTower scoring={makeScoring([v1, v2])} variant="rail" />);

      expect(screen.getByTestId('timing-tower-rail')).toBeInTheDocument();
      const headers = screen.getAllByRole('columnheader');
      const headerTexts = headers.map((h) => h.textContent.trim());
      expect(headerTexts).toEqual(['P', '#']);

      const row = screen.getByTestId('standings-row-1');
      const cells = row.querySelectorAll('td');
      expect(cells).toHaveLength(2);
      expect(cells[0]?.textContent).toBe('1');
      expect(cells[1]?.textContent).toBe('51');
    });

    it('expands rail mode on click or Enter, and collapses on Escape or collapse button', () => {
      const v = makeVehicle({ slot_id: 1, driver: 'A. Pier Guidi' });
      render(<TimingTower scoring={makeScoring([v])} variant="rail" />);

      // Initially in collapsed rail mode
      expect(screen.getByTestId('timing-tower-rail')).toBeInTheDocument();
      expect(screen.queryByTestId('timing-tower-rail-expanded')).toBeNull();

      // Click rail to expand
      fireEvent.click(screen.getByTestId('timing-tower-rail'));
      expect(screen.getByTestId('timing-tower-rail-expanded')).toBeInTheDocument();
      expect(screen.getByText('A. Pier Guidi')).toBeInTheDocument();

      // Press Escape to collapse
      fireEvent.keyDown(window, { key: 'Escape' });
      expect(screen.getByTestId('timing-tower-rail')).toBeInTheDocument();
      expect(screen.queryByTestId('timing-tower-rail-expanded')).toBeNull();

      // Expand again using Enter key
      fireEvent.keyDown(screen.getByTestId('timing-tower-rail'), { key: 'Enter' });
      expect(screen.getByTestId('timing-tower-rail-expanded')).toBeInTheDocument();

      // Collapse using collapse button
      const collapseBtn = screen.getByRole('button', { name: 'Collapse tower' });
      fireEvent.click(collapseBtn);
      expect(screen.getByTestId('timing-tower-rail')).toBeInTheDocument();
    });
  });

  describe('W3.3 Own-row pinning mechanism', () => {
    it('pins driver row at the top when driver is scrolled above viewport (pinnedOverride="top")', () => {
      const p1 = makeVehicle({ slot_id: 1, place: 1, driver: 'Leader' });
      const player = makeVehicle({ slot_id: 2, place: 2, driver: 'P. Driver', is_player: true });
      const onSelect = vi.fn();

      render(
        <TimingTower
          scoring={makeScoring([p1, player])}
          pinnedOverride="top"
          onSelectSlot={onSelect}
        />,
      );

      const pinnedRow = screen.getByTestId('standings-pinned-driver-top');
      expect(pinnedRow).toBeInTheDocument();
      expect(pinnedRow).toHaveAttribute('data-pinned', 'top');
      expect(pinnedRow).toHaveClass('standings-row-pinned--top');
      expect(pinnedRow.querySelector('.standings-driver-name')?.textContent).toBe('P. Driver');
      expect(pinnedRow.querySelector('.driver-pill')).toHaveTextContent('DRIVER');

      // Clicking pinned row triggers slot selection
      fireEvent.click(pinnedRow);
      expect(onSelect).toHaveBeenCalledWith(2);
    });

    it('pins driver row at the bottom when driver is scrolled below viewport (pinnedOverride="bottom")', () => {
      const p1 = makeVehicle({ slot_id: 1, place: 1, driver: 'Leader' });
      const player = makeVehicle({ slot_id: 2, place: 20, driver: 'P. Driver', is_player: true });

      render(
        <TimingTower
          scoring={makeScoring([p1, player])}
          pinnedOverride="bottom"
        />,
      );

      const pinnedRow = screen.getByTestId('standings-pinned-driver-bottom');
      expect(pinnedRow).toBeInTheDocument();
      expect(pinnedRow).toHaveAttribute('data-pinned', 'bottom');
      expect(pinnedRow).toHaveClass('standings-row-pinned--bottom');
      expect(pinnedRow.querySelector('.standings-driver-name')?.textContent).toBe('P. Driver');
    });

    it('does not render pinned row when driver is within viewport (pinnedOverride="none")', () => {
      const player = makeVehicle({ slot_id: 1, place: 1, driver: 'P. Driver', is_player: true });

      render(
        <TimingTower
          scoring={makeScoring([player])}
          pinnedOverride="none"
        />,
      );

      expect(screen.queryByTestId('standings-pinned-driver-top')).toBeNull();
      expect(screen.queryByTestId('standings-pinned-driver-bottom')).toBeNull();
      // Regular row is still present in tbody
      expect(screen.getByTestId('standings-row-1')).toBeInTheDocument();
    });

    it('computes pinning dynamically on scroll events', () => {
      // Create a 20-car grid where player is at position 15
      const vehicles = Array.from({ length: 20 }, (_, i) =>
        makeVehicle({
          slot_id: i + 1,
          place: i + 1,
          driver: `Driver ${i + 1}`,
          is_player: i === 14, // P15
        }),
      );

      const { container } = render(<TimingTower scoring={makeScoring(vehicles)} />);
      const tableContainer = container.querySelector('.standings-table-container');
      expect(tableContainer).not.toBeNull();

      if (tableContainer) {
        // Mock scrollHeight = 800, clientHeight = 200, scrollTop = 0 (player at P15 is below viewport)
        Object.defineProperty(tableContainer, 'scrollHeight', { value: 800, configurable: true });
        Object.defineProperty(tableContainer, 'clientHeight', { value: 200, configurable: true });
        Object.defineProperty(tableContainer, 'scrollTop', { value: 0, configurable: true, writable: true });

        const playerRow = screen.getByTestId('standings-row-15');
        Object.defineProperty(playerRow, 'offsetTop', { value: 330, configurable: true, writable: true });
        Object.defineProperty(playerRow, 'offsetHeight', { value: 22, configurable: true });

        fireEvent.scroll(tableContainer);
        expect(screen.getByTestId('standings-pinned-driver-bottom')).toBeInTheDocument();

        // Scroll down so player is in view (scrollTop = 250, rowTop = 330 is within [250, 450])
        tableContainer.scrollTop = 250;
        fireEvent.scroll(tableContainer);
        expect(screen.queryByTestId('standings-pinned-driver-bottom')).toBeNull();
        expect(screen.queryByTestId('standings-pinned-driver-top')).toBeNull();

        // Scroll further down so player is scrolled above viewport (scrollTop = 400)
        tableContainer.scrollTop = 400;
        fireEvent.scroll(tableContainer);
        expect(screen.getByTestId('standings-pinned-driver-top')).toBeInTheDocument();
      }
    });
  });

  describe('W3.3 Class filter segmented chips', () => {
    it('sets aria-pressed correctly on segmented filter buttons', () => {
      const hy = makeVehicle({ slot_id: 1, class_name: 'Hypercar' });
      const lmp = makeVehicle({ slot_id: 2, class_name: 'LMP2' });

      render(<TimingTower scoring={makeScoring([hy, lmp])} />);

      const allBtn = screen.getByRole('button', { name: 'ALL' });
      const hyBtn = screen.getByRole('button', { name: 'HY' });
      const p2Btn = screen.getByRole('button', { name: 'P2' });

      expect(allBtn).toHaveAttribute('aria-pressed', 'true');
      expect(hyBtn).toHaveAttribute('aria-pressed', 'false');
      expect(p2Btn).toHaveAttribute('aria-pressed', 'false');

      fireEvent.click(hyBtn);
      expect(allBtn).toHaveAttribute('aria-pressed', 'false');
      expect(hyBtn).toHaveAttribute('aria-pressed', 'true');
      expect(p2Btn).toHaveAttribute('aria-pressed', 'false');
    });
  });
});
